// SPDX-License-Identifier: Apache-2.0
//! A small toy app ("notes") used to test the kernel without any real app in the way.
#![allow(dead_code)]

use nuah_adapters::*;
use nuah_kernel::*;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct Notes {
    pub seen: Arc<Mutex<Vec<String>>>,
}

fn m(v: Value) -> Manifest {
    serde_json::from_value(v).unwrap()
}

impl Plugin for Notes {
    fn manifest(&self) -> Manifest {
        m(json!({
            "id": "@test/notes", "version": "1.0.0", "namespaces": ["notes"],
            "requires": {"capabilities": ["emit", "call"], "extension_points": ["tell.channel"], "plugins": ["@test/tell"]},
            "data": {"collections": ["notes.books", "notes.notes"], "reads": [], "version": 2,
                     "indexes": [{"collection": "notes.notes", "field": "book"}]},
            "permissions": {
                "notes.read": {"default": ["member", "viewer"]},
                "notes.write": {"default": ["member"]}
            },
            "resources": ["notes.book"], "scopes": ["notes.readers"],
            "events": {
                "notes.added": {"data": {"type": "object", "required": ["id", "book"]}},
                "notes.removed": {}
            },
            "operations": {
                "notes.book.create": {"kind": "command", "permission": "notes.write",
                    "input": {"type":"object","required":["title"],"properties":{"title":{"type":"string","minLength":1,"maxLength":50}},"additionalProperties":false},
                    "output": {"type":"object","required":["id"]}},
                "notes.book.share": {"kind": "command", "permission": "notes.write", "resource": {"type":"notes.book","id":"book"},
                    "input": {"type":"object","required":["book","with"],"properties":{"book":{"type":"string"},"with":{"type":"string"}}}},
                "notes.book.unshare": {"kind": "command", "permission": "notes.write", "resource": {"type":"notes.book","id":"book"},
                    "input": {"type":"object","required":["book","with"],"properties":{"book":{"type":"string"},"with":{"type":"string"}}}},
                "notes.book.get": {"kind": "query", "resource": {"type":"notes.book","id":"book"},
                    "affordances": {"type":"notes.book","id":"id"},
                    "input": {"type":"object","required":["book"],"properties":{"book":{"type":"string"}}}},
                "notes.book.list": {"kind": "query", "affordances": {"type":"notes.book","id":"id","at":"books"}},
                "notes.add": {"kind": "command", "permission": "notes.write", "resource": {"type":"notes.book","id":"book"},
                    "reasons": {"notes.too_many": "A book holds at most 3 notes"},
                    "input": {"type":"object","required":["book","text"],"properties":{"book":{"type":"string"},"text":{"type":"string","minLength":1,"maxLength":200}},"additionalProperties":false},
                    "output": {"type":"object","required":["id","text"]}},
                "notes.add_two": {"kind": "command", "permission": "notes.write", "resource": {"type":"notes.book","id":"book"},
                    "input": {"type":"object","required":["book"],"properties":{"book":{"type":"string"}}}},
                "notes.add_then_fail": {"kind": "command", "permission": "notes.write", "resource": {"type":"notes.book","id":"book"},
                    "input": {"type":"object","required":["book"],"properties":{"book":{"type":"string"}}}},
                "notes.panic": {"kind": "command", "permission": "notes.write"},
                "notes.sneaky_write": {"kind": "query"},
                "notes.list": {"kind": "query", "permission": "notes.read", "resource": {"type":"notes.book","id":"book"},
                    "input": {"type":"object","required":["book"],"properties":{"book":{"type":"string"}}}},
                "notes.tell_all": {"kind": "command", "permission": "notes.write", "input": {"type":"object","properties":{"text":{"type":"string"}}}}
            },
            "hooks": [
                {"stage": "before", "on": "notes.add", "order": 10, "handler": "notes.no_spam"},
                {"stage": "after", "on": "notes.*", "order": 0, "handler": "notes.remember"}
            ],
            "listeners": [{"id": "notes.count", "on": ["notes.added"], "handler": "notes.count"}]
        }))
    }

