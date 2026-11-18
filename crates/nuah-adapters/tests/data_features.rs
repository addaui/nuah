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
            "id": "@test/feat", "version": "1.0.0", "namespaces": ["feat"],
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
        serde_json::from_value(json!({"id": "@nuah/db-bare", "version": "1", "name": "Bare", "description": "Only the required contract", "fills": "db"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Db(Arc::new(Bare(MemoryDb::new())))))
    }
}

/// `{"collections": [..], ..}` written the old way, as a schema file with open Collection schemas at `version` (default 1).
fn legacy(mut data: Value) -> Value {
    let o = data.as_object_mut().unwrap();
    let Some(cols) = o.remove("collections") else { return data };
    let version = o.remove("version").and_then(|v| v.as_u64()).unwrap_or(1);
    let indexes = o.remove("indexes").unwrap_or(json!([]));
    let schemas = o.remove("schemas").unwrap_or(json!({}));
    let mut colls = serde_json::Map::new();
    for c in cols.as_array().unwrap() {
        let c = c.as_str().unwrap();
        let mut s = schemas.get(c).cloned().unwrap_or(json!({"type": "object", "additionalProperties": true}));
        let ix: Vec<Value> = indexes.as_array().unwrap().iter().filter(|i| i["collection"] == c)
            .map(|i| json!({"fields": if i["fields"].is_array() { i["fields"].clone() } else { json!([i["field"]]) }, "unique": i["unique"] == true})).collect();
        if !ix.is_empty() {
            s["indexes"] = json!(ix);
        }
        colls.insert(c.to_string(), s);
    }
    o.insert("schema".into(), json!({"version": version, "collections": colls}));
    data
}

fn start(db: &str, data: Value) -> std::result::Result<Core, Vec<Problem>> {
    let data = legacy(data);
    let clock = FixedClock::at(1_000_000);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Feat(data)));
    cat.0.push(Arc::new(AdapterBare));
    let mut p = profile(&[db, "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/feat"]);
    p.grants.clear();
    Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p })
}

fn put(c: &Core, id: &str, body: Value) -> Result<Value> {
    c.call_as(&person("a", &["member"]), "feat.put", json!({"id": id, "body": body}), None).map(|r| r.output)
}

#[test]
fn a_required_feature_the_adapter_lacks_stops_the_start_and_names_everything() {
    let data = json!({"collections": ["feat.docs"], "requires": ["index.unique"]});
    assert!(start("@nuah/db-memory", data.clone()).is_ok());
    let p = start("@nuah/db-bare", data).err().expect("must not start");
    assert_eq!(p[0].code, "unsupported");
    assert_eq!(p[0].plugins, ["@test/feat"]);
    for word in ["feat", "index.unique", "bare"] {
        assert!(p[0].message.contains(word), "{}", p[0].message);
    }
}

#[test]
fn a_preferred_feature_may_be_missing_and_the_plugin_can_ask() {
    let data = json!({"collections": ["feat.docs"], "prefers": ["backup.snapshot"]});
    for (db, want_unique) in [("@nuah/db-memory", true), ("@nuah/db-bare", false)] {
        let c = start(db, data.clone()).expect("a preference never stops the start");
        let r = c.call_as(&person("a", &["member"]), "feat.has", json!({"name": "index.unique"}), None).unwrap().output;
        assert_eq!(r["has"], want_unique, "{db}");
        assert_eq!(c.call_as(&person("a", &["member"]), "feat.has", json!({"name": "backup.snapshot"}), None).unwrap().output["has"], false);
    }
}

#[test]
fn a_unique_index_is_a_conflict_with_a_reason() {
    let c = start("@nuah/db-memory", json!({"collections": ["feat.docs"], "indexes": [{"collection": "feat.docs", "field": "email", "unique": true}]})).unwrap();
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
        let p = start("@nuah/db-bare", json!({"collections": ["feat.docs"], "indexes": [ix]})).err().expect("must not start");
        assert_eq!(p[0].code, "unsupported");
    }
    // a plain index is only a hint, so any Adapter takes it
    assert!(start("@nuah/db-bare", json!({"collections": ["feat.docs"], "indexes": [{"collection": "feat.docs", "field": "e"}]})).is_ok());
}


