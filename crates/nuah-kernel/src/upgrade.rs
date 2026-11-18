// SPDX-License-Identifier: Apache-2.0
//! Collection schemas and upgrades ([ADR 0091]).
//!
//! [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
//!
//! A plugin ships one schema file: its Data version, optionally its Upgrade floor and the versions that have an
//! Upgrade step, and a Collection schema per Collection. A Collection schema is JSON Schema (the subset in
//! [`crate::schema`]) plus the Nuah keywords `default`, `renamed_from` (every Earlier name), `removed`, `index` and
//! `unique` on a field, and `indexes` on the Collection.

use crate::db::IndexSpec;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A plugin's schema file.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaFile {
    /// The Data version.
    pub version: u32,
    /// The Upgrade floor: the oldest Data version this plugin can still upgrade from.
    pub upgrades_from: Option<u32>,
    /// The Data versions that have an Upgrade step.
    pub steps: Vec<u32>,
    /// Every Collection schema, including Collections marked `removed`.
    pub collections: BTreeMap<String, Value>,
}

impl SchemaFile {
    pub fn parse(v: &Value) -> Result<SchemaFile, String> {
        let o = v.as_object().ok_or("a schema file is a JSON object")?;
        let version = o.get("version").and_then(|x| x.as_u64()).filter(|n| *n >= 1 && *n <= u32::MAX as u64).ok_or("a schema file needs `version`, a whole number of at least 1")? as u32;
        let upgrades_from = match o.get("upgrades_from") {
            None | Some(Value::Null) => None,
            Some(x) => Some(x.as_u64().filter(|n| *n >= 1 && *n <= version as u64).ok_or("`upgrades_from` is a Data version from 1 up to `version`")? as u32),
        };
        let steps = match o.get("steps") {
            None => vec![],
            Some(x) => {
                let a = x.as_array().ok_or("`steps` is a list of Data versions")?;
                let mut s = vec![];
                for n in a {
                    let n = n.as_u64().filter(|n| *n >= 2 && *n <= version as u64).ok_or("each of `steps` is a Data version from 2 up to `version`")? as u32;
                    s.push(n);
                }
                s.sort_unstable();
                s.dedup();
                s
            }
        };
        let cols = o.get("collections").and_then(|c| c.as_object()).ok_or("a schema file needs `collections`, an object of Collection schemas")?;
        let mut collections = BTreeMap::new();
        for (name, schema) in cols {
            if !schema.is_object() {
                return Err(format!("the Collection schema of {name} is not an object"));
            }
            collections.insert(name.clone(), schema.clone());
        }
        for k in o.keys() {
            if !matches!(k.as_str(), "version" | "upgrades_from" | "steps" | "collections") {
                return Err(format!("`{k}` is not part of a schema file"));
            }
        }
        Ok(SchemaFile { version, upgrades_from, steps, collections })
    }

    /// The Collections the plugin owns: every one not marked `removed`.
    pub fn live_collections(&self) -> Vec<String> {
        self.collections.iter().filter(|(_, s)| !is_removed(s)).map(|(n, _)| n.clone()).collect()
    }

    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("version".into(), self.version.into());
        if let Some(f) = self.upgrades_from {
            o.insert("upgrades_from".into(), f.into());
        }
        if !self.steps.is_empty() {
            o.insert("steps".into(), self.steps.clone().into());
        }
        o.insert("collections".into(), Value::Object(self.collections.clone().into_iter().collect()));
        Value::Object(o)
    }
}

pub(crate) fn is_removed(s: &Value) -> bool {
    s.get("removed") == Some(&Value::Bool(true))
}

/// The schema the kernel checks a Document against: fields marked `removed` are dropped, and every object schema that
/// lists `properties` and says nothing about `additionalProperties` refuses fields it does not list. An object with no
/// `properties` (a rich-text body) stays opaque; its owner checks what is inside.
pub fn strict(schema: &Value) -> Value {
    let Some(o) = schema.as_object() else { return schema.clone() };
    let mut out = o.clone();
    if let Some(props) = o.get("properties").and_then(|p| p.as_object()) {
        let kept: Map<String, Value> = props.iter().filter(|(_, s)| !is_removed(s)).map(|(k, s)| (k.clone(), strict(s))).collect();
        out.insert("properties".into(), Value::Object(kept));
        out.entry("additionalProperties").or_insert(Value::Bool(false));
    }
    if let Some(extra @ Value::Object(_)) = o.get("additionalProperties") {
        out.insert("additionalProperties".into(), strict(extra));
    }
    if let Some(items) = o.get("items") {
        out.insert("items".into(), strict(items));
    }
    Value::Object(out)
}

