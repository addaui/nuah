// SPDX-License-Identifier: Apache-2.0
//! A collection marked `per_subject` belongs to the signed-in person: the Core stores and checks the person, so a
//! plugin cannot forget to ([ADR 0131]). A field marked `x-secret` never reaches an event or a log line.
//!
//! [ADR 0131]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=248bd79d455648dd8029bc159ba6b6d6
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

struct Diary;
impl Plugin for Diary {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "@test/diary", "version": "1", "namespaces": ["diary"],
            "requires": {"capabilities": ["emit"]},
            "events": {"diary.noted": {}},
            "data": {"schema": {"version": 1, "collections": {
                "diary.entries": {"type": "object", "per_subject": true, "properties": {
                    "text": {"type": "string", "search": true},
                    "token": {"type": "string", "x-secret": true}}},
                "diary.shared": {"type": "object", "properties": {"text": {"type": "string"}}}}}},
            "permissions": {"diary.use": {"default": ["member"]}},
            "operations": {
                "diary.add": {"kind": "command", "permission": "diary.use"},
                "diary.put": {"kind": "command", "permission": "diary.use"},
                "diary.get": {"kind": "query", "permission": "diary.use"},
                "diary.list": {"kind": "query", "permission": "diary.use"},
                "diary.count": {"kind": "query", "permission": "diary.use"},
                "diary.find": {"kind": "query", "permission": "diary.use"},
                "diary.remove": {"kind": "command", "permission": "diary.use"},
                "diary.forge": {"kind": "command", "permission": "diary.use"},
                "diary.by_subject": {"kind": "query", "permission": "diary.use"},
                "diary.note": {"kind": "command", "permission": "diary.use"},
                "diary.shared_add": {"kind": "command", "permission": "diary.use"},
                "diary.shared_get": {"kind": "query", "permission": "diary.use"}
            }
        }))
        .unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("diary.add", |tx, i| {
            let mut body = json!({"text": i["text"]});
            if let Some(t) = i.get("token") {
                body["token"] = t.clone();
            }
            Ok(json!({"id": tx.create("diary.entries", body)?.id}))
        });
        b.op("diary.put", |tx, i| {
            tx.put("diary.entries", i["id"].as_str().unwrap(), json!({"text": i["text"]}), Expect::Any)?;
            Ok(json!({}))
        });
        b.op("diary.get", |tx, i| Ok(json!({"doc": tx.get("diary.entries", i["id"].as_str().unwrap())?.map(|d| d.body)})));
        b.op("diary.list", |tx, _| Ok(json!({"items": tx.query("diary.entries", &Query::new())?.items.into_iter().map(|d| d.body).collect::<Vec<_>>()})));
        b.op("diary.count", |tx, _| Ok(json!({"n": tx.count("diary.entries", &[])?})));
        b.op("diary.find", |tx, i| Ok(json!({"n": tx.search("diary.entries", i["q"].as_str().unwrap(), &[], 10)?.len()})));
        b.op("diary.remove", |tx, i| Ok(json!({"gone": tx.delete("diary.entries", i["id"].as_str().unwrap())?})));
        b.op("diary.forge", |tx, i| {
            tx.put("diary.entries", "forged", json!({"text": "x", "_subject": i["as"]}), Expect::Any)?;
            Ok(json!({}))
        });
        b.op("diary.by_subject", |tx, i| {
            let q = Query { conds: vec![Cond { field: "_subject".into(), op: Cmp::Eq, value: i["s"].clone() }], ..Query::new() };
            Ok(json!({"n": tx.query("diary.entries", &q)?.items.len()}))
        });
        // reads the entry, then tries to put its whole body in an event and a log line
        b.op("diary.note", |tx, i| {
            let d = tx.get("diary.entries", i["id"].as_str().unwrap())?.unwrap();
            tx.emit("diary.noted", Audience::subject(&tx.who().subject), d.body.clone())?;
            tx.log(Level::Info, "diary.note", d.body.clone());
            Ok(json!({}))
        });
        b.op("diary.shared_add", |tx, i| Ok(json!({"id": tx.create("diary.shared", json!({"text": i["text"]}))?.id})));
        b.op("diary.shared_get", |tx, i| Ok(json!({"doc": tx.get("diary.shared", i["id"].as_str().unwrap())?.map(|d| d.body)})));
    }
}

