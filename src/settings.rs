//! Plugin configuration: the schema deckwatch renders in Settings → Plugins,
//! and parsing plus validation of the values deckwatch passes back.
//!
//! deckwatch injects only saved values into the plugin's config, never the
//! schema defaults, so every default is applied here as well.

use std::collections::BTreeMap;

use deckwatch_plugin_sdk::{ConfigField, ConfigFieldType};

use crate::secrets::{self, Header};
use crate::validate::{parse_duration, parse_image, parse_quantity};

pub const OTLP_ENDPOINT: &str = "OTLP_ENDPOINT";
pub const OTLP_PROTOCOL: &str = "OTLP_PROTOCOL";
pub const OTLP_INSECURE: &str = "OTLP_INSECURE";
pub const OTLP_HEADERS: &str = "OTLP_HEADERS";
pub const SIGNALS: &str = "SIGNALS";
pub const APP_OTLP_PROTOCOL: &str = "APP_OTLP_PROTOCOL";
pub const COLLECTOR_IMAGE: &str = "COLLECTOR_IMAGE";
pub const COLLECTOR_CONFIG_DIR: &str = "COLLECTOR_CONFIG_DIR";
pub const COLLECTOR_CPU_REQUEST: &str = "COLLECTOR_CPU_REQUEST";
pub const COLLECTOR_CPU_LIMIT: &str = "COLLECTOR_CPU_LIMIT";
pub const COLLECTOR_MEMORY_REQUEST: &str = "COLLECTOR_MEMORY_REQUEST";
pub const COLLECTOR_MEMORY_LIMIT: &str = "COLLECTOR_MEMORY_LIMIT";
pub const BATCH_TIMEOUT: &str = "BATCH_TIMEOUT";
pub const BATCH_SEND_SIZE: &str = "BATCH_SEND_SIZE";
pub const BATCH_MAX_SIZE: &str = "BATCH_MAX_SIZE";
pub const MEMORY_CHECK_INTERVAL: &str = "MEMORY_CHECK_INTERVAL";
pub const MEMORY_LIMIT_PERCENTAGE: &str = "MEMORY_LIMIT_PERCENTAGE";
pub const MEMORY_SPIKE_PERCENTAGE: &str = "MEMORY_SPIKE_PERCENTAGE";

/// Every config key the plugin reads, in schema order.
pub const ALL_KEYS: &[&str] = &[
    OTLP_ENDPOINT,
    OTLP_PROTOCOL,
    OTLP_INSECURE,
    OTLP_HEADERS,
    SIGNALS,
    APP_OTLP_PROTOCOL,
    COLLECTOR_IMAGE,
    COLLECTOR_CONFIG_DIR,
    COLLECTOR_CPU_REQUEST,
    COLLECTOR_CPU_LIMIT,
    COLLECTOR_MEMORY_REQUEST,
    COLLECTOR_MEMORY_LIMIT,
    BATCH_TIMEOUT,
    BATCH_SEND_SIZE,
    BATCH_MAX_SIZE,
    MEMORY_CHECK_INTERVAL,
    MEMORY_LIMIT_PERCENTAGE,
    MEMORY_SPIKE_PERCENTAGE,
];

/// otelcol-contrib 0.165.0 (2026-11-07), multi-arch index digest.
pub const DEFAULT_IMAGE: &str = "otel/opentelemetry-collector-contrib:0.165.0@sha256:e40d61f10c9a617cca1ffd1be4d7e2b3a4b0f0001798ff07e58cabe82e618cea";
const DEFAULT_PROTOCOL: &str = "grpc";
const DEFAULT_SIGNALS: &str = "traces";
const DEFAULT_CPU_REQUEST: &str = "100m";
const DEFAULT_CPU_LIMIT: &str = "500m";
const DEFAULT_MEMORY_REQUEST: &str = "128Mi";
const DEFAULT_MEMORY_LIMIT: &str = "256Mi";
const DEFAULT_BATCH_TIMEOUT: &str = "200ms";
const DEFAULT_BATCH_SEND_SIZE: u32 = 8192;
const DEFAULT_MEMORY_CHECK_INTERVAL: &str = "1s";
const DEFAULT_MEMORY_LIMIT_PERCENTAGE: u32 = 80;
const DEFAULT_MEMORY_SPIKE_PERCENTAGE: u32 = 25;

/// OTLP transport, used both for the exporter and for the app → sidecar hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Grpc,
    HttpProtobuf,
}

impl Protocol {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "grpc" => Some(Self::Grpc),
            "http/protobuf" => Some(Self::HttpProtobuf),
            _ => None,
        }
    }

    /// Value for `OTEL_EXPORTER_OTLP_PROTOCOL`.
    pub fn as_env(self) -> &'static str {
        match self {
            Self::Grpc => "grpc",
            Self::HttpProtobuf => "http/protobuf",
        }
    }
}