/// The indexes a Collection schema asks for: `index` or `unique` on a top-level field, and `indexes` on the Collection.
/// Sorted, so the same schema always gives the same list.
pub fn index_specs(collection: &str, schema: &Value) -> Vec<IndexSpec> {
    let mut out = vec![];
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        for (name, p) in props.iter().filter(|(_, p)| !is_removed(p)) {
            let unique = p.get("unique") == Some(&Value::Bool(true));
            if unique || p.get("index") == Some(&Value::Bool(true)) {
                out.push(IndexSpec { collection: collection.into(), fields: vec![name.clone()], unique });
            }
        }
    }
    for ix in schema.get("indexes").and_then(|i| i.as_array()).into_iter().flatten() {
        let fields: Vec<String> = ix.get("fields").and_then(|f| f.as_array()).into_iter().flatten().filter_map(|f| f.as_str().map(String::from)).collect();
        out.push(IndexSpec { collection: collection.into(), fields, unique: ix.get("unique") == Some(&Value::Bool(true)) });
    }
    out.sort_by(|a, b| a.fields.cmp(&b.fields).then(a.unique.cmp(&b.unique)));
    out.dedup();
    out
}

// ---- the Schema check ----------------------------------------------------------------------------

/// One reason a Schema check refuses. `code` is one of: `field_missing`, `collection_missing`, `type_changed`,
/// `limit_tightened`, `required_without_default`, `name_reused`, `earlier_name_twice`, `earlier_name_dropped`,
/// `below_floor`, `data_newer`, `schema_changed_without_version`. `path` is the field inside the Collection, dotted,
/// with `[]` for "each item of a list" (empty for the plugin or the Collection itself).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UpgradeProblem {
    pub plugin: String,
    pub collection: String,
    pub path: String,
    pub code: String,
    pub params: Value,
}

impl UpgradeProblem {
    fn new(plugin: &str, collection: &str, path: &[String], code: &str, params: Value) -> UpgradeProblem {
        UpgradeProblem { plugin: plugin.into(), collection: collection.into(), path: path.join("."), code: code.into(), params }
    }

    /// The problem in one English sentence, for the log and the copied report. A Face words it from `code` and `params`.
    pub fn message(&self) -> String {
        let at = if self.path.is_empty() { self.collection.clone() } else { format!("{} › \"{}\"", self.collection, self.path) };
        let p = &self.params;
        match self.code.as_str() {
            "field_missing" => format!("{at} is missing from the new version, and the version does not say whether it was renamed or removed"),
            "collection_missing" => format!("the collection {} is missing from the new version, and the version does not say whether it was renamed or removed", self.collection),
            "type_changed" => format!("{at} changed from {} to {}; changing what a field holds needs an Upgrade step the version does not include", p["from"], p["to"]),
            "limit_tightened" => format!("{at} allows less than before ({}); stored data may not fit, and the version includes no Upgrade step", p["limit"].as_str().unwrap_or("a limit")),
            "required_without_default" => format!("{at} is required in the new version but has no default for the data stored without it"),
            "name_reused" => format!("{at} uses a name that was once an earlier or removed name; a name cannot mean two things"),
            "earlier_name_twice" => format!("{at}: the earlier name is claimed by more than one field, or is also marked removed"),
            "earlier_name_dropped" => format!("{at}: an earlier name was deleted from the schema; earlier names stay until the Upgrade floor is raised"),
            "below_floor" => format!("the stored data is at version {}, older than the oldest this version can upgrade from ({}); install version {} first", p["stored"], p["floor"], p["floor"]),
            "data_newer" => format!("the stored data is at version {}, newer than this version of the plugin ({})", p["stored"], p["version"]),
            "schema_changed_without_version" => format!("the schema changed but its version ({}) did not", p["version"]),
            "data_without_schema" => "the stored data was written by a version from before schemas, so it cannot be carried over".to_string(),
            "interrupted_upgrade" => format!("an earlier upgrade to version {} stopped half-way; finish it with that version first (this one is version {})", p["to"], p["version"]),
            "names_merge" => format!("{at}: two stored names become this one name; one of them would overwrite the other"),
            "documents_do_not_fit" => format!("{at}: {} stored document(s) do not fit the new schema after the declared changes (for example {}: {})", p["count"], p["example"], p["reason"]),
            "unique_conflict" => format!("{at}: {} stored document(s) share values the new unique index on {} forbids (for example {})", p["count"], p["fields"], p["example"]),
            other => format!("{at}: {other}"),
        }
    }
}

/// What the runner does to the Documents of one stored Collection. `from` is the stored name, `to` the new one.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionPlan {
    pub from: String,
    pub to: String,
    /// The Collection is marked `removed`: every Document goes.
    pub removed: bool,
    pub ops: Vec<FieldOp>,
}

/// A change to each Document. Paths are field names from the Document's root; `[]` means each item of a list.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldOp {
    Rename { from: Vec<String>, to: Vec<String> },
    Remove(Vec<String>),
    Default(Vec<String>, Value),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpgradePlan {
    /// Only the Collections with work to do.
    pub collections: Vec<CollectionPlan>,
}

/// The names of one level (the Collections of a file, or the fields of one object): live names, earlier names with
/// the name that claims them, and names marked removed.
struct Names<'a> {
    live: Vec<(&'a str, &'a Value)>,
    earlier: Vec<(&'a str, &'a str)>,
    removed: Vec<&'a str>,
}