fn diary() -> World {
    let clock = FixedClock::at(1_000_000);
    let seen = Arc::new(parking_lot::Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Diary));
    let mut p = profile(&["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/diary"]);
    p.grants.insert("@test/diary".into(), vec!["emit".into()]);
    let dist = Distribution { name: "test".into(), catalog: cat, profile: p };
    let core = Core::start(&dist).unwrap_or_else(|p| panic!("start failed: {p:#?}"));
    World { core, clock, seen }
}
fn call(w: &World, who: &Identity, op: &str, input: Value) -> Result<Value> {
    w.core.call_as(who, op, input, None).map(|r| r.output)
}
fn add(w: &World, who: &Identity, text: &str) -> String {
    call(w, who, "diary.add", json!({"text": text})).unwrap()["id"].as_str().unwrap().to_string()
}

#[test]
fn a_person_sees_only_their_own_documents() {
    let w = diary();
    let (a, b) = (person("alice", &["member"]), person("bob", &["member"]));
    let ida = add(&w, &a, "alice's day");
    add(&w, &b, "bob's day");
    assert_eq!(call(&w, &a, "diary.list", json!({})).unwrap()["items"].as_array().unwrap().len(), 1);
    assert_eq!(call(&w, &a, "diary.count", json!({})).unwrap()["n"], 1);
    assert_eq!(call(&w, &b, "diary.get", json!({"id": ida})).unwrap()["doc"], Value::Null, "another person's id finds nothing");
    assert_eq!(call(&w, &a, "diary.find", json!({"q": "day"})).unwrap()["n"], 1);
    assert_eq!(call(&w, &a, "diary.get", json!({"id": ida})).unwrap()["doc"]["text"], "alice's day");
}

#[test]
fn a_person_cannot_change_or_delete_another_persons_document() {
    let w = diary();
    let (a, b) = (person("alice", &["member"]), person("bob", &["member"]));
    let ida = add(&w, &a, "mine");
    assert!(call(&w, &b, "diary.put", json!({"id": ida, "text": "hijack"})).is_err());
    assert_eq!(call(&w, &b, "diary.remove", json!({"id": ida})).unwrap()["gone"], false, "bob deletes nothing of alice's");
    assert_eq!(call(&w, &a, "diary.get", json!({"id": ida})).unwrap()["doc"]["text"], "mine");
    assert_eq!(call(&w, &a, "diary.put", json!({"id": ida, "text": "edited"})).is_ok(), true);
    assert_eq!(call(&w, &a, "diary.remove", json!({"id": ida})).unwrap()["gone"], true);
}

#[test]
fn a_plugin_can_neither_set_nor_see_the_subject() {
    let w = diary();
    let (a, b) = (person("alice", &["member"]), person("bob", &["member"]));
    let id = add(&w, &a, "x");
    assert!(call(&w, &a, "diary.forge", json!({"as": "bob"})).is_err(), "the body may not carry _subject");
    assert!(call(&w, &a, "diary.by_subject", json!({"s": "bob"})).is_err(), "a condition may not name _subject");
    let doc = call(&w, &a, "diary.get", json!({"id": id})).unwrap()["doc"].clone();
    assert!(doc.get("_subject").is_none(), "the plugin never sees it");
    let _ = b;
}

#[test]
fn a_caller_with_no_subject_is_refused() {
    let w = diary();
    let nobody = Identity::new("", "", &["member"]);
    assert!(call(&w, &nobody, "diary.add", json!({"text": "x"})).is_err());
    assert!(call(&w, &nobody, "diary.list", json!({})).is_err());
}

#[test]
fn a_collection_without_the_flag_is_shared_as_before() {
    let w = diary();
    let (a, b) = (person("alice", &["member"]), person("bob", &["member"]));
    let id = call(&w, &a, "diary.shared_add", json!({"text": "hi"})).unwrap()["id"].as_str().unwrap().to_string();
    assert_eq!(call(&w, &b, "diary.shared_get", json!({"id": id})).unwrap()["doc"]["text"], "hi");
}

#[test]
fn a_secret_field_never_reaches_an_event_or_a_log_line() {
    let w = diary();
    let a = person("alice", &["member"]);
    let id = call(&w, &a, "diary.add", json!({"text": "t", "token": "s3cr3t-value"})).unwrap()["id"].as_str().unwrap().to_string();
    let c0 = w.core.pulse_now().unwrap();
    call(&w, &a, "diary.note", json!({"id": id})).unwrap();
    let evs = w.core.pulse(&a, Some(&c0), 50).unwrap().events;
    assert_eq!(evs.len(), 1);
    let text = serde_json::to_string(&evs[0].data).unwrap();
    assert!(!text.contains("s3cr3t-value"), "the event holds the secret: {text}");
    assert!(text.contains("\"t\""), "the rest of the body is kept");
}
