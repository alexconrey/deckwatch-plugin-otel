//! Deckwatch plugin: an OpenTelemetry Collector sidecar for deployments and
//! applications.
//!
//! Enabling the plugin for a deployment (annotation `deckwatch.io/otel:
//! enabled`) or for an application (the `tracing` resource) adds:
//!
//! - an `otel-collector` sidecar whose OTLP receivers listen on 127.0.0.1
//!   only, running `memory_limiter` → `batch` → `otlp_grpc`/`otlp_http`;
//! - `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_PROTOCOL`,
//!   `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` and
//!   `OTEL_<SIGNAL>_EXPORTER` on the application container;
//! - a ConfigMap with the collector config and, when exporter headers are
//!   configured, a Secret with their values, both in the workload namespace.
//!
//! ## WASM exports
//!
//! - `metadata()`: config schema, the `tracing` resource, capabilities.
//! - `apply()`: deployment-level injection, on every deckwatch create/update.
//! - `provision()`: application-level injection, persisted by deckwatch and
//!   applied by its reconciler.
//! - `deprovision()`: explains what deckwatch removes and what it leaves.
//!
//! The logic lives in pure functions ([`metadata_value`], [`apply_inner`],
//! [`provision_inner`], [`deprovision_inner`]) that run in host-target tests.
//! The plugin makes no outbound requests.

pub mod collector;
pub mod host;
pub mod inject;
pub mod secrets;
pub mod settings;
pub mod validate;

#[cfg(test)]
mod host_sim;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use deckwatch_plugin_sdk::{
    EnvVarSpec, PluginContext, PluginMetadata, PluginResult, ProvisioningStatus,
    ResourceDeprovisionRequest, ResourceDeprovisionResult, ResourceProvisionRequest,
    ResourceProvisionResult,
};
use serde_json::Value;

use crate::inject::{Enablement, ObjectNames};
use crate::settings::{RawConfig, Settings};
use crate::validate::is_dns_subdomain;

/// Plugin name reported in metadata. Configure the plugin in deckwatch under
/// this name so its annotations read `deckwatch.plugin-env/otel`.
pub const PLUGIN_NAME: &str = "otel";

/// `metadata()` output.
pub fn metadata_value() -> Value {
    host::metadata(&PluginMetadata {
        name: PLUGIN_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description: "Injects an OpenTelemetry Collector sidecar and OTEL_* environment \
                      variables into deployments (annotation deckwatch.io/otel: enabled) or \
                      whole applications (the OpenTelemetry Collector infrastructure resource)."
            .into(),
        provides: vec!["sidecar:otel-collector".into(), "observability:otel".into()],
        depends_on: vec![],
        optional_depends_on: vec![],
        config_schema: settings::schema(),
        resources: vec![inject::resource_decl()],
        mcp_tuning_fields: vec![],
        ..Default::default()
    })
}

/// The collector's Kubernetes objects and container for one owner (a
/// deployment or an application), from a single rendering of its config.
struct Rendered {
    objects: Vec<Value>,
    sidecar: host::Sidecar,
}

fn render(settings: &Settings, namespace: &str, owner: &str) -> Rendered {
    let config_yaml = collector::render(settings);
    let names: ObjectNames = inject::object_names(owner);
    Rendered {
        objects: inject::collector_objects(settings, namespace, &names, &config_yaml),
        sidecar: inject::collector_sidecar(settings, &names, &collector::config_hash(&config_yaml)),
    }
}

/// `apply()` output: the deployment-level injection, or nothing when the
/// deployment has not opted in. Invalid config injects nothing and reports
/// errors, which deckwatch logs.
pub fn apply_inner(ctx: &PluginContext, raw: &RawConfig) -> Value {
    let mut result = PluginResult::default();
    let (enablement, warnings) = deployment_enablement_checked(ctx);
    result.warnings = warnings;
    if enablement != Enablement::Enabled {
        return host::plugin_result(&result, &[]);
    }
    let parsed = match Settings::parse(raw) {
        Ok(p) => p,
        Err(errors) => {
            result.errors = errors
                .into_iter()
                .map(|e| {
                    format!(
                        "otel-collector not injected into {}: {e}",
                        ctx.deployment_name
                    )
                })
                .collect();
            return host::plugin_result(&result, &[]);
        }
    };
    result.warnings.extend(parsed.warnings);
    let settings = parsed.settings;
    let application = ctx
        .labels
        .get(inject::APPLICATION_LABEL)
        .map(String::as_str)
        .filter(|app| is_dns_subdomain(app));
    let rendered = render(&settings, &ctx.namespace, &ctx.deployment_name);
    result.env_vars =
        inject::deployment_env(&settings, &ctx.namespace, &ctx.deployment_name, application)
            .into_iter()
            .map(|(name, value)| EnvVarSpec::value(name, value))
            .collect();
    result.kubernetes_resources = rendered.objects;
    host::plugin_result(&result, &[rendered.sidecar])
}

