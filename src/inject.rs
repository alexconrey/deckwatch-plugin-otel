//! What enabling the plugin adds to a deployment or an application.
//!
//! Deployment level: deckwatch calls `apply()` when it creates or updates a
//! deployment; the plugin answers for deployments that opted in with
//! `deckwatch.io/otel: enabled`. Application level: an operator provisions
//! the `tracing` resource; deckwatch persists the result of `provision()` and
//! its reconciler applies it to every deployment and cronjob in the
//! application.

use std::collections::{BTreeMap, HashMap};

use deckwatch_plugin_sdk::{ConfigField, ConfigFieldType, PluginContext, PluginResource};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::collector;
use crate::host::{EnvVar, Sidecar, Volume, VolumeMount};
use crate::secrets;
use crate::settings::{Protocol, Settings};
use crate::validate::percent_encode;

/// Opt-in key, as a Deployment annotation or a pod template label. Values:
/// `enabled` / `true` to opt in, `disabled` / `false` to opt out.
pub const ENABLE_KEY: &str = "deckwatch.io/otel";
/// Stamped (via `provision()`) on every deployment of an application that has
/// the collector at application level. Deployment-level injection leaves
/// those deployments to the application reconciler.
pub const APPLICATION_ANNOTATION: &str = "deckwatch.io/otel-application";
/// deckwatch's application membership label. It sits on Deployment metadata,
/// which plugins cannot see, so it is only used when the pod template has it.
pub const APPLICATION_LABEL: &str = "deckwatch.io/application";

/// The application-level resource id (kept from 0.1.0 so existing
/// provisioned resources keep working).
pub const RESOURCE_ID: &str = "tracing";

pub const SIDECAR_NAME: &str = "otel-collector";
pub const CONFIG_VOLUME: &str = "otelcol-config";
pub const HEADERS_VOLUME: &str = "otelcol-headers";
/// Sidecar env var carrying the config hash, so a config change rolls the pods.
pub const CONFIG_HASH_ENV: &str = "OTELCOL_CONFIG_SHA256";
const MANAGED_BY: &str = "deckwatch-plugin-otel";

pub const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
pub const OTEL_EXPORTER_OTLP_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_PROTOCOL";
pub const OTEL_SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
pub const OTEL_RESOURCE_ATTRIBUTES: &str = "OTEL_RESOURCE_ATTRIBUTES";

/// Resource attribute keys the plugin sets; operators cannot override them.
const MANAGED_ATTRIBUTES: &[&str] = &[
    "k8s.namespace.name",
    "k8s.deployment.name",
    "deckwatch.application.name",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enablement {
    Enabled,
    NotRequested,
    OptedOut,
    /// The application has the collector at application level.
    ManagedByApplication,
}

/// Whether deployment-level injection applies, plus warnings for the operator.
pub fn deployment_enablement(ctx: &PluginContext) -> (Enablement, Vec<String>) {
    let mut warnings = Vec::new();
    let mut requested = false;
    let mut opted_out = false;
    let sources = [
        ("annotation", ctx.annotations.get(ENABLE_KEY)),
        ("pod label", ctx.labels.get(ENABLE_KEY)),
    ];
    for (source, value) in sources {
        let Some(value) = value else { continue };
        match value.trim().to_ascii_lowercase().as_str() {
            "enabled" | "true" => requested = true,
            "disabled" | "false" => opted_out = true,
            _ => warnings.push(format!(
                "ignoring {ENABLE_KEY} {source} {value:?}: use \"enabled\" or \"disabled\""
            )),
        }
    }
    let enablement = if opted_out {
        Enablement::OptedOut
    } else if ctx.annotations.contains_key(APPLICATION_ANNOTATION) {
        if requested {
            warnings.push(format!(
                "{ENABLE_KEY} is ignored: the collector is enabled for this deployment's \
                 application, which manages the sidecar"
            ));
        }
        Enablement::ManagedByApplication
    } else if requested {
        Enablement::Enabled
    } else {
        Enablement::NotRequested
    };
    (enablement, warnings)
}

/// Names of the ConfigMap and Secret created for one deployment or application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectNames {
    pub config_map: String,
    pub secret: String,
}

/// `<owner>-otelcol` and `<owner>-otelcol-headers`, shortened with a hash
/// when they would exceed the 253-character object name limit.
pub fn object_names(owner: &str) -> ObjectNames {
    ObjectNames {
        config_map: scoped_name(owner, "-otelcol"),
        secret: scoped_name(owner, "-otelcol-headers"),
    }
}