fn names(level: &Map<String, Value>) -> Names<'_> {
    let mut n = Names { live: vec![], earlier: vec![], removed: vec![] };
    for (k, s) in level {
        if is_removed(s) {
            n.removed.push(k);
            continue;
        }
        n.live.push((k, s));
        for e in s.get("renamed_from").and_then(|r| r.as_array()).into_iter().flatten().filter_map(|e| e.as_str()) {
            n.earlier.push((e, k));
        }
    }
    n
}

fn retired(level: &Map<String, Value>) -> Vec<String> {
    let n = names(level);
    n.earlier.iter().map(|(e, _)| e.to_string()).chain(n.removed.iter().map(|r| r.to_string())).collect()
}

/// The name rules inside one schema file: no live name is a Retired name, and an Earlier name belongs to one field
/// (and is not also marked removed). Checked on every level: the Collections, each object, each list item.
pub fn static_problems(plugin: &str, file: &SchemaFile) -> Vec<UpgradeProblem> {
    let level: Map<String, Value> = file.collections.clone().into_iter().collect();
    let mut out = vec![];
    level_rules(plugin, "", &[], &level, true, &mut out);
    for (c, s) in &file.collections {
        if !is_removed(s) {
            walk_static(plugin, c, &[], s, &mut out);
        }
    }
    out
}

fn walk_static(plugin: &str, collection: &str, path: &[String], schema: &Value, out: &mut Vec<UpgradeProblem>) {
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        level_rules(plugin, collection, path, props, false, out);
        for (k, s) in props.iter().filter(|(_, s)| !is_removed(s)) {
            walk_static(plugin, collection, &[path, &[k.clone()]].concat(), s, out);
        }
    }
    if let Some(items) = schema.get("items") {
        walk_static(plugin, collection, &[path, &["[]".to_string()]].concat(), items, out);
    }
}

fn level_rules(plugin: &str, collection: &str, path: &[String], level: &Map<String, Value>, collections: bool, out: &mut Vec<UpgradeProblem>) {
    let n = names(level);
    // a problem about a Collection's name is reported on that Collection; about a field, on the field's path
    let at = |name: &str| if collections { (name.to_string(), vec![]) } else { (collection.to_string(), [path, &[name.to_string()]].concat()) };
    for (live, _) in &n.live {
        if n.earlier.iter().any(|(e, _)| e == live) || n.removed.contains(live) {
            let (c, p) = at(live);
            out.push(UpgradeProblem::new(plugin, &c, &p, "name_reused", serde_json::json!({})));
        }
    }
    let mut seen: Vec<&str> = vec![];
    for (e, _) in &n.earlier {
        if seen.contains(e) {
            continue;
        }
        seen.push(e);
        let claims = n.earlier.iter().filter(|(x, _)| x == e).map(|(_, owner)| *owner).collect::<std::collections::BTreeSet<_>>().len();
        if claims > 1 || n.removed.contains(e) {
            let (c, p) = at(e);
            out.push(UpgradeProblem::new(plugin, &c, &p, "earlier_name_twice", serde_json::json!({})));
        }
    }
}

/// Compare the schema the stored data was written under with the plugin's schema now, and say what to do to every
/// Document, or every reason it cannot be done. Nothing is refused for one problem alone: all of them are listed.
pub fn plan(plugin: &str, stored: &SchemaFile, new: &SchemaFile) -> Result<UpgradePlan, Vec<UpgradeProblem>> {
    let whole = |code: &str, params: Value| Err(vec![UpgradeProblem::new(plugin, "", &[], code, params)]);
    if new.version < stored.version {
        return whole("data_newer", serde_json::json!({"stored": stored.version, "version": new.version}));
    }
    if new.upgrades_from.is_some_and(|f| stored.version < f) {
        return whole("below_floor", serde_json::json!({"stored": stored.version, "floor": new.upgrades_from}));
    }
    if new.version == stored.version {
        return if new.to_value() == stored.to_value() { Ok(UpgradePlan::default()) } else { whole("schema_changed_without_version", serde_json::json!({"version": new.version})) };
    }
    let covered = new.steps.iter().any(|s| *s > stored.version && *s <= new.version);
    let keep_earlier = new.upgrades_from == stored.upgrades_from;
    let mut d = Diff { plugin, covered, keep_earlier, problems: vec![], collection: String::new() };
    let mut out = UpgradePlan::default();
    let new_level: Map<String, Value> = new.collections.clone().into_iter().collect();
    let stored_level: Map<String, Value> = stored.collections.clone().into_iter().collect();
    let n = names(&new_level);
    if keep_earlier {
        for r in retired(&stored_level) {
            if !retired(&new_level).contains(&r) {
                d.problems.push(UpgradeProblem::new(plugin, &r, &[], "earlier_name_dropped", serde_json::json!({})));
            }
        }
    }
    let mut claimed: std::collections::BTreeMap<String, usize> = Default::default();
    for (sc, ss) in stored.collections.iter().filter(|(_, s)| !is_removed(s)) {
        let target = n.live.iter().find(|(name, _)| *name == sc.as_str()).or_else(|| n.earlier.iter().find(|(e, _)| *e == sc.as_str()).and_then(|(_, owner)| n.live.iter().find(|(name, _)| name == owner)));
        match target {
            Some((to, ns)) => {
                *claimed.entry(to.to_string()).or_default() += 1;
                if claimed[*to] == 2 {
                    d.problems.push(UpgradeProblem::new(plugin, to, &[], "names_merge", serde_json::json!({})));
                    continue;
                }
                d.collection = to.to_string();
                let mut ops = vec![];
                d.object(&[], ss, ns, &mut ops);
                if *to != sc.as_str() || !ops.is_empty() {
                    out.collections.push(CollectionPlan { from: sc.clone(), to: to.to_string(), removed: false, ops });
                }
            }
            None if n.removed.contains(&sc.as_str()) => out.collections.push(CollectionPlan { from: sc.clone(), to: sc.clone(), removed: true, ops: vec![] }),
            None => d.problems.push(UpgradeProblem::new(plugin, sc, &[], "collection_missing", serde_json::json!({}))),
        }
    }
    if d.problems.is_empty() { Ok(out) } else { Err(d.problems) }
}