#[test]
fn the_kernel_checks_a_declared_collection_schema_on_every_put() {
    let schema = json!({"type": "object", "required": ["title"], "properties": {"title": {"type": "string", "minLength": 1}, "n": {"type": "integer"}}, "additionalProperties": false});
    let c = start("@nuah/db-memory", json!({"collections": ["feat.docs"], "schemas": {"feat.docs": schema}})).unwrap();
    put(&c, "ok", json!({"title": "t", "n": 1})).unwrap();
    for bad in [json!({}), json!({"title": ""}), json!({"title": "t", "n": "x"}), json!({"title": "t", "extra": 1})] {
        let e = put(&c, "bad", bad.clone()).unwrap_err();
        assert_eq!(e.category, Category::Internal, "{bad}");
        assert!(e.message.contains("feat.docs"));
    }
}

/// One memory database kept across Core starts, counting the copies the Core asked for.
struct Counting(Arc<MemoryDb>, Arc<std::sync::atomic::AtomicUsize>);
impl Db for Counting {
    fn name(&self) -> &str {
        "counting"
    }
    fn features(&self) -> Features {
        self.0.features()
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
    fn before_migration(&self, _what: &str) -> Result<()> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    fn retain_indexes(&self, collection: &str, keep: &[IndexSpec]) -> Result<()> {
        self.0.retain_indexes(collection, keep)
    }
}
struct AdapterCounting(Arc<MemoryDb>, Arc<std::sync::atomic::AtomicUsize>);
impl Plugin for AdapterCounting {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@nuah/db-counting", "version": "1", "name": "Counting", "description": "Counts", "fills": "db"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Db(Arc::new(Counting(self.0.clone(), self.1.clone())))))
    }
}

#[test]
fn the_adapter_is_told_before_data_that_exists_is_migrated_and_never_for_a_new_database() {
    let (mem, calls) = (Arc::new(MemoryDb::new()), Arc::new(std::sync::atomic::AtomicUsize::new(0)));
    let boot = |version: u32| {
        let clock = FixedClock::at(1_000_000);
        let seen = Default::default();
        let mut cat = catalog(&clock, &seen);
        cat.0.push(Arc::new(Feat(legacy(json!({"collections": ["feat.docs"], "version": version})))));
        cat.0.push(Arc::new(AdapterCounting(mem.clone(), calls.clone())));
        let mut p = profile(&["@nuah/db-counting", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/feat"]);
        p.grants.clear();
        Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p }).unwrap();
        calls.load(std::sync::atomic::Ordering::SeqCst)
    };
    assert_eq!(boot(1), 0, "a new database has nothing to lose");
    assert_eq!(boot(1), 0, "nothing to migrate");
    assert_eq!(boot(2), 1, "version 1 data is about to become version 2");
    assert_eq!(boot(2), 1, "and not again");
}

#[test]
fn sqlite_writes_a_copy_before_migrating_when_asked() {
    let dir = std::env::temp_dir().join(format!("nuah-bm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (path, backups) = (dir.join("live.db"), dir.join("backups"));
    let boot = |version: u32| {
        let clock = FixedClock::at(1_000_000);
        let seen = Default::default();
        let mut cat = catalog(&clock, &seen);
        cat.0.push(Arc::new(Feat(legacy(json!({"collections": ["feat.docs"], "version": version})))));
        cat.0.push(Arc::new(AdapterSqlite));
        let mut p = profile(&["@nuah/db-sqlite", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/feat"]);
        p.grants.clear();
        p.settings.insert("@nuah/db-sqlite".into(), json!({"path": path.to_str().unwrap(), "backup_dir": backups.to_str().unwrap()}));
        Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p }).unwrap();
    };
    boot(1);
    assert!(!backups.exists(), "no copy for a new database");
    boot(2);
    let files: Vec<_> = std::fs::read_dir(&backups).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].starts_with("before-migration-"));
    let copy = SqliteDb::open(backups.join(&files[0]).to_str().unwrap()).unwrap();
    copy.read(&mut |r| {
        assert_eq!(r.get("_meta", "version:@test/feat")?.unwrap().body["version"], 1, "the copy is from before the migration");
        Ok(())
    })
    .unwrap();
    std::fs::remove_dir_all(dir).ok();
}