/// Telemetry signals the collector forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    pub traces: bool,
    pub metrics: bool,
    pub logs: bool,
}

impl Signals {
    /// `(signal, enabled)` in a fixed order.
    pub fn iter(self) -> [(&'static str, bool); 3] {
        [
            ("traces", self.traces),
            ("metrics", self.metrics),
            ("logs", self.logs),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exporter {
    pub endpoint: String,
    pub protocol: Protocol,
    pub insecure: bool,
    pub headers: Vec<Header>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    pub cpu_request: Option<String>,
    pub cpu_limit: Option<String>,
    pub memory_request: Option<String>,
    pub memory_limit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub timeout: String,
    pub send_batch_size: u32,
    pub send_batch_max_size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLimiter {
    pub check_interval: String,
    pub limit_percentage: u32,
    pub spike_limit_percentage: u32,
}

/// Validated settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub exporter: Exporter,
    pub signals: Signals,
    pub app_protocol: Protocol,
    pub image: String,
    /// Directory the image reads `config.yaml` from (its `--config` default).
    pub config_dir: String,
    pub resources: Resources,
    pub batch: Batch,
    pub memory_limiter: MemoryLimiter,
}

/// Settings plus non-fatal findings worth surfacing to the operator.
#[derive(Debug)]
pub struct Parsed {
    pub settings: Settings,
    pub warnings: Vec<String>,
}

/// Raw config values as deckwatch passes them. Blank values count as unset.
#[derive(Debug, Clone, Default)]
pub struct RawConfig(BTreeMap<String, String>);

impl RawConfig {
    pub fn new(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for RawConfig {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl Settings {
    /// Validate the raw config. Collects every problem rather than stopping
    /// at the first, so the operator can fix them in one pass.
    pub fn parse(raw: &RawConfig) -> Result<Parsed, Vec<String>> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut err = |key: &str, msg: String| errors.push(format!("{key} {msg}"));

        let protocol = parse_protocol(raw, OTLP_PROTOCOL, &mut err);
        let app_protocol = parse_protocol(raw, APP_OTLP_PROTOCOL, &mut err);
        let insecure = parse_bool(raw, OTLP_INSECURE, &mut err);

        let endpoint = match raw.get(OTLP_ENDPOINT) {
            None => {
                err(
                    OTLP_ENDPOINT,
                    "is required: set the OTLP endpoint the collector exports to, \
                     usually deckwatch's tracing.otlp_endpoint"
                        .into(),
                );
                String::new()
            }
            Some(e) => {
                if let Err(msg) = check_endpoint(e, protocol, insecure) {
                    err(OTLP_ENDPOINT, msg);
                }
                e.to_string()
            }
        };

        let headers = match raw.get(OTLP_HEADERS) {
            None => Vec::new(),
            Some(h) => secrets::parse_headers(h).unwrap_or_else(|msg| {
                err(OTLP_HEADERS, msg);
                Vec::new()
            }),
        };
        if !headers.is_empty() && insecure {
            warnings.push(
                "OTLP_HEADERS are sent over a plaintext connection because OTLP_INSECURE is true"
                    .into(),
            );
        }

        let signals =
            parse_signals(raw.get(SIGNALS).unwrap_or(DEFAULT_SIGNALS)).unwrap_or_else(|msg| {
                err(SIGNALS, msg);
                Signals {
                    traces: true,
                    metrics: false,
                    logs: false,
                }
            });

        let image = raw.get(COLLECTOR_IMAGE).unwrap_or(DEFAULT_IMAGE);
        let config_dir = match parse_image(image) {
            Err(msg) => {
                err(COLLECTOR_IMAGE, msg);
                String::new()
            }
            Ok(img) => {
                if img.digest.is_none() {
                    warnings.push(format!(
                        "COLLECTOR_IMAGE {image} is pinned by tag only; add @sha256:<digest> so \
                         the image cannot change under the same tag"
                    ));
                }
                match raw.get(COLLECTOR_CONFIG_DIR) {
                    Some(dir) => {
                        if let Err(msg) = check_config_dir(dir) {
                            err(COLLECTOR_CONFIG_DIR, msg);
                        }
                        dir.trim_end_matches('/').to_string()
                    }
                    None => default_config_dir(img.name()).to_string(),
                }
            }
        };

        let quantity = |key: &str, default: &str, errs: &mut dyn FnMut(&str, String)| {
            // An explicit "none" leaves the request or limit unset.
            let value = raw.get(key).unwrap_or(default);
            if value.eq_ignore_ascii_case("none") {
                return None;
            }
            match parse_quantity(value) {
                Ok(n) => Some((value.to_string(), n)),
                Err(msg) => {
                    errs(key, msg);
                    None
                }
            }
        };
        let cpu_request = quantity(COLLECTOR_CPU_REQUEST, DEFAULT_CPU_REQUEST, &mut err);
        let cpu_limit = quantity(COLLECTOR_CPU_LIMIT, DEFAULT_CPU_LIMIT, &mut err);
        let memory_request = quantity(COLLECTOR_MEMORY_REQUEST, DEFAULT_MEMORY_REQUEST, &mut err);
        let memory_limit = quantity(COLLECTOR_MEMORY_LIMIT, DEFAULT_MEMORY_LIMIT, &mut err);
        for (req, lim, req_key, lim_key) in [
            (
                &cpu_request,
                &cpu_limit,
                COLLECTOR_CPU_REQUEST,
                COLLECTOR_CPU_LIMIT,
            ),
            (
                &memory_request,
                &memory_limit,
                COLLECTOR_MEMORY_REQUEST,
                COLLECTOR_MEMORY_LIMIT,
            ),
        ] {
            if let (Some((_, r)), Some((_, l))) = (req, lim) {
                if r > l {
                    err(req_key, format!("must not exceed {lim_key}"));
                }
            }
        }
        if memory_limit.is_none() {
            warnings.push(
                "COLLECTOR_MEMORY_LIMIT is unset, so the memory_limiter percentages apply to the \
                 node's memory instead of the container's"
                    .into(),
            );
        }

        let batch_timeout = duration(raw, BATCH_TIMEOUT, DEFAULT_BATCH_TIMEOUT, &mut err);
        let send_batch_size = number(raw, BATCH_SEND_SIZE, DEFAULT_BATCH_SEND_SIZE, 1, &mut err);
        let send_batch_max_size = raw
            .get(BATCH_MAX_SIZE)
            .and_then(|v| match v.parse::<u32>() {
                Ok(0) => None,
                Ok(n) if n >= send_batch_size => Some(n),
                Ok(_) => {
                    err(
                        BATCH_MAX_SIZE,
                        "must be 0 (no limit) or at least BATCH_SEND_SIZE".into(),
                    );
                    None
                }
                Err(_) => {
                    err(BATCH_MAX_SIZE, "must be a whole number".into());
                    None
                }
            });

        let check_interval = duration(
            raw,
            MEMORY_CHECK_INTERVAL,
            DEFAULT_MEMORY_CHECK_INTERVAL,
            &mut err,
        );
        let limit_percentage = number(
            raw,
            MEMORY_LIMIT_PERCENTAGE,
            DEFAULT_MEMORY_LIMIT_PERCENTAGE,
            1,
            &mut err,
        );
        let spike_limit_percentage = number(
            raw,
            MEMORY_SPIKE_PERCENTAGE,
            DEFAULT_MEMORY_SPIKE_PERCENTAGE,
            1,
            &mut err,
        );
        if limit_percentage > 100 {
            err(MEMORY_LIMIT_PERCENTAGE, "must be between 1 and 100".into());
        } else if spike_limit_percentage >= limit_percentage {
            err(
                MEMORY_SPIKE_PERCENTAGE,
                "must be lower than MEMORY_LIMIT_PERCENTAGE".into(),
            );
        }

        if !errors.is_empty() {
            return Err(errors);
        }
        let to_string = |q: Option<(String, f64)>| q.map(|(s, _)| s);
        Ok(Parsed {
            settings: Settings {
                exporter: Exporter {
                    endpoint,
                    protocol,
                    insecure,
                    headers,
                },
                signals,
                app_protocol,
                image: image.to_string(),
                config_dir,
                resources: Resources {
                    cpu_request: to_string(cpu_request),
                    cpu_limit: to_string(cpu_limit),
                    memory_request: to_string(memory_request),
                    memory_limit: to_string(memory_limit),
                },
                batch: Batch {
                    timeout: batch_timeout,
                    send_batch_size,
                    send_batch_max_size,
                },
                memory_limiter: MemoryLimiter {
                    check_interval,
                    limit_percentage,
                    spike_limit_percentage,
                },
            },
            warnings,
        })
    }
}

fn parse_protocol(raw: &RawConfig, key: &str, err: &mut dyn FnMut(&str, String)) -> Protocol {
    let value = raw.get(key).unwrap_or(DEFAULT_PROTOCOL);
    Protocol::parse(value).unwrap_or_else(|| {
        err(key, "must be grpc or http/protobuf".into());
        Protocol::Grpc
    })
}

fn parse_bool(raw: &RawConfig, key: &str, err: &mut dyn FnMut(&str, String)) -> bool {
    match raw.get(key).map(str::to_ascii_lowercase).as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(_) => {
            err(key, "must be true or false".into());
            false
        }
    }
}

fn duration(
    raw: &RawConfig,
    key: &str,
    default: &str,
    err: &mut dyn FnMut(&str, String),
) -> String {
    let value = raw.get(key).unwrap_or(default);
    if let Err(msg) = parse_duration(value) {
        err(key, msg);
    }
    value.to_string()
}

fn number(
    raw: &RawConfig,
    key: &str,
    default: u32,
    min: u32,
    err: &mut dyn FnMut(&str, String),
) -> u32 {
    match raw.get(key) {
        None => default,
        Some(v) => match v.parse::<u32>() {
            Ok(n) if n >= min => n,
            _ => {
                err(key, format!("must be a whole number of at least {min}"));
                default
            }
        },
    }
}

fn parse_signals(value: &str) -> Result<Signals, String> {
    let mut signals = Signals {
        traces: false,
        metrics: false,
        logs: false,
    };
    for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "traces" => signals.traces = true,
            "metrics" => signals.metrics = true,
            "logs" => signals.logs = true,
            _ => return Err("must list one or more of traces, metrics, logs".into()),
        }
    }
    if signals.traces || signals.metrics || signals.logs {
        Ok(signals)
    } else {
        Err("must list one or more of traces, metrics, logs".into())
    }
}

/// Where each official collector distribution reads its config. Without
/// sidecar `args` support in deckwatch the plugin cannot pass `--config`, so
/// the ConfigMap is mounted over the image's default config directory.
fn default_config_dir(image_name: &str) -> &'static str {
    match image_name {
        "opentelemetry-collector" => "/etc/otelcol",
        "opentelemetry-collector-k8s" => "/etc/otelcol-k8s",
        "opentelemetry-collector-otlp" => "/etc/otelcol-otlp",
        _ => "/etc/otelcol-contrib",
    }
}

fn check_config_dir(dir: &str) -> Result<(), String> {
    let dir = dir.trim_end_matches('/');
    let safe_chars = dir
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'));
    let segments: Vec<&str> = dir.split('/').skip(1).collect();
    if !dir.starts_with('/')
        || !safe_chars
        || segments.len() < 2
        || segments
            .iter()
            .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err("must be an absolute path at least two levels deep, e.g. /etc/otelcol".into());
    }
    // Mounting over these would hide the CA bundle or the headers Secret.
    for reserved in ["/etc/ssl", secrets::SECRETS_MOUNT_PATH] {
        let overlaps = dir == reserved
            || dir.starts_with(&format!("{reserved}/"))
            || reserved.starts_with(&format!("{dir}/"));
        if overlaps {
            return Err(format!("must not overlap {reserved}"));
        }
    }
    Ok(())
}

/// Validate the exporter endpoint and its consistency with `OTLP_INSECURE`.
fn check_endpoint(endpoint: &str, protocol: Protocol, insecure: bool) -> Result<(), String> {
    if endpoint.len() > 2048 {
        return Err("is too long".into());
    }
    // `$` would be expanded by the collector; quotes and braces have no place
    // in an endpoint and would only help an injection attempt.
    if endpoint
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "$\"'`\\{}<>".contains(c))
    {
        return Err("contains characters that are not valid in an endpoint".into());
    }
    let (scheme, rest) = match endpoint.split_once("://") {
        Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
        None => (None, endpoint),
    };
    match scheme.as_deref() {
        Some("http") if !insecure => {
            return Err(
                "uses http:// (plaintext) but OTLP_INSECURE is false; set OTLP_INSECURE to true \
                 to allow plaintext, or use https://"
                    .into(),
            )
        }
        Some("https") if insecure => {
            return Err("uses https:// but OTLP_INSECURE is true; use one or the other".into())
        }
        Some("http" | "https") => {}
        Some(_) => return Err("must use http:// or https://".into()),
        None if protocol == Protocol::HttpProtobuf => {
            return Err("must be a URL starting with http:// or https:// for http/protobuf".into())
        }
        None => {}
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err("must name a host (user info is not allowed)".into());
    }
    // gRPC needs host:port; with a scheme the port can default (443 / 80).
    if scheme.is_none() && !has_port(authority) {
        return Err("must be host:port for grpc, e.g. tempo.observability.svc:4317".into());
    }
    Ok(())
}

