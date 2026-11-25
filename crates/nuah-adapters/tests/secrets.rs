// SPDX-License-Identifier: Apache-2.0
//! A person's secret kept for a plugin can be set, replaced and removed, shows only its last four characters, is read back by
//! nobody but the plugin's own outbound work for that person, and never reaches an event ([ADR 0138]).
//!
//! [ADR 0138]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=12864c0f5dba4225887439c5714e66a0
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

struct Vault(&'static str);
impl Plugin for Vault {
    fn manifest(&self) -> Manifest {
        let ns = self.0;
        serde_json::from_value(json!({
            "id": format!("@test/{ns}"), "version": "1", "namespaces": [ns],
            "requires": {"capabilities": ["secrets", "emit", "call"]},
            "events": {format!("{ns}.set"): {}},
            "permissions": {format!("{ns}.use"): {"default": ["member"]}},
            "operations": {
                format!("{ns}.set"): {"kind": "command", "permission": format!("{ns}.use")},
                format!("{ns}.hint"): {"kind": "query", "permission": format!("{ns}.use")},
                format!("{ns}.delete"): {"kind": "command", "permission": format!("{ns}.use")},
                format!("{ns}.use"): {"kind": "outbound", "permission": format!("{ns}.use")}
            }
        }))
        .unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        let ns = self.0;
        b.op(&format!("{ns}.set"), move |tx, i| {
            tx.set_secret("api_key", i["value"].as_str().unwrap())?;
            // a careless plugin puts the value in an event: the Core hides it
            tx.emit(&format!("{ns}.set"), Audience::subject(&tx.who().subject.clone()), json!({"echo": i["value"]}))?;
            Ok(json!({}))
        });
        b.op(&format!("{ns}.hint"), |tx, _| Ok(json!({"hint": tx.secret_hint("api_key")?})));
        b.op(&format!("{ns}.delete"), |tx, _| Ok(json!({"deleted": tx.delete_secret("api_key")?})));
        b.outbound(&format!("{ns}.use"), |cx, _| Ok(json!({"key": cx.secret("api_key")?})));
    }
}
struct NoCap;
impl Plugin for NoCap {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/nocap", "version": "1", "namespaces": ["nocap"], "permissions": {"nocap.use": {"default": ["member"]}},
            "operations": {"nocap.try": {"kind": "command", "permission": "nocap.use"}}}))
        .unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("nocap.try", |tx, _| {
            tx.set_secret("x", "y")?;
            Ok(json!({}))
        });
    }
}

fn world() -> World {
    let clock = FixedClock::at(1_000_000);
    let seen = Arc::new(parking_lot::Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Vault("vaulta")));
    cat.0.push(Arc::new(Vault("vaultb")));
    cat.0.push(Arc::new(NoCap));
    let mut p = profile(&["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/vaulta", "@test/vaultb", "@test/nocap"]);
    for id in ["@test/vaulta", "@test/vaultb"] {
        p.grants.insert(id.into(), vec!["secrets".into(), "emit".into(), "call".into()]);
    }
    let dist = Distribution { name: "test".into(), catalog: cat, profile: p };
    let core = Core::start(&dist).unwrap_or_else(|p| panic!("start failed: {p:#?}"));
    World { core, clock, seen }
}
fn call(w: &World, who: &Identity, op: &str, input: Value) -> Result<Value> {
    w.core.call_as(who, op, input, None).map(|r| r.output)
}

#[test]
fn a_secret_is_the_persons_and_the_plugins_own_and_only_outbound_work_reads_it() {
    let w = world();
    let (a, b) = (person("alice", &["member"]), person("bob", &["member"]));
    assert_eq!(call(&w, &a, "vaulta.hint", json!({})).unwrap()["hint"], Value::Null, "nothing yet");
    call(&w, &a, "vaulta.set", json!({"value": "sk-alice-1234"})).unwrap();
    assert_eq!(call(&w, &a, "vaulta.hint", json!({})).unwrap()["hint"], "1234", "only the last four");
    assert_eq!(call(&w, &a, "vaulta.use", json!({})).unwrap()["key"], "sk-alice-1234", "the plugin's own outbound work reads it");
    // another person has none; another plugin has none
    assert_eq!(call(&w, &b, "vaulta.use", json!({})).unwrap()["key"], Value::Null);
    assert_eq!(call(&w, &a, "vaultb.use", json!({})).unwrap()["key"], Value::Null);
    call(&w, &b, "vaulta.set", json!({"value": "sk-bob-9999"})).unwrap();
    assert_eq!(call(&w, &a, "vaulta.use", json!({})).unwrap()["key"], "sk-alice-1234", "bob's does not replace alice's");
    // replaced, then removed
    call(&w, &a, "vaulta.set", json!({"value": "sk-alice-5678"})).unwrap();
    assert_eq!(call(&w, &a, "vaulta.hint", json!({})).unwrap()["hint"], "5678");
    assert_eq!(call(&w, &a, "vaulta.delete", json!({})).unwrap()["deleted"], true);
    assert_eq!(call(&w, &a, "vaulta.use", json!({})).unwrap()["key"], Value::Null);
    assert_eq!(call(&w, &a, "vaulta.delete", json!({})).unwrap()["deleted"], false);
}

#[test]
fn a_secret_never_reaches_an_event_even_when_the_plugin_puts_it_there() {
    let w = world();
    let a = person("alice", &["member"]);
    let c0 = w.core.pulse_now().unwrap();
    call(&w, &a, "vaulta.set", json!({"value": "sk-super-secret"})).unwrap();
    let evs = w.core.pulse(&a, Some(&c0), 50).unwrap().events;
    assert_eq!(evs.len(), 1);
    assert!(!serde_json::to_string(&evs[0].data).unwrap().contains("sk-super-secret"), "{:?}", evs[0].data);
}

#[test]
fn a_plugin_without_the_capability_cannot_keep_a_secret() {
    let w = world();
    assert!(call(&w, &person("alice", &["member"]), "nocap.try", json!({})).is_err());
}
