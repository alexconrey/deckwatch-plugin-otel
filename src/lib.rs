//! Deckwatch plugin — provisions an OpenTelemetry Collector sidecar via the Infrastructure tab.
//!
//! ## WASM entry points
//!
//! - [`metadata`] — declares the "tracing" provisionable resource; called once at load time.
//! - [`provision`] — injects an OTel Collector sidecar and OTEL env vars when an operator
//!   adds the resource on the Application Infrastructure tab.
//! - [`deprovision`] — called when the resource is removed; returns a message explaining
//!   the reconcile behaviour.
//!
//! ## OTEL_SERVICE_NAME resolution
//!
//! The service name is resolved from the `OTEL_SERVICE_NAME` provision field when set;
//! when unset the field defaults to an empty string and nothing is injected — the
//! application is expected to set `OTEL_SERVICE_NAME` itself.
//!
//! ## Architecture
//!
//! Association is managed at the application level via the Infrastructure tab —
//! no deployment annotation required. Deckwatch calls `provision()` to persist state
//! and sidecars, then injects them into all application deployments on the next
//! reconcile cycle.

#[cfg(target_arch = "wasm32")]
use deckwatch_plugin_sdk::PluginMetadata;
use deckwatch_plugin_sdk::{
    ConfigField, ConfigFieldType, PluginResource, ResourceDeprovisionRequest,
    ResourceDeprovisionResult, ResourceProvisionRequest, ResourceProvisionResult, SidecarSpec,
};
#[cfg(target_arch = "wasm32")]
use extism_pdk::*;

const DEFAULT_IMAGE: &str = "otel/opentelemetry-collector:latest";
const DEFAULT_ENDPOINT: &str = "http://localhost:4317";
const OTEL_PORT: i32 = 4317;

// ── Resource declaration ───────────────────────────────────────────────────────

/// Declares the "tracing" provisionable resource shown on the Infrastructure tab.
///
/// Not `#[cfg(target_arch = "wasm32")]`-gated so it can be called in host-target
/// unit tests.
pub fn resource_decl() -> PluginResource {
    PluginResource {
        id: "tracing".into(),
        label: "OpenTelemetry Collector".into(),
        icon: "mdi-chart-timeline-variant".into(),
        description: "Provision an OpenTelemetry Collector sidecar for this application. Injects OTEL_EXPORTER_OTLP_ENDPOINT and optionally OTEL_SERVICE_NAME.".into(),
        singleton: true,
        fields: vec![
            ConfigField {
                key: "OTEL_IMAGE".into(),
                label: "OTel Collector Image".into(),
                description: "OpenTelemetry Collector container image to inject as a sidecar.".into(),
                field_type: ConfigFieldType::String,
                default: Some(DEFAULT_IMAGE.into()),
                required: false,
                options: vec![],
                env_source: None,
            },
            ConfigField {
                key: "OTEL_SERVICE_NAME".into(),
                label: "Service Name".into(),
                description: "Value for OTEL_SERVICE_NAME injected into all deployments. Leave empty to let each service set it independently.".into(),
                field_type: ConfigFieldType::String,
                default: None,
                required: false,
                options: vec![],
                env_source: None,
            },
        ],
        output_keys: vec![
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
        ],
    }
}

// ── Pure core: provision ───────────────────────────────────────────────────────

/// Provision an OpenTelemetry Collector sidecar for an application.
///
/// Returns a [`ResourceProvisionResult`] with:
/// - `state`: `OTEL_EXPORTER_OTLP_ENDPOINT` (always); `OTEL_SERVICE_NAME` when set
/// - `sidecars`: the `otel-collector` container on port 4317
/// - `deployment_annotations`: stamps `deckwatch.io/otel: "provisioned"`
///
/// Testable on the native target without extism.
pub fn provision_inner(req: &ResourceProvisionRequest) -> ResourceProvisionResult {
    let mut result = ResourceProvisionResult::default();

    let image = req
        .fields
        .get("OTEL_IMAGE")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| DEFAULT_IMAGE.to_string());

    result.state.insert(
        "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
        DEFAULT_ENDPOINT.to_string(),
    );

    // Only inject OTEL_SERVICE_NAME when explicitly set; leave it unset otherwise
    // so individual services can declare their own identity.
    let service_name = req
        .fields
        .get("OTEL_SERVICE_NAME")
        .filter(|s| !s.is_empty())
        .cloned();

    if let Some(name) = service_name {
        result.state.insert("OTEL_SERVICE_NAME".into(), name);
    }

    result
        .deployment_annotations
        .insert("deckwatch.io/otel".into(), "provisioned".into());

    result.sidecars.push(SidecarSpec {
        name: "otel-collector".into(),
        image,
        env: vec![],
        port: Some(OTEL_PORT),
        cpu: Some("100m".into()),
        memory: Some("128Mi".into()),
    });

    result
}

// ── Pure core: deprovision ─────────────────────────────────────────────────────

/// Deprovision — the OTel Collector is stateless; the deckwatch record is removed and
/// the sidecar disappears from deployments on the next reconcile via SSA.
pub fn deprovision_inner(_req: &ResourceDeprovisionRequest) -> ResourceDeprovisionResult {
    ResourceDeprovisionResult {
        errors: vec![],
        message: "OpenTelemetry Collector sidecar will be removed from all application deployments on the next reconcile.".into(),
    }
}

// ── WASM entry points ──────────────────────────────────────────────────────────

