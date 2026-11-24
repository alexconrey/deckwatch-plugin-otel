//! End-to-end tests of the plugin's outputs: the pod-spec patch for enable
//! and disable, idempotency, the application-level resource, secret handling,
//! and compatibility with deckwatch's plugin types.

use std::collections::{BTreeMap, HashMap};

use deckwatch_plugin_sdk::{PluginContext, ResourceDeprovisionRequest, ResourceProvisionRequest};
use serde_json::{json, Value};

use crate::host_sim::{self, Deployment};
use crate::inject::{APPLICATION_ANNOTATION, APPLICATION_LABEL, ENABLE_KEY};
use crate::settings::{
    RawConfig, COLLECTOR_IMAGE, OTLP_ENDPOINT, OTLP_HEADERS, OTLP_INSECURE, SIGNALS,
};
use crate::{apply_inner, deprovision_inner, metadata_value, provision_inner, PLUGIN_NAME};

const HEADER_SECRET: &str = "hny_s3cr3t-token";

fn config(extra: &[(&str, &str)]) -> RawConfig {
    let mut pairs = vec![
        (OTLP_ENDPOINT, "tempo.observability.svc:4317"),
        (OTLP_INSECURE, "true"),
    ];
    pairs.extend_from_slice(extra);
    pairs.into_iter().collect()
}

fn ctx(annotations: &[(&str, &str)]) -> PluginContext {
    PluginContext {
        namespace: "shop".into(),
        deployment_name: "checkout".into(),
        annotations: annotations
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        labels: HashMap::from([("app".to_string(), "checkout".to_string())]),
        plugin_outputs: HashMap::new(),
        provisioned_resources: HashMap::new(),
    }
}

fn enabled() -> PluginContext {
    ctx(&[(ENABLE_KEY, "enabled")])
}

/// A Deployment as deckwatch creates it, with one env var set by the user.
fn deployment() -> Deployment {
    Deployment {
        annotations: BTreeMap::from([(ENABLE_KEY.to_string(), "enabled".to_string())]),
        pod: json!({
            "containers": [{
                "name": "checkout",
                "image": "registry.local/shop/checkout:1.4.2",
                "env": [
                    { "name": "LOG_LEVEL", "value": "info" },
                    { "name": "OTEL_SERVICE_NAME", "value": "checkout-svc" },
                ],
            }],
        }),
    }
}