struct Diff<'a> {
    plugin: &'a str,
    /// An Upgrade step lies between the stored version and the new one, so type and limit changes are its business.
    covered: bool,
    keep_earlier: bool,
    problems: Vec<UpgradeProblem>,
    collection: String,
}

fn types(s: &Value) -> Option<Vec<String>> {
    match s.get("type")? {
        Value::String(t) => Some(vec![t.clone()]),
        Value::Array(a) => Some(a.iter().filter_map(|t| t.as_str().map(String::from)).collect()),
        _ => None,
    }
}

impl Diff<'_> {
    fn problem(&mut self, path: &[String], code: &str, params: Value) {
        let p = UpgradeProblem::new(self.plugin, &self.collection, path, code, params);
        self.problems.push(p);
    }

    /// Compare the fields of one object level and collect the operations, renames and removals before defaults.
    fn object(&mut self, path: &[String], stored: &Value, new: &Value, ops: &mut Vec<FieldOp>) {
        let empty = Map::new();
        let sp = stored.get("properties").and_then(|p| p.as_object()).unwrap_or(&empty);
        let np = new.get("properties").and_then(|p| p.as_object()).unwrap_or(&empty);
        if sp.is_empty() && np.is_empty() {
            return;
        }
        let n = names(np);
        if self.keep_earlier {
            let now = retired(np);
            for r in retired(sp) {
                if !now.contains(&r) {
                    self.problem(&[path, &[r]].concat(), "earlier_name_dropped", serde_json::json!({}));
                }
            }
        }
        let required = |s: &Value, k: &str| s.get("required").and_then(|r| r.as_array()).is_some_and(|r| r.iter().any(|x| x == k));
        let mut matched: Vec<&str> = vec![];
        for (f, sf) in sp.iter().filter(|(_, s)| !is_removed(s)) {
            let here = [path, &[f.clone()]].concat();
            let target = n.live.iter().find(|(name, _)| *name == f.as_str()).or_else(|| n.earlier.iter().find(|(e, _)| *e == f.as_str()).and_then(|(_, owner)| n.live.iter().find(|(name, _)| name == owner)));
            match target {
                Some((to, nf)) => {
                    let there = [path, &[to.to_string()]].concat();
                    if matched.contains(to) {
                        self.problem(&there, "names_merge", serde_json::json!({}));
                        continue;
                    }
                    matched.push(to);
                    // a value stored without the field gets the default (a field that becomes required needs it)
                    if let Some(dv) = nf.get("default") {
                        ops.push(FieldOp::Default(there.clone(), dv.clone()));
                    }
                    if *to != f.as_str() {
                        ops.push(FieldOp::Rename { from: here.clone(), to: there.clone() });
                    }
                    if !required(stored, f) && required(new, to) && nf.get("default").is_none() {
                        self.problem(&there, "required_without_default", serde_json::json!({}));
                    }
                    self.field(&there, sf, nf, ops);
                }
                None if n.removed.contains(&f.as_str()) => ops.push(FieldOp::Remove(here)),
                None => self.problem(&here, "field_missing", serde_json::json!({})),
            }
        }
        for (k, nf) in &n.live {
            if matched.contains(k) {
                continue;
            }
            let here = [path, &[k.to_string()]].concat();
            match nf.get("default") {
                Some(dv) => ops.push(FieldOp::Default(here, dv.clone())),
                None if required(new, k) => self.problem(&here, "required_without_default", serde_json::json!({})),
                None => {}
            }
        }
    }

    fn field(&mut self, path: &[String], stored: &Value, new: &Value, ops: &mut Vec<FieldOp>) {
        if let (Some(st), Some(nt)) = (types(stored), types(new)) {
            let fits = |t: &String| nt.contains(t) || (t == "integer" && nt.iter().any(|x| x == "number"));
            if !st.iter().all(fits) && !self.covered {
                self.problem(path, "type_changed", serde_json::json!({"from": stored["type"], "to": new["type"]}));
                return;
            }
        }
        if !self.covered {
            if let Some(limit) = tightened(stored, new) {
                self.problem(path, "limit_tightened", serde_json::json!({"limit": limit}));
            }
        }
        self.object(path, stored, new, ops);
        if let (Some(si), Some(ni)) = (stored.get("items"), new.get("items")) {
            self.field(&[path, &["[]".to_string()]].concat(), si, ni, ops);
        }
    }
}

