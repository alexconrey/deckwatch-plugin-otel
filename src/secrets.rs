//! Exporter headers (usually credentials) and how they reach the collector.
//!
//! The `OTLP_HEADERS` value never appears in the pod spec, in an environment
//! variable, in the collector ConfigMap or in the resource rows deckwatch
//! stores for an application. It goes into a namespaced Kubernetes Secret
//! that is mounted read-only into the collector, and the collector config
//! refers to each header as `${file:/etc/otelcol-secrets/<name>}`.
//!
//! Files rather than `secretKeyRef` env vars because deckwatch's
//! application-level reconciler drops `value_from` from sidecar env vars (it
//! only renders `value`), while it does mount `secret` volumes. Files work
//! for both deployment- and application-level injection.

use std::fmt;

use base64::Engine;
use serde_json::{json, Map, Value};

use crate::validate::percent_decode;

/// Where the headers Secret is mounted in the collector container.
pub const SECRETS_MOUNT_PATH: &str = "/etc/otelcol-secrets";

const CIPHERTEXT_MSG: &str = "looks like a value encrypted by deckwatch. deckwatch encrypts \
    Secret fields saved from the plugin settings form but passes the ciphertext to plugins \
    unchanged, so it cannot be used. Set OTLP_HEADERS with inherit_env_keys or the \
    update_plugin_config MCP tool instead (see the README)";

/// One exporter header. `Debug` redacts the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl fmt::Debug for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Header")
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Parse `OTLP_HEADERS`: comma-separated `name=value` pairs, the format of
/// `OTEL_EXPORTER_OTLP_HEADERS`. Values may be %-encoded. Names are
/// lowercased because gRPC metadata keys must be lowercase. The result is
/// sorted by name so the rendered config is deterministic.
///
/// Error messages never quote the input, which holds credentials.
pub fn parse_headers(raw: &str) -> Result<Vec<Header>, String> {
    if looks_like_deckwatch_ciphertext(raw) {
        return Err(CIPHERTEXT_MSG.into());
    }
    let mut headers: Vec<Header> = Vec::new();
    for (i, entry) in raw.split(',').enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let n = i + 1;
        let (name, value) = entry
            .split_once('=')
            .ok_or_else(|| format!("entry {n} is not in name=value form"))?;
        let name = name.trim().to_ascii_lowercase();
        if !is_valid_header_name(&name) {
            return Err(format!(
                "entry {n} has an invalid header name (use lowercase letters, digits, '-', '_' or '.')"
            ));
        }
        let value = percent_decode(value.trim()).map_err(|e| format!("entry {n} value {e}"))?;
        if value.is_empty() {
            return Err(format!("entry {n} has an empty value"));
        }
        if value.chars().any(char::is_control) {
            return Err(format!("entry {n} value contains control characters"));
        }
        // The collector expands ${...} in resolved values; refuse rather than
        // let a credential be reinterpreted as a config reference.
        if value.contains("${") {
            return Err(format!("entry {n} value contains '${{'"));
        }
        if headers.iter().any(|h| h.name == name) {
            return Err(format!("entry {n} repeats a header name"));
        }
        headers.push(Header { name, value });
    }
    headers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(headers)
}

/// A header name that is a valid gRPC metadata key, HTTP field name and
/// Secret data key: `[a-z0-9][a-z0-9._-]*`.
fn is_valid_header_name(name: &str) -> bool {
    name.len() <= 253
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        })
}

/// deckwatch's `crypto::encrypt` output is standard base64 of a 12-byte nonce
/// plus ciphertext plus a 16-byte tag. A valid header list always has a `=`
/// followed by a value somewhere before the end, which base64 never does, so
/// this cannot match a usable value.
pub fn looks_like_deckwatch_ciphertext(raw: &str) -> bool {
    let s = raw.trim();
    let body = s.trim_end_matches('=');
    s.len() >= 40
        && s.len().is_multiple_of(4)
        && s.len() - body.len() <= 2
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

/// Path of the file holding a header's value inside the collector.
pub fn header_file(name: &str) -> String {
    format!("{SECRETS_MOUNT_PATH}/{name}")
}

/// The namespaced Secret holding the header values, base64-encoded in `data`
/// (not `stringData`, which server-side apply does not track reliably).
pub fn secret_manifest(namespace: &str, name: &str, labels: &Value, headers: &[Header]) -> Value {
    let data: Map<String, Value> = headers
        .iter()
        .map(|h| {
            let encoded = base64::engine::general_purpose::STANDARD.encode(h.value.as_bytes());
            (h.name.clone(), Value::from(encoded))
        })
        .collect();
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "metadata": { "name": name, "namespace": namespace, "labels": labels },
        "type": "Opaque",
        "data": data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, value: &str) -> Header {
        Header {
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn parses_otel_header_format() {
        let headers =
            parse_headers(" X-Honeycomb-Team = abc123 , authorization=Bearer%20tok%3D%3D,, ")
                .unwrap();
        assert_eq!(
            headers,
            vec![
                h("authorization", "Bearer tok=="),
                h("x-honeycomb-team", "abc123")
            ]
        );
    }

    #[test]
    fn value_may_contain_equals_signs() {
        assert_eq!(
            parse_headers("authorization=Basic dXNlcjpwYXNz==").unwrap(),
            vec![h("authorization", "Basic dXNlcjpwYXNz==")]
        );
    }

    #[test]
    fn empty_input_means_no_headers() {
        assert!(parse_headers("").unwrap().is_empty());
        assert!(parse_headers(" , ").unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_entries_without_echoing_them() {
        let secret = "s3cr3t-token-value";
        for raw in [
            secret.to_string(),
            format!("bad name={secret}"),
            format!("x:y={secret}"),
            "authorization=".to_string(),
            format!("a={secret}%0d%0aInjected: 1"),
            format!("a={secret}%zz"),
            format!("a=${{env:HOME}}{secret}"),
            format!("a={secret},A=other"),
        ] {
            let err = parse_headers(&raw).unwrap_err();
            assert!(!err.contains(secret), "error leaked input: {err}");
        }
    }

    #[test]
    fn detects_deckwatch_ciphertext() {
        // Shape of crypto::encrypt output: base64(12-byte nonce || ct || 16-byte tag).
        let ciphertext = base64::engine::general_purpose::STANDARD.encode([7u8; 45]);
        assert!(looks_like_deckwatch_ciphertext(&ciphertext));
        let err = parse_headers(&ciphertext).unwrap_err();
        assert!(err.contains("encrypted by deckwatch"));
        assert!(!looks_like_deckwatch_ciphertext(
            "authorization=Bearer abcdefghijklmnopqrstuvwxyz0123456789"
        ));
        assert!(!looks_like_deckwatch_ciphertext("api-key=abc"));
    }

    #[test]
    fn debug_redacts_values() {
        let dbg = format!("{:?}", h("authorization", "Bearer secret"));
        assert!(dbg.contains("authorization"));
        assert!(!dbg.contains("secret"));
    }

    #[test]
    fn secret_manifest_base64_encodes_values() {
        let labels = json!({"app.kubernetes.io/managed-by": "deckwatch-plugin-otel"});
        let secret = secret_manifest(
            "shop",
            "checkout-otelcol-headers",
            &labels,
            &[h("authorization", "Bearer tok")],
        );
        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["namespace"], "shop");
        assert_eq!(secret["data"]["authorization"], "QmVhcmVyIHRvaw==");
        assert!(secret.get("stringData").is_none());
        assert!(!secret.to_string().contains("Bearer tok"));
    }
}