fn env_of(container: &Value) -> BTreeMap<String, String> {
    container["env"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn provision_req(fields: &[(&str, &str)]) -> ResourceProvisionRequest {
    ResourceProvisionRequest {
        application_name: "shop-web".into(),
        namespace: "shop".into(),
        resource_id: "tracing".into(),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

// ── Deployment level: the pod-spec patch ─────────────────────────────────────

#[test]
fn enable_adds_sidecar_env_and_volumes() {
    let result = apply_inner(&enabled(), &config(&[]));
    assert!(result.get("errors").is_none(), "{result}");
    let mut dep = deployment();
    host_sim::merge(&mut dep, PLUGIN_NAME, &result);

    let containers = dep.pod["containers"].as_array().unwrap();
    assert_eq!(containers.len(), 2);
    let sidecar = &containers[1];
    assert_eq!(sidecar["name"], "otel-collector");
    assert_eq!(sidecar["image"], crate::settings::DEFAULT_IMAGE);
    assert!(
        sidecar.get("ports").is_none(),
        "receivers are loopback-only"
    );
    assert_eq!(
        sidecar["resources"],
        json!({
            "requests": { "cpu": "100m", "memory": "128Mi" },
            "limits": { "cpu": "500m", "memory": "256Mi" },
        })
    );
    assert_eq!(
        sidecar["volumeMounts"],
        json!([{ "name": "otelcol-config", "mountPath": "/etc/otelcol-contrib", "readOnly": true }])
    );
    let sidecar_env = env_of(sidecar);
    assert_eq!(sidecar_env.len(), 1);
    assert_eq!(sidecar_env["OTELCOL_CONFIG_SHA256"].len(), 64);
    assert_eq!(
        dep.pod["volumes"],
        json!([{ "name": "otelcol-config", "configMap": { "name": "checkout-otelcol" } }])
    );

    let env = env_of(&containers[0]);
    assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://localhost:4317");
    assert_eq!(env["OTEL_EXPORTER_OTLP_PROTOCOL"], "grpc");
    assert_eq!(
        env["OTEL_RESOURCE_ATTRIBUTES"],
        "k8s.namespace.name=shop,k8s.deployment.name=checkout"
    );
    assert_eq!(env["OTEL_TRACES_EXPORTER"], "otlp");
    assert_eq!(env["OTEL_METRICS_EXPORTER"], "none");
    assert_eq!(env["OTEL_LOGS_EXPORTER"], "none");
    assert_eq!(env["LOG_LEVEL"], "info");

    assert_eq!(
        dep.annotations["deckwatch.plugin-sidecar/otel"],
        "otel-collector"
    );
}

#[test]
fn enable_keeps_values_the_user_set() {
    let result = apply_inner(&enabled(), &config(&[]));
    let mut dep = deployment();
    host_sim::merge(&mut dep, PLUGIN_NAME, &result);
    let env = env_of(&dep.pod["containers"][0]);
    assert_eq!(env["OTEL_SERVICE_NAME"], "checkout-svc");
    // Only names deckwatch actually added are recorded for removal.
    let recorded = &dep.annotations["deckwatch.plugin-env/otel"];
    assert!(!recorded.split(',').any(|n| n == "OTEL_SERVICE_NAME"));
    assert!(recorded
        .split(',')
        .any(|n| n == "OTEL_EXPORTER_OTLP_ENDPOINT"));
}

#[test]
fn service_name_defaults_to_deployment_name() {
    let result = apply_inner(&enabled(), &config(&[]));
    let mut dep = deployment();
    dep.pod["containers"][0]["env"] = json!([]);
    host_sim::merge(&mut dep, PLUGIN_NAME, &result);
    assert_eq!(
        env_of(&dep.pod["containers"][0])["OTEL_SERVICE_NAME"],
        "checkout"
    );
}

#[test]
fn application_name_from_pod_label() {
    let mut c = enabled();
    c.labels.insert(APPLICATION_LABEL.into(), "shop-web".into());
    let result = apply_inner(&c, &config(&[]));
    let attrs = result["env_vars"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "OTEL_RESOURCE_ATTRIBUTES")
        .unwrap()["value"]
        .clone();
    assert_eq!(
        attrs,
        "k8s.namespace.name=shop,k8s.deployment.name=checkout,deckwatch.application.name=shop-web"
    );
}

#[test]
fn http_protobuf_app_protocol() {
    let result = apply_inner(
        &enabled(),
        &config(&[("APP_OTLP_PROTOCOL", "http/protobuf")]),
    );
    let mut dep = deployment();
    host_sim::merge(&mut dep, PLUGIN_NAME, &result);
    let env = env_of(&dep.pod["containers"][0]);
    assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://localhost:4318");
    assert_eq!(env["OTEL_EXPORTER_OTLP_PROTOCOL"], "http/protobuf");
}

#[test]
fn enable_is_idempotent() {
    let first = apply_inner(&enabled(), &config(&[]));
    let mut dep = deployment();
    host_sim::merge(&mut dep, PLUGIN_NAME, &first);
    let after_first = dep.clone();

    // deckwatch re-runs the plugin on every update, with the live annotations.
    let mut annotations: Vec<(&str, &str)> = after_first
        .annotations
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    annotations.sort();
    let second = apply_inner(&ctx(&annotations), &config(&[]));
    assert_eq!(first, second, "same input must give byte-identical output");
    host_sim::merge(&mut dep, PLUGIN_NAME, &second);
    assert_eq!(dep, after_first, "re-applying must not change the pod spec");
}

#[test]
fn config_change_changes_the_pod_template() {
    let a = apply_inner(&enabled(), &config(&[]));
    let b = apply_inner(&enabled(), &config(&[("BATCH_TIMEOUT", "1s")]));
    assert_ne!(
        a["sidecars"][0]["env"], b["sidecars"][0]["env"],
        "the config hash must change so the pods roll"
    );
}

#[test]
fn not_enabled_injects_nothing() {
    for annotations in [
        vec![],
        vec![(ENABLE_KEY, "disabled")],
        vec![(ENABLE_KEY, "on")],
        vec![
            (ENABLE_KEY, "enabled"),
            (APPLICATION_ANNOTATION, "shop-web"),
        ],
    ] {
        let result = apply_inner(&ctx(&annotations), &config(&[]));
        assert_eq!(result["sidecars"], json!([]), "{annotations:?}");
        assert_eq!(result["env_vars"], json!([]));
        assert_eq!(result["kubernetes_resources"], json!([]));
        let mut dep = deployment();
        let before = dep.clone();
        host_sim::merge(&mut dep, PLUGIN_NAME, &result);
        assert_eq!(dep, before);
    }
}

#[test]
fn disable_removes_everything_enable_added() {
    // With headers, so the Secret volume is part of the round trip.
    let cfg = config(&[(
        OTLP_HEADERS,
        &format!("authorization=Bearer {HEADER_SECRET}"),
    )]);
    let original = deployment();
    let mut dep = original.clone();
    host_sim::merge(&mut dep, PLUGIN_NAME, &apply_inner(&enabled(), &cfg));
    assert_eq!(dep.pod["volumes"].as_array().unwrap().len(), 2);

    // Disabled: the plugin returns nothing, and deckwatch has to undo the
    // previous injection from its records (proposed host change; see
    // host_sim). The result is the original Deployment, user values intact.
    dep.annotations.insert(ENABLE_KEY.into(), "disabled".into());
    let disabled = apply_inner(&ctx(&[(ENABLE_KEY, "disabled")]), &cfg);
    host_sim::unmerge(&mut dep, PLUGIN_NAME);
    host_sim::merge(&mut dep, PLUGIN_NAME, &disabled);
    let mut expected = original;
    expected
        .annotations
        .insert(ENABLE_KEY.into(), "disabled".into());
    assert_eq!(dep, expected);
}

#[test]
fn reapply_after_config_change_replaces_the_sidecar() {
    // Proposed host behaviour: undo then merge on every update, so a new
    // config hash (or image) reaches the pod spec.
    let mut dep = deployment();
    host_sim::merge(
        &mut dep,
        PLUGIN_NAME,
        &apply_inner(&enabled(), &config(&[])),
    );
    let changed = apply_inner(&enabled(), &config(&[("BATCH_TIMEOUT", "1s")]));
    host_sim::unmerge(&mut dep, PLUGIN_NAME);
    host_sim::merge(&mut dep, PLUGIN_NAME, &changed);
    assert_eq!(dep.pod["containers"].as_array().unwrap().len(), 2);
    assert_eq!(
        dep.pod["containers"][1]["env"][0]["value"],
        changed["sidecars"][0]["env"][0]["value"]
    );
    assert_eq!(
        env_of(&dep.pod["containers"][0])["OTEL_SERVICE_NAME"],
        "checkout-svc"
    );
}

#[test]
fn invalid_config_injects_nothing_and_reports_errors() {
    let raw: RawConfig = [(
        COLLECTOR_IMAGE,
        "otel/opentelemetry-collector-contrib:latest",
    )]
    .into_iter()
    .collect();
    let result = apply_inner(&enabled(), &raw);
    assert_eq!(result["sidecars"], json!([]));
    assert_eq!(result["env_vars"], json!([]));
    assert_eq!(result["kubernetes_resources"], json!([]));
    let errors: Vec<&str> = result["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e.as_str().unwrap())
        .collect();
    assert!(errors
        .iter()
        .any(|e| e.contains("OTLP_ENDPOINT is required")));
    assert!(errors.iter().any(|e| e.contains("COLLECTOR_IMAGE")));
}

#[test]
fn objects_are_namespaced_configmap_and_secret_only() {
    let cfg = config(&[(
        OTLP_HEADERS,
        &format!("authorization=Bearer {HEADER_SECRET}"),
    )]);
    let result = apply_inner(&enabled(), &cfg);
    let objects = result["kubernetes_resources"].as_array().unwrap();
    let kinds: Vec<&str> = objects
        .iter()
        .map(|o| o["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["ConfigMap", "Secret"]);
    for o in objects {
        assert_eq!(o["apiVersion"], "v1");
        assert_eq!(o["metadata"]["namespace"], "shop");
        assert_eq!(
            o["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "deckwatch-plugin-otel"
        );
        assert!(o["metadata"].get("ownerReferences").is_none());
    }
    assert_eq!(objects[0]["metadata"]["name"], "checkout-otelcol");
    assert_eq!(objects[1]["metadata"]["name"], "checkout-otelcol-headers");
    let config_yaml = objects[0]["data"]["config.yaml"].as_str().unwrap();
    assert!(config_yaml.contains("127.0.0.1:4317"));
}

// ── Secret handling ──────────────────────────────────────────────────────────

#[test]
fn header_values_only_exist_inside_the_secret() {
    let cfg = config(&[(
        OTLP_HEADERS,
        &format!("authorization=Bearer%20{HEADER_SECRET},x-scope-orgid=tenant-a"),
    )]);
    let apply = apply_inner(&enabled(), &cfg);
    let provision = provision_inner(&provision_req(&[]), &cfg).unwrap();

    for (what, output) in [("apply", &apply), ("provision", &provision)] {
        // The Secret's base64 data is the only place the value may appear.
        let mut without_secret = output.clone();
        without_secret["kubernetes_resources"]
            .as_array_mut()
            .unwrap()
            .retain(|o| o["kind"] != "Secret");
        let text = without_secret.to_string();
        assert!(
            !text.contains(HEADER_SECRET),
            "{what} leaks the header value"
        );
        assert!(!text.contains("tenant-a"), "{what} leaks the header value");
        assert!(
            !output.to_string().contains(HEADER_SECRET),
            "{what}: not even in the Secret in clear"
        );

        let secret = output["kubernetes_resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["kind"] == "Secret")
            .unwrap();
        let decoded = |key: &str| {
            use base64::Engine;
            let b64 = secret["data"][key].as_str().unwrap();
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .unwrap(),
            )
            .unwrap()
        };
        assert_eq!(decoded("authorization"), format!("Bearer {HEADER_SECRET}"));
        assert_eq!(decoded("x-scope-orgid"), "tenant-a");

        let sidecar = &output["sidecars"][0];
        assert!(sidecar["volumes"]
            .as_array()
            .unwrap()
            .contains(&json!({ "name": "otelcol-headers", "volume_type": "secret", "source_name": secret["metadata"]["name"] })));
        assert!(sidecar["volume_mounts"]
            .as_array()
            .unwrap()
            .contains(&json!({ "name": "otelcol-headers", "mount_path": "/etc/otelcol-secrets", "read_only": true })));
        // No env var is sourced from the Secret: deckwatch's application
        // reconciler would drop valueFrom.
        assert!(!sidecar.to_string().contains("value_from"));
    }
}

#[test]
fn encrypted_header_value_is_refused() {
    use base64::Engine;
    let ciphertext = base64::engine::general_purpose::STANDARD.encode([42u8; 60]);
    let cfg = config(&[(OTLP_HEADERS, &ciphertext)]);
    let result = apply_inner(&enabled(), &cfg);
    assert_eq!(result["sidecars"], json!([]));
    assert!(result["errors"][0]
        .as_str()
        .unwrap()
        .contains("encrypted by deckwatch"));
    assert!(!result.to_string().contains(&ciphertext));
    let err = provision_inner(&provision_req(&[]), &cfg).unwrap_err();
    assert!(err.contains("encrypted by deckwatch"));
}

// ── Application level ────────────────────────────────────────────────────────

#[test]
fn provision_application_level() {
    let result = provision_inner(&provision_req(&[]), &config(&[])).unwrap();
    assert_eq!(result["status"], "Available");
    assert_eq!(
        result["state"],
        json!({
            "OTEL_EXPORTER_OTLP_ENDPOINT": "http://localhost:4317",
            "OTEL_EXPORTER_OTLP_PROTOCOL": "grpc",
            "OTEL_RESOURCE_ATTRIBUTES": "k8s.namespace.name=shop,deckwatch.application.name=shop-web",
            "OTEL_TRACES_EXPORTER": "otlp",
        })
    );
    assert_eq!(
        result["deployment_annotations"],
        json!({ APPLICATION_ANNOTATION: "shop-web" })
    );
    let sidecars = result["sidecars"].as_array().unwrap();
    assert_eq!(sidecars.len(), 1);
    assert_eq!(sidecars[0]["name"], "otel-collector");
    assert_eq!(
        sidecars[0]["volumes"][0],
        json!({ "name": "otelcol-config", "volume_type": "configMap", "source_name": "shop-web-otelcol" })
    );
    let objects = result["kubernetes_resources"].as_array().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["metadata"]["name"], "shop-web-otelcol");
    assert_eq!(objects[0]["metadata"]["namespace"], "shop");
}

#[test]
fn provision_fields_and_signals() {
    let result = provision_inner(
        &provision_req(&[
            ("OTEL_SERVICE_NAME", "shop"),
            ("OTEL_RESOURCE_ATTRIBUTES", "deployment.environment=prod"),
        ]),
        &config(&[(SIGNALS, "traces,logs")]),
    )
    .unwrap();
    let state = &result["state"];
    assert_eq!(state["OTEL_SERVICE_NAME"], "shop");
    assert_eq!(
        state["OTEL_RESOURCE_ATTRIBUTES"],
        "k8s.namespace.name=shop,deckwatch.application.name=shop-web,deployment.environment=prod"
    );
    assert_eq!(state["OTEL_LOGS_EXPORTER"], "otlp");
    // Never "none": the reconciler would force it over a user's own exporter.
    assert!(state.get("OTEL_METRICS_EXPORTER").is_none());
}

#[test]
fn provision_rejects_invalid_input() {
    let err = provision_inner(&provision_req(&[]), &RawConfig::default()).unwrap_err();
    assert!(err.contains("OTLP_ENDPOINT is required"));
    let mut req = provision_req(&[]);
    req.resource_id = "cache".into();
    assert!(provision_inner(&req, &config(&[])).is_err());
    let err = provision_inner(
        &provision_req(&[("OTEL_RESOURCE_ATTRIBUTES", "k8s.namespace.name=x")]),
        &config(&[]),
    )
    .unwrap_err();
    assert!(err.contains("k8s.namespace.name"));
    let mut req = provision_req(&[]);
    req.application_name = "Bad_Name".into();
    assert!(provision_inner(&req, &config(&[])).is_err());
}

#[test]
fn provision_is_deterministic() {
    let cfg = config(&[(OTLP_HEADERS, "a=1,b=2")]);
    assert_eq!(
        provision_inner(&provision_req(&[]), &cfg).unwrap(),
        provision_inner(&provision_req(&[]), &cfg).unwrap()
    );
}

#[test]
fn deprovision_names_what_is_left_behind() {
    let result = deprovision_inner(&ResourceDeprovisionRequest {
        application_name: "shop-web".into(),
        namespace: "shop".into(),
        resource_id: "tracing".into(),
        state: HashMap::new(),
        fields: HashMap::new(),
    });
    assert!(result.errors.is_empty());
    assert!(result.message.contains("shop-web-otelcol"));
    assert!(result.message.contains("shop-web-otelcol-headers"));
    assert!(result.message.contains("'shop'"));
}

// ── Compatibility with deckwatch's plugin types ──────────────────────────────
//
// deckwatch deserializes plugin output into its own copy of the SDK types
// (src/plugins.rs). These mirror them at deckwatch 45e6a50 (0.6.7+), with
// deny_unknown_fields added so a field deckwatch would silently ignore fails
// the test instead.

mod deckwatch_types {
    #![allow(dead_code)]
    use std::collections::HashMap;

    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PluginResult {
        #[serde(default)]
        pub env_vars: Vec<EnvVarSpec>,
        #[serde(default)]
        pub sidecars: Vec<SidecarSpec>,
        #[serde(default)]
        pub kubernetes_resources: Vec<Value>,
        #[serde(default)]
        pub service_account_name: Option<String>,
        #[serde(default)]
        pub wants_service_account: Option<Value>,
        #[serde(default)]
        pub outputs: HashMap<String, String>,
        #[serde(default)]
        pub deployment_annotations: HashMap<String, String>,
        #[serde(default)]
        pub deployment_labels: HashMap<String, String>,
        #[serde(default)]
        pub init_containers: Vec<SidecarSpec>,
        #[serde(default)]
        pub errors: Vec<String>,
        #[serde(default)]
        pub warnings: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct EnvVarSpec {
        pub name: String,
        #[serde(default)]
        pub value: String,
        #[serde(default)]
        pub value_from: Option<Value>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct VolumeSpec {
        pub name: String,
        pub volume_type: String,
        #[serde(default)]
        pub source_name: Option<String>,
        #[serde(default)]
        pub size_limit: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct VolumeMountSpec {
        pub name: String,
        pub mount_path: String,
        #[serde(default)]
        pub read_only: bool,
        #[serde(default)]
        pub sub_path: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SidecarSpec {
        pub name: String,
        pub image: String,
        #[serde(default)]
        pub env: Vec<EnvVarSpec>,
        #[serde(default)]
        pub port: Option<i32>,
        #[serde(default)]
        pub cpu_request: Option<String>,
        #[serde(default)]
        pub cpu_limit: Option<String>,
        #[serde(default)]
        pub memory_request: Option<String>,
        #[serde(default)]
        pub memory_limit: Option<String>,
        #[serde(default)]
        pub cpu: Option<String>,
        #[serde(default)]
        pub memory: Option<String>,
        #[serde(default)]
        pub volumes: Vec<VolumeSpec>,
        #[serde(default)]
        pub volume_mounts: Vec<VolumeMountSpec>,
        #[serde(default)]
        pub primary_volume_mounts: Vec<VolumeMountSpec>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    pub enum ProvisioningStatus {
        Pending,
        Available,
        Failed,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ResourceProvisionResult {
        #[serde(default)]
        pub state: HashMap<String, String>,
        #[serde(default)]
        pub deployment_annotations: HashMap<String, String>,
        #[serde(default)]
        pub sidecars: Vec<SidecarSpec>,
        #[serde(default)]
        pub init_containers: Vec<SidecarSpec>,
        #[serde(default)]
        pub kubernetes_resources: Vec<Value>,
        #[serde(default)]
        pub errors: Vec<String>,
        #[serde(default)]
        pub warnings: Vec<String>,
        pub status: ProvisioningStatus,
        #[serde(default)]
        pub retry_after_secs: Option<u64>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum ConfigFieldType {
        String,
        Secret,
        Bool,
        Select,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ConfigField {
        pub key: String,
        pub label: String,
        #[serde(default)]
        pub description: String,
        pub field_type: ConfigFieldType,
        #[serde(default)]
        pub default: Option<String>,
        #[serde(default)]
        pub required: bool,
        #[serde(default)]
        pub options: Vec<String>,
        #[serde(default)]
        pub env_source: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PluginResource {
        pub id: String,
        pub label: String,
        #[serde(default)]
        pub icon: String,
        #[serde(default)]
        pub description: String,
        #[serde(default)]
        pub singleton: bool,
        #[serde(default)]
        pub fields: Vec<ConfigField>,
        #[serde(default)]
        pub output_keys: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PluginMetadata {
        pub name: String,
        #[serde(default)]
        pub version: String,
        #[serde(default)]
        pub description: String,
        #[serde(default)]
        pub provides: Vec<String>,
        #[serde(default)]
        pub depends_on: Vec<String>,
        #[serde(default)]
        pub optional_depends_on: Vec<String>,
        #[serde(default)]
        pub config_schema: Vec<ConfigField>,
        #[serde(default)]
        pub resources: Vec<PluginResource>,
        #[serde(default)]
        pub min_deckwatch_version: Option<String>,
        #[serde(default)]
        pub mcp_tuning_fields: Vec<Value>,
    }
}

#[test]
fn apply_output_matches_deckwatch_types() {
    let cfg = config(&[(OTLP_HEADERS, "authorization=Bearer x")]);
    let out: deckwatch_types::PluginResult =
        serde_json::from_value(apply_inner(&enabled(), &cfg)).unwrap();
    assert_eq!(out.sidecars.len(), 1);
    let sidecar = &out.sidecars[0];
    assert_eq!(sidecar.cpu_request.as_deref(), Some("100m"));
    assert_eq!(sidecar.memory_limit.as_deref(), Some("256Mi"));
    assert!(sidecar.cpu.is_none() && sidecar.memory.is_none() && sidecar.port.is_none());
    assert_eq!(sidecar.volumes.len(), 2);
    assert!(sidecar.volume_mounts.iter().all(|m| m.read_only));
    assert_eq!(out.env_vars.len(), 7);
    assert!(out
        .env_vars
        .iter()
        .all(|e| e.value_from.is_none() && !e.value.is_empty()));
    assert_eq!(out.kubernetes_resources.len(), 2);
}

#[test]
fn provision_output_matches_deckwatch_types() {
    let out: deckwatch_types::ResourceProvisionResult =
        serde_json::from_value(provision_inner(&provision_req(&[]), &config(&[])).unwrap())
            .unwrap();
    assert_eq!(out.status, deckwatch_types::ProvisioningStatus::Available);
    assert_eq!(out.sidecars.len(), 1);
    assert!(out.init_containers.is_empty());
}

#[test]
fn metadata_matches_deckwatch_types() {
    let meta: deckwatch_types::PluginMetadata = serde_json::from_value(metadata_value()).unwrap();
    assert_eq!(meta.name, "otel");
    assert_eq!(meta.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(meta.min_deckwatch_version.as_deref(), Some("0.6.4"));
    assert_eq!(meta.config_schema.len(), crate::settings::ALL_KEYS.len());
    let secret_fields: Vec<&str> = meta
        .config_schema
        .iter()
        .filter(|f| f.field_type == deckwatch_types::ConfigFieldType::Secret)
        .map(|f| f.key.as_str())
        .collect();
    assert_eq!(secret_fields, ["OTLP_HEADERS"]);
    assert_eq!(meta.resources.len(), 1);
    assert_eq!(meta.resources[0].id, "tracing");
    assert!(meta.depends_on.is_empty());
}
