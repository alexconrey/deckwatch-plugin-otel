//! The collector configuration (`config.yaml`) the sidecar runs with.

use sha2::{Digest, Sha256};

use crate::secrets;
use crate::settings::{Protocol, Settings};

/// OTLP receivers listen on loopback only. The application container shares
/// the pod's network namespace and can reach them; other pods cannot.
pub const GRPC_RECEIVER: &str = "127.0.0.1:4317";
pub const HTTP_RECEIVER: &str = "127.0.0.1:4318";

/// Key in the ConfigMap, and file name in the mounted config directory.
pub const CONFIG_FILE: &str = "config.yaml";

/// Render `config.yaml`.
///
/// The structure is fixed; operator-supplied strings are emitted as
/// JSON-quoted scalars (valid YAML double-quoted scalars), so a value cannot
/// change the document's structure. Header values are not in the file: each
/// header refers to its file in the mounted Secret. Output is deterministic,
/// so identical settings always produce an identical ConfigMap.
pub fn render(settings: &Settings) -> String {
    let exporter = exporter_name(settings.exporter.protocol);
    let ml = &settings.memory_limiter;
    let batch = &settings.batch;
    let mut y = String::from("# Managed by deckwatch-plugin-otel. Changes are overwritten.\n");

    y.push_str("receivers:\n  otlp:\n    protocols:\n");
    y.push_str(&format!(
        "      grpc:\n        endpoint: {}\n",
        q(GRPC_RECEIVER)
    ));
    y.push_str(&format!(
        "      http:\n        endpoint: {}\n",
        q(HTTP_RECEIVER)
    ));

    y.push_str("processors:\n  memory_limiter:\n");
    y.push_str(&format!("    check_interval: {}\n", q(&ml.check_interval)));
    y.push_str(&format!("    limit_percentage: {}\n", ml.limit_percentage));
    y.push_str(&format!(
        "    spike_limit_percentage: {}\n",
        ml.spike_limit_percentage
    ));
    y.push_str("  batch:\n");
    y.push_str(&format!("    timeout: {}\n", q(&batch.timeout)));
    y.push_str(&format!("    send_batch_size: {}\n", batch.send_batch_size));
    if let Some(max) = batch.send_batch_max_size {
        y.push_str(&format!("    send_batch_max_size: {max}\n"));
    }

    y.push_str(&format!("exporters:\n  {exporter}:\n"));
    y.push_str(&format!(
        "    endpoint: {}\n",
        q(&settings.exporter.endpoint)
    ));
    if settings.exporter.protocol == Protocol::Grpc {
        // For otlp_http the endpoint scheme decides; for gRPC this flag does.
        y.push_str(&format!(
            "    tls:\n      insecure: {}\n",
            settings.exporter.insecure
        ));
    }
    if !settings.exporter.headers.is_empty() {
        y.push_str("    headers:\n");
        for h in &settings.exporter.headers {
            let reference = format!("${{file:{}}}", secrets::header_file(&h.name));
            y.push_str(&format!("      {}: {}\n", q(&h.name), q(&reference)));
        }
    }

    y.push_str("service:\n");
    // Internal metrics would listen on localhost:8888 and could collide with
    // an application port in the shared network namespace.
    y.push_str("  telemetry:\n    metrics:\n      level: none\n");
    y.push_str("  pipelines:\n");
    for (signal, enabled) in settings.signals.iter() {
        if enabled {
            y.push_str(&format!(
                "    {signal}:\n      receivers: [otlp]\n      processors: [memory_limiter, batch]\n      exporters: [{exporter}]\n"
            ));
        }
    }
    y
}

/// Component name of the exporter (collector 0.144.0 renamed `otlp` and
/// `otlphttp` to these).
pub fn exporter_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Grpc => "otlp_grpc",
        Protocol::HttpProtobuf => "otlp_http",
    }
}

fn q(s: &str) -> String {
    serde_json::to_string(s).expect("a string always serializes")
}