/// Like [`inject::deployment_enablement`], but refuses names that are not
/// valid Kubernetes names, since they become part of object names.
fn deployment_enablement_checked(ctx: &PluginContext) -> (Enablement, Vec<String>) {
    let (enablement, mut warnings) = inject::deployment_enablement(ctx);
    if enablement == Enablement::Enabled
        && !(is_dns_subdomain(&ctx.namespace) && is_dns_subdomain(&ctx.deployment_name))
    {
        warnings.push("not injecting: namespace or deployment name is not a valid name".into());
        return (Enablement::NotRequested, warnings);
    }
    (enablement, warnings)
}

/// `provision()` output for the application-level `tracing` resource.
///
/// Returns `Err` for invalid input so deckwatch rejects the request with the
/// message instead of saving a resource that injects nothing.
pub fn provision_inner(req: &ResourceProvisionRequest, raw: &RawConfig) -> Result<Value, String> {
    if req.resource_id != inject::RESOURCE_ID {
        return Err(format!(
            "unknown resource '{}'; this plugin provides '{}'",
            req.resource_id,
            inject::RESOURCE_ID
        ));
    }
    if !is_dns_subdomain(&req.namespace) || !is_dns_subdomain(&req.application_name) {
        return Err("namespace and application name must be valid Kubernetes names".into());
    }
    let parsed = Settings::parse(raw).map_err(|errors| {
        format!(
            "fix the otel plugin settings first (Settings → Plugins): {}",
            errors.join("; ")
        )
    })?;
    let fields = inject::ApplicationFields::parse(&req.fields)?;
    let settings = parsed.settings;
    let rendered = render(&settings, &req.namespace, &req.application_name);
    let state: HashMap<String, String> =
        inject::application_state(&settings, &req.namespace, &req.application_name, &fields)
            .into_iter()
            .collect();
    let result = ResourceProvisionResult {
        state,
        deployment_annotations: HashMap::from([(
            inject::APPLICATION_ANNOTATION.to_string(),
            req.application_name.clone(),
        )]),
        kubernetes_resources: rendered.objects,
        warnings: parsed.warnings,
        status: ProvisioningStatus::Available,
        ..Default::default()
    };
    Ok(host::provision_result(&result, &[rendered.sidecar]))
}

/// `deprovision()` output. The collector is stateless, so there is nothing
/// external to tear down; the message tells the operator what deckwatch
/// does not clean up by itself.
pub fn deprovision_inner(req: &ResourceDeprovisionRequest) -> ResourceDeprovisionResult {
    let names = inject::object_names(&req.application_name);
    ResourceDeprovisionResult {
        errors: vec![],
        message: format!(
            "OpenTelemetry Collector removed from application '{app}'. deckwatch drops the \
             {sidecar} sidecar, its volumes and the OTEL_* variables on its next reconcile \
             only if the application still has another plugin resource; otherwise remove them \
             from the workloads' YAML. deckwatch does not delete ConfigMap '{cm}' or Secret \
             '{secret}' in namespace '{ns}'; delete them once no workload mounts them.",
            app = req.application_name,
            sidecar = inject::SIDECAR_NAME,
            cm = names.config_map,
            secret = names.secret,
            ns = req.namespace,
        ),
        ..Default::default()
    }
}

// ── WASM exports ──────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
mod exports {
    use std::collections::BTreeMap;

    use deckwatch_plugin_sdk::{
        PluginContext, ResourceDeprovisionRequest, ResourceDeprovisionResult,
        ResourceProvisionRequest,
    };
    use extism_pdk::{config, plugin_fn, FnResult, Json, WithReturnCode};
    use serde_json::Value;

    use crate::settings::{RawConfig, ALL_KEYS};

    /// Read every known key from the extism config deckwatch injects.
    fn read_config() -> RawConfig {
        let values: BTreeMap<String, String> = ALL_KEYS
            .iter()
            .filter_map(|key| {
                config::get(key)
                    .ok()
                    .flatten()
                    .map(|v| (key.to_string(), v))
            })
            .collect();
        RawConfig::new(values)
    }

    #[plugin_fn]
    pub fn metadata() -> FnResult<Json<Value>> {
        Ok(Json(crate::metadata_value()))
    }

    #[plugin_fn]
    pub fn apply(Json(ctx): Json<PluginContext>) -> FnResult<Json<Value>> {
        Ok(Json(crate::apply_inner(&ctx, &read_config())))
    }

    #[plugin_fn]
    pub fn provision(Json(req): Json<ResourceProvisionRequest>) -> FnResult<Json<Value>> {
        crate::provision_inner(&req, &read_config())
            .map(Json)
            .map_err(|msg| WithReturnCode::new(extism_pdk::Error::msg(msg), 1))
    }

    #[plugin_fn]
    pub fn deprovision(
        Json(req): Json<ResourceDeprovisionRequest>,
    ) -> FnResult<Json<ResourceDeprovisionResult>> {
        Ok(Json(crate::deprovision_inner(&req)))
    }
}
