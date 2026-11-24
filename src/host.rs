//! Wire types for fields deckwatch accepts but the SDK does not expose yet.
//!
//! deckwatch's copy of the SDK types (`src/plugins.rs` in deckwatch) has been
//! ahead of `deckwatch-plugin-sdk` main since deckwatch 0.6.4. It already
//! deserializes:
//!
//! - `SidecarSpec.{cpu_request, cpu_limit, memory_request, memory_limit,
//!   volumes, volume_mounts}` (SDK PR #13), and
//! - `PluginMetadata.min_deckwatch_version` (SDK PR #11).
//!
//! The collector sidecar needs volumes (to mount its config and the exporter
//! headers) and separate requests and limits. This module serializes those
//! fields itself and splices them into the JSON of the SDK structs, so the
//! rest of the plugin keeps using the SDK types. Delete it once the SDK ships
//! these fields.

use deckwatch_plugin_sdk::{PluginMetadata, PluginResult, ResourceProvisionResult};
use serde::Serialize;
use serde_json::Value;

/// Oldest deckwatch release whose plugin host understands every field this
/// plugin emits. 0.6.4 is also the first release that checks
/// `min_deckwatch_version`; earlier releases load the plugin anyway and drop
/// the sidecar volumes, so the collector would run the image's default config.
pub const MIN_DECKWATCH_VERSION: &str = "0.6.4";

/// A plain `name=value` environment variable on the sidecar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// deckwatch's `VolumeSpec`. `volume_type` is `configMap` or `secret` here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Volume {
    pub name: String,
    pub volume_type: String,
    pub source_name: String,
}

/// deckwatch's `VolumeMountSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    pub read_only: bool,
}

/// deckwatch's `SidecarSpec`, limited to the fields this plugin sets.
///
/// No `port`: the collector only listens on 127.0.0.1, so declaring a
/// container port would suggest it is reachable from other pods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sidecar {
    pub name: String,
    pub image: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<EnvVar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_limit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_request: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<Volume>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volume_mounts: Vec<VolumeMount>,
}

/// Serialize a value that contains only strings, numbers, bools, vecs and
/// string-keyed maps. That cannot fail, so a failure is a programming error.
fn to_value<T: Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("plugin output types always serialize")
}

/// `PluginResult` JSON with `sidecars` replaced by host sidecars.
pub fn plugin_result(result: &PluginResult, sidecars: &[Sidecar]) -> Value {
    let mut v = to_value(result);
    v["sidecars"] = to_value(&sidecars);
    v
}

/// `ResourceProvisionResult` JSON with `sidecars` replaced by host sidecars.
pub fn provision_result(result: &ResourceProvisionResult, sidecars: &[Sidecar]) -> Value {
    let mut v = to_value(result);
    v["sidecars"] = to_value(&sidecars);
    v
}

/// `PluginMetadata` JSON with `min_deckwatch_version` added.
pub fn metadata(meta: &PluginMetadata) -> Value {
    let mut v = to_value(meta);
    v["min_deckwatch_version"] = Value::from(MIN_DECKWATCH_VERSION);
    v
}