/// Lowercase hex SHA-256 of the rendered config. It goes on the sidecar as an
/// env var so a config change changes the pod template and rolls the pods.
pub fn config_hash(rendered: &str) -> String {
    Sha256::digest(rendered.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{RawConfig, Settings, OTLP_ENDPOINT, OTLP_HEADERS, OTLP_INSECURE};
    use yaml_rust2::{Yaml, YamlLoader};

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        let raw: RawConfig = pairs.iter().copied().collect();
        Settings::parse(&raw).unwrap().settings
    }

    fn yaml(s: &Settings) -> Yaml {
        YamlLoader::load_from_str(&render(s)).unwrap().remove(0)
    }

    fn base() -> Vec<(&'static str, &'static str)> {
        vec![
            (OTLP_ENDPOINT, "tempo.observability.svc:4317"),
            (OTLP_INSECURE, "true"),
        ]
    }

    #[test]
    fn receivers_bind_loopback_only() {
        let doc = yaml(&settings(&base()));
        let protocols = &doc["receivers"]["otlp"]["protocols"];
        assert_eq!(
            protocols["grpc"]["endpoint"].as_str(),
            Some("127.0.0.1:4317")
        );
        assert_eq!(
            protocols["http"]["endpoint"].as_str(),
            Some("127.0.0.1:4318")
        );
        assert!(!render(&settings(&base())).contains("0.0.0.0"));
    }

    #[test]
    fn default_pipeline() {
        let doc = yaml(&settings(&base()));
        let ml = &doc["processors"]["memory_limiter"];
        assert_eq!(ml["check_interval"].as_str(), Some("1s"));
        assert_eq!(ml["limit_percentage"].as_i64(), Some(80));
        assert_eq!(ml["spike_limit_percentage"].as_i64(), Some(25));
        let batch = &doc["processors"]["batch"];
        assert_eq!(batch["timeout"].as_str(), Some("200ms"));
        assert_eq!(batch["send_batch_size"].as_i64(), Some(8192));
        assert!(batch["send_batch_max_size"].is_badvalue());

        let exporter = &doc["exporters"]["otlp_grpc"];
        assert_eq!(
            exporter["endpoint"].as_str(),
            Some("tempo.observability.svc:4317")
        );
        assert_eq!(exporter["tls"]["insecure"].as_bool(), Some(true));
        assert!(exporter["headers"].is_badvalue());

        let pipelines = doc["service"]["pipelines"].as_hash().unwrap();
        assert_eq!(pipelines.len(), 1);
        let traces = &doc["service"]["pipelines"]["traces"];
        let names = |y: &Yaml| -> Vec<String> {
            y.as_vec()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(&traces["receivers"]), ["otlp"]);
        // memory_limiter must run before batch.
        assert_eq!(names(&traces["processors"]), ["memory_limiter", "batch"]);
        assert_eq!(names(&traces["exporters"]), ["otlp_grpc"]);
        assert_eq!(
            doc["service"]["telemetry"]["metrics"]["level"].as_str(),
            Some("none")
        );
    }

    #[test]
    fn tls_on_by_default_for_grpc() {
        let doc = yaml(&settings(&[(OTLP_ENDPOINT, "otlp.example.com:443")]));
        assert_eq!(
            doc["exporters"]["otlp_grpc"]["tls"]["insecure"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn http_exporter_and_tuning() {
        let doc = yaml(&settings(&[
            (OTLP_ENDPOINT, "https://otlp.example.com"),
            ("OTLP_PROTOCOL", "http/protobuf"),
            ("SIGNALS", "traces,metrics,logs"),
            ("BATCH_TIMEOUT", "1s"),
            ("BATCH_SEND_SIZE", "512"),
            ("BATCH_MAX_SIZE", "1024"),
            ("MEMORY_CHECK_INTERVAL", "2s"),
            ("MEMORY_LIMIT_PERCENTAGE", "75"),
            ("MEMORY_SPIKE_PERCENTAGE", "20"),
        ]));
        assert!(doc["exporters"]["otlp_grpc"].is_badvalue());
        let exporter = &doc["exporters"]["otlp_http"];
        assert_eq!(
            exporter["endpoint"].as_str(),
            Some("https://otlp.example.com")
        );
        assert!(exporter["tls"].is_badvalue());
        assert_eq!(doc["processors"]["batch"]["timeout"].as_str(), Some("1s"));
        assert_eq!(
            doc["processors"]["batch"]["send_batch_size"].as_i64(),
            Some(512)
        );
        assert_eq!(
            doc["processors"]["batch"]["send_batch_max_size"].as_i64(),
            Some(1024)
        );
        assert_eq!(
            doc["processors"]["memory_limiter"]["check_interval"].as_str(),
            Some("2s")
        );
        assert_eq!(
            doc["processors"]["memory_limiter"]["limit_percentage"].as_i64(),
            Some(75)
        );
        let pipelines = doc["service"]["pipelines"].as_hash().unwrap();
        assert_eq!(pipelines.len(), 3);
        for signal in ["traces", "metrics", "logs"] {
            assert_eq!(
                doc["service"]["pipelines"][signal]["exporters"][0].as_str(),
                Some("otlp_http")
            );
        }
    }

    #[test]
    fn headers_reference_secret_files_not_values() {
        let mut pairs = base();
        pairs.push((
            OTLP_HEADERS,
            "Authorization=Bearer%20s3cr3t,x-scope-orgid=tenant-1",
        ));
        let s = settings(&pairs);
        let rendered = render(&s);
        assert!(!rendered.contains("s3cr3t"));
        assert!(!rendered.contains("tenant-1"));
        let doc = yaml(&s);
        let headers = &doc["exporters"]["otlp_grpc"]["headers"];
        assert_eq!(
            headers["authorization"].as_str(),
            Some("${file:/etc/otelcol-secrets/authorization}")
        );
        assert_eq!(
            headers["x-scope-orgid"].as_str(),
            Some("${file:/etc/otelcol-secrets/x-scope-orgid}")
        );
    }

    #[test]
    fn quoting_keeps_structure() {
        // Validation rejects such values; quoting is the second line of defence.
        let mut s = settings(&base());
        s.exporter.endpoint = "evil:4317\"\nexporters:\n  debug: {}\n#".into();
        s.batch.timeout = "1s\nreceivers: {}".into();
        let doc = yaml(&s);
        assert_eq!(doc["exporters"].as_hash().unwrap().len(), 1);
        assert_eq!(
            doc["exporters"]["otlp_grpc"]["endpoint"].as_str(),
            Some(s.exporter.endpoint.as_str())
        );
        assert_eq!(doc["receivers"].as_hash().unwrap().len(), 1);
    }

    #[test]
    fn rendering_is_deterministic() {
        let mut pairs = base();
        pairs.push((OTLP_HEADERS, "b=2,a=1"));
        let a = render(&settings(&pairs));
        let b = render(&settings(&pairs));
        assert_eq!(a, b);
        assert_eq!(config_hash(&a), config_hash(&b));
        assert_eq!(config_hash(&a).len(), 64);
        let mut other = base();
        other.push(("BATCH_TIMEOUT", "1s"));
        assert_ne!(config_hash(&a), config_hash(&render(&settings(&other))));
    }
}