/// Two plugins; the second needs a feature the Adapter lacks. Nothing of the first may be migrated or claimed.
struct Pair(&'static str, u32, Value);
impl Plugin for Pair {
    fn manifest(&self) -> Manifest {
        let mut m = json!({"id": format!("@test/{}", self.0), "version": "1", "namespaces": [self.0]});
        let mut d = self.2.clone();
        d["version"] = json!(self.1);
        m["data"] = legacy(d);
        serde_json::from_value(m).unwrap()
    }
}

#[test]
fn a_refused_start_writes_nothing() {
    let mem = Arc::new(MemoryDb::new());
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let boot = |second: Value| {
        let clock = FixedClock::at(1_000_000);
        let seen = Default::default();
        let mut cat = catalog(&clock, &seen);
        cat.0.push(Arc::new(Pair("aaa", 1, json!({"collections": ["aaa.docs"]}))));
        cat.0.push(Arc::new(Pair("zzz", 1, second)));
        cat.0.push(Arc::new(AdapterCounting(mem.clone(), calls.clone())));
        let mut p = profile(&["@nuah/db-counting", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/aaa", "@test/zzz"]);
        p.grants.clear();
        Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p })
    };
    assert!(boot(json!({"collections": ["zzz.docs"], "requires": ["query.text"]})).is_err());
    mem.read(&mut |r| {
        assert!(r.scan("", "", 10)?.is_empty(), "nothing was claimed or migrated");
        Ok(())
    })
    .unwrap();
    assert!(boot(json!({"collections": ["zzz.docs"]})).is_ok());
}

// ---- Collection schemas (ADR 0091) ---------------------------------------------------------------

fn schema_data(collections: Value) -> Value {
    json!({"schema": {"version": 1, "collections": collections}})
}

#[test]
fn a_document_with_a_field_its_schema_does_not_declare_is_refused_naming_collection_and_field() {
    let c = start("@nuah/db-memory", schema_data(json!({"feat.docs": {"type": "object", "properties": {"title": {"type": "string"}}}}))).unwrap();
    assert!(put(&c, "a", json!({"title": "ok"})).is_ok());
    let e = put(&c, "b", json!({"title": "ok", "colour": "red"})).unwrap_err();
    assert_eq!(e.category, Category::Internal);
    assert!(e.message.contains("feat.docs") && e.message.contains("colour"), "{}", e.message);
}

#[test]
fn indexes_come_from_the_schema_file() {
    let c = start("@nuah/db-memory", schema_data(json!({"feat.docs": {"type": "object", "properties": {"key": {"type": "string", "unique": true}}}}))).unwrap();
    put(&c, "a", json!({"key": "k"})).unwrap();
    let e = put(&c, "b", json!({"key": "k"})).unwrap_err();
    assert_eq!(e.reason.as_deref(), Some("kernel.unique"));
}

/// A plugin that reads `feat.docs` with the fields the test names.
struct Reader(Value);
impl Plugin for Reader {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/reader", "version": "1.0.0", "namespaces": ["reader"], "data": {"reads": {"feat.docs": self.0.clone()}}})).unwrap()
    }
}

