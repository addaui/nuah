// SPDX-License-Identifier: Apache-2.0
//! The data Port ([ADR 0001], [ADR 0011], [ADR 0002]).
//!
//! The kernel states *intent*: read handles see one consistent snapshot; write handles are
//! serializable with each other and may be run more than once by the adapter, so the closure given to
//! [`Db::write`] must be safe to repeat. How an adapter achieves that (one connection, a pool,
//! optimistic retries) is its own business. The Port stays synchronous.
//!
//! Every stored document is wrapped in an envelope kept by the adapter and the kernel (revision, change
//! time, tombstone, owning plugin), never by plugins.
//!
//! [ADR 0001]: https://docs.addaui.com/nuah/adr/0001-ports-state-intent-adapters-choose-mechanism/
//! [ADR 0011]: https://docs.addaui.com/nuah/adr/0011-kernel-owned-document-envelope/
//! [ADR 0002]: https://docs.addaui.com/nuah/adr/0002-ports-stay-synchronous-for-now/

use crate::error::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Millis = i64;

/// A stored document with its envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    pub id: String,
    pub collection: String,
    /// Starts at 1, grows by one on every write (including the tombstone).
    pub rev: u64,
    pub changed_at: Millis,
    pub deleted: bool,
    /// The plugin that owns the collection.
    pub owner: String,
    pub body: Value,
}

/// What a write expects to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Any,
    /// The document must not exist (or be a tombstone).
    Absent,
    /// The live document must have exactly this revision.
    Rev(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Eq,
    Ne,
    In,
    Lt,
    Lte,
    Gt,
    Gte,
    /// The field is an array containing the value.
    Has,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cond {
    /// A dotted path into the body, such as `status` or `due.at`. Letters, digits, `_` and `.` only.
    pub field: String,
    pub op: Cmp,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub conds: Vec<Cond>,
    pub order: Option<(String, bool)>,
    pub limit: usize,
    pub offset: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query { conds: vec![], order: None, limit: 200, offset: 0 }
    }
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn eq(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Eq, value: value.into() });
        self
    }
    pub fn ne(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Ne, value: value.into() });
        self
    }
    pub fn is_in(mut self, field: &str, values: Vec<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::In, value: Value::Array(values) });
        self
    }
    pub fn lt(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Lt, value: value.into() });
        self
    }
    pub fn lte(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Lte, value: value.into() });
        self
    }
    pub fn gt(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Gt, value: value.into() });
        self
    }
    pub fn gte(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Gte, value: value.into() });
        self
    }
    pub fn has(mut self, field: &str, value: impl Into<Value>) -> Self {
        self.conds.push(Cond { field: field.into(), op: Cmp::Has, value: value.into() });
        self
    }
    pub fn asc(mut self, field: &str) -> Self {
        self.order = Some((field.into(), false));
        self
    }
    pub fn desc(mut self, field: &str) -> Self {
        self.order = Some((field.into(), true));
        self
    }
    pub fn limit(mut self, n: usize) -> Self {
        self.limit = n;
        self
    }
    pub fn offset(mut self, n: usize) -> Self {
        self.offset = n;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub items: Vec<Doc>,
    /// The offset to ask for next, when there is more.
    pub next: Option<usize>,
}

// ---- the change record (ADR 0003) -------------------------------------------------------------

/// Who an [`Event`] is addressed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Audience {
    Subject { subject: String },
    /// A group declared by a plugin (such as the members of a project), resolved when Pulse is read.
    Scope { scope: String, id: String },
}

