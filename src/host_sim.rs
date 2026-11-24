//! Test-only model of how deckwatch merges `apply()` output into a
//! Deployment, so tests can check the resulting pod spec.
//!
//! [`merge`] mirrors deckwatch 0.6.x (`src/plugins.rs`: `apply_env_vars`,
//! `apply_sidecars`, `build_kube_env_var`, `volume_spec_to_k8s`,
//! `build_resources`): env vars go into the first container unless the name
//! exists, sidecars are added unless a container with that name exists,
//! volumes are added unless the name exists, and injected names are recorded
//! in `deckwatch.plugin-env/<plugin>` and `deckwatch.plugin-sidecar/<plugin>`.
//!
//! [`unmerge`] is what deckwatch does not do yet: undo a plugin's previous
//! injection from those records when the plugin is disabled or before
//! re-applying it. It also needs the plugin's volumes recorded, which
//! deckwatch does not write today ([`VOLUME_ANNOTATION`]).

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

pub const ENV_ANNOTATION: &str = "deckwatch.plugin-env/";
pub const SIDECAR_ANNOTATION: &str = "deckwatch.plugin-sidecar/";
/// Proposed: deckwatch does not record injected volumes today.
pub const VOLUME_ANNOTATION: &str = "deckwatch.plugin-volume/";

#[derive(Debug, Clone, PartialEq)]
pub struct Deployment {
    pub annotations: BTreeMap<String, String>,
    pub pod: Value,
}

fn list<'a>(obj: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    obj.as_object_mut()
        .expect("object")
        .entry(key)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("array")
}

fn kube_env(spec: &Value) -> Value {
    if let Some(skr) = spec.pointer("/value_from/secret_key_ref") {
        return json!({
            "name": spec["name"],
            "valueFrom": { "secretKeyRef": { "name": skr["name"], "key": skr["key"], "optional": false } },
        });
    }
    match spec["value"].as_str() {
        Some(v) if !v.is_empty() => json!({ "name": spec["name"], "value": v }),
        _ => json!({ "name": spec["name"] }),
    }
}

fn kube_volume(spec: &Value) -> Value {
    match spec["volume_type"].as_str() {
        Some("configMap") => {
            json!({ "name": spec["name"], "configMap": { "name": spec["source_name"] } })
        }
        Some("secret") => {
            json!({ "name": spec["name"], "secret": { "secretName": spec["source_name"] } })
        }
        _ => json!({ "name": spec["name"] }),
    }
}

fn kube_container(spec: &Value) -> Value {
    let mut c = Map::new();
    c.insert("name".into(), spec["name"].clone());
    c.insert("image".into(), spec["image"].clone());
    let env: Vec<Value> = spec["env"]
        .as_array()
        .into_iter()
        .flatten()
        .map(kube_env)
        .collect();
    if !env.is_empty() {
        c.insert("env".into(), Value::from(env));
    }
    if let Some(port) = spec["port"].as_i64() {
        c.insert("ports".into(), json!([{ "containerPort": port }]));
    }
    let pick = |split: &str, legacy: &str| {
        spec.get(split)
            .filter(|v| !v.is_null())
            .or_else(|| spec.get(legacy).filter(|v| !v.is_null()))
            .cloned()
    };
    let mut requests = Map::new();
    let mut limits = Map::new();
    for (resource, req, lim, legacy) in [
        ("cpu", "cpu_request", "cpu_limit", "cpu"),
        ("memory", "memory_request", "memory_limit", "memory"),
    ] {
        if let Some(v) = pick(req, legacy) {
            requests.insert(resource.into(), v);
        }
        if let Some(v) = pick(lim, legacy) {
            limits.insert(resource.into(), v);
        }
    }
    if !requests.is_empty() || !limits.is_empty() {
        let mut resources = Map::new();
        if !requests.is_empty() {
            resources.insert("requests".into(), Value::Object(requests));
        }
        if !limits.is_empty() {
            resources.insert("limits".into(), Value::Object(limits));
        }
        c.insert("resources".into(), Value::Object(resources));
    }
    let mounts: Vec<Value> = spec["volume_mounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| {
            let mut mount = json!({ "name": m["name"], "mountPath": m["mount_path"] });
            if m["read_only"].as_bool() == Some(true) {
                mount["readOnly"] = json!(true);
            }
            mount
        })
        .collect();
    if !mounts.is_empty() {
        c.insert("volumeMounts".into(), Value::from(mounts));
    }
    Value::Object(c)
}

/// deckwatch 0.6.x: merge one plugin's `apply()` result into a Deployment.
pub fn merge(dep: &mut Deployment, plugin: &str, result: &Value) {
    let env_vars = result["env_vars"].as_array().cloned().unwrap_or_default();
    if !env_vars.is_empty() {
        if let Some(primary) = dep.pod["containers"].get_mut(0) {
            let env = list(primary, "env");
            let mut injected = Vec::new();
            for spec in &env_vars {
                let name = spec["name"].as_str().expect("env var name");
                if env.iter().any(|e| e["name"] == name) {
                    continue;
                }
                env.push(kube_env(spec));
                injected.push(name.to_string());
            }
            if !injected.is_empty() {
                dep.annotations
                    .insert(format!("{ENV_ANNOTATION}{plugin}"), injected.join(","));
            }
        }
    }

    let sidecars = result["sidecars"].as_array().cloned().unwrap_or_default();
    let mut injected_volumes = Vec::new();
    for spec in &sidecars {
        for vol in spec["volumes"].as_array().into_iter().flatten() {
            let volumes = list(&mut dep.pod, "volumes");
            if !volumes.iter().any(|v| v["name"] == vol["name"]) {
                volumes.push(kube_volume(vol));
                injected_volumes.push(vol["name"].as_str().expect("volume name").to_string());
            }
        }
    }
    let mut injected = Vec::new();
    for spec in &sidecars {
        let containers = list(&mut dep.pod, "containers");
        if containers.iter().any(|c| c["name"] == spec["name"]) {
            continue;
        }
        containers.push(kube_container(spec));
        injected.push(spec["name"].as_str().expect("sidecar name").to_string());
    }
    if !injected.is_empty() {
        dep.annotations
            .insert(format!("{SIDECAR_ANNOTATION}{plugin}"), injected.join(","));
    }
    if !injected_volumes.is_empty() {
        dep.annotations.insert(
            format!("{VOLUME_ANNOTATION}{plugin}"),
            injected_volumes.join(","),
        );
    }
}

fn remove_named(obj: &mut Value, key: &str, names: &str) {
    let names: Vec<&str> = names.split(',').collect();
    if let Some(items) = obj.get_mut(key).and_then(Value::as_array_mut) {
        items.retain(|item| !names.contains(&item["name"].as_str().unwrap_or_default()));
        if items.is_empty() {
            obj.as_object_mut().expect("object").remove(key);
        }
    }
}

/// Proposed deckwatch behaviour: remove what a plugin recorded as injected.
pub fn unmerge(dep: &mut Deployment, plugin: &str) {
    if let Some(names) = dep
        .annotations
        .remove(&format!("{SIDECAR_ANNOTATION}{plugin}"))
    {
        remove_named(&mut dep.pod, "containers", &names);
    }
    if let Some(names) = dep
        .annotations
        .remove(&format!("{VOLUME_ANNOTATION}{plugin}"))
    {
        remove_named(&mut dep.pod, "volumes", &names);
    }
    if let Some(names) = dep.annotations.remove(&format!("{ENV_ANNOTATION}{plugin}")) {
        if let Some(primary) = dep.pod["containers"].get_mut(0) {
            remove_named(primary, "env", &names);
        }
    }
}
