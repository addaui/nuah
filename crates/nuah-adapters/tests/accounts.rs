// SPDX-License-Identifier: Apache-2.0
//! Accounts of `auth-password`: kept in the database, identified by an opaque subject that survives a rename ([ADR 0039]).
//!
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn core() -> Core {
    // a seed that still says `name`, as every Profile saved before ADR 0089 does
    core_with(json!([{"username": "Root", "password_hash": AuthPassword::hash_password("first-password"), "roles": ["admin"]}]))
}

fn core_with(accounts: Value) -> Core {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({ "accounts": accounts }));
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds)];
    Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"))
}

fn anon() -> Identity {
    Identity::new("", "", &[])
}

fn sign_in(c: &Core, name: &str, pw: &str) -> Result<Value> {
    c.call_as(&anon(), "auth.sign_in", json!({"username": name, "password": pw}), None).map(|r| r.output)
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

    as_who(&c, &root, "auth.account.update", json!({"subject": subject, "username": "boss"})).unwrap();
    assert!(sign_in(&c, "root", "first-password").is_err(), "the old name no longer signs in");
    let again = sign_in(&c, "boss", "first-password").unwrap()["who"].clone();
    assert_eq!(again["subject"], root["subject"], "the same person keeps the same subject");
}

#[test]
fn accounts_are_managed_by_the_people_allowed_to_and_the_last_admin_stays() {
    let c = core();
    let root = sign_in(&c, "Root", "first-password").unwrap()["who"].clone();
    let made = as_who(&c, &root, "auth.account.create", json!({"username": "sam", "password": "sam-secret-1", "roles": ["member"]})).unwrap();
    let sam = sign_in(&c, "sam", "sam-secret-1").unwrap()["who"].clone();
    assert_eq!(sam["subject"], made["subject"]);
    // a member may not manage accounts, and a taken name is a conflict
    assert!(as_who(&c, &sam, "auth.account.list", json!({})).is_err());
    let taken = as_who(&c, &root, "auth.account.create", json!({"username": "SAM", "password": "another-pass"})).unwrap_err();
    assert_eq!(taken.reason.as_deref(), Some("auth.username_taken"));
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

#[test]
fn a_seed_account_may_keep_the_subject_of_the_owner_so_what_they_made_stays_theirs() {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"username": "Sam", "subject": "owner", "password_hash": AuthPassword::hash_password("first-password"), "roles": ["member", "admin"]}]}));
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds)];
    let c = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let who = sign_in(&c, "sam", "first-password").unwrap()["who"].clone();
    assert_eq!(who["subject"], "owner", "the Owner of an instance without sign-in is this account");
    // an account made later by the usual route still gets an opaque id, and cannot take an existing one
    let made = as_who(&c, &who, "auth.account.create", json!({"username": "Bea", "password": "another-password"})).unwrap();
    assert_ne!(made["subject"], "owner");
}

#[test]
fn a_person_changes_their_own_password_by_giving_the_current_one() {
    let c = core();
    let root = sign_in(&c, "Root", "first-password").unwrap()["who"].clone();
    as_who(&c, &root, "auth.account.create", json!({"username": "sam", "password": "sam-secret-1", "roles": ["member"]})).unwrap();
    let sam = sign_in(&c, "sam", "sam-secret-1").unwrap()["who"].clone();

    // a wrong current password changes nothing
    let wrong = as_who(&c, &sam, "auth.password.change", json!({"current": "not-the-password", "new": "sam-secret-2"})).unwrap_err();
    assert_eq!(wrong.reason.as_deref(), Some("auth.bad_credentials"));
    assert!(sign_in(&c, "sam", "sam-secret-1").is_ok());

    // a new password that is too short is refused before anything is checked
    assert!(as_who(&c, &sam, "auth.password.change", json!({"current": "sam-secret-1", "new": "short"})).is_err());

    // the right current password changes it, and only theirs: the old one stops working, the new one works
    as_who(&c, &sam, "auth.password.change", json!({"current": "sam-secret-1", "new": "sam-secret-2"})).unwrap();
    assert!(sign_in(&c, "sam", "sam-secret-1").is_err());
    assert!(sign_in(&c, "sam", "sam-secret-2").is_ok());
    assert!(sign_in(&c, "Root", "first-password").is_ok(), "nobody else's password moved");

    // someone who is not signed in cannot
    assert!(c.call_as(&anon(), "auth.password.change", json!({"current": "x", "new": "yyyyyyyy"}), None).is_err());
}

// ---- the username (ADR 0089) -------------------------------------------------------------------