/// The first limit the new field schema makes stricter than the stored one, by name.
fn tightened(stored: &Value, new: &Value) -> Option<&'static str> {
    let num = |s: &Value, k: &str| s.get(k).and_then(|x| x.as_f64());
    // a limit only the new schema has is stricter; one only the stored schema had was loosened
    let upper = |k: &'static str| match (num(stored, k), num(new, k)) {
        (_, None) => None,
        (None, Some(_)) => Some(k),
        (Some(a), Some(b)) => (b < a).then_some(k),
    };
    let lower = |k: &'static str| match (num(stored, k), num(new, k)) {
        (_, None) => None,
        (None, Some(_)) => Some(k),
        (Some(a), Some(b)) => (b > a).then_some(k),
    };
    let enum_tighter = match (stored.get("enum").and_then(|e| e.as_array()), new.get("enum").and_then(|e| e.as_array())) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(a), Some(b)) => a.iter().any(|x| !b.contains(x)),
    };
    upper("maxLength").or(lower("minLength")).or(upper("maximum")).or(lower("minimum")).or(upper("maxItems")).or(lower("minItems")).or(enum_tighter.then_some("enum"))
}

// ---- applying a plan -----------------------------------------------------------------------------

/// How many Documents the runner upgrades in one transaction.
pub const BATCH: usize = 500;

/// Apply the operations to one Document body, in order. A path that does not exist in this Document is skipped; a
/// default never replaces a value that is there (an explicit `null` included).
pub fn apply(body: &mut Value, ops: &[FieldOp]) {
    for op in ops {
        match op {
            FieldOp::Rename { from, to } => {
                let (parent, old, new) = (&from[..from.len() - 1], &from[from.len() - 1], &to[to.len() - 1]);
                each_object(body, parent, &mut |o| {
                    if let Some(v) = o.remove(old) {
                        o.insert(new.clone(), v);
                    }
                });
            }
            FieldOp::Remove(path) => {
                let (parent, name) = (&path[..path.len() - 1], &path[path.len() - 1]);
                each_object(body, parent, &mut |o| {
                    o.remove(name);
                });
            }
            FieldOp::Default(path, value) => {
                let (parent, name) = (&path[..path.len() - 1], &path[path.len() - 1]);
                each_object(body, parent, &mut |o| {
                    o.entry(name.clone()).or_insert_with(|| value.clone());
                });
            }
        }
    }
}