fn has_port(authority: &str) -> bool {
    let port = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && (!host.contains(':') || host.ends_with(']')) => {
            port
        }
        _ => return false,
    };
    matches!(port.parse::<u16>(), Ok(p) if p > 0)
}

fn field(
    key: &str,
    label: &str,
    description: &str,
    field_type: ConfigFieldType,
    default: Option<&str>,
) -> ConfigField {
    ConfigField {
        key: key.into(),
        label: label.into(),
        description: description.into(),
        field_type,
        default: default.map(str::to_string),
        required: false,
        options: vec![],
        env_source: None,
    }
}

/// The plugin-wide settings form (Settings → Plugins → otel).
pub fn schema() -> Vec<ConfigField> {
    use ConfigFieldType as T;
    let protocol_options = vec!["grpc".to_string(), "http/protobuf".to_string()];
    vec![
        ConfigField {
            required: true,
            ..field(
                OTLP_ENDPOINT,
                "Exporter endpoint",
                "Where every collector sidecar sends telemetry. Usually the same value as \
                 deckwatch's tracing.otlp_endpoint, which plugins cannot read. grpc: host:port \
                 (tempo.observability.svc:4317). http/protobuf: a base URL \
                 (https://otlp.example.com); the collector appends /v1/traces.",
                T::String,
                None,
            )
        },
        ConfigField {
            options: protocol_options.clone(),
            ..field(
                OTLP_PROTOCOL,
                "Exporter protocol",
                "Protocol the collector uses to reach the exporter endpoint.",
                T::Select,
                Some(DEFAULT_PROTOCOL),
            )
        },
        field(
            OTLP_INSECURE,
            "Plaintext export (no TLS)",
            "Send to the endpoint without TLS, like deckwatch's tracing.otlp_insecure. Typical \
             for an in-cluster Tempo or Jaeger; leave off for managed backends. Must agree with \
             an http:// or https:// endpoint scheme.",
            T::Bool,
            Some("false"),
        ),
        field(
            OTLP_HEADERS,
            "Exporter headers",
            "Optional headers for the exporter, usually credentials: name=value pairs separated \
             by commas, as in OTEL_EXPORTER_OTLP_HEADERS (values may be %-encoded). Stored in a \
             Kubernetes Secret in each application's namespace and mounted into the collector; \
             never placed in the pod spec or environment.",
            T::Secret,
            None,
        ),
        field(
            SIGNALS,
            "Signals",
            "Comma-separated signals the collector forwards: traces, metrics, logs. Apps are \
             told to export only these (OTEL_<SIGNAL>_EXPORTER).",
            T::String,
            Some(DEFAULT_SIGNALS),
        ),
        ConfigField {
            options: protocol_options,
            ..field(
                APP_OTLP_PROTOCOL,
                "App → sidecar protocol",
                "Protocol the application's SDK uses to reach the sidecar: grpc \
                 (http://localhost:4317) or http/protobuf (http://localhost:4318). Sets \
                 OTEL_EXPORTER_OTLP_PROTOCOL to match.",
                T::Select,
                Some(DEFAULT_PROTOCOL),
            )
        },
        field(
            COLLECTOR_IMAGE,
            "Collector image",
            "OpenTelemetry Collector image for the sidecar. Must be pinned to a version, \
             preferably with a digest; :latest and untagged images are rejected. Custom images \
             must be collector 0.144.0 or later.",
            T::String,
            Some(DEFAULT_IMAGE),
        ),
        field(
            COLLECTOR_CONFIG_DIR,
            "Collector config directory",
            "Directory the image reads config.yaml from. Leave empty to derive it from the \
             image name (/etc/otelcol-contrib, or /etc/otelcol for the core image). Only needed \
             for custom images.",
            T::String,
            None,
        ),
        field(
            COLLECTOR_CPU_REQUEST,
            "Sidecar CPU request",
            "CPU request for the collector container, e.g. 100m. Set to none to leave unset.",
            T::String,
            Some(DEFAULT_CPU_REQUEST),
        ),
        field(
            COLLECTOR_CPU_LIMIT,
            "Sidecar CPU limit",
            "CPU limit for the collector container, e.g. 500m. Set to none to leave unset.",
            T::String,
            Some(DEFAULT_CPU_LIMIT),
        ),
        field(
            COLLECTOR_MEMORY_REQUEST,
            "Sidecar memory request",
            "Memory request for the collector container, e.g. 128Mi. Set to none to leave unset.",
            T::String,
            Some(DEFAULT_MEMORY_REQUEST),
        ),
        field(
            COLLECTOR_MEMORY_LIMIT,
            "Sidecar memory limit",
            "Memory limit for the collector container, e.g. 256Mi. The memory_limiter \
             percentages are relative to it. Set to none to leave unset (not recommended).",
            T::String,
            Some(DEFAULT_MEMORY_LIMIT),
        ),
        field(
            BATCH_TIMEOUT,
            "Batch timeout",
            "batch processor: longest a batch waits before it is sent, e.g. 200ms or 1s.",
            T::String,
            Some(DEFAULT_BATCH_TIMEOUT),
        ),
        field(
            BATCH_SEND_SIZE,
            "Batch size",
            "batch processor: number of spans, data points or log records that triggers a send.",
            T::String,
            Some("8192"),
        ),
        field(
            BATCH_MAX_SIZE,
            "Batch maximum size",
            "batch processor: upper bound on a batch; larger batches are split. Empty or 0 \
             means no limit. Must be at least the batch size.",
            T::String,
            None,
        ),
        field(
            MEMORY_CHECK_INTERVAL,
            "Memory check interval",
            "memory_limiter processor: how often memory use is measured, e.g. 1s.",
            T::String,
            Some(DEFAULT_MEMORY_CHECK_INTERVAL),
        ),
        field(
            MEMORY_LIMIT_PERCENTAGE,
            "Memory limit (%)",
            "memory_limiter processor: hard limit as a percentage of the sidecar memory limit. \
             Above it the collector refuses data and forces garbage collection.",
            T::String,
            Some("80"),
        ),
        field(
            MEMORY_SPIKE_PERCENTAGE,
            "Memory spike allowance (%)",
            "memory_limiter processor: headroom below the hard limit, as a percentage of the \
             sidecar memory limit; the soft limit is the hard limit minus this. Must be lower \
             than the memory limit percentage.",
            T::String,
            Some("25"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pairs: &[(&str, &str)]) -> RawConfig {
        pairs.iter().copied().collect()
    }

    fn minimal() -> Vec<(&'static str, &'static str)> {
        vec![
            (OTLP_ENDPOINT, "tempo.observability.svc:4317"),
            (OTLP_INSECURE, "true"),
        ]
    }

    fn parse(pairs: &[(&str, &str)]) -> Result<Parsed, Vec<String>> {
        Settings::parse(&raw(pairs))
    }

    fn errors_with(extra: &[(&str, &str)]) -> Vec<String> {
        let mut pairs = minimal();
        pairs.extend_from_slice(extra);
        parse(&pairs).unwrap_err()
    }

    #[test]
    fn defaults() {
        let parsed = parse(&minimal()).unwrap();
        let s = parsed.settings;
        assert_eq!(s.exporter.endpoint, "tempo.observability.svc:4317");
        assert_eq!(s.exporter.protocol, Protocol::Grpc);
        assert!(s.exporter.insecure);
        assert!(s.exporter.headers.is_empty());
        assert_eq!(
            s.signals,
            Signals {
                traces: true,
                metrics: false,
                logs: false
            }
        );
        assert_eq!(s.app_protocol, Protocol::Grpc);
        assert_eq!(s.image, DEFAULT_IMAGE);
        assert_eq!(s.config_dir, "/etc/otelcol-contrib");
        assert_eq!(s.resources.cpu_request.as_deref(), Some("100m"));
        assert_eq!(s.resources.cpu_limit.as_deref(), Some("500m"));
        assert_eq!(s.resources.memory_request.as_deref(), Some("128Mi"));
        assert_eq!(s.resources.memory_limit.as_deref(), Some("256Mi"));
        assert_eq!(s.batch.timeout, "200ms");
        assert_eq!(s.batch.send_batch_size, 8192);
        assert_eq!(s.batch.send_batch_max_size, None);
        assert_eq!(s.memory_limiter.check_interval, "1s");
        assert_eq!(s.memory_limiter.limit_percentage, 80);
        assert_eq!(s.memory_limiter.spike_limit_percentage, 25);
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
    }

    #[test]
    fn default_image_is_pinned_by_digest() {
        let img = parse_image(DEFAULT_IMAGE).unwrap();
        assert_eq!(img.name(), "opentelemetry-collector-contrib");
        assert!(img.digest.is_some());
        assert_ne!(img.tag.as_deref(), Some("latest"));
    }

    #[test]
    fn endpoint_is_required() {
        let errs = parse(&[]).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.starts_with("OTLP_ENDPOINT is required")));
    }

    #[test]
    fn blank_values_count_as_unset() {
        let errs = parse(&[(OTLP_ENDPOINT, "   ")]).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| e.starts_with("OTLP_ENDPOINT is required")));
        let s = parse(&[
            (OTLP_ENDPOINT, "tempo:4317"),
            (OTLP_INSECURE, "true"),
            (COLLECTOR_IMAGE, " "),
        ])
        .unwrap()
        .settings;
        assert_eq!(s.image, DEFAULT_IMAGE);
    }

    #[test]
    fn endpoint_scheme_must_agree_with_insecure() {
        let ok = |pairs: &[(&str, &str)]| parse(pairs).is_ok();
        assert!(ok(&[
            (OTLP_ENDPOINT, "tempo:4317"),
            (OTLP_INSECURE, "true")
        ]));
        assert!(ok(&[(OTLP_ENDPOINT, "otlp.example.com:443")]));
        assert!(ok(&[(OTLP_ENDPOINT, "https://otlp.example.com")]));
        assert!(ok(&[
            (OTLP_ENDPOINT, "http://tempo:4317"),
            (OTLP_INSECURE, "true")
        ]));
        // Plaintext must be opted into explicitly.
        assert!(!ok(&[(OTLP_ENDPOINT, "http://tempo:4317")]));
        assert!(!ok(&[
            (OTLP_ENDPOINT, "https://otlp.example.com"),
            (OTLP_INSECURE, "true")
        ]));
    }

    #[test]
    fn endpoint_shape() {
        let bad = |endpoint: &str, protocol: &str| {
            parse(&[
                (OTLP_ENDPOINT, endpoint),
                (OTLP_PROTOCOL, protocol),
                (OTLP_INSECURE, "true"),
            ])
            .is_err()
        };
        assert!(bad("tempo", "grpc"), "grpc needs a port");
        assert!(bad("tempo:0", "grpc"));
        assert!(bad("tempo:http", "grpc"));
        assert!(bad("tempo:4318", "http/protobuf"), "http needs a URL");
        assert!(bad("ftp://tempo:4317", "grpc"));
        assert!(bad("http://user:pw@tempo:4318", "http/protobuf"));
        assert!(bad("tempo:4317\nexporters: {}", "grpc"));
        assert!(bad("${env:HOME}:4317", "grpc"));
        assert!(bad("\"tempo:4317\"", "grpc"));
        assert!(!bad("[::1]:4317", "grpc"));
        assert!(!bad("10.0.0.5:4317", "grpc"));
        assert!(!bad("http://tempo:4318", "http/protobuf"));
    }

    #[test]
    fn invalid_values_are_all_reported() {
        let errs = errors_with(&[
            (OTLP_PROTOCOL, "thrift"),
            (OTLP_INSECURE, "yes"),
            (SIGNALS, "traces,profiles"),
            (
                COLLECTOR_IMAGE,
                "otel/opentelemetry-collector-contrib:latest",
            ),
            (COLLECTOR_CPU_REQUEST, "lots"),
            (BATCH_TIMEOUT, "5"),
            (BATCH_SEND_SIZE, "0"),
            (MEMORY_CHECK_INTERVAL, "0s"),
        ]);
        for key in [
            OTLP_PROTOCOL,
            OTLP_INSECURE,
            SIGNALS,
            COLLECTOR_IMAGE,
            COLLECTOR_CPU_REQUEST,
            BATCH_TIMEOUT,
            BATCH_SEND_SIZE,
            MEMORY_CHECK_INTERVAL,
        ] {
            assert!(
                errs.iter().any(|e| e.starts_with(key)),
                "no error for {key}: {errs:?}"
            );
        }
    }

    #[test]
    fn requests_must_not_exceed_limits() {
        let errs = errors_with(&[(COLLECTOR_CPU_REQUEST, "1"), (COLLECTOR_CPU_LIMIT, "500m")]);
        assert!(errs[0].starts_with("COLLECTOR_CPU_REQUEST must not exceed"));
        let errs = errors_with(&[
            (COLLECTOR_MEMORY_REQUEST, "1Gi"),
            (COLLECTOR_MEMORY_LIMIT, "512Mi"),
        ]);
        assert!(errs[0].starts_with("COLLECTOR_MEMORY_REQUEST must not exceed"));
    }

    #[test]
    fn resources_can_be_left_unset() {
        let mut pairs = minimal();
        pairs.push((COLLECTOR_CPU_LIMIT, "none"));
        pairs.push((COLLECTOR_MEMORY_LIMIT, "None"));
        let parsed = parse(&pairs).unwrap();
        assert_eq!(parsed.settings.resources.cpu_limit, None);
        assert_eq!(parsed.settings.resources.memory_limit, None);
        assert!(parsed.warnings[0].contains("COLLECTOR_MEMORY_LIMIT is unset"));
    }

    #[test]
    fn memory_limiter_bounds() {
        let errs = errors_with(&[(MEMORY_LIMIT_PERCENTAGE, "101")]);
        assert!(errs[0].starts_with(MEMORY_LIMIT_PERCENTAGE));
        let errs = errors_with(&[
            (MEMORY_LIMIT_PERCENTAGE, "50"),
            (MEMORY_SPIKE_PERCENTAGE, "50"),
        ]);
        assert!(errs[0].starts_with(MEMORY_SPIKE_PERCENTAGE));
    }

    #[test]
    fn batch_max_size() {
        let mut pairs = minimal();
        pairs.push((BATCH_MAX_SIZE, "16384"));
        assert_eq!(
            parse(&pairs).unwrap().settings.batch.send_batch_max_size,
            Some(16384)
        );
        let mut pairs = minimal();
        pairs.push((BATCH_MAX_SIZE, "0"));
        assert_eq!(
            parse(&pairs).unwrap().settings.batch.send_batch_max_size,
            None
        );
        assert!(errors_with(&[(BATCH_MAX_SIZE, "100")])[0].starts_with(BATCH_MAX_SIZE));
    }

    #[test]
    fn signals_list() {
        let mut pairs = minimal();
        pairs.push((SIGNALS, " Logs , traces,metrics,traces"));
        assert_eq!(
            parse(&pairs).unwrap().settings.signals,
            Signals {
                traces: true,
                metrics: true,
                logs: true
            }
        );
        assert!(errors_with(&[(SIGNALS, ",")])[0].starts_with(SIGNALS));
    }

    #[test]
    fn config_dir_follows_image() {
        let dir = |image: &str| {
            let mut pairs = minimal();
            pairs.push((COLLECTOR_IMAGE, image));
            parse(&pairs).unwrap().settings.config_dir
        };
        assert_eq!(dir("otel/opentelemetry-collector:0.165.0"), "/etc/otelcol");
        assert_eq!(
            dir("ghcr.io/open-telemetry/opentelemetry-collector-releases/opentelemetry-collector-k8s:0.165.0"),
            "/etc/otelcol-k8s"
        );
        assert_eq!(
            dir("registry.corp/mirror/otelcol-contrib:0.165.0"),
            "/etc/otelcol-contrib"
        );
    }

    #[test]
    fn config_dir_override_is_validated() {
        let mut pairs = minimal();
        pairs.push((COLLECTOR_CONFIG_DIR, "/conf/otel/"));
        assert_eq!(parse(&pairs).unwrap().settings.config_dir, "/conf/otel");
        for bad in [
            "etc/otel",
            "/etc",
            "/",
            "/etc/ssl",
            "/etc/ssl/certs",
            "/etc/../x",
            "/etc/otelcol-secrets",
            "/etc/otel col",
        ] {
            let errs = errors_with(&[(COLLECTOR_CONFIG_DIR, bad)]);
            assert!(errs[0].starts_with(COLLECTOR_CONFIG_DIR), "{bad}: {errs:?}");
        }
    }

    #[test]
    fn tag_only_image_warns() {
        let mut pairs = minimal();
        pairs.push((
            COLLECTOR_IMAGE,
            "otel/opentelemetry-collector-contrib:0.165.0",
        ));
        let parsed = parse(&pairs).unwrap();
        assert!(parsed
            .warnings
            .iter()
            .any(|w| w.contains("pinned by tag only")));
    }

    #[test]
    fn headers_over_plaintext_warn() {
        let mut pairs = minimal();
        pairs.push((OTLP_HEADERS, "authorization=Bearer x"));
        let parsed = parse(&pairs).unwrap();
        assert_eq!(parsed.settings.exporter.headers.len(), 1);
        assert!(parsed.warnings.iter().any(|w| w.contains("plaintext")));
    }

    #[test]
    fn header_errors_do_not_leak_values() {
        let errs = errors_with(&[(OTLP_HEADERS, "authorization=Bearer s3cr3t%0a")]);
        assert!(errs[0].starts_with(OTLP_HEADERS));
        assert!(!errs.concat().contains("s3cr3t"));
    }

    #[test]
    fn schema_fields_are_documented() {
        let schema = schema();
        let keys: Vec<&str> = schema.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(keys, ALL_KEYS);
        for f in &schema {
            assert!(!f.label.is_empty(), "{} has no label", f.key);
            assert!(f.description.len() > 20, "{} has no description", f.key);
            if f.field_type == ConfigFieldType::Select {
                assert!(f.options.contains(f.default.as_ref().unwrap()), "{}", f.key);
            }
        }
        let headers = schema.iter().find(|f| f.key == OTLP_HEADERS).unwrap();
        assert_eq!(headers.field_type, ConfigFieldType::Secret);
        assert!(headers.default.is_none());
        let endpoint = schema.iter().find(|f| f.key == OTLP_ENDPOINT).unwrap();
        assert!(endpoint.required);
        assert!(schema.iter().filter(|f| f.required).count() == 1);
    }

    #[test]
    fn schema_defaults_parse() {
        // Saving the form untouched (defaults filled in) must produce the same
        // settings as leaving every key unset.
        let mut pairs: Vec<(String, String)> = schema()
            .into_iter()
            .filter_map(|f| f.default.map(|d| (f.key, d)))
            .collect();
        pairs.push((OTLP_ENDPOINT.into(), "tempo:4317".into()));
        pairs.push((OTLP_INSECURE.into(), "true".into()));
        let from_defaults = Settings::parse(&pairs.into_iter().collect()).unwrap();
        let from_unset = parse(&[(OTLP_ENDPOINT, "tempo:4317"), (OTLP_INSECURE, "true")]).unwrap();
        assert_eq!(from_defaults.settings, from_unset.settings);
    }
}
