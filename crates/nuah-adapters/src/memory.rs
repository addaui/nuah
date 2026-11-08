// SPDX-License-Identifier: Apache-2.0
//! The in-memory data Adapter: the reference for what the data Port means, and the fast test Adapter.
//! A reader holds an immutable snapshot; a writer works on a private copy that replaces the state on commit.

use nuah_kernel::*;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Default)]
struct State {
    docs: BTreeMap<(String, String), Doc>,
    changes: BTreeMap<u64, ChangeRecord>,
    last_seq: u64,
    /// Unique indexes; a put checks them by looking at the other live documents of the collection.
    uniques: Vec<IndexSpec>,
}

pub struct MemoryDb {
    state: Mutex<Arc<State>>,
    writer: Mutex<()>,
}

impl MemoryDb {
    pub fn new() -> Self {
        MemoryDb { state: Mutex::new(Arc::new(State::default())), writer: Mutex::new(()) }
    }
}
impl Default for MemoryDb {
    fn default() -> Self {
        Self::new()
    }
}

fn check_fields(conds: &[Cond], order: &Option<(String, bool)>) -> Result<()> {
    for c in conds {
        if !valid_field(&c.field) {
            return Err(Error::internal(format!("bad field path {}", c.field)));
        }
    }
    if let Some((f, _)) = order {
        if !valid_field(f) {
            return Err(Error::internal(format!("bad field path {f}")));
        }
    }
    Ok(())
}

/// The values of `fields` in `body`, or `None` when any is missing or null (such a document is exempt from a
/// unique index).
fn key_of<'a>(body: &'a Value, fields: &[String]) -> Option<Vec<&'a Value>> {
    fields.iter().map(|f| field_of(body, f).filter(|v| !v.is_null())).collect()
}

impl State {
    /// Does another live document of the collection share `body`'s values for the index?
    fn clashes(&self, spec: &IndexSpec, id: &str, body: &Value) -> bool {
        let Some(mine) = key_of(body, &spec.fields) else { return false };
        self.live(&spec.collection).any(|d| {
            d.id != id && key_of(&d.body, &spec.fields).is_some_and(|theirs| theirs.iter().zip(&mine).all(|(a, b)| compare(a, b).is_eq()))
        })
    }
    fn live<'a>(&'a self, collection: &'a str) -> impl Iterator<Item = &'a Doc> + 'a {
        self.docs
            .range((collection.to_string(), String::new())..)
            .take_while(move |((c, _), _)| c == collection)
            .map(|(_, d)| d)
            .filter(|d| !d.deleted)
    }
    fn do_get(&self, c: &str, id: &str) -> Option<Doc> {
        self.docs.get(&(c.to_string(), id.to_string())).cloned()
    }
    fn do_query(&self, c: &str, q: &Query) -> Result<Page> {
        check_fields(&q.conds, &q.order)?;
        let mut hits: Vec<&Doc> = self.live(c).filter(|d| q.conds.iter().all(|k| matches(&d.body, k))).collect();
        if let Some((f, desc)) = &q.order {
            hits.sort_by(|a, b| {
                let o = compare(field_of(&a.body, f).unwrap_or(&Value::Null), field_of(&b.body, f).unwrap_or(&Value::Null));
                let o = if *desc { o.reverse() } else { o };
                o.then_with(|| a.id.cmp(&b.id))
            });
        }
        let total = hits.len();
        let items: Vec<Doc> = hits.into_iter().skip(q.offset).take(q.limit).cloned().collect();
        let next = if q.offset + items.len() < total { Some(q.offset + items.len()) } else { None };
        Ok(Page { items, next })
    }
    fn do_count(&self, c: &str, conds: &[Cond]) -> Result<u64> {
        check_fields(conds, &None)?;
        Ok(self.live(c).filter(|d| conds.iter().all(|k| matches(&d.body, k))).count() as u64)
    }
}

struct Reader(Arc<State>);

impl DbRead for Reader {
    fn get(&self, c: &str, id: &str) -> Result<Option<Doc>> {
        Ok(self.0.do_get(c, id))
    }
    fn query(&self, c: &str, q: &Query) -> Result<Page> {
        self.0.do_query(c, q)
    }
    fn count(&self, c: &str, conds: &[Cond]) -> Result<u64> {
        self.0.do_count(c, conds)
    }
    fn changes_after(&self, after: u64, limit: usize) -> Result<Vec<ChangeRecord>> {
        Ok(self.0.changes.range(after + 1..).take(limit).map(|(_, r)| r.clone()).collect())
    }
    fn oldest_change(&self) -> Result<u64> {
        Ok(self.0.changes.keys().next().copied().unwrap_or(0))
    }
    fn latest_change(&self) -> Result<u64> {
        Ok(self.0.last_seq)
    }
}

struct Writer {
    work: State,
    stack: Vec<State>,
}