    fn bind(&self, b: &mut Bindings) {
        b.resource("notes.book", |db, who, id| {
            let Some(d) = db.get("notes.books", id)?.filter(|d| !d.deleted) else { return Ok(None) };
            if d.body["owner"] == who.subject.as_str() {
                return Ok(Some(vec!["notes.read".into(), "notes.write".into()]));
            }
            if d.body["readers"].as_array().is_some_and(|r| r.iter().any(|x| x == who.subject.as_str())) {
                return Ok(Some(vec!["notes.read".into()]));
            }
            Ok(None)
        });
        b.scope("notes.readers", |db, id, subject| {
            let Some(d) = db.get("notes.books", id)?.filter(|d| !d.deleted) else { return Ok(false) };
            Ok(d.body["owner"] == subject || d.body["readers"].as_array().is_some_and(|r| r.iter().any(|x| x == subject)))
        });
        b.op("notes.book.create", |tx, i| {
            let d = tx.create("notes.books", json!({"title": i["title"], "owner": tx.who().subject, "readers": []}))?;
            Ok(json!({"id": d.id}))
        });
        b.op("notes.book.share", |tx, i| {
            let d = tx.get("notes.books", i["book"].as_str().unwrap())?.unwrap();
            let mut body = d.body.clone();
            body["readers"].as_array_mut().unwrap().push(i["with"].clone());
            tx.put("notes.books", &d.id, body, Expect::Rev(d.rev))?;
            Ok(json!({}))
        });
        b.op("notes.book.unshare", |tx, i| {
            let d = tx.get("notes.books", i["book"].as_str().unwrap())?.unwrap();
            let mut body = d.body.clone();
            body["readers"].as_array_mut().unwrap().retain(|x| x != &i["with"]);
            tx.put("notes.books", &d.id, body, Expect::Rev(d.rev))?;
            Ok(json!({}))
        });
        b.op("notes.book.get", |tx, i| {
            let d = tx.get("notes.books", i["book"].as_str().unwrap())?.unwrap();
            Ok(json!({"id": d.id, "title": d.body["title"]}))
        });
        b.op("notes.book.list", |tx, _| {
            let all = tx.query("notes.books", &Query::new())?;
            let me = tx.who().subject.clone();
            let mine: Vec<Value> = all.items.iter().filter(|d| d.body["owner"] == me.as_str() || d.body["readers"].as_array().is_some_and(|r| r.iter().any(|x| x == me.as_str()))).map(|d| json!({"id": d.id, "title": d.body["title"]})).collect();
            Ok(json!({"books": mine}))
        });
        b.op("notes.add", |tx, i| {
            let book = i["book"].as_str().unwrap();
            if tx.count("notes.notes", &[Cond { field: "book".into(), op: Cmp::Eq, value: json!(book) }])? >= 3 {
                return Err(Error::reason(Category::Conflict, "notes.too_many", "A book holds at most 3 notes"));
            }
            let d = tx.create("notes.notes", json!({"book": book, "text": i["text"]}))?;
            tx.emit("notes.added", Audience::scope("notes.readers", book), json!({"id": d.id, "book": book}))?;
            Ok(json!({"id": d.id, "text": i["text"]}))
        });
        b.op("notes.add_two", |tx, i| {
            tx.call("notes.add", json!({"book": i["book"], "text": "one"}))?;
            tx.call("notes.add", json!({"book": i["book"], "text": "two"}))?;
            Ok(json!({}))
        });
        b.op("notes.add_then_fail", |tx, i| {
            tx.create("notes.notes", json!({"book": i["book"], "text": "ghost"}))?;
            tx.emit("notes.added", Audience::scope("notes.readers", i["book"].as_str().unwrap()), json!({"id": "x", "book": i["book"]}))?;
            Err(Error::internal("failed after writing"))
        });
        b.op("notes.panic", |_tx, _| panic!("a bug in a plugin"));
        b.op("notes.sneaky_write", |tx, _| {
            tx.put("notes.notes", "x", json!({}), Expect::Any)?;
            Ok(json!({}))
        });
        b.op("notes.list", |tx, i| {
            let p = tx.query("notes.notes", &Query::new().eq("book", i["book"].clone()).asc("text"))?;
            Ok(json!({"notes": p.items.iter().map(|d| d.body["text"].clone()).collect::<Vec<_>>()}))
        });
        b.op("notes.tell_all", |tx, i| tx.extension_call("tell.channel", json!({"text": i.get("text").cloned().unwrap_or(json!("hi"))})));
        b.before("notes.no_spam", |_tx, _op, input| {
            if input["text"] == "spam" {
                return Err(Error::invalid("text", "looks like spam"));
            }
            Ok(())
        });
        let seen = self.seen.clone();
        b.after("notes.remember", move |_tx, op, _i, _o| {
            seen.lock().push(op.to_string());
            Ok(())
        });
        b.listener("notes.count", |tx, d| {
            let book = d.event.data["book"].as_str().unwrap_or("?").to_string();
            let _ = (&tx, &book);
            Ok(())
        });
    }