#[test]
fn a_username_has_one_rule_when_it_is_made_or_changed() {
    let c = core();
    let root = sign_in(&c, "Root", "first-password").unwrap()["who"].clone();
    for bad in ["a", "has space", "way-too-long-way-too-long-way-too-long-way", "no@at", "émile"] {
        let e = as_who(&c, &root, "auth.account.create", json!({"username": bad, "password": "long-enough-1"})).unwrap_err();
        assert_eq!(e.reason.as_deref(), Some("auth.bad_username"), "{bad:?} must be refused");
    }
    let made = as_who(&c, &root, "auth.account.create", json!({"username": "Ana.B-1_2", "password": "long-enough-1"})).unwrap();
    // never case-sensitive, and the same rule when it is renamed
    assert!(sign_in(&c, "ana.b-1_2", "long-enough-1").is_ok());
    let e = as_who(&c, &root, "auth.account.update", json!({"subject": made["subject"], "username": "not valid"})).unwrap_err();
    assert_eq!(e.reason.as_deref(), Some("auth.bad_username"));
    // the list says `username`
    let list = as_who(&c, &root, "auth.account.list", json!({})).unwrap();
    assert!(list["accounts"].as_array().unwrap().iter().all(|a| a["username"].is_string() && a.get("name").is_none()));
}


#[test]
fn every_account_operation_declares_what_it_returns() {
    // a Face's generated contract types the answers, and the Core checks each answer against its schema
    for p in [Arc::new(AuthPassword) as Arc<dyn Plugin>, Arc::new(AuthLocal), Arc::new(AuthOpen)] {
        let m = p.manifest();
        for (op, decl) in &m.operations {
            assert!(!decl.output.is_null(), "{} {op} declares no output schema", m.id);
        }
    }
}

fn with_credential(c: &Core, credential: &str, op: &str, input: Value) -> Result<Value> {
    c.call(CallRequest { credential: Some(credential.into()), op: op.into(), input, key: None, contract: None }).map(|r| r.output)
}

#[test]
fn an_api_key_acts_as_its_owner_until_it_is_revoked() {
    let c = core();
    let root = sign_in(&c, "root", "first-password").unwrap()["who"].clone();
    let made = as_who(&c, &root, "auth.keys.create", json!({"name": "my agent"})).unwrap();
    let key = made["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("nuah_") && key.len() > 30, "{key}");
    // the key signs in as its owner, with the owner's rights
    assert_eq!(with_credential(&c, &key, "auth.who", json!({})).unwrap()["subject"], root["subject"]);
    assert!(with_credential(&c, &key, "auth.account.list", json!({})).is_ok(), "an admin's key can do what the admin can");
    // the list names it and never shows it again
    let list = as_who(&c, &root, "auth.keys.list", json!({})).unwrap();
    assert_eq!(list["keys"][0]["name"], "my agent");
    assert_eq!(list["keys"][0]["id"], made["id"]);
    assert!(!list.to_string().contains(&key), "{list}");
    // revoked: it is nobody
    as_who(&c, &root, "auth.keys.revoke", json!({"id": made["id"]})).unwrap();
    assert_eq!(with_credential(&c, &key, "auth.who", json!({})).unwrap_err().category, Category::Unauthenticated);
    assert_eq!(as_who(&c, &root, "auth.keys.list", json!({})).unwrap()["keys"], json!([]));
}

#[test]
fn a_key_stops_with_its_account_and_only_its_owner_revokes_it() {
    let c = core();
    let root = sign_in(&c, "root", "first-password").unwrap()["who"].clone();
    let sam = as_who(&c, &root, "auth.account.create", json!({"username": "sam", "password": "sam-password-1", "roles": ["member"]})).unwrap()["subject"].clone();
    let sam_who = sign_in(&c, "sam", "sam-password-1").unwrap()["who"].clone();
    let made = as_who(&c, &sam_who, "auth.keys.create", json!({"name": "laptop"})).unwrap();
    let key = made["key"].as_str().unwrap().to_string();
    // nobody else sees or revokes it
    assert_eq!(as_who(&c, &root, "auth.keys.list", json!({})).unwrap()["keys"], json!([]));
    assert!(as_who(&c, &root, "auth.keys.revoke", json!({"id": made["id"]})).is_err());
    assert!(with_credential(&c, &key, "auth.who", json!({})).is_ok());
    // a member's key cannot do what a member cannot
    assert_eq!(with_credential(&c, &key, "auth.account.list", json!({})).unwrap_err().category, Category::Denied);
    // a disabled account's keys stop at once
    as_who(&c, &root, "auth.account.update", json!({"subject": sam, "disabled": true})).unwrap();
    assert_eq!(with_credential(&c, &key, "auth.who", json!({})).unwrap_err().category, Category::Unauthenticated);
}
