// SPDX-License-Identifier: Apache-2.0
//! Accounts of `auth-password`: kept in the database, identified by an opaque subject that survives a rename ([ADR 0039]).
//!
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn core() -> Core {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"name": "Root", "password_hash": AuthPassword::hash_password("first-password"), "roles": ["admin"]}]}));
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds)];
    Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"))
}

fn anon() -> Identity {
    Identity::new("", "", &[])
}

fn sign_in(c: &Core, name: &str, pw: &str) -> Result<Value> {
    c.call_as(&anon(), "auth.sign_in", json!({"name": name, "password": pw}), None).map(|r| r.output)
}

fn as_who(c: &Core, who: &Value, op: &str, input: Value) -> Result<Value> {
    let who: Identity = serde_json::from_value(who.clone()).unwrap();
    c.call_as(&who, op, input, None).map(|r| r.output)
}

#[test]
fn the_subject_is_an_opaque_id_not_the_login_name_and_survives_a_rename() {
    let c = core();
    let root = sign_in(&c, "root", "first-password").unwrap()["who"].clone(); // names match without case
    let subject = root["subject"].as_str().unwrap().to_string();
    assert_ne!(subject.to_lowercase(), "root");

    as_who(&c, &root, "auth.account.update", json!({"subject": subject, "name": "boss"})).unwrap();
    assert!(sign_in(&c, "root", "first-password").is_err(), "the old name no longer signs in");
    let again = sign_in(&c, "boss", "first-password").unwrap()["who"].clone();
    assert_eq!(again["subject"], root["subject"], "the same person keeps the same subject");
}

#[test]
fn accounts_are_managed_by_the_people_allowed_to_and_the_last_admin_stays() {
    let c = core();
    let root = sign_in(&c, "Root", "first-password").unwrap()["who"].clone();
    let made = as_who(&c, &root, "auth.account.create", json!({"name": "sam", "password": "sam-secret-1", "roles": ["member"]})).unwrap();
    let sam = sign_in(&c, "sam", "sam-secret-1").unwrap()["who"].clone();
    assert_eq!(sam["subject"], made["subject"]);
    // a member may not manage accounts, and a taken name is a conflict
    assert!(as_who(&c, &sam, "auth.account.list", json!({})).is_err());
    let taken = as_who(&c, &root, "auth.account.create", json!({"name": "SAM", "password": "another-pass"})).unwrap_err();
    assert_eq!(taken.reason.as_deref(), Some("auth.name_taken"));
    // the only administrator cannot be disabled or demoted
    let root_subject = root["subject"].as_str().unwrap();
    for change in [json!({"disabled": true}), json!({"roles": ["member"]})] {
        let mut input = json!({"subject": root_subject});
        input.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
        assert_eq!(as_who(&c, &root, "auth.account.update", input).unwrap_err().reason.as_deref(), Some("auth.last_admin"));
    }
    // a disabled account cannot sign in, and a session it already holds stops working
    let token = sign_in(&c, "sam", "sam-secret-1").unwrap()["token"].as_str().unwrap().to_string();
    assert!(c.authenticate(Some(&token)).is_ok());
    as_who(&c, &root, "auth.account.update", json!({"subject": made["subject"], "disabled": true})).unwrap();
    assert!(sign_in(&c, "sam", "sam-secret-1").is_err());
    assert!(c.authenticate(Some(&token)).is_err());
    // a new password replaces the old one
    as_who(&c, &root, "auth.account.set_password", json!({"subject": root_subject, "password": "second-password"})).unwrap();
    assert!(sign_in(&c, "root", "first-password").is_err());
    assert!(sign_in(&c, "root", "second-password").is_ok());
}