impl DbRead for Writer {
    fn get(&self, c: &str, id: &str) -> Result<Option<Doc>> {
        Ok(self.work.do_get(c, id))
    }
    fn query(&self, c: &str, q: &Query) -> Result<Page> {
        self.work.do_query(c, q)
    }
    fn count(&self, c: &str, conds: &[Cond]) -> Result<u64> {
        self.work.do_count(c, conds)
    }
    fn changes_after(&self, after: u64, limit: usize) -> Result<Vec<ChangeRecord>> {
        Ok(self.work.changes.range(after + 1..).take(limit).map(|(_, r)| r.clone()).collect())
    }
    fn oldest_change(&self) -> Result<u64> {
        Ok(self.work.changes.keys().next().copied().unwrap_or(0))
    }
    fn latest_change(&self) -> Result<u64> {
        Ok(self.work.last_seq)
    }
}

fn check_expect(existing: &Option<Doc>, expect: Expect) -> Result<()> {
    let live = existing.as_ref().filter(|d| !d.deleted);
    match expect {
        Expect::Any => Ok(()),
        Expect::Absent if live.is_some() => Err(Error::conflict("kernel.exists", "already exists")),
        Expect::Absent => Ok(()),
        Expect::Rev(n) => match live {
            Some(d) if d.rev == n => Ok(()),
            _ => Err(Error::conflict("kernel.stale_revision", "changed since it was read")),
        },
    }
}

impl DbWrite for Writer {
    fn put(&mut self, c: &str, id: &str, owner: &str, body: Value, expect: Expect, now: Millis) -> Result<Doc> {
        let prev = self.work.do_get(c, id);
        check_expect(&prev, expect)?;
        if let Some(spec) = self.work.uniques.iter().find(|u| u.collection == c && self.work.clashes(u, id, &body)) {
            return Err(unique_violation(&spec.fields));
        }
        let doc = Doc { id: id.into(), collection: c.into(), rev: prev.map(|p| p.rev + 1).unwrap_or(1), changed_at: now, deleted: false, owner: owner.into(), body };
        self.work.docs.insert((c.into(), id.into()), doc.clone());
        Ok(doc)
    }
    fn delete(&mut self, c: &str, id: &str, expect: Expect, now: Millis) -> Result<Option<Doc>> {
        let prev = self.work.do_get(c, id);
        check_expect(&prev, expect)?;
        match prev {
            Some(p) if !p.deleted => {
                let t = Doc { rev: p.rev + 1, changed_at: now, deleted: true, body: json!({}), ..p };
                self.work.docs.insert((c.into(), id.into()), t.clone());
                Ok(Some(t))
            }
            _ => Ok(None),
        }
    }
    fn append_change(&mut self, mut rec: ChangeRecord) -> Result<u64> {
        self.work.last_seq += 1;
        rec.seq = self.work.last_seq;
        self.work.changes.insert(rec.seq, rec);
        Ok(self.work.last_seq)
    }
    fn prune_changes(&mut self, keep: usize) -> Result<()> {
        while self.work.changes.len() > keep {
            let first = *self.work.changes.keys().next().unwrap();
            self.work.changes.remove(&first);
        }
        Ok(())
    }
    fn begin_nested(&mut self) -> Result<()> {
        self.stack.push(self.work.clone());
        Ok(())
    }
    fn end_nested(&mut self, keep: bool) -> Result<()> {
        let saved = self.stack.pop().ok_or_else(|| Error::internal("no nested transaction open"))?;
        if !keep {
            self.work = saved;
        }
        Ok(())
    }
}

impl Db for MemoryDb {
    fn name(&self) -> &str {
        "memory"
    }
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()> {
        let snap = self.state.lock().clone();
        f(&Reader(snap))
    }
    fn write(&self, f: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()> {
        let _one_writer = self.writer.lock();
        let base = self.state.lock().clone();
        let mut w = Writer { work: (*base).clone(), stack: vec![] };
        f(&mut w)?;
        *self.state.lock() = Arc::new(w.work);
        Ok(())
    }
    fn features(&self) -> Features {
        Features::default().with(feature::INDEX_UNIQUE, Support::Emulated).with(feature::INDEX_COMPOSITE, Support::Native).fact("storage_version", 0)
    }
    fn ensure_index(&self, spec: &IndexSpec) -> Result<()> {
        spec.check()?;
        if !spec.unique {
            return Ok(());
        }
        let _one_writer = self.writer.lock();
        let mut next = (**self.state.lock()).clone();
        if next.uniques.contains(spec) {
            return Ok(());
        }
        let taken = next.live(&spec.collection).any(|d| next.clashes(spec, &d.id, &d.body));
        if taken {
            return Err(Error::internal(format!("cannot make a unique index on {}: existing documents already share values", spec.collection)));
        }
        next.uniques.push(spec.clone());
        *self.state.lock() = Arc::new(next);
        Ok(())
    }
}
