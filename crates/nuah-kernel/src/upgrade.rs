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
}
