// SPDX-License-Identifier: Apache-2.0
//! Features of the data Adapter as plugins meet them: required, preferred, unique indexes, collection schemas.
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

/// A plugin whose Manifest is chosen by the test.
struct Feat(Value);
impl Plugin for Feat {
    fn manifest(&self) -> Manifest {
        let mut m = json!({
            "id": "feat", "version": "1.0.0", "namespaces": ["feat"],
            "permissions": {"feat.write": {"default": ["member"]}},
            "operations": {
                "feat.put": {"kind": "command", "permission": "feat.write", "input": {"type": "object", "required": ["id", "body"]}},
                "feat.has": {"kind": "query", "input": {"type": "object", "required": ["name"]}}
            }
        });
        m["data"] = self.0.clone();
        serde_json::from_value(m).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("feat.put", |tx, i| {
            tx.put("feat.docs", i["id"].as_str().unwrap(), i["body"].clone(), Expect::Any)?;
            Ok(json!({}))
        });
        b.op("feat.has", |tx, i| Ok(json!({"has": tx.has_feature(i["name"].as_str().unwrap())})));
    }
}

/// An Adapter that provides only the required contract.
struct Bare(MemoryDb);
impl Db for Bare {
    fn name(&self) -> &str {
        "bare"
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()> {
        self.0.read(f)
    }
    fn write(&self, f: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()> {
        self.0.write(f)
    }
    fn ensure_index(&self, spec: &IndexSpec) -> Result<()> {
        self.0.ensure_index(spec)
    }
}
struct AdapterBare;
impl Plugin for AdapterBare {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "db-bare", "version": "1", "name": "Bare", "description": "Only the required contract", "fills": "db"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Db(Arc::new(Bare(MemoryDb::new())))))
    }
}

fn start(db: &str, data: Value) -> std::result::Result<Core, Vec<Problem>> {
    let clock = FixedClock::at(1_000_000);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Feat(data)));
    cat.0.push(Arc::new(AdapterBare));
    let mut p = profile(&[db, "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "feat"]);
    p.grants.clear();
    Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p })
}

fn put(c: &Core, id: &str, body: Value) -> Result<Value> {
    c.call_as(&person("a", &["member"]), "feat.put", json!({"id": id, "body": body}), None).map(|r| r.output)
}

#[test]
fn a_required_feature_the_adapter_lacks_stops_the_start_and_names_everything() {
    let data = json!({"collections": ["feat.docs"], "requires": ["index.unique"]});
    assert!(start("db-memory", data.clone()).is_ok());
    let p = start("db-bare", data).err().expect("must not start");
    assert_eq!(p[0].code, "unsupported");
    assert_eq!(p[0].plugins, ["feat"]);
    for word in ["feat", "index.unique", "bare"] {
        assert!(p[0].message.contains(word), "{}", p[0].message);
    }
}

#[test]
fn a_preferred_feature_may_be_missing_and_the_plugin_can_ask() {
    let data = json!({"collections": ["feat.docs"], "prefers": ["backup.snapshot"]});
    for (db, want_unique) in [("db-memory", true), ("db-bare", false)] {
        let c = start(db, data.clone()).expect("a preference never stops the start");
        let r = c.call_as(&person("a", &["member"]), "feat.has", json!({"name": "index.unique"}), None).unwrap().output;
        assert_eq!(r["has"], want_unique, "{db}");
        assert_eq!(c.call_as(&person("a", &["member"]), "feat.has", json!({"name": "backup.snapshot"}), None).unwrap().output["has"], false);
    }
}

#[test]
fn a_unique_index_is_a_conflict_with_a_reason() {
    let c = start("db-memory", json!({"collections": ["feat.docs"], "indexes": [{"collection": "feat.docs", "field": "email", "unique": true}]})).unwrap();
    put(&c, "a", json!({"email": "x"})).unwrap();
    let e = put(&c, "b", json!({"email": "x"})).unwrap_err();
    assert_eq!((e.category, e.reason.as_deref()), (Category::Conflict, Some("kernel.unique")));
    put(&c, "a", json!({"email": "x", "n": 1})).unwrap();
    put(&c, "c", json!({})).unwrap();
    put(&c, "d", json!({})).unwrap();
}

#[test]
fn a_unique_or_composite_index_needs_the_feature() {
    for ix in [json!({"collection": "feat.docs", "field": "e", "unique": true}), json!({"collection": "feat.docs", "fields": ["a", "b"]})] {
        let p = start("db-bare", json!({"collections": ["feat.docs"], "indexes": [ix]})).err().expect("must not start");
        assert_eq!(p[0].code, "unsupported");
    }
    // a plain index is only a hint, so any Adapter takes it
    assert!(start("db-bare", json!({"collections": ["feat.docs"], "indexes": [{"collection": "feat.docs", "field": "e"}]})).is_ok());
}

#[test]
fn an_index_on_a_collection_the_plugin_does_not_own_is_refused() {
    let p = start("db-memory", json!({"collections": ["feat.docs"], "indexes": [{"collection": "other.docs", "field": "e"}]})).err().unwrap();
    assert_eq!(p[0].code, "storage");
}

#[test]
fn the_kernel_checks_a_declared_collection_schema_on_every_put() {
    let schema = json!({"type": "object", "required": ["title"], "properties": {"title": {"type": "string", "minLength": 1}, "n": {"type": "integer"}}, "additionalProperties": false});
    let c = start("db-memory", json!({"collections": ["feat.docs"], "schemas": {"feat.docs": schema}})).unwrap();
    put(&c, "ok", json!({"title": "t", "n": 1})).unwrap();
    for bad in [json!({}), json!({"title": ""}), json!({"title": "t", "n": "x"}), json!({"title": "t", "extra": 1})] {
        let e = put(&c, "bad", bad.clone()).unwrap_err();
        assert_eq!(e.category, Category::Internal, "{bad}");
        assert!(e.message.contains("feat.docs"));
    }
}