fn check_with_reader(data: Value, fields: Value) -> Vec<Problem> {
    let clock = FixedClock::at(1_000_000);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Feat(data)));
    cat.0.push(Arc::new(Reader(fields)));
    let mut p = profile(&["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/feat", "@test/reader"]);
    p.grants.clear();
    Core::check(&Distribution { name: "t".into(), catalog: cat, profile: p })
}

#[test]
fn a_reader_of_a_field_the_owner_does_not_have_is_a_problem_naming_both() {
    let data = schema_data(json!({"feat.docs": {"type": "object", "properties": {"title": {"type": "string"}, "old": {"removed": true}}}}));
    assert!(check_with_reader(data.clone(), json!(["title"])).is_empty());
    let ps = check_with_reader(data, json!(["title", "old"]));
    let p = ps.iter().find(|p| p.code == "reader_breaks").unwrap_or_else(|| panic!("{ps:?}"));
    assert_eq!(p.plugins, vec!["@test/reader".to_string(), "@test/feat".to_string()]);
    assert!(p.message.contains("old"));
}

#[test]
fn the_name_rules_of_a_schema_file_are_problems_at_check_time() {
    let data = schema_data(json!({"feat.docs": {"type": "object", "properties": {"c": {"type": "string", "renamed_from": ["a"]}, "a": {"type": "string"}}}}));
    let ps = check_with_reader(data, json!([]));
    assert!(ps.iter().any(|p| p.code == "name_reused" && p.message.contains("feat.docs")), "{ps:?}");
}

// ---- the upgrade runner (ADR 0091) ---------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

/// A plugin whose schema file is chosen per start, with an Upgrade step that marks every Document it sees
/// (`stepped`), and that fails once on the batch numbered `fail_at` when set.
struct Up {
    file: Value,
    batches: Arc<AtomicUsize>,
    fail_at: Option<usize>,
    failed: Arc<AtomicUsize>,
}
impl Plugin for Up {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/up", "version": "1.0.0", "namespaces": ["up"],
            "permissions": {"up.write": {"default": ["member"]}},
            "operations": {"up.put": {"kind": "command", "permission": "up.write", "input": {"type": "object", "required": ["c", "id", "body"]}}},
            "data": {"schema": self.file}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("up.put", |tx, i| {
            tx.put(i["c"].as_str().unwrap(), i["id"].as_str().unwrap(), i["body"].clone(), Expect::Any)?;
            Ok(json!({}))
        });
    }
    fn upgrade(&self, _to: u32, _collection: &str, batch: &mut Vec<(String, Value)>, _tx: &mut Tx) -> Result<()> {
        let n = self.batches.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        if self.fail_at == Some(n) && self.failed.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
            return Err(Error::internal("the step failed on purpose"));
        }
        for (_, body) in batch.iter_mut() {
            assert!(body.get("a").is_none(), "declared renames are applied before the step");
            body["stepped"] = json!(body["stepped"].as_i64().unwrap_or(0) + 1);
        }
        Ok(())
    }
}

struct Rig {
    mem: Arc<MemoryDb>,
    backups: Arc<AtomicUsize>,
    batches: Arc<AtomicUsize>,
    failed: Arc<AtomicUsize>,
}
impl Rig {
    fn new() -> Rig {
        Rig { mem: Arc::new(MemoryDb::new()), backups: Default::default(), batches: Default::default(), failed: Default::default() }
    }
    fn boot(&self, file: Value, fail_at: Option<usize>) -> std::result::Result<Core, Vec<Problem>> {
        let clock = FixedClock::at(1_000_000);
        let seen = Default::default();
        let mut cat = catalog(&clock, &seen);
        cat.0.push(Arc::new(Up { file, batches: self.batches.clone(), fail_at, failed: self.failed.clone() }));
        cat.0.push(Arc::new(AdapterCounting(self.mem.clone(), self.backups.clone())));
        let mut p = profile(&["@nuah/db-counting", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/up"]);
        p.grants.clear();
        Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p })
    }
    fn docs(&self, collection: &str) -> Vec<Doc> {
        let mut out = vec![];
        self.mem.read(&mut |r| {
            out = r.scan(collection, "", 100_000)?.into_iter().filter(|d| d.collection == collection && !d.deleted).collect();
            Ok(())
        }).unwrap();
        out
    }
    fn meta(&self, key: &str) -> Option<Value> {
        let mut out = None;
        self.mem.read(&mut |r| {
            out = r.get("_meta", key)?.filter(|d| !d.deleted).map(|d| d.body);
            Ok(())
        }).unwrap();
        out
    }
}