fn scoped_name(owner: &str, suffix: &str) -> String {
    const MAX: usize = 253;
    if owner.len() + suffix.len() <= MAX {
        return format!("{owner}{suffix}");
    }
    let hash: String = Sha256::digest(owner.as_bytes())[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Owner names are DNS subdomains (ASCII), so byte slicing is safe.
    let keep = MAX - suffix.len() - hash.len() - 1;
    let head = owner[..keep].trim_end_matches(['-', '.']);
    format!("{head}-{hash}{suffix}")
}

/// The collector container.
pub fn collector_sidecar(settings: &Settings, names: &ObjectNames, config_hash: &str) -> Sidecar {
    let mut volumes = vec![Volume {
        name: CONFIG_VOLUME.into(),
        volume_type: "configMap".into(),
        source_name: names.config_map.clone(),
    }];
    let mut volume_mounts = vec![VolumeMount {
        name: CONFIG_VOLUME.into(),
        mount_path: settings.config_dir.clone(),
        read_only: true,
    }];
    if !settings.exporter.headers.is_empty() {
        volumes.push(Volume {
            name: HEADERS_VOLUME.into(),
            volume_type: "secret".into(),
            source_name: names.secret.clone(),
        });
        volume_mounts.push(VolumeMount {
            name: HEADERS_VOLUME.into(),
            mount_path: secrets::SECRETS_MOUNT_PATH.into(),
            read_only: true,
        });
    }
    let r = &settings.resources;
    Sidecar {
        name: SIDECAR_NAME.into(),
        image: settings.image.clone(),
        env: vec![EnvVar {
            name: CONFIG_HASH_ENV.into(),
            value: config_hash.into(),
        }],
        cpu_request: r.cpu_request.clone(),
        cpu_limit: r.cpu_limit.clone(),
        memory_request: r.memory_request.clone(),
        memory_limit: r.memory_limit.clone(),
        volumes,
        volume_mounts,
    }
}

fn object_labels() -> Value {
    json!({
        "app.kubernetes.io/managed-by": MANAGED_BY,
        "app.kubernetes.io/component": SIDECAR_NAME,
    })
}

/// The ConfigMap with the collector config and, when headers are set, the
/// Secret with their values. Both live in the workload's namespace; the
/// plugin never emits cluster-scoped objects or RBAC.
pub fn collector_objects(
    settings: &Settings,
    namespace: &str,
    names: &ObjectNames,
    config_yaml: &str,
) -> Vec<Value> {
    let labels = object_labels();
    let mut data = Map::new();
    data.insert(collector::CONFIG_FILE.into(), Value::from(config_yaml));
    let mut objects = vec![json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": { "name": names.config_map, "namespace": namespace, "labels": labels },
        "data": data,
    })];
    if !settings.exporter.headers.is_empty() {
        objects.push(secrets::secret_manifest(
            namespace,
            &names.secret,
            &labels,
            &settings.exporter.headers,
        ));
    }
    objects
}

/// Where the application's SDK sends telemetry: the sidecar's receiver.
fn app_endpoint(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Grpc => "http://localhost:4317",
        Protocol::HttpProtobuf => "http://localhost:4318",
    }
}

fn exporter_env(signal: &str) -> String {
    format!("OTEL_{}_EXPORTER", signal.to_ascii_uppercase())
}

fn resource_attributes<'a>(attrs: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    attrs
        .into_iter()
        .map(|(k, v)| format!("{k}={}", percent_encode(v)))
        .collect::<Vec<_>>()
        .join(",")
}

/// Env vars for a deployment's primary container. deckwatch skips any name
/// the container already defines, so values set by the user win.
pub fn deployment_env(
    settings: &Settings,
    namespace: &str,
    deployment: &str,
    application: Option<&str>,
) -> Vec<(String, String)> {
    let mut attrs = vec![
        ("k8s.namespace.name", namespace),
        ("k8s.deployment.name", deployment),
    ];
    if let Some(app) = application {
        attrs.push(("deckwatch.application.name", app));
    }
    let mut env = vec![
        (
            OTEL_EXPORTER_OTLP_ENDPOINT.to_string(),
            app_endpoint(settings.app_protocol).to_string(),
        ),
        (
            OTEL_EXPORTER_OTLP_PROTOCOL.to_string(),
            settings.app_protocol.as_env().to_string(),
        ),
        (OTEL_SERVICE_NAME.to_string(), deployment.to_string()),
        (
            OTEL_RESOURCE_ATTRIBUTES.to_string(),
            resource_attributes(attrs),
        ),
    ];
    // Signals the collector does not forward are switched off so the SDK does
    // not keep retrying an export the sidecar rejects.
    for (signal, enabled) in settings.signals.iter() {
        let exporter = if enabled { "otlp" } else { "none" };
        env.push((exporter_env(signal), exporter.to_string()));
    }
    env
}

/// Operator input on the application-level resource form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplicationFields {
    pub service_name: Option<String>,
    pub extra_attributes: Vec<(String, String)>,
}

