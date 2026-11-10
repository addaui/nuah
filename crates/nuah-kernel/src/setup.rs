// SPDX-License-Identifier: Apache-2.0
//! What a setup Face needs, without starting anything: the Catalog as plain data, a strict reader
//! for Profiles, and the secret handling of [ADR 0014].
//!
//! [ADR 0014]: https://docs.addaui.com/nuah/adr/0014-setup-wizard-is-a-separate-mode-with-a-setup-code-and-no-secrets/

use crate::resolver::{Distribution, Profile};
use crate::ports::PORTS;
use serde_json::{json, Map, Value};

const PROFILE_KEYS: [&str; 6] = ["name", "contract", "plugins", "bindings", "permissions", "telemetry"];

/// Read a Profile strictly: an unknown key or a wrong shape is an error, so a typo does not
/// silently switch a plugin off.
pub fn profile_from_json(v: &Value) -> Result<Profile, String> {
    let obj = v.as_object().ok_or("a profile must be a JSON object")?;
    for k in obj.keys() {
        if !PROFILE_KEYS.contains(&k.as_str()) {
            return Err(format!("profile has an unknown key `{k}` (known: {})", PROFILE_KEYS.join(", ")));
        }
    }
    if obj.get("name").and_then(|n| n.as_str()).map_or(true, |n| n.trim().is_empty()) {
        return Err("profile needs a `name`".into());
    }
    serde_json::from_value(v.clone()).map_err(|e| format!("profile is malformed: {e}"))
}

/// A setting that points at its value instead of holding it: `{"$env": "NAME"}` or `{"$file": "/path"}`.
/// Works with container secrets, and a Profile with references can be shared or kept in a repository.
pub fn is_ref(v: &Value) -> bool {
    match v.as_object() {
        Some(o) if o.len() == 1 => o.get("$env").or_else(|| o.get("$file")).is_some_and(|x| x.is_string()),
        _ => false,
    }
}

/// Replace every reference in `v` by the text it points at.
pub fn resolve_refs(v: &Value) -> Result<Value, String> {
    if is_ref(v) {
        let o = v.as_object().unwrap();
        if let Some(name) = o.get("$env").and_then(|x| x.as_str()) {
            return std::env::var(name).map(Value::String).map_err(|_| format!("the environment variable {name} is not set"));
        }
        let path = o.get("$file").and_then(|x| x.as_str()).unwrap_or_default();
        return std::fs::read_to_string(path).map(|s| Value::String(s.trim_end_matches(['\r', '\n']).to_string())).map_err(|e| format!("cannot read {path}: {e}"));
    }
    match v {
        Value::Object(o) => o.iter().map(|(k, x)| resolve_refs(x).map(|x| (k.clone(), x))).collect::<Result<Map<_, _>, _>>().map(Value::Object),
        Value::Array(a) => a.iter().map(resolve_refs).collect::<Result<Vec<_>, _>>().map(Value::Array),
        x => Ok(x.clone()),
    }
}

/// The Profile with every setting's references resolved. A reference that cannot be resolved is a Problem
/// and the setting is left as it was, so the Resolver goes on and reports everything else too.
pub fn resolve_profile_refs(p: &Profile, problems: &mut Vec<crate::resolver::Problem>) -> Profile {
    let mut out = p.clone();
    for (id, v) in out.settings.iter_mut() {
        match resolve_refs(v) {
            Ok(r) => *v = r,
            Err(m) => problems.push(crate::resolver::Problem { code: "bad_secret_ref".into(), plugins: vec![id.clone()], message: format!("settings of {id}: {m}") }),
        }
    }
    out
}

/// Where a setting lives and how a change takes effect (ADR 0033): `x-tier` is `bootstrap` (needed before the
/// database opens) or `operator`; `x-apply` is `live`, `service` (restart that plugin's service) or `restart`.
/// Returns what is missing or misspelled for every top-level property of a settings schema, so a plugin's own
/// test can keep its schema honest.
pub fn settings_annotation_problems(schema: &Value) -> Vec<String> {
    let mut out = vec![];
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else { return out };
    for (name, p) in props {
        match p.get("x-tier").and_then(|t| t.as_str()) {
            Some("bootstrap") | Some("operator") => {}
            other => out.push(format!("{name}: x-tier must be bootstrap or operator, not {other:?}")),
        }
        match p.get("x-apply").and_then(|t| t.as_str()) {
            Some("live") | Some("service") | Some("restart") => {}
            other => out.push(format!("{name}: x-apply must be live, service or restart, not {other:?}")),
        }
    }
    out
}

/// A secret setting is marked `"x-secret": true` in the plugin's `settings_schema`.
fn is_secret(schema: &Value) -> bool {
    schema.get("x-secret") == Some(&Value::Bool(true))
}

fn marker() -> Value {
    json!({"$secret": true})
}

fn is_marker(v: &Value) -> bool {
    v.get("$secret") == Some(&Value::Bool(true))
}