    fn migrate(&self, from: u32, _tx: &mut Tx) -> Result<()> {
        assert!(from < 2);
        Ok(())
    }
}

/// Defines the extension point and a first provider.
pub struct Tell;
impl Plugin for Tell {
    fn manifest(&self) -> Manifest {
        m(json!({"id": "@test/tell", "version": "1.0.0", "namespaces": ["tell"],
            "defines": {"extension_points": {"tell.channel": {"policy": "many", "input": {"type":"object","required":["text"],"properties":{"text":{"type":"string"}}}}}}}))
    }
}
pub struct TellA {
    pub fail: bool,
}
impl Plugin for TellA {
    fn manifest(&self) -> Manifest {
        m(json!({"id": if self.fail { "@test/tell-bad" } else { "@test/tell-a" }, "version": "1.0.0", "requires": {"plugins": ["@test/tell"]}, "provides": [{"extension_point": "tell.channel"}]}))
    }
    fn bind(&self, b: &mut Bindings) {
        let fail = self.fail;
        b.provide("tell.channel", move |_tx, i| if fail { Err(Error::internal("channel down")) } else { Ok(json!({"told": i["text"]})) });
    }
}

/// Another plugin wrapping notes.add with an around hook and a veto-capable before hook.
pub struct Wrapper;
impl Plugin for Wrapper {
    fn manifest(&self) -> Manifest {
        m(json!({"id": "@test/wrapper", "version": "1.0.0", "namespaces": ["wrapper"],
            "hooks": [
                {"stage": "around", "on": "notes.add", "order": 0, "handler": "wrapper.shout"},
                {"stage": "before", "on": "notes.add", "order": 5, "handler": "wrapper.first"}
            ]}))
    }
    fn bind(&self, b: &mut Bindings) {
        b.around("wrapper.shout", |tx, _op, mut input, next| {
            let t = input["text"].as_str().unwrap_or("").to_uppercase();
            input["text"] = json!(t);
            let out = next(tx, input)?;
            let _ = next(tx, json!({})).expect_err("next is single use");
            Ok(out)
        });
        b.before("wrapper.first", |_tx, _op, input| {
            let t = input["text"].as_str().unwrap_or("").trim().to_string();
            input["text"] = json!(t);
            Ok(())
        });
    }
}

pub struct World {
    pub core: Core,
    pub clock: Arc<FixedClock>,
    pub seen: Arc<Mutex<Vec<String>>>,
}

pub fn profile(enabled: &[&str]) -> Profile {
    let mut p = Profile { name: "test".into(), enabled: enabled.iter().map(|s| s.to_string()).collect(), ..Default::default() };
    p.grants.insert("@test/notes".into(), vec!["emit".into(), "call".into()]);
    p
}

pub fn catalog(clock: &Arc<FixedClock>, seen: &Arc<Mutex<Vec<String>>>) -> Catalog {
    let list: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(AdapterMemory), Arc::new(AuthLocal), Arc::new(PolicyRoles), Arc::new(PolicyAllowAll),
        Arc::new(TestClock(clock.clone())), Arc::new(TestIds),
        Arc::new(Notes { seen: seen.clone() }), Arc::new(Tell), Arc::new(TellA { fail: false }), Arc::new(TellA { fail: true }), Arc::new(Wrapper),
    ];
    Catalog(list)
}

pub fn world_with(extra: &[&str]) -> World {
    let clock = FixedClock::at(1_000_000);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut enabled = vec!["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/notes", "@test/tell", "@test/tell-a"];
    enabled.extend_from_slice(extra);
    let dist = Distribution { name: "test".into(), catalog: catalog(&clock, &seen), profile: profile(&enabled) };
    let core = Core::start(&dist).unwrap_or_else(|p| panic!("start failed: {p:#?}"));
    World { core, clock, seen }
}

pub fn world() -> World {
    world_with(&[])
}

pub fn person(name: &str, roles: &[&str]) -> Identity {
    Identity::new(name, name, roles)
}