impl ApplicationFields {
    pub fn parse(fields: &HashMap<String, String>) -> Result<Self, String> {
        let get = |key: &str| fields.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
        let service_name = match get(OTEL_SERVICE_NAME) {
            None => None,
            Some(name) if name.len() <= 255 && !name.chars().any(char::is_control) => {
                Some(name.to_string())
            }
            Some(_) => {
                return Err(format!(
                    "{OTEL_SERVICE_NAME} must be at most 255 characters with no control characters"
                ))
            }
        };
        let mut extra_attributes: Vec<(String, String)> = Vec::new();
        for pair in get(OTEL_RESOURCE_ATTRIBUTES)
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            let (key, value) = pair
                .split_once('=')
                .map(|(k, v)| (k.trim(), v.trim()))
                .ok_or_else(|| format!("{OTEL_RESOURCE_ATTRIBUTES} entries must be key=value"))?;
            let key_ok = !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
            // Baggage-octets: printable ASCII except space, '"', ',', ';', '\'.
            let value_ok = !value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b',' | b';' | b'\\'));
            if !key_ok || !value_ok {
                return Err(format!(
                    "{OTEL_RESOURCE_ATTRIBUTES} has an invalid entry; keys use letters, digits, \
                     '.', '_' or '-', and values must be %-encoded"
                ));
            }
            if MANAGED_ATTRIBUTES.contains(&key) {
                return Err(format!(
                    "{OTEL_RESOURCE_ATTRIBUTES} must not set {key}; the plugin sets it"
                ));
            }
            if extra_attributes.iter().any(|(k, _)| k == key) {
                return Err(format!("{OTEL_RESOURCE_ATTRIBUTES} repeats {key}"));
            }
            extra_attributes.push((key.to_string(), value.to_string()));
        }
        Ok(Self {
            service_name,
            extra_attributes,
        })
    }
}

/// Env vars deckwatch's reconciler sets on every deployment and cronjob of
/// the application.
///
/// The reconciler applies these with force, replacing a same-named value the
/// user set, and the values are the same for every workload. So this sets
/// only what the enabled pipelines need: no per-deployment service name
/// (unless the operator chose one for the whole application) and no
/// `OTEL_<SIGNAL>_EXPORTER=none` that could override a user's exporter.
pub fn application_state(
    settings: &Settings,
    namespace: &str,
    application: &str,
    fields: &ApplicationFields,
) -> BTreeMap<String, String> {
    let mut state = BTreeMap::new();
    state.insert(
        OTEL_EXPORTER_OTLP_ENDPOINT.to_string(),
        app_endpoint(settings.app_protocol).to_string(),
    );
    state.insert(
        OTEL_EXPORTER_OTLP_PROTOCOL.to_string(),
        settings.app_protocol.as_env().to_string(),
    );
    let attrs = [
        ("k8s.namespace.name", namespace),
        ("deckwatch.application.name", application),
    ];
    let mut attributes = resource_attributes(attrs);
    for (key, value) in &fields.extra_attributes {
        attributes.push_str(&format!(",{key}={value}"));
    }
    state.insert(OTEL_RESOURCE_ATTRIBUTES.to_string(), attributes);
    if let Some(name) = &fields.service_name {
        state.insert(OTEL_SERVICE_NAME.to_string(), name.clone());
    }
    for (signal, enabled) in settings.signals.iter() {
        if enabled {
            state.insert(exporter_env(signal), "otlp".to_string());
        }
    }
    state
}

