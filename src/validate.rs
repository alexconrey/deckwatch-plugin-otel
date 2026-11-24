//! Validation and encoding helpers for operator-supplied values.
//!
//! Everything here is pure so it can be unit tested on the host target.

/// RFC 1123 DNS subdomain, the format Kubernetes requires for ConfigMap and
/// Secret names (and for Deployment and namespace names).
pub fn is_dns_subdomain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

/// A parsed container image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// Everything before the tag and digest, e.g. `otel/opentelemetry-collector-contrib`.
    pub repository: String,
    pub tag: Option<String>,
    pub digest: Option<String>,
}

impl ImageRef {
    /// Last path segment of the repository, e.g. `opentelemetry-collector-contrib`.
    pub fn name(&self) -> &str {
        self.repository
            .rsplit('/')
            .next()
            .unwrap_or(&self.repository)
    }
}

/// Parse an image reference and require it to be pinned: a digest, or a tag
/// other than `latest`. An untagged reference means `:latest`, so it is
/// rejected too.
pub fn parse_image(s: &str) -> Result<ImageRef, String> {
    if s.is_empty() || s.len() > 512 {
        return Err("must be a container image reference".into());
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("must not contain whitespace".into());
    }
    let (name_tag, digest) = match s.split_once('@') {
        Some((n, d)) => (n, Some(d)),
        None => (s, None),
    };
    if let Some(d) = digest {
        let hex = d
            .strip_prefix("sha256:")
            .ok_or("digest must be sha256:<64 hex characters>")?;
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err("digest must be sha256:<64 lowercase hex characters>".into());
        }
    }
    // The tag is after the last ':' that follows the last '/', so a registry
    // port (`registry:5000/repo`) is not mistaken for a tag.
    let path_start = name_tag.rfind('/').map_or(0, |i| i + 1);
    let (repository, tag) = match name_tag[path_start..].rfind(':') {
        Some(i) => (
            &name_tag[..path_start + i],
            Some(&name_tag[path_start + i + 1..]),
        ),
        None => (name_tag, None),
    };
    validate_repository(repository)?;
    if let Some(t) = tag {
        let valid = !t.is_empty()
            && t.len() <= 128
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
            && !t.starts_with(['.', '-']);
        if !valid {
            return Err("has an invalid tag".into());
        }
    }
    match (tag, digest) {
        (None, None) => Err(
            "must be pinned: add a version tag and preferably a digest (name:1.2.3@sha256:...); \
             an untagged image means :latest"
                .into(),
        ),
        (Some(t), None) if t.eq_ignore_ascii_case("latest") => {
            Err("must not use the mutable :latest tag; pin a version and digest".into())
        }
        _ => Ok(ImageRef {
            repository: repository.to_string(),
            tag: tag.map(str::to_string),
            digest: digest.map(str::to_string),
        }),
    }
}

fn validate_repository(repo: &str) -> Result<(), String> {
    let mut parts: Vec<&str> = repo.split('/').collect();
    // A first component with '.', ':' or "localhost" is a registry host.
    if parts.len() > 1 && (parts[0].contains(['.', ':']) || parts[0] == "localhost") {
        let host = parts.remove(0);
        let ok = host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'));
        if !ok {
            return Err("has an invalid registry host".into());
        }
    }
    let path_ok = !parts.is_empty()
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
                })
                && p.starts_with(|c: char| c.is_ascii_alphanumeric())
                && p.ends_with(|c: char| c.is_ascii_alphanumeric())
        });
    if path_ok {
        Ok(())
    } else {
        Err("has an invalid repository path".into())
    }
}

/// Parse a Kubernetes resource quantity (`100m`, `0.5`, `128Mi`, `1G`) into
/// base units: cores for CPU, bytes for memory. Must be positive.
pub fn parse_quantity(s: &str) -> Result<f64, String> {
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (num, suffix) = s.split_at(split);
    let multiplier = match suffix {
        "" => 1.0,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "P" => 1e15,
        "E" => 1e18,
        "Ki" => 1024f64,
        "Mi" => 1024f64.powi(2),
        "Gi" => 1024f64.powi(3),
        "Ti" => 1024f64.powi(4),
        "Pi" => 1024f64.powi(5),
        "Ei" => 1024f64.powi(6),
        _ => return Err("must be a Kubernetes quantity such as 100m, 0.5, 128Mi or 1Gi".into()),
    };
    let value = parse_decimal(num)
        .ok_or("must be a Kubernetes quantity such as 100m, 0.5, 128Mi or 1Gi")?
        * multiplier;
    if value > 0.0 {
        Ok(value)
    } else {
        Err("must be greater than zero".into())
    }
}

/// Parse a Go duration (`200ms`, `1s`, `1m30s`) into seconds. Must be positive.
pub fn parse_duration(s: &str) -> Result<f64, String> {
    const MSG: &str = "must be a duration such as 200ms, 1s or 1m30s";
    if s.is_empty() {
        return Err(MSG.into());
    }
    let mut rest = s;
    let mut total = 0.0;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let value = parse_decimal(&rest[..num_end]).ok_or(MSG)?;
        rest = &rest[num_end..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = match &rest[..unit_end] {
            "ns" => 1e-9,
            "us" | "µs" | "μs" => 1e-6,
            "ms" => 1e-3,
            "s" => 1.0,
            "m" => 60.0,
            "h" => 3600.0,
            _ => return Err(MSG.into()),
        };
        rest = &rest[unit_end..];
        total += value * unit;
    }
    if total > 0.0 {
        Ok(total)
    } else {
        Err("must be greater than zero".into())
    }
}