fn v1() -> Value {
    json!({"version": 1, "collections": {"up.docs": {"type": "object", "properties": {"a": {"type": "string"}, "notes": {"type": "string"}, "k": {"type": "string", "unique": true}}}}})
}
fn v2(steps: Value) -> Value {
    json!({"version": 2, "steps": steps, "collections": {"up.docs": {"type": "object", "properties": {
        "c": {"type": "string", "renamed_from": ["a"]}, "notes": {"removed": true}, "kind": {"type": "string", "default": "task"},
        "k": {"type": "string"}, "stepped": {"type": "integer"}}}}})
}
fn seed(c: &Core, n: usize) {
    for i in 0..n {
        c.call_as(&person("a", &["member"]), "up.put", json!({"c": "up.docs", "id": format!("d{i:05}"), "body": {"a": format!("v{i}"), "notes": "n"}}), None).unwrap();
    }
}

#[test]
fn new_data_is_recorded_with_its_schema() {
    let rig = Rig::new();
    rig.boot(v1(), None).unwrap();
    let s = rig.meta("schema:@test/up").unwrap();
    assert_eq!(s["version"], 1);
    assert_eq!(s["file"], v1());
    assert_eq!(s["release"], "1.0.0");
    assert_eq!(rig.backups.load(AtomicOrdering::SeqCst), 0, "a new database has nothing to lose");
}