/// The application-level resource shown on the application's
/// Infrastructure tab.
pub fn resource_decl() -> PluginResource {
    let field = |key: &str, label: &str, description: &str| ConfigField {
        key: key.into(),
        label: label.into(),
        description: description.into(),
        field_type: ConfigFieldType::String,
        default: None,
        required: false,
        options: vec![],
        env_source: None,
    };
    PluginResource {
        id: RESOURCE_ID.into(),
        label: "OpenTelemetry Collector".into(),
        icon: "mdi-chart-timeline-variant".into(),
        description: "Adds an OpenTelemetry Collector sidecar to every deployment and cronjob \
                      in this application and points OTEL_EXPORTER_OTLP_ENDPOINT at it. The \
                      exporter is configured in Settings → Plugins."
            .into(),
        singleton: true,
        fields: vec![
            field(
                OTEL_SERVICE_NAME,
                "Service name",
                "Optional OTEL_SERVICE_NAME for every workload in the application. Leave empty \
                 so each deployment keeps its own (deckwatch looks traces up by deployment \
                 name).",
            ),
            field(
                OTEL_RESOURCE_ATTRIBUTES,
                "Extra resource attributes",
                "Optional key=value pairs, comma-separated, added to OTEL_RESOURCE_ATTRIBUTES \
                 after k8s.namespace.name and deckwatch.application.name. Values must be \
                 %-encoded.",
            ),
        ],
        output_keys: vec![
            OTEL_EXPORTER_OTLP_ENDPOINT.into(),
            OTEL_EXPORTER_OTLP_PROTOCOL.into(),
            OTEL_RESOURCE_ATTRIBUTES.into(),
            "OTEL_TRACES_EXPORTER".into(),
            OTEL_SERVICE_NAME.into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(annotations: &[(&str, &str)], labels: &[(&str, &str)]) -> PluginContext {
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        PluginContext {
            namespace: "shop".into(),
            deployment_name: "checkout".into(),
            annotations: map(annotations),
            labels: map(labels),
            plugin_outputs: HashMap::new(),
            provisioned_resources: HashMap::new(),
            ..Default::default()
        }
    }

    #[test]
    fn enablement() {
        use Enablement::*;
        let e = |a: &[(&str, &str)], l: &[(&str, &str)]| deployment_enablement(&ctx(a, l)).0;
        assert_eq!(e(&[], &[]), NotRequested);
        assert_eq!(e(&[(ENABLE_KEY, "enabled")], &[]), Enabled);
        assert_eq!(e(&[(ENABLE_KEY, " TRUE ")], &[]), Enabled);
        assert_eq!(e(&[], &[(ENABLE_KEY, "enabled")]), Enabled);
        assert_eq!(
            e(&[(ENABLE_KEY, "disabled")], &[(ENABLE_KEY, "enabled")]),
            OptedOut
        );
        assert_eq!(
            e(&[(ENABLE_KEY, "enabled")], &[(ENABLE_KEY, "false")]),
            OptedOut
        );
        assert_eq!(
            e(
                &[(ENABLE_KEY, "enabled"), (APPLICATION_ANNOTATION, "shop")],
                &[]
            ),
            ManagedByApplication
        );
        assert_eq!(e(&[(ENABLE_KEY, "yes")], &[]), NotRequested);
    }

    #[test]
    fn enablement_warnings() {
        let (_, warnings) = deployment_enablement(&ctx(&[(ENABLE_KEY, "on")], &[]));
        assert!(warnings[0].contains("use \"enabled\""));
        let (_, warnings) = deployment_enablement(&ctx(
            &[(ENABLE_KEY, "enabled"), (APPLICATION_ANNOTATION, "shop")],
            &[],
        ));
        assert!(warnings[0].contains("application"));
    }

    #[test]
    fn object_names_fit_kubernetes_limits() {
        let names = object_names("checkout");
        assert_eq!(names.config_map, "checkout-otelcol");
        assert_eq!(names.secret, "checkout-otelcol-headers");
        let long = format!("{}-x", "a".repeat(250));
        let names = object_names(&long);
        for name in [&names.config_map, &names.secret] {
            assert!(name.len() <= 253, "{name}");
            assert!(crate::validate::is_dns_subdomain(name), "{name}");
        }
        assert_ne!(
            object_names(&long),
            object_names(&format!("{}-y", "a".repeat(250)))
        );
    }

    #[test]
    fn application_fields() {
        let fields = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            ApplicationFields::parse(&fields(&[])).unwrap(),
            ApplicationFields::default()
        );
        let parsed = ApplicationFields::parse(&fields(&[
            (OTEL_SERVICE_NAME, " shop "),
            (
                OTEL_RESOURCE_ATTRIBUTES,
                "deployment.environment=prod, team=payments",
            ),
        ]))
        .unwrap();
        assert_eq!(parsed.service_name.as_deref(), Some("shop"));
        assert_eq!(
            parsed.extra_attributes,
            vec![
                ("deployment.environment".to_string(), "prod".to_string()),
                ("team".to_string(), "payments".to_string())
            ]
        );
        for bad in [
            "novalue",
            "k8s.namespace.name=other",
            "a=1,a=2",
            "a=has space",
            "a=\"q\"",
            "bad key=1",
        ] {
            assert!(
                ApplicationFields::parse(&fields(&[(OTEL_RESOURCE_ATTRIBUTES, bad)])).is_err(),
                "{bad}"
            );
        }
        assert!(ApplicationFields::parse(&fields(&[(OTEL_SERVICE_NAME, "a\nb")])).is_err());
    }

    #[test]
    fn resource_declaration() {
        let r = resource_decl();
        assert_eq!(r.id, "tracing");
        assert!(r.singleton);
        for f in &r.fields {
            assert!(!f.label.is_empty() && !f.description.is_empty());
            assert_ne!(
                f.field_type,
                ConfigFieldType::Secret,
                "resource fields are stored in clear"
            );
        }
    }
}
