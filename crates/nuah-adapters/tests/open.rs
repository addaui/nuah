// SPDX-License-Identifier: Apache-2.0
//! `auth-open` (ADR 0076): no sign-in for the app, a password for the console.

use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

/// A plugin with one operation only an administrator may call (what the console's operations are).
struct Secret;
impl Plugin for Secret {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/secret", "version": "1", "namespaces": ["t"],
            "permissions": {"t.admin": {"description": "x", "default": ["admin"]}, "t.use": {"description": "x", "default": ["member"]}},
            "operations": {"t.secret": {"kind": "query", "permission": "t.admin", "output": {"type": "object"}}, "t.use": {"kind": "query", "permission": "t.use", "output": {"type": "object"}}}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("t.secret", |_tx, _| Ok(json!({"ok": true})));
        b.op("t.use", |_tx, _| Ok(json!({"ok": true})));
    }
}

fn core(settings: Value) -> Core {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-open", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@test/secret"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-open".into(), settings);
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthOpen), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds), Arc::new(Secret)];
    Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"))
}
fn call(c: &Core, credential: Option<&str>, op: &str, input: Value) -> Result<Value> {
    c.call(CallRequest { op: op.into(), input, credential: credential.map(String::from), key: None, contract: None }).map(|r| r.output)
}
fn open() -> Core { core(json!({"owner_name": "Sam", "admin_password_hash": AuthPassword::hash_password("console-secret")})) }

#[test]
fn the_app_needs_no_sign_in_and_the_visitor_is_the_owner_but_not_the_admin() {
    let c = open();
    let who = call(&c, None, "auth.who", json!({})).unwrap();
    assert_eq!((who["subject"].as_str(), who["display"].as_str()), (Some("owner"), Some("Sam")));
    assert_eq!(who["roles"], json!(["member"]));
    assert!(call(&c, None, "t.use", json!({})).is_ok(), "an Owner uses the app");
    assert_eq!(call(&c, None, "t.secret", json!({})).unwrap_err().category, Category::Denied, "and cannot administer the instance");
}

#[test]
fn the_admin_password_opens_a_session_that_adds_admin_to_the_same_person() {
    let c = open();
    let s = call(&c, None, "auth.sign_in", json!({"password": "console-secret"})).unwrap();
    let token = s["token"].as_str().unwrap().to_string();
    assert_eq!(s["who"]["subject"], "owner", "the same subject, so what the Owner made stays theirs");
    assert!(call(&c, Some(&token), "t.secret", json!({})).is_ok());
    assert!(call(&c, Some(&token), "t.use", json!({})).is_ok(), "an Admin session still uses the app");
    call(&c, Some(&token), "auth.sign_out", json!({"token": token})).unwrap();
    assert_eq!(call(&c, Some(&token), "t.secret", json!({})).unwrap_err().category, Category::Denied, "signed out: the Owner again, not nobody");
    assert!(call(&c, Some(&token), "t.use", json!({})).is_ok());
}

#[test]
fn a_wrong_or_missing_admin_password_opens_nothing() {
    let c = open();
    assert_eq!(call(&c, None, "auth.sign_in", json!({"password": "nope"})).unwrap_err().reason.as_deref(), Some("auth.bad_credentials"));
    let none = core(json!({}));
    assert_eq!(call(&none, None, "auth.sign_in", json!({"password": "anything"})).unwrap_err().reason.as_deref(), Some("auth.bad_credentials"), "no admin password set: nobody can be Admin");
    assert!(call(&none, None, "t.use", json!({})).is_ok(), "the app still works");
}

#[test]
fn an_unknown_session_is_the_owner_not_nobody() {
    let c = open();
    assert!(call(&c, Some("an-old-cookie"), "t.use", json!({})).is_ok());
    assert!(call(&c, Some("an-old-cookie"), "t.secret", json!({})).is_err());
}

#[test]
fn the_admin_session_ends_by_itself() {
    let c = core(json!({"admin_password_hash": AuthPassword::hash_password("pw-pw-pw-pw"), "admin_session_minutes": 5}));
    let t = call(&c, None, "auth.sign_in", json!({"password": "pw-pw-pw-pw"})).unwrap();
    assert!(t["expires_at"].as_i64().unwrap() - c.now() <= 5 * 60_000 + 1000);
}