#[test]
fn rename_default_remove_applied_to_every_document() {
    let rig = Rig::new();
    seed(&rig.boot(v1(), None).unwrap(), 2501);
    rig.boot(v2(json!([])), None).unwrap();
    let docs = rig.docs("up.docs");
    assert_eq!(docs.len(), 2501);
    for d in &docs {
        assert!(d.body.get("a").is_none() && d.body.get("notes").is_none(), "{}", d.body);
        assert_eq!(d.body["kind"], "task");
        assert!(d.body["c"].as_str().unwrap().starts_with('v'));
    }
    assert_eq!(rig.meta("schema:@test/up").unwrap()["version"], 2);
    assert_eq!(rig.meta("version:@test/up").unwrap()["version"], 2);
    assert!(rig.meta("upgrade:@test/up").is_none(), "no progress left behind");
    assert_eq!(rig.backups.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(rig.batches.load(AtomicOrdering::SeqCst), 0, "no Upgrade step for this version");
}

#[test]
fn interrupted_upgrade_resumes_and_steps_each_document_once() {
    let rig = Rig::new();
    seed(&rig.boot(v1(), None).unwrap(), 2501);
    let e = rig.boot(v2(json!([2])), Some(4)).err().unwrap();
    assert!(e.iter().any(|p| p.message.contains("failed on purpose")), "{e:?}");
    let done: Vec<_> = rig.docs("up.docs").into_iter().filter(|d| d.body.get("stepped").is_some()).collect();
    assert_eq!(done.len(), 1500, "three batches of 500 were finished");
    assert!(rig.meta("upgrade:@test/up").is_some());
    rig.boot(v2(json!([2])), Some(4)).unwrap();
    let docs = rig.docs("up.docs");
    assert_eq!(docs.len(), 2501);
    assert!(docs.iter().all(|d| d.body["stepped"] == 1), "every Document stepped exactly once");
    assert!(rig.meta("upgrade:@test/up").is_none());
}

#[test]
fn refused_upgrade_writes_nothing_and_lists_details() {
    let rig = Rig::new();
    seed(&rig.boot(v1(), None).unwrap(), 3);
    let bad = json!({"version": 2, "collections": {"up.docs": {"type": "object", "properties": {"k": {"type": "string"}}}}});
    let ps = rig.boot(bad, None).err().unwrap();
    assert_eq!(ps.len(), 1);
    assert_eq!(ps[0].code, "upgrade_refused");
    assert_eq!(ps[0].plugins, vec!["@test/up".to_string()]);
    let paths: Vec<&str> = ps[0].details["problems"].as_array().unwrap().iter().map(|d| d["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["a", "notes"]);
    assert_eq!(ps[0].details["plugins"]["@test/up"], json!({"name": "@test/up", "from_release": "1.0.0", "to_release": "1.0.0", "from_version": 1, "to_version": 2}));
    assert_eq!(rig.backups.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(rig.meta("schema:@test/up").unwrap()["version"], 1);
    assert!(rig.docs("up.docs").iter().all(|d| d.body.get("a").is_some()));
}

#[test]
fn collection_rename_moves_documents_and_ownership() {
    let rig = Rig::new();
    seed(&rig.boot(v1(), None).unwrap(), 3);
    let mut f = v1();
    f["version"] = json!(2);
    let s = f["collections"]["up.docs"].clone();
    f["collections"] = json!({"up.items": s});
    f["collections"]["up.items"]["renamed_from"] = json!(["up.docs"]);
    rig.boot(f, None).unwrap();
    assert!(rig.docs("up.docs").is_empty());
    assert_eq!(rig.docs("up.items").len(), 3);
    assert_eq!(rig.meta("collection:up.items").unwrap()["owner"], "@test/up");
    assert!(rig.meta("collection:up.docs").is_none());
}

#[test]
fn a_unique_index_the_schema_no_longer_asks_for_is_dropped() {
    let rig = Rig::new();
    let c = rig.boot(v1(), None).unwrap();
    seed(&c, 1);
    let put = |c: &Core, id: &str| c.call_as(&person("a", &["member"]), "up.put", json!({"c": "up.docs", "id": id, "body": {"k": "same"}}), None);
    put(&c, "x").unwrap();
    assert!(put(&c, "y").is_err(), "k is unique at version 1");
    let c = rig.boot(v2(json!([])), None).unwrap();
    put(&c, "y").unwrap();
}

#[test]
fn a_schema_change_without_a_new_version_is_refused() {
    let rig = Rig::new();
    rig.boot(v1(), None).unwrap();
    let mut f = v1();
    f["collections"]["up.docs"]["properties"]["extra"] = json!({"type": "string"});
    let ps = rig.boot(f, None).err().unwrap();
    assert_eq!(ps[0].details["problems"][0]["code"], "schema_changed_without_version");
}

#[test]
fn sqlite_drops_a_unique_index_the_schema_no_longer_asks_for() {
    let dir = std::env::temp_dir().join(format!("nuah-ix-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("live.db");
    let rig = Rig::new();
    let boot = |file: Value| {
        let clock = FixedClock::at(1_000_000);
        let seen = Default::default();
        let mut cat = catalog(&clock, &seen);
        cat.0.push(Arc::new(Up { file, batches: rig.batches.clone(), fail_at: None, failed: rig.failed.clone() }));
        cat.0.push(Arc::new(AdapterSqlite));
        let mut p = profile(&["@nuah/db-sqlite", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/up"]);
        p.grants.clear();
        p.settings.insert("@nuah/db-sqlite".into(), json!({"path": path.to_str().unwrap()}));
        Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p }).unwrap_or_else(|e| panic!("{e:?}"))
    };
    let put = |c: &Core, id: &str| c.call_as(&person("a", &["member"]), "up.put", json!({"c": "up.docs", "id": id, "body": {"k": "same"}}), None);
    let c = boot(v1());
    put(&c, "x").unwrap();
    assert!(put(&c, "y").is_err(), "k is unique at version 1");
    drop(c);
    let c = boot(v2(json!([])));
    put(&c, "y").unwrap();
    drop(c);
    std::fs::remove_dir_all(dir).ok();
}

// ---- Read declarations (ADR 0091) ----------------------------------------------------------------

/// Reads `feat.docs`, declaring only `title`.
struct Peek;
impl Plugin for Peek {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/peek", "version": "1.0.0", "namespaces": ["peek"], "data": {"reads": {"feat.docs": ["title"]}},
            "operations": {"peek.get": {"kind": "query", "input": {"type": "object", "required": ["id"]}},
                           "peek.find": {"kind": "query", "input": {"type": "object", "required": ["field", "value"]}}}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("peek.get", |tx, i| Ok(tx.get("feat.docs", i["id"].as_str().unwrap())?.map(|d| d.body).unwrap_or(Value::Null)));
        b.op("peek.find", |tx, i| {
            let page = tx.query("feat.docs", &Query::new().eq(i["field"].as_str().unwrap(), i["value"].clone()))?;
            Ok(json!(page.items.into_iter().map(|d| d.body).collect::<Vec<_>>()))
        });
    }
}

fn start_with_peek() -> Core {
    let clock = FixedClock::at(1_000_000);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Feat(schema_data(json!({"feat.docs": {"type": "object", "properties": {"title": {"type": "string"}, "secret": {"type": "string"}}}})))));
    cat.0.push(Arc::new(Peek));
    let mut p = profile(&["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/feat", "@test/peek"]);
    p.grants.clear();
    Core::start(&Distribution { name: "t".into(), catalog: cat, profile: p }).unwrap_or_else(|e| panic!("{e:?}"))
}

#[test]
fn reader_sees_only_declared_fields() {
    let c = start_with_peek();
    put(&c, "a", json!({"title": "t", "secret": "s"})).unwrap();
    let got = c.call_as(&person("a", &["member"]), "peek.get", json!({"id": "a"}), None).unwrap().output;
    assert_eq!(got, json!({"title": "t"}));
    let found = c.call_as(&person("a", &["member"]), "peek.find", json!({"field": "title", "value": "t"}), None).unwrap().output;
    assert_eq!(found, json!([{"title": "t"}]));
}

#[test]
fn reader_query_on_undeclared_field_is_refused() {
    let c = start_with_peek();
    put(&c, "a", json!({"title": "t", "secret": "s"})).unwrap();
    let e = c.call_as(&person("a", &["member"]), "peek.find", json!({"field": "secret", "value": "s"}), None).unwrap_err();
    assert_eq!(e.category, Category::Internal);
}

#[test]
fn catalog_info_lists_reads_by_owner() {
    let clock = FixedClock::at(1_000_000);
    let seen = Default::default();
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Feat(schema_data(json!({"feat.docs": {"type": "object", "properties": {"title": {"type": "string"}}}})))));
    cat.0.push(Arc::new(Peek));
    let p = profile(&["@nuah/db-memory", "@test/feat", "@test/peek"]);
    let info = nuah_kernel::setup::catalog_info(&Distribution { name: "t".into(), catalog: cat, profile: p });
    let peek = info["plugins"].as_array().unwrap().iter().find(|x| x["id"] == "@test/peek").unwrap_or_else(|| panic!("{info}"));
    assert_eq!(peek["reads"], json!([{"owner": "@test/feat", "collection": "feat.docs", "fields": ["title"]}]));
}

#[test]
fn a_plugin_that_owns_collections_without_a_schema_file_is_a_problem() {
    let ps = check_with_reader(json!({"collections": ["feat.docs"], "version": 1}), json!([]));
    let p = ps.iter().find(|p| p.code == "schema_missing").unwrap_or_else(|| panic!("{ps:?}"));
    assert_eq!(p.plugins, vec!["@test/feat".to_string()]);
}