/// Return static plugin metadata including the "tracing" resource declaration.
/// Called once by deckwatch at load time.
#[cfg(target_arch = "wasm32")]
#[plugin_fn]
pub fn metadata() -> FnResult<Json<PluginMetadata>> {
    Ok(Json(PluginMetadata {
        name: "otel".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        description:
            "Provisions an OpenTelemetry Collector sidecar via the Application Infrastructure tab"
                .into(),
        provides: vec!["sidecar:otel-collector".into(), "observability:otel".into()],
        depends_on: vec![],
        optional_depends_on: vec![],
        config_schema: vec![ConfigField {
            key: "OTEL_IMAGE".into(),
            label: "OTel Collector Image".into(),
            description: "Default OTel Collector container image (can be overridden per-resource)."
                .into(),
            field_type: ConfigFieldType::String,
            default: Some(DEFAULT_IMAGE.into()),
            required: false,
            options: vec![],
            env_source: None,
        }],
        resources: vec![resource_decl()],
    }))
}

/// Provision an OTel Collector sidecar. Called when an operator clicks
/// "Add OpenTelemetry Collector" and submits the provisioning form on the
/// Application Infrastructure tab.
#[cfg(target_arch = "wasm32")]
#[plugin_fn]
pub fn provision(
    Json(req): Json<ResourceProvisionRequest>,
) -> FnResult<Json<ResourceProvisionResult>> {
    Ok(Json(provision_inner(&req)))
}

/// Deprovision the OTel Collector resource. The sidecar is removed from all
/// deployments on the next reconcile cycle via server-side apply.
#[cfg(target_arch = "wasm32")]
#[plugin_fn]
pub fn deprovision(
    Json(req): Json<ResourceDeprovisionRequest>,
) -> FnResult<Json<ResourceDeprovisionResult>> {
    Ok(Json(deprovision_inner(&req)))
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn provision_req(fields: &[(&str, &str)]) -> ResourceProvisionRequest {
        ResourceProvisionRequest {
            application_name: "my-app".into(),
            namespace: "production".into(),
            resource_id: "tracing".into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn deprovision_req() -> ResourceDeprovisionRequest {
        ResourceDeprovisionRequest {
            application_name: "my-app".into(),
            namespace: "production".into(),
            resource_id: "tracing".into(),
            state: HashMap::new(),
            fields: HashMap::new(),
        }
    }

    #[test]
    fn resource_decl_has_expected_id_and_output_keys() {
        let r = resource_decl();
        assert_eq!(r.id, "tracing");
        assert_eq!(r.label, "OpenTelemetry Collector");
        assert!(r.singleton, "otel collector must be a singleton resource");
        assert!(
            r.output_keys
                .contains(&"OTEL_EXPORTER_OTLP_ENDPOINT".to_string()),
            "OTEL_EXPORTER_OTLP_ENDPOINT must be declared as an output key"
        );
        assert!(
            r.fields.iter().any(|f| f.key == "OTEL_IMAGE"),
            "OTEL_IMAGE field must be declared"
        );
        assert!(
            r.fields.iter().any(|f| f.key == "OTEL_SERVICE_NAME"),
            "OTEL_SERVICE_NAME field must be declared"
        );
    }

    #[test]
    fn provision_defaults_inject_endpoint_and_sidecar() {
        let result = provision_inner(&provision_req(&[]));
        assert!(
            result.errors.is_empty(),
            "no errors expected: {:?}",
            result.errors
        );
        assert_eq!(
            result
                .state
                .get("OTEL_EXPORTER_OTLP_ENDPOINT")
                .map(String::as_str),
            Some(DEFAULT_ENDPOINT)
        );
        assert_eq!(result.sidecars.len(), 1);
        let sc = &result.sidecars[0];
        assert_eq!(sc.name, "otel-collector");
        assert_eq!(sc.image, DEFAULT_IMAGE);
        assert_eq!(sc.port, Some(OTEL_PORT));
    }

    #[test]
    fn provision_service_name_not_set_when_empty() {
        let result = provision_inner(&provision_req(&[]));
        assert!(
            !result.state.contains_key("OTEL_SERVICE_NAME"),
            "OTEL_SERVICE_NAME should not be injected when not specified"
        );
    }

    #[test]
    fn provision_service_name_injected_when_set() {
        let result = provision_inner(&provision_req(&[("OTEL_SERVICE_NAME", "payment-service")]));
        assert_eq!(
            result.state.get("OTEL_SERVICE_NAME").map(String::as_str),
            Some("payment-service")
        );
    }

    #[test]
    fn provision_custom_image_is_used() {
        let result = provision_inner(&provision_req(&[(
            "OTEL_IMAGE",
            "otel/opentelemetry-collector:0.100.0",
        )]));
        assert_eq!(
            result.sidecars[0].image,
            "otel/opentelemetry-collector:0.100.0"
        );
    }

    #[test]
    fn provision_stamps_annotation() {
        let result = provision_inner(&provision_req(&[]));
        assert_eq!(
            result
                .deployment_annotations
                .get("deckwatch.io/otel")
                .map(String::as_str),
            Some("provisioned")
        );
    }

    #[test]
    fn provision_sidecar_has_resource_requests() {
        let result = provision_inner(&provision_req(&[]));
        assert!(result.sidecars[0].cpu.is_some(), "cpu request must be set");
        assert!(
            result.sidecars[0].memory.is_some(),
            "memory request must be set"
        );
    }

    #[test]
    fn deprovision_returns_non_empty_message() {
        let result = deprovision_inner(&deprovision_req());
        assert!(result.errors.is_empty());
        assert!(
            !result.message.is_empty(),
            "deprovision must return a user-visible message"
        );
    }
}
