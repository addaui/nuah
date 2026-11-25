// SPDX-License-Identifier: Apache-2.0
//! An API key can be limited to some permissions and an expiry, acts for its person, is named in what it did, and cannot make
//! a broader key ([ADR 0132]).
//!
//! [ADR 0132]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=dbc2140764d64b149f93a6011782bd54
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

struct Notebook;
impl Plugin for Notebook {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "@test/notebook", "version": "1", "namespaces": ["notebook"],
            "requires": {"capabilities": ["emit"]},
            "events": {"notebook.written": {}},
            "permissions": {"notebook.read": {"default": ["admin", "member"]}, "notebook.write": {"default": ["admin", "member"]}},
            "operations": {
                "notebook.read": {"kind": "query", "permission": "notebook.read"},
                "notebook.write": {"kind": "command", "permission": "notebook.write"}
            }
        }))
        .unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("notebook.read", |_, _| Ok(json!({"ok": true})));
        b.op("notebook.write", |tx, _| {
            tx.emit("notebook.written", Audience::subject(&tx.who().subject.clone()), json!({}))?;
            Ok(json!({"ok": true}))
        });
    }
}

fn world() -> (Core, Arc<FixedClock>) {
    let clock = FixedClock::at(1_000_000);
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(TestClock(clock.clone())), Arc::new(StdIds), Arc::new(Notebook)];
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-random", "@test/notebook"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"username": "sara", "password_hash": AuthPassword::hash_password("sara-password-1"), "roles": ["member"]}]}));
    p.grants.insert("@test/notebook".into(), vec!["emit".into()]);
    (Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}")), clock)
}
fn sara(c: &Core) -> Identity {
    let who = c.call_as(&Identity::new("", "", &[]), "auth.sign_in", json!({"username": "sara", "password": "sara-password-1"}), None).unwrap().output["who"].clone();
    serde_json::from_value(who).unwrap()
}
fn with_key(c: &Core, key: &str, op: &str) -> Result<Value> {
    c.call(CallRequest { credential: Some(key.into()), op: op.into(), input: json!({}), key: None, contract: None }).map(|r| r.output)
}

#[test]
fn a_key_limited_to_some_permissions_can_do_only_those() {
    let (c, _) = world();
    let me = sara(&c);
    let made = c.call_as(&me, "auth.keys.create", json!({"name": "reader", "permissions": ["notebook.read"]}), None).unwrap().output;
    let key = made["key"].as_str().unwrap();
    assert_eq!(made["permissions"], json!(["notebook.read"]));
    assert!(with_key(&c, key, "notebook.read").is_ok());
    let e = with_key(&c, key, "notebook.write").unwrap_err();
    assert_eq!((e.category, e.reason.as_deref()), (Category::Denied, Some("auth.key_limited")));
    assert!(with_key(&c, key, "auth.who").is_ok(), "an operation that needs no permission stays open");
    // her own session is not limited, and a key with no limits is as before
    assert!(c.call_as(&me, "notebook.write", json!({}), None).is_ok());
    let open = c.call_as(&me, "auth.keys.create", json!({"name": "all"}), None).unwrap().output["key"].as_str().unwrap().to_string();
    assert!(with_key(&c, &open, "notebook.write").is_ok());
    // the list shows the limits, never the key
    let list = c.call_as(&me, "auth.keys.list", json!({}), None).unwrap().output;
    assert!(list["keys"].as_array().unwrap().iter().any(|k| k["permissions"] == json!(["notebook.read"])));
    assert!(!list.to_string().contains(key));
    assert!(c.call_as(&me, "auth.keys.create", json!({"name": "bad", "permissions": []}), None).is_err(), "a limit of nothing is not a limit");
}

#[test]
fn a_key_expires_and_then_is_nobody() {
    let (c, clock) = world();
    let me = sara(&c);
    let key = c.call_as(&me, "auth.keys.create", json!({"name": "short", "expires_in_days": 1}), None).unwrap().output["key"].as_str().unwrap().to_string();
    assert!(with_key(&c, &key, "auth.who").is_ok());
    clock.advance(2 * 86_400_000);
    assert_eq!(with_key(&c, &key, "auth.who").unwrap_err().category, Category::Unauthenticated);
}

#[test]
fn a_limited_key_cannot_make_or_revoke_keys_but_an_unlimited_one_acts_as_its_person() {
    let (c, _) = world();
    let me = sara(&c);
    let key = c.call_as(&me, "auth.keys.create", json!({"name": "narrow", "permissions": ["notebook.read"]}), None).unwrap().output["key"].as_str().unwrap().to_string();
    for (op, input) in [("auth.keys.create", json!({"name": "wider"})), ("auth.keys.revoke", json!({"id": "x"}))] {
        let e = c.call(CallRequest { credential: Some(key.clone()), op: op.into(), input, key: None, contract: None }).unwrap_err();
        assert_eq!(e.reason.as_deref(), Some("auth.key_cannot_mint"), "{op}");
    }
    // a key that only has an end date is limited too: it could otherwise make a key that never ends
    let dated = c.call_as(&me, "auth.keys.create", json!({"name": "dated", "expires_in_days": 5}), None).unwrap().output["key"].as_str().unwrap().to_string();
    let e = c.call(CallRequest { credential: Some(dated), op: "auth.keys.create".into(), input: json!({"name": "forever"}), key: None, contract: None }).unwrap_err();
    assert_eq!(e.reason.as_deref(), Some("auth.key_cannot_mint"));
    // a key with no limits is as it was
    let open = c.call_as(&me, "auth.keys.create", json!({"name": "open"}), None).unwrap().output["key"].as_str().unwrap().to_string();
    assert!(c.call(CallRequest { credential: Some(open), op: "auth.keys.create".into(), input: json!({"name": "another"}), key: None, contract: None }).is_ok());
}

#[test]
fn what_a_key_does_names_the_key_in_the_events() {
    let (c, _) = world();
    let me = sara(&c);
    let made = c.call_as(&me, "auth.keys.create", json!({"name": "agent"}), None).unwrap().output;
    let (key, id) = (made["key"].as_str().unwrap().to_string(), made["id"].as_str().unwrap().to_string());
    let c0 = c.pulse_now().unwrap();
    with_key(&c, &key, "notebook.write").unwrap();
    c.call_as(&me, "notebook.write", json!({}), None).unwrap();
    let evs = c.pulse(&me, Some(&c0), 50).unwrap().events;
    let ev: Vec<&PulseEvent> = evs.iter().filter(|e| e.kind == "notebook.written").collect();
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].via.as_deref(), Some(id.as_str()), "the key is named");
    assert_eq!(ev[1].via, None, "her own session has no key");
    assert_eq!(ev[0].actor, me.subject, "it still acted for her");
}
