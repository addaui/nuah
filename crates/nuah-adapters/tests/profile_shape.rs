// SPDX-License-Identifier: Apache-2.0
//! The identity file has one entry per plugin and plugin ids are scoped (ADR 0042).

use nuah_adapters::{AdapterMemory, AuthLocal, PolicyRoles, StdClock, StdIds};
use nuah_kernel::{setup, Catalog, Core, Distribution, Manifest, Plugin};
use serde_json::{json, Value};
use std::sync::Arc;

fn catalog() -> Catalog {
    Catalog(vec![Arc::new(AdapterMemory), Arc::new(AuthLocal), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds)])
}

fn dist(file: Value) -> Distribution {
    Distribution { name: "t".into(), catalog: catalog(), profile: setup::profile_from_json(&file).unwrap() }
}

struct Odd(&'static str, Value);
impl Plugin for Odd {
    fn manifest(&self) -> Manifest {
        let mut m = json!({"id": self.0, "version": "1"});
        for (k, v) in self.1.as_object().unwrap() {
            m[k] = v.clone();
        }
        serde_json::from_value(m).unwrap()
    }
}

#[test]
fn the_new_shape_has_one_entry_per_plugin_and_a_disabled_plugin_keeps_its_settings() {
    let file = json!({"name": "t", "plugins": {
        "@nuah/db-memory": {}, "@nuah/auth-local": {}, "@nuah/policy-roles": {}, "@nuah/clock-system": {}, "@nuah/ids-random": {},
        "@nuah/policy-allow-all": {"enabled": false, "settings": {"note": "kept"}}
    }});
    let p = setup::profile_from_json(&file).unwrap();
    assert!(p.enabled.contains(&"@nuah/db-memory".to_string()));
    assert!(!p.enabled.contains(&"@nuah/policy-allow-all".to_string()));
    assert_eq!(p.settings["@nuah/policy-allow-all"]["note"], "kept");
    // written back, the switched-off plugin is still there with its settings and enabled false
    let out = serde_json::to_value(&p).unwrap();
    assert_eq!(out["plugins"]["@nuah/policy-allow-all"], json!({"enabled": false, "settings": {"note": "kept"}}));
    assert_eq!(out["plugins"]["@nuah/db-memory"], json!({"enabled": true}));
    assert!(out.get("enabled").is_none() && out.get("settings").is_none() && out.get("grants").is_none(), "the old keys are never written");
    assert!(Core::check(&Distribution { name: "t".into(), catalog: Catalog(catalog().0), profile: p }).is_empty(), "a switched-off plugin with settings is not a Problem");
}

#[test]
fn the_old_shape_and_the_old_capability_name_are_refused_with_the_key_named() {
    let e = setup::profile_from_json(&json!({"name": "t", "enabled": ["db-memory"]})).unwrap_err();
    assert!(e.contains("enabled"), "{e}");
    let mut p = setup::profile_from_json(&json!({"name": "t", "plugins": {"@nuah/auth-local": {"grants": ["host"]}}})).unwrap();
    p.enabled.push("@nuah/auth-local".into());
    let problems = Core::check(&Distribution { name: "t".into(), catalog: catalog(), profile: p });
    assert!(problems.iter().any(|p| p.message.contains("host")), "{problems:?}");
}

#[test]
fn a_typo_in_an_entry_is_an_error_not_a_silent_default() {
    let e = setup::profile_from_json(&json!({"name": "t", "plugins": {"@nuah/db-memory": {"enable": true}}})).unwrap_err();
    assert!(e.contains("enable"), "{e}");
}

#[test]
fn an_id_must_be_scoped_and_the_platform_never_needs_a_product() {
    let mut list = catalog().0;
    list.push(Arc::new(Odd("unscoped", json!({}))));
    list.push(Arc::new(Odd("@acme/widget", json!({}))));
    list.push(Arc::new(Odd("@nuah/greedy", json!({"requires": {"plugins": ["@acme/widget"]}}))));
    let mut profile = setup::profile_from_json(&json!({"name": "t", "plugins": {
        "@nuah/db-memory": {}, "@nuah/auth-local": {}, "@nuah/policy-roles": {}, "@nuah/clock-system": {}, "@nuah/ids-random": {},
        "@acme/widget": {}, "@nuah/greedy": {}
    }})).unwrap();
    profile.enabled.push("unscoped".into());
    let problems = Core::check(&Distribution { name: "t".into(), catalog: Catalog(list), profile });
    assert!(problems.iter().any(|p| p.code == "bad_plugin_id" && p.plugins == ["unscoped"]), "{problems:?}");
    assert!(problems.iter().any(|p| p.code == "platform_needs_product" && p.plugins == ["@nuah/greedy", "@acme/widget"]), "{problems:?}");
}

#[test]
fn describe_and_the_catalog_report_each_plugins_scope() {
    let d = dist(json!({"name": "t", "plugins": {"@nuah/db-memory": {}}}));
    let info = setup::catalog_info(&d);
    assert!(info["plugins"].as_array().unwrap().iter().all(|p| p["scope"] == "nuah"));
}