/// Call `f` on every object found at `path` (`[]` steps into each item of a list).
fn each_object(v: &mut Value, path: &[String], f: &mut dyn FnMut(&mut Map<String, Value>)) {
    match path.split_first() {
        None => {
            if let Some(o) = v.as_object_mut() {
                f(o);
            }
        }
        Some((seg, rest)) if seg == "[]" => {
            for item in v.as_array_mut().into_iter().flatten() {
                each_object(item, rest, f);
            }
        }
        Some((seg, rest)) => {
            if let Some(child) = v.get_mut(seg.as_str()) {
                each_object(child, rest, f);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::IndexSpec;
    use serde_json::json;

    fn file(v: serde_json::Value) -> SchemaFile {
        SchemaFile::parse(&v).unwrap()
    }

    #[test]
    fn a_schema_file_has_a_version_and_live_collections() {
        let f = file(json!({"version": 2, "upgrades_from": 1, "steps": [2], "collections": {
            "t.tasks": {"type": "object", "properties": {"title": {"type": "string"}}},
            "t.old": {"removed": true}
        }}));
        assert_eq!(f.version, 2);
        assert_eq!(f.upgrades_from, Some(1));
        assert_eq!(f.steps, vec![2]);
        assert_eq!(f.live_collections(), vec!["t.tasks".to_string()]);
    }

    #[test]
    fn a_schema_file_needs_a_version_of_at_least_one() {
        assert!(SchemaFile::parse(&json!({"version": 0, "collections": {}})).is_err());
        assert!(SchemaFile::parse(&json!({"collections": {}})).is_err());
    }

    #[test]
    fn strict_refuses_undeclared_field() {
        let s = strict(&json!({"type": "object", "properties": {"title": {"type": "string"}, "meta": {"type": "object", "properties": {"a": {"type": "string"}}}}}));
        assert!(crate::schema::validate(&s, &json!({"title": "x"})).is_ok());
        assert!(crate::schema::validate(&s, &json!({"title": "x", "other": 1})).is_err());
        assert!(crate::schema::validate(&s, &json!({"title": "x", "meta": {"b": 1}})).is_err(), "nested objects are strict too");
    }

    #[test]
    fn strict_keeps_an_explicit_open_map_and_opaque_objects() {
        let s = strict(&json!({"type": "object", "properties": {"custom": {"type": "object", "additionalProperties": {"type": "string"}}, "body": {"type": "object"}}}));
        assert!(crate::schema::validate(&s, &json!({"custom": {"any": "x"}, "body": {"type": "doc", "content": []}})).is_ok());
    }

    #[test]
    fn removed_field_is_refused() {
        let s = strict(&json!({"type": "object", "properties": {"title": {"type": "string"}, "notes": {"removed": true}}}));
        assert!(crate::schema::validate(&s, &json!({"title": "x", "notes": "old"})).is_err());
    }

    #[test]
    fn strict_applies_inside_list_items() {
        let s = strict(&json!({"type": "object", "properties": {"statuses": {"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}}}));
        assert!(crate::schema::validate(&s, &json!({"statuses": [{"id": "a"}]})).is_ok());
        assert!(crate::schema::validate(&s, &json!({"statuses": [{"id": "a", "x": 1}]})).is_err());
    }

    #[test]
    fn index_specs_from_field_and_collection() {
        let s = json!({"type": "object", "properties": {"project": {"type": "string", "index": true}, "key": {"type": "string", "unique": true}, "status": {"type": "string"}},
            "indexes": [{"fields": ["project", "status"]}, {"fields": ["status"], "unique": true}]});
        let got = index_specs("t.tasks", &s);
        let want = vec![
            IndexSpec { collection: "t.tasks".into(), fields: vec!["key".into()], unique: true },
            IndexSpec { collection: "t.tasks".into(), fields: vec!["project".into()], unique: false },
            IndexSpec { collection: "t.tasks".into(), fields: vec!["project".into(), "status".into()], unique: false },
            IndexSpec { collection: "t.tasks".into(), fields: vec!["status".into()], unique: true },
        ];
        assert_eq!(got, want);
    }

    // ---- the Schema check ----------------------------------------------------------------------

    fn v(version: u32, collections: serde_json::Value) -> SchemaFile {
        file(json!({"version": version, "collections": collections}))
    }
    fn obj(props: serde_json::Value) -> serde_json::Value {
        json!({"type": "object", "properties": props})
    }
    fn codes(r: Result<UpgradePlan, Vec<UpgradeProblem>>) -> Vec<(String, String)> {
        r.unwrap_err().into_iter().map(|p| (p.code, p.path)).collect()
    }
    fn ops(r: Result<UpgradePlan, Vec<UpgradeProblem>>) -> Vec<CollectionPlan> {
        r.unwrap_or_else(|p| panic!("refused: {p:?}")).collections
    }
    fn path(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn same_schema_same_version_needs_nothing() {
        let a = v(1, json!({"t.a": obj(json!({"x": {"type": "string"}}))}));
        assert!(ops(plan("@t/p", &a, &a.clone())).is_empty());
    }

    #[test]
    fn same_version_different_schema_refused() {
        let a = v(1, json!({"t.a": obj(json!({"x": {"type": "string"}}))}));
        let b = v(1, json!({"t.a": obj(json!({"x": {"type": "string"}, "y": {"type": "string"}}))}));
        assert_eq!(codes(plan("@t/p", &a, &b)), vec![("schema_changed_without_version".into(), "".into())]);
    }

    #[test]
    fn data_newer_than_the_plugin_is_refused() {
        let a = v(3, json!({}));
        assert_eq!(codes(plan("@t/p", &a, &v(2, json!({})))), vec![("data_newer".into(), "".into())]);
    }

    #[test]
    fn below_floor_is_refused() {
        let stored = v(1, json!({}));
        let new = file(json!({"version": 5, "upgrades_from": 3, "collections": {}}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("below_floor".into(), "".into())]);
    }

    #[test]
    fn chain_a_b_c_upgrades_from_a() {
        let stored = v(1, json!({"t.a": obj(json!({"a": {"type": "string"}}))}));
        let new = v(3, json!({"t.a": obj(json!({"c": {"type": "string", "renamed_from": ["b", "a"]}}))}));
        let p = ops(plan("@t/p", &stored, &new));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].ops, vec![FieldOp::Rename { from: path(&["a"]), to: path(&["c"]) }]);
    }

    #[test]
    fn rename_inside_list_items_and_nested_objects() {
        let stored = v(1, json!({"t.a": obj(json!({
            "statuses": {"type": "array", "items": obj(json!({"colour": {"type": "string"}}))},
            "settings": obj(json!({"font": {"type": "string"}}))}))}));
        let new = v(2, json!({"t.a": obj(json!({
            "statuses": {"type": "array", "items": obj(json!({"color": {"type": "string", "renamed_from": ["colour"]}}))},
            "settings": obj(json!({"face": {"type": "string", "renamed_from": ["font"]}}))}))}));
        let p = ops(plan("@t/p", &stored, &new));
        assert_eq!(p[0].ops, vec![
            FieldOp::Rename { from: path(&["settings", "font"]), to: path(&["settings", "face"]) },
            FieldOp::Rename { from: path(&["statuses", "[]", "colour"]), to: path(&["statuses", "[]", "color"]) },
        ]);
    }

    #[test]
    fn removed_and_defaulted_fields_are_planned() {
        let stored = v(1, json!({"t.a": obj(json!({"notes": {"type": "string"}, "title": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"notes": {"removed": true}, "title": {"type": "string"}, "kind": {"type": "string", "default": "task"}}))}));
        let p = ops(plan("@t/p", &stored, &new));
        assert_eq!(p[0].ops, vec![FieldOp::Remove(path(&["notes"])), FieldOp::Default(path(&["kind"]), json!("task"))]);
    }

    #[test]
    fn undeclared_disappearance_is_field_missing() {
        let stored = v(1, json!({"t.a": obj(json!({"level": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"access_level": {"type": "string"}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("field_missing".into(), "level".into())]);
    }

    #[test]
    fn type_change_needs_a_step() {
        let stored = v(1, json!({"t.a": obj(json!({"due": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"due": {"type": "integer"}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &new.clone())), vec![("type_changed".into(), "due".into())]);
        let with_step = file(json!({"version": 2, "steps": [2], "collections": new.collections}));
        assert!(ops(plan("@t/p", &stored, &with_step)).is_empty());
    }

    #[test]
    fn widening_a_type_is_allowed() {
        let stored = v(1, json!({"t.a": obj(json!({"due": {"type": "string"}, "n": {"type": "integer"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"due": {"type": ["string", "null"]}, "n": {"type": "number"}}))}));
        assert!(ops(plan("@t/p", &stored, &new)).is_empty());
    }

    #[test]
    fn tighter_limits_refused() {
        let stored = v(1, json!({"t.a": obj(json!({"t": {"type": "string", "maxLength": 200}, "k": {"type": "string", "enum": ["a", "b"]}}))}));
        let new = v(2, json!({"t.a": obj(json!({"t": {"type": "string", "maxLength": 100}, "k": {"type": "string", "enum": ["a"]}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("limit_tightened".into(), "k".into()), ("limit_tightened".into(), "t".into())]);
    }

    #[test]
    fn new_required_without_default_refused() {
        let stored = v(1, json!({"t.a": {"type": "object", "required": ["t"], "properties": {"t": {"type": "string"}}}}));
        let new = v(2, json!({"t.a": {"type": "object", "required": ["t", "kind"], "properties": {"t": {"type": "string"}, "kind": {"type": "string"}}}}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("required_without_default".into(), "kind".into())]);
    }

    #[test]
    fn a_field_that_becomes_required_without_default_is_refused() {
        let stored = v(1, json!({"t.a": obj(json!({"t": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": {"type": "object", "required": ["t"], "properties": {"t": {"type": "string"}}}}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("required_without_default".into(), "t".into())]);
    }

    #[test]
    fn collection_renamed_and_removed() {
        let stored = v(1, json!({"t.a": obj(json!({})), "t.b": obj(json!({}))}));
        let new = v(2, json!({"t.aa": {"type": "object", "properties": {}, "renamed_from": ["t.a"]}, "t.b": {"removed": true}}));
        let p = ops(plan("@t/p", &stored, &new));
        assert_eq!(p.iter().map(|c| (c.from.as_str(), c.to.as_str(), c.removed)).collect::<Vec<_>>(), vec![("t.a", "t.aa", false), ("t.b", "t.b", true)]);
    }

    #[test]
    fn a_collection_that_disappears_unannounced_is_refused() {
        let stored = v(1, json!({"t.a": obj(json!({}))}));
        assert_eq!(codes(plan("@t/p", &stored, &v(2, json!({})))), vec![("collection_missing".into(), "".into())]);
    }

    #[test]
    fn earlier_name_dropped_unless_floor_raised() {
        let stored = v(2, json!({"t.a": obj(json!({"c": {"type": "string", "renamed_from": ["a"]}}))}));
        let dropped = v(3, json!({"t.a": obj(json!({"c": {"type": "string"}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &dropped)), vec![("earlier_name_dropped".into(), "a".into())]);
        let floor = file(json!({"version": 3, "upgrades_from": 2, "collections": dropped.collections}));
        assert!(plan("@t/p", &stored, &floor).is_ok());
    }

    #[test]
    fn every_problem_is_listed_at_once() {
        let stored = v(1, json!({"t.a": obj(json!({"a": {"type": "string"}, "b": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"b": {"type": "integer"}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &new)).len(), 2);
    }

    #[test]
    fn retired_name_reused() {
        let f = v(2, json!({"t.a": obj(json!({"c": {"type": "string", "renamed_from": ["a"]}, "a": {"type": "string"}, "n": {"removed": true}, "m": {"type": "string", "renamed_from": ["n"]}}))}));
        let got: Vec<(String, String)> = static_problems("@t/p", &f).into_iter().map(|p| (p.code, p.path)).collect();
        assert_eq!(got, vec![("name_reused".into(), "a".into()), ("earlier_name_twice".into(), "n".into())]);
    }

    #[test]
    fn earlier_name_on_two_fields() {
        let f = v(2, json!({"t.a": obj(json!({"c": {"type": "string", "renamed_from": ["a"]}, "d": {"type": "string", "renamed_from": ["a"]}}))}));
        let got: Vec<String> = static_problems("@t/p", &f).into_iter().map(|p| p.code).collect();
        assert_eq!(got, vec!["earlier_name_twice".to_string()]);
    }

    #[test]
    fn collection_names_follow_the_same_rules() {
        let f = v(2, json!({"t.a": {"type": "object", "renamed_from": ["t.old"]}, "t.old": {"type": "object"}}));
        let got: Vec<(String, String)> = static_problems("@t/p", &f).into_iter().map(|p| (p.code, p.collection)).collect();
        assert_eq!(got, vec![("name_reused".into(), "t.old".into())]);
    }

    #[test]
    fn a_problem_reads_as_a_sentence() {
        let p = UpgradeProblem { plugin: "@t/p".into(), collection: "t.a".into(), path: "level".into(), code: "field_missing".into(), params: json!({}) };
        assert!(p.message().contains("t.a") && p.message().contains("level"), "{}", p.message());
    }

    #[test]
    fn apply_renames_removes_and_defaults_at_any_depth() {
        let mut b = json!({"a": 1, "notes": "x", "settings": {"font": "serif"}, "statuses": [{"colour": "red"}, {"colour": "blue"}, {"other": 1}]});
        apply(&mut b, &[
            FieldOp::Rename { from: path(&["a"]), to: path(&["c"]) },
            FieldOp::Remove(path(&["notes"])),
            FieldOp::Rename { from: path(&["settings", "font"]), to: path(&["settings", "face"]) },
            FieldOp::Rename { from: path(&["statuses", "[]", "colour"]), to: path(&["statuses", "[]", "color"]) },
            FieldOp::Default(path(&["kind"]), json!("task")),
            FieldOp::Default(path(&["statuses", "[]", "color"]), json!("grey")),
            FieldOp::Default(path(&["missing", "x"]), json!(1)),
        ]);
        assert_eq!(b, json!({"c": 1, "kind": "task", "settings": {"face": "serif"}, "statuses": [{"color": "red"}, {"color": "blue"}, {"other": 1, "color": "grey"}]}));
    }

    #[test]
    fn apply_keeps_an_explicit_null_and_does_not_overwrite() {
        let mut b = json!({"kind": null, "c": 2});
        apply(&mut b, &[FieldOp::Default(path(&["kind"]), json!("task")), FieldOp::Rename { from: path(&["a"]), to: path(&["c"]) }]);
        assert_eq!(b, json!({"kind": null, "c": 2}));
    }

    #[test]
    fn two_stored_fields_onto_one_new_field_are_refused() {
        let stored = v(1, json!({"t.a": obj(json!({"a": {"type": "string"}, "c": {"type": "string"}}))}));
        let new = v(2, json!({"t.a": obj(json!({"c": {"type": "string", "renamed_from": ["a"]}}))}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("names_merge".into(), "c".into())]);
    }

    #[test]
    fn two_stored_collections_onto_one_are_refused() {
        let stored = v(1, json!({"t.a": obj(json!({})), "t.b": obj(json!({}))}));
        let new = v(2, json!({"t.b": {"type": "object", "properties": {}, "renamed_from": ["t.a"]}}));
        assert_eq!(codes(plan("@t/p", &stored, &new)), vec![("names_merge".into(), "".into())]);
    }

    #[test]
    fn a_field_that_becomes_required_with_a_default_gets_it() {
        let stored = v(1, json!({"t.a": obj(json!({"due": {"type": ["string", "null"]}}))}));
        let new = v(2, json!({"t.a": {"type": "object", "required": ["due"], "properties": {"due": {"type": ["string", "null"], "default": null}}}}));
        assert_eq!(ops(plan("@t/p", &stored, &new))[0].ops, vec![FieldOp::Default(path(&["due"]), json!(null))]);
    }

    #[test]
    fn every_code_reads_as_a_sentence() {
        for code in ["data_without_schema", "interrupted_upgrade", "names_merge", "documents_do_not_fit", "unique_conflict"] {
            let p = UpgradeProblem { plugin: "@t/p".into(), collection: "t.a".into(), path: "".into(), code: code.into(), params: json!({"to": 2, "version": 3, "stored": 1, "count": 4, "example": "x", "reason": "r", "fields": ["k"]}) };
            assert!(!p.message().ends_with(code), "{code}: {}", p.message());
        }
    }
}