/// `12`, `0.5`, `.5` or `5.` — digits with at most one decimal point.
fn parse_decimal(s: &str) -> Option<f64> {
    let valid = !s.is_empty()
        && s != "."
        && s.matches('.').count() <= 1
        && s.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    valid.then(|| s.parse().ok()).flatten()
}

/// Percent-encode a value for `OTEL_RESOURCE_ATTRIBUTES`. Everything except
/// RFC 3986 unreserved characters is encoded, which covers the `,` and `=`
/// the OpenTelemetry spec requires.
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode `%XX` escapes (not `+`, which stays literal, as in
/// `OTEL_EXPORTER_OTLP_HEADERS`). Fails on a bad escape or invalid UTF-8.
pub fn percent_decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or("contains an invalid %-escape")?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "is not valid UTF-8 after %-decoding".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dns_subdomain() {
        assert!(is_dns_subdomain("checkout-api"));
        assert!(is_dns_subdomain("a.b-c.d1"));
        assert!(!is_dns_subdomain(""));
        assert!(!is_dns_subdomain("Checkout"));
        assert!(!is_dns_subdomain("-x"));
        assert!(!is_dns_subdomain("x-"));
        assert!(!is_dns_subdomain("a..b"));
        assert!(!is_dns_subdomain("a_b"));
        assert!(!is_dns_subdomain(&"a".repeat(254)));
    }

    #[test]
    fn image_pinned_by_tag_and_digest() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let img = parse_image(&format!(
            "otel/opentelemetry-collector-contrib:0.165.0@{digest}"
        ))
        .unwrap();
        assert_eq!(img.repository, "otel/opentelemetry-collector-contrib");
        assert_eq!(img.name(), "opentelemetry-collector-contrib");
        assert_eq!(img.tag.as_deref(), Some("0.165.0"));
        assert_eq!(img.digest.as_deref(), Some(digest.as_str()));
    }

    #[test]
    fn image_registry_port_is_not_a_tag() {
        let img = parse_image("registry.local:5000/mirror/otelcol:1.0").unwrap();
        assert_eq!(img.repository, "registry.local:5000/mirror/otelcol");
        assert_eq!(img.tag.as_deref(), Some("1.0"));
        assert!(parse_image("registry.local:5000/mirror/otelcol").is_err());
    }

    #[test]
    fn image_latest_and_untagged_are_rejected() {
        assert!(parse_image("otel/opentelemetry-collector-contrib").is_err());
        assert!(parse_image("otel/opentelemetry-collector-contrib:latest").is_err());
        assert!(parse_image("otel/opentelemetry-collector-contrib:LATEST").is_err());
        // A digest pins the image even when the tag says latest.
        let digest = format!("sha256:{}", "0".repeat(64));
        assert!(parse_image(&format!("otel/x:latest@{digest}")).is_ok());
        assert!(parse_image(&format!("otel/x@{digest}")).is_ok());
    }

    #[test]
    fn image_malformed_is_rejected() {
        assert!(parse_image("").is_err());
        assert!(parse_image("otel/x:1.0 ; rm -rf").is_err());
        assert!(parse_image("Otel/X:1.0").is_err());
        assert!(parse_image("otel/x:1.0@sha256:abc").is_err());
        assert!(parse_image(&format!("otel/x:1.0@sha256:{}", "A".repeat(64))).is_err());
        assert!(parse_image("otel/x:-bad").is_err());
        assert!(parse_image("otel//x:1.0").is_err());
    }

    #[test]
    fn quantities() {
        assert_eq!(parse_quantity("100m").unwrap(), 0.1);
        assert_eq!(parse_quantity("0.5").unwrap(), 0.5);
        assert_eq!(parse_quantity("2").unwrap(), 2.0);
        assert_eq!(parse_quantity("128Mi").unwrap(), 128.0 * 1024.0 * 1024.0);
        assert_eq!(parse_quantity("1G").unwrap(), 1e9);
        assert!(parse_quantity("").is_err());
        assert!(parse_quantity("0").is_err());
        assert!(parse_quantity("1.2.3").is_err());
        assert!(parse_quantity("10 Mi").is_err());
        assert!(parse_quantity("10mb").is_err());
        assert!(parse_quantity("-1").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("200ms").unwrap(), 0.2);
        assert_eq!(parse_duration("1s").unwrap(), 1.0);
        assert_eq!(parse_duration("1m30s").unwrap(), 90.0);
        assert_eq!(parse_duration("1.5h").unwrap(), 5400.0);
        assert!((parse_duration("250µs").unwrap() - 0.00025).abs() < 1e-12);
        assert!(parse_duration("").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("5").is_err());
        assert!(parse_duration("5 s").is_err());
        assert!(parse_duration("-5s").is_err());
        assert!(parse_duration("5d").is_err());
    }

    #[test]
    fn percent_round_trip() {
        assert_eq!(percent_encode("checkout-api.v2"), "checkout-api.v2");
        assert_eq!(percent_encode("a,b=c d"), "a%2Cb%3Dc%20d");
        assert_eq!(
            percent_decode("Bearer%20abc%3D%3D").unwrap(),
            "Bearer abc=="
        );
        assert_eq!(percent_decode("a+b").unwrap(), "a+b");
        assert!(percent_decode("bad%2").is_err());
        assert!(percent_decode("bad%zz").is_err());
        assert!(percent_decode("%ff").is_err());
    }
}