impl Audience {
    pub fn subject(s: &str) -> Self {
        Audience::Subject { subject: s.into() }
    }
    pub fn scope(scope: &str, id: &str) -> Self {
        Audience::Scope { scope: scope.into(), id: id.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub audience: Audience,
    pub data: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocRef {
    pub collection: String,
    pub id: String,
    pub rev: u64,
    pub kind: ChangeKind,
}

/// One record per commit, in commit order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub seq: u64,
    pub at: Millis,
    pub actor: String,
    pub request: String,
    pub operation: String,
    pub docs: Vec<DocRef>,
    pub events: Vec<Event>,
}

// ---- the handles ------------------------------------------------------------------------------

pub trait DbRead {
    /// The document including a tombstone (`deleted: true`), or `None` if it never existed.
    fn get(&self, collection: &str, id: &str) -> Result<Option<Doc>>;
    /// Live documents only.
    fn query(&self, collection: &str, q: &Query) -> Result<Page>;
    fn count(&self, collection: &str, conds: &[Cond]) -> Result<u64>;
    /// Change records with `seq > after`, in commit order.
    fn changes_after(&self, after: u64, limit: usize) -> Result<Vec<ChangeRecord>>;
    /// The smallest `seq` still kept (0 when the log is empty).
    fn oldest_change(&self) -> Result<u64>;
    /// The largest `seq` ever assigned (0 when none).
    fn latest_change(&self) -> Result<u64>;
}

pub trait DbWrite: DbRead {
    /// Create or replace. `owner` is the plugin that owns the collection.
    fn put(&mut self, collection: &str, id: &str, owner: &str, body: Value, expect: Expect, now: Millis) -> Result<Doc>;
    /// Tombstone. Returns the tombstone, or `None` if there was nothing live to delete.
    fn delete(&mut self, collection: &str, id: &str, expect: Expect, now: Millis) -> Result<Option<Doc>>;
    /// Append a change record; the adapter assigns `seq` (commit order) and returns it.
    fn append_change(&mut self, rec: ChangeRecord) -> Result<u64>;
    /// Drop change records older than the newest `keep`.
    fn prune_changes(&mut self, keep: usize) -> Result<()>;
    /// Nested work that can be undone without undoing the caller: begin, then end with keep or discard.
    fn begin_nested(&mut self) -> Result<()>;
    fn end_nested(&mut self, keep: bool) -> Result<()>;
}

pub trait Db: Send + Sync {
    fn name(&self) -> &str;
    /// Run `f` against one consistent snapshot.
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()>;
    /// Run `f` as one atomic, serializable write. The adapter may run `f` again after a failed attempt
    /// to commit, so `f` must start from scratch every time. An `Err` rolls everything back.
    fn write(&self, f: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()>;
    /// Make queries on `field` of `collection` fast. A hint, never required for correctness.
    fn ensure_index(&self, collection: &str, field: &str) -> Result<()>;
}

/// Field paths are placed in queries and indexes, so they are checked wherever they enter.
pub fn valid_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= 64
        && !field.starts_with('.')
        && !field.ends_with('.')
        && !field.contains("..")
        && field.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Follow a dotted path into a JSON value.
pub fn field_of<'a>(body: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = body;
    for part in path.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

/// The one ordering every adapter must agree on: null < bool < number < string < array < object.
pub fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Null => 0,
            Value::Bool(_) => 1,
            Value::Number(_) => 2,
            Value::String(_) => 3,
            Value::Array(_) => 4,
            Value::Object(_) => 5,
        }
    }
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Number(x), Value::Number(y)) => {
            if let (Some(i), Some(j)) = (x.as_i64(), y.as_i64()) {
                i.cmp(&j)
            } else {
                x.as_f64().partial_cmp(&y.as_f64()).unwrap_or(Equal)
            }
        }
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => rank(a).cmp(&rank(b)).then_with(|| a.to_string().cmp(&b.to_string())),
    }
}

/// Does a body satisfy a condition? A missing field behaves like null.
pub fn matches(body: &Value, c: &Cond) -> bool {
    let got = field_of(body, &c.field).unwrap_or(&Value::Null);
    match c.op {
        Cmp::Eq => compare(got, &c.value).is_eq(),
        Cmp::Ne => !compare(got, &c.value).is_eq(),
        Cmp::In => c.value.as_array().is_some_and(|a| a.iter().any(|v| compare(got, v).is_eq())),
        // Ordered comparisons never match a missing field.
        Cmp::Lt => !got.is_null() && compare(got, &c.value).is_lt(),
        Cmp::Lte => !got.is_null() && compare(got, &c.value).is_le(),
        Cmp::Gt => !got.is_null() && compare(got, &c.value).is_gt(),
        Cmp::Gte => !got.is_null() && compare(got, &c.value).is_ge(),
        Cmp::Has => got.as_array().is_some_and(|a| a.iter().any(|v| compare(v, &c.value).is_eq())),
    }
}