/// Replace every secret value with the marker. Reads the schema; the data never leaves.
pub fn redact(value: &Value, schema: &Value) -> Value {
    if is_ref(value) {
        return value.clone();
    }
    // Nothing is there to hide when the value is empty, and a form needs to see "no accounts yet".
    let empty = matches!(value, Value::Null) || value.as_array().is_some_and(|a| a.is_empty());
    if is_secret(schema) && !empty {
        return marker();
    }
    match value {
        Value::Object(o) => {
            let props = schema.get("properties");
            Value::Object(o.iter().map(|(k, v)| (k.clone(), redact(v, props.and_then(|p| p.get(k)).unwrap_or(&Value::Null)))).collect())
        }
        Value::Array(a) => {
            let items = schema.get("items").unwrap_or(&Value::Null);
            Value::Array(a.iter().map(|v| redact(v, items)).collect())
        }
        v => v.clone(),
    }
}

/// Put the stored value back wherever a draft still holds the marker. A secret that the draft
/// replaces is taken as typed. Arrays are matched by position.
pub fn restore(draft: &Value, stored: &Value, schema: &Value) -> Value {
    if is_marker(draft) {
        return stored.clone();
    }
    match draft {
        Value::Object(o) => {
            let props = schema.get("properties");
            let empty = Map::new();
            let st = stored.as_object().unwrap_or(&empty);
            Value::Object(o.iter().map(|(k, v)| (k.clone(), restore(v, st.get(k).unwrap_or(&Value::Null), props.and_then(|p| p.get(k)).unwrap_or(&Value::Null)))).collect())
        }
        Value::Array(a) => {
            let items = schema.get("items").unwrap_or(&Value::Null);
            let st = stored.as_array().cloned().unwrap_or_default();
            Value::Array(a.iter().enumerate().map(|(i, v)| restore(v, st.get(i).unwrap_or(&Value::Null), items)).collect())
        }
        v => v.clone(),
    }
}

/// Redact all settings of a Profile with their plugins' schemas.
pub fn redact_profile(dist: &Distribution) -> Value {
    let mut p = serde_json::to_value(&dist.profile).unwrap_or(Value::Null);
    if let Some(entries) = p.get_mut("plugins").and_then(|s| s.as_object_mut()) {
        for (id, entry) in entries.iter_mut() {
            let Some(v) = entry.get_mut("settings") else { continue };
            if let Some(pl) = dist.catalog.0.iter().find(|pl| pl.manifest().id == *id) {
                *v = redact(v, &pl.manifest().settings_schema);
            }
        }
    }
    p
}

/// Restore the markers in a draft Profile from the stored one.
pub fn restore_profile(draft: &mut Profile, stored: &Profile, dist: &Distribution) {
    for (id, v) in draft.settings.iter_mut() {
        let schema = dist.catalog.0.iter().find(|pl| pl.manifest().id == *id).map(|pl| pl.manifest().settings_schema).unwrap_or(Value::Null);
        *v = restore(v, stored.settings.get(id).unwrap_or(&Value::Null), &schema);
    }
}

/// The Catalog and the current choices as JSON for a setup Face. Reads manifests only; secrets are
/// redacted.
pub fn catalog_info(dist: &Distribution) -> Value {
    let mut plugins: Vec<Value> = dist
        .catalog
        .0
        .iter()
        .map(|pl| {
            let m = pl.manifest();
            json!({
                "id": m.id, "scope": m.scope(), "version": m.version, "name": m.name, "description": m.description,
                "kind": m.kind(), "quality": m.quality, "runtime": m.runtime,
                "fills": m.fills,
                "defines": m.defines.extension_points.iter().map(|(n, d)| json!({"name": n, "policy": d.policy})).collect::<Vec<_>>(),
                "provides": m.provides.iter().map(|p| &p.extension_point).collect::<Vec<_>>(),
                "requires": {"plugins": m.requires.plugins, "ports": m.requires.ports, "extension_points": m.requires.extension_points, "capabilities": m.requires.capabilities},
                "operations": m.operations.keys().collect::<Vec<_>>(),
                "settings_schema": m.settings_schema,
            })
        })
        .collect();
    plugins.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    json!({"distribution": dist.name, "profile": redact_profile(dist), "core_ports": PORTS, "plugins": plugins})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_profiles() {
        assert!(profile_from_json(&json!({"name":"a","enabeld":[]})).unwrap_err().contains("enabeld"));
        assert!(profile_from_json(&json!({"plugins":{}})).is_err());
        assert!(profile_from_json(&json!({"name":"a","plugins":"x"})).is_err());
        assert!(profile_from_json(&json!({"name":"a","enabled":["x"]})).unwrap_err().contains("enabled"));
        assert_eq!(profile_from_json(&json!({"name":"a","plugins":{"@a/x":{}}})).unwrap().enabled, vec!["@a/x"]);
    }

    #[test]
    fn secrets_are_hidden_and_restored() {
        let schema = json!({"type":"object","properties":{"accounts":{"x-secret":true},"hours":{"type":"integer"}}});
        let stored = json!({"accounts":[{"name":"a","password_hash":"h"}],"hours":3});
        let shown = redact(&stored, &schema);
        assert_eq!(shown, json!({"accounts": {"$secret": true}, "hours": 3}));
        assert!(!shown.to_string().contains("password_hash"));
        // Untouched: the stored value comes back. Replaced: the typed value wins.
        assert_eq!(restore(&shown, &stored, &schema), stored);
        let typed = json!({"accounts":[{"name":"b","password_hash":"x"}],"hours":4});
        assert_eq!(restore(&typed, &stored, &schema), typed);
    }
}
