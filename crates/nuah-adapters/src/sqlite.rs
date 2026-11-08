// SPDX-License-Identifier: Apache-2.0
//! The SQLite data Adapter. A file database has one writer connection (writes are serialized, as SQLite
//! requires) and a pool of reader connections (WAL), so many people can read at once, each on one
//! snapshot, while a write is going on ([ADR 0031]). `:memory:` has one connection for everything.
//!
//! [ADR 0031]: https://docs.addaui.com/nuah/adr/0031-products-add-to-nuah-and-never-edit-it-ports-declare-cardinality-projections-are-plugins/

use nuah_kernel::*;
use parking_lot::{Condvar, Mutex};
use rusqlite::{params, params_from_iter, types::Value as Sql, Connection, OptionalExtension};
use serde_json::Value;

/// The layout of the tables this Adapter creates (kept in `PRAGMA user_version`).
const LAYOUT: u32 = 1;

pub struct SqliteDb {
    /// The writer; also serves reads when there is no pool.
    conn: Mutex<Connection>,
    readers: Option<Readers>,
}

/// Idle reader connections. A read takes one and puts it back; when all are busy it waits for one.
struct Readers {
    idle: Mutex<Vec<Connection>>,
    back: Condvar,
}

impl Readers {
    fn take(&self) -> Connection {
        let mut idle = self.idle.lock();
        loop {
            if let Some(c) = idle.pop() {
                return c;
            }
            self.back.wait(&mut idle);
        }
    }
    fn give(&self, c: Connection) {
        self.idle.lock().push(c);
        self.back.notify_one();
    }
}

/// How many reader connections a file database gets unless the operator says: one per core, 2 to 16.
pub fn default_readers() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16)
}

fn db_err(e: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(f, _) = &e {
        if f.code == rusqlite::ErrorCode::ConstraintViolation {
            return Error::conflict("kernel.unique", "another document already has the same value in a unique index");
        }
    }
    Error::internal(format!("sqlite: {e}"))
}
fn json_err(e: serde_json::Error) -> Error {
    Error::internal(format!("stored json: {e}"))
}

impl SqliteDb {
    pub fn open(path: &str) -> Result<Self> {
        Self::open_with(path, default_readers())
    }

    /// `readers`: reader connections for a file database (0 or `:memory:` means one shared connection).
    pub fn open_with(path: &str, readers: usize) -> Result<Self> {
        let conn = if path == ":memory:" { Connection::open_in_memory() } else { Connection::open(path) }.map_err(db_err)?;
        if path != ":memory:" {
            conn.pragma_update(None, "journal_mode", "WAL").map_err(db_err)?;
            // With WAL a power cut can lose the last commits but never corrupts; it makes every commit one fsync cheaper.
            conn.pragma_update(None, "synchronous", "NORMAL").map_err(db_err)?;
            conn.pragma_update(None, "cache_size", -32_000).map_err(db_err)?;
        }
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(db_err)?;
        // the layout version this Adapter writes; a database from a newer Adapter is refused, not guessed at
        let layout: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(db_err)?;
        if layout > LAYOUT {
            return Err(Error::internal(format!("this database has layout {layout}, newer than the {LAYOUT} this version of the SQLite Adapter understands; update Nuah")));
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS docs (collection TEXT NOT NULL, id TEXT NOT NULL, rev INTEGER NOT NULL, changed_at INTEGER NOT NULL,
               deleted INTEGER NOT NULL, owner TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (collection, id)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS changes (seq INTEGER PRIMARY KEY AUTOINCREMENT, record TEXT NOT NULL);",
        )
        .map_err(db_err)?;
        if layout < LAYOUT {
            conn.pragma_update(None, "user_version", LAYOUT).map_err(db_err)?;
        }
        let readers = if path == ":memory:" || readers == 0 {
            None
        } else {
            let mut idle = Vec::with_capacity(readers);
            for _ in 0..readers {
                let r = Connection::open(path).map_err(db_err)?;
                r.busy_timeout(std::time::Duration::from_secs(5)).map_err(db_err)?;
                r.pragma_update(None, "query_only", true).map_err(db_err)?;
                r.pragma_update(None, "cache_size", -16_000).map_err(db_err)?;
                idle.push(r);
            }
            Some(Readers { idle: Mutex::new(idle), back: Condvar::new() })
        };
        Ok(SqliteDb { conn: Mutex::new(conn), readers })
    }
}

fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Null => Sql::Null,
        Value::Bool(b) => Sql::Integer(*b as i64),
        Value::Number(n) => n.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => Sql::Text(s.clone()),
        other => Sql::Text(other.to_string()),
    }
}

fn path(field: &str) -> Result<String> {
    if valid_field(field) { Ok(format!("json_extract(body,'$.{field}')")) } else { Err(Error::internal(format!("bad field path {field}"))) }
}

/// The rank of a value in the Port's ordering (null < bool < number < string < array < object), computed from
/// a SQLite JSON type name. SQLite itself would compare a stored `true` equal to `1` and sort arrays among
/// strings, so every comparison is made on the rank first.
fn rank_of(type_expr: &str) -> String {
    format!("(CASE {type_expr} WHEN 'true' THEN 1 WHEN 'false' THEN 1 WHEN 'integer' THEN 2 WHEN 'real' THEN 2 WHEN 'text' THEN 3 WHEN 'array' THEN 4 WHEN 'object' THEN 5 ELSE 0 END)")
}

fn field_rank(field: &str) -> Result<String> {
    if !valid_field(field) {
        return Err(Error::internal(format!("bad field path {field}")));
    }
    Ok(rank_of(&format!("json_type(body,'$.{field}')")))
}

fn value_rank(v: &Value) -> i64 {
    match v {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

/// `rank = the value's rank AND the value is equal`, as the Port defines equality.
fn eq_sql(rank: &str, val: &str, v: &Value, args: &mut Vec<Sql>) -> String {
    if v.is_null() {
        return format!("{rank}=0");
    }
    args.push(to_sql(v));
    format!("({rank}={} AND {val} IS ?)", value_rank(v))
}

/// WHERE fragments (all literals are validated names; values are bound).
fn where_sql(collection: &str, conds: &[Cond]) -> Result<(String, Vec<Sql>)> {
    if !plain_name(collection) {
        return Err(Error::internal("bad collection name"));
    }
    let mut sql = format!("collection='{collection}' AND deleted=0");
    let mut args = vec![];
    for c in conds {
        let p = path(&c.field)?;
        let r = field_rank(&c.field)?;
        match c.op {
            Cmp::Eq => sql += &format!(" AND {}", eq_sql(&r, &p, &c.value, &mut args)),
            Cmp::Ne => sql += &format!(" AND NOT {}", eq_sql(&r, &p, &c.value, &mut args)),
            Cmp::In => {
                let list = c.value.as_array().cloned().unwrap_or_default();
                let mut parts: Vec<String> = list.iter().map(|v| eq_sql(&r, &p, v, &mut args)).collect();
                if parts.is_empty() {
                    parts.push("0".into());
                }
                sql += &format!(" AND ({})", parts.join(" OR "));
            }
            Cmp::Has => {
                let inner = eq_sql(&rank_of("type"), "value", &c.value, &mut args);
                sql += &format!(" AND json_type(body,'$.{f}')='array' AND EXISTS (SELECT 1 FROM json_each(body,'$.{f}') WHERE {inner})", f = c.field);
            }
            Cmp::Lt | Cmp::Lte | Cmp::Gt | Cmp::Gte => {
                // never matches a missing or null field; a lower rank is smaller, the same rank compares values
                let (op, below, above) = match c.op {
                    Cmp::Lt => ("<", true, false),
                    Cmp::Lte => ("<=", true, false),
                    Cmp::Gt => (">", false, true),
                    _ => (">=", false, true),
                };
                let rv = value_rank(&c.value);
                let across = if below { format!("{r}<{rv}") } else if above { format!("{r}>{rv}") } else { "0".into() };
                sql += &format!(" AND {r}>0 AND ({across} OR ({r}={rv} AND {p} {op} ?))");
                args.push(to_sql(&c.value));
            }
        }
    }
    Ok((sql, args))
}

fn row_doc(r: &rusqlite::Row) -> rusqlite::Result<(Doc, String)> {
    let body: String = r.get(6)?;
    Ok((
        Doc { collection: r.get(0)?, id: r.get(1)?, rev: r.get::<_, i64>(2)? as u64, changed_at: r.get(3)?, deleted: r.get::<_, i64>(4)? != 0, owner: r.get(5)?, body: Value::Null },
        body,
    ))
}

fn finish(rows: Vec<(Doc, String)>) -> Result<Vec<Doc>> {
    rows.into_iter().map(|(mut d, b)| { d.body = serde_json::from_str(&b).map_err(json_err)?; Ok(d) }).collect()
}

struct Conn<'a> {
    c: &'a Connection,
    nested: u32,
}

impl DbRead for Conn<'_> {
    fn get(&self, collection: &str, id: &str) -> Result<Option<Doc>> {
        let row = self.c.query_row("SELECT collection,id,rev,changed_at,deleted,owner,body FROM docs WHERE collection=?1 AND id=?2", params![collection, id], row_doc).optional().map_err(db_err)?;
        Ok(match row { Some(r) => finish(vec![r])?.pop(), None => None })
    }
    fn query(&self, collection: &str, q: &Query) -> Result<Page> {
        let (w, mut args) = where_sql(collection, &q.conds)?;
        let order = match &q.order {
            Some((f, desc)) => {
                let dir = if *desc { "DESC" } else { "ASC" };
                format!(" ORDER BY {} {dir}, {} {dir}, id", field_rank(f)?, path(f)?)
            },
            None => " ORDER BY id".into(),
        };
        // ask for one more than the limit to know whether there is a next page
        let sql = format!("SELECT collection,id,rev,changed_at,deleted,owner,body FROM docs WHERE {w}{order} LIMIT ? OFFSET ?");
        args.push(Sql::Integer(q.limit as i64 + 1));
        args.push(Sql::Integer(q.offset as i64));
        let mut st = self.c.prepare_cached(&sql).map_err(db_err)?;
        let rows = st.query_map(params_from_iter(args), row_doc).map_err(db_err)?.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)?;
        let mut items = finish(rows)?;
        let next = if items.len() > q.limit { items.truncate(q.limit); Some(q.offset + q.limit) } else { None };
        Ok(Page { items, next })
    }
    fn count(&self, collection: &str, conds: &[Cond]) -> Result<u64> {
        let (w, args) = where_sql(collection, conds)?;
        let n: i64 = self.c.query_row(&format!("SELECT COUNT(*) FROM docs WHERE {w}"), params_from_iter(args), |r| r.get(0)).map_err(db_err)?;
        Ok(n as u64)
    }
    fn changes_after(&self, after: u64, limit: usize) -> Result<Vec<ChangeRecord>> {
        let mut st = self.c.prepare_cached("SELECT seq, record FROM changes WHERE seq>?1 ORDER BY seq LIMIT ?2").map_err(db_err)?;
        let rows = st.query_map(params![after as i64, limit as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).map_err(db_err)?.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)?;
        rows.into_iter().map(|(seq, rec)| { let mut r: ChangeRecord = serde_json::from_str(&rec).map_err(json_err)?; r.seq = seq as u64; Ok(r) }).collect()
    }
    fn oldest_change(&self) -> Result<u64> {
        let n: Option<i64> = self.c.query_row("SELECT MIN(seq) FROM changes", [], |r| r.get(0)).map_err(db_err)?;
        Ok(n.unwrap_or(0) as u64)
    }
    fn latest_change(&self) -> Result<u64> {
        let n: Option<i64> = self.c.query_row("SELECT seq FROM sqlite_sequence WHERE name='changes'", [], |r| r.get(0)).optional().map_err(db_err)?;
        Ok(n.unwrap_or(0) as u64)
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

impl DbWrite for Conn<'_> {
    fn put(&mut self, collection: &str, id: &str, owner: &str, body: Value, expect: Expect, now: Millis) -> Result<Doc> {
        let prev = self.get(collection, id)?;
        check_expect(&prev, expect)?;
        let rev = prev.map(|p| p.rev + 1).unwrap_or(1);
        self.c.execute("INSERT INTO docs (collection,id,rev,changed_at,deleted,owner,body) VALUES (?1,?2,?3,?4,0,?5,?6)
             ON CONFLICT (collection,id) DO UPDATE SET rev=excluded.rev, changed_at=excluded.changed_at, deleted=0, owner=excluded.owner, body=excluded.body", params![collection, id, rev as i64, now, owner, body.to_string()]).map_err(db_err)?;
        Ok(Doc { id: id.into(), collection: collection.into(), rev, changed_at: now, deleted: false, owner: owner.into(), body })
    }
    fn delete(&mut self, collection: &str, id: &str, expect: Expect, now: Millis) -> Result<Option<Doc>> {
        let prev = self.get(collection, id)?;
        check_expect(&prev, expect)?;
        match prev {
            Some(p) if !p.deleted => {
                self.c.execute("UPDATE docs SET rev=?3, changed_at=?4, deleted=1, body='{}' WHERE collection=?1 AND id=?2", params![collection, id, (p.rev + 1) as i64, now]).map_err(db_err)?;
                Ok(Some(Doc { rev: p.rev + 1, changed_at: now, deleted: true, body: serde_json::json!({}), ..p }))
            }
            _ => Ok(None),
        }
    }
    fn append_change(&mut self, rec: ChangeRecord) -> Result<u64> {
        self.c.execute("INSERT INTO changes (record) VALUES (?1)", params![serde_json::to_string(&rec).map_err(json_err)?]).map_err(db_err)?;
        Ok(self.c.last_insert_rowid() as u64)
    }
    fn prune_changes(&mut self, keep: usize) -> Result<()> {
        let latest = self.latest_change()? as i64;
        self.c.execute("DELETE FROM changes WHERE seq <= ?1", params![latest - keep as i64]).map_err(db_err)?;
        Ok(())
    }
    fn begin_nested(&mut self) -> Result<()> {
        self.nested += 1;
        self.c.execute_batch(&format!("SAVEPOINT n{}", self.nested)).map_err(db_err)
    }
    fn end_nested(&mut self, keep: bool) -> Result<()> {
        if self.nested == 0 {
            return Err(Error::internal("no nested transaction open"));
        }
        let n = self.nested;
        self.nested -= 1;
        let sql = if keep { format!("RELEASE n{n}") } else { format!("ROLLBACK TO n{n}; RELEASE n{n}") };
        self.c.execute_batch(&sql).map_err(db_err)
    }
}

impl Db for SqliteDb {
    fn name(&self) -> &str {
        "sqlite"
    }
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()> {
        if let Some(pool) = &self.readers {
            let c = pool.take();
            let begun = c.execute_batch("BEGIN").map_err(db_err);
            let r = begun.and_then(|_| f(&Conn { c: &c, nested: 0 }));
            let _ = c.execute_batch("ROLLBACK");
            pool.give(c);
            return r;
        }
        let guard = self.conn.lock();
        guard.execute_batch("BEGIN").map_err(db_err)?;
        let r = f(&Conn { c: &guard, nested: 0 });
        let _ = guard.execute_batch("ROLLBACK");
        r
    }
    fn write(&self, f: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()> {
        let guard = self.conn.lock();
        guard.execute_batch("BEGIN IMMEDIATE").map_err(db_err)?;
        let mut c = Conn { c: &guard, nested: 0 };
        match f(&mut c) {
            Ok(()) => guard.execute_batch("COMMIT").map_err(db_err),
            Err(e) => {
                let _ = guard.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
    fn features(&self) -> Features {
        Features::default()
            .with(feature::INDEX_UNIQUE, Support::Native)
            .with(feature::INDEX_COMPOSITE, Support::Native)
            .with(feature::BACKUP_SNAPSHOT, Support::Native)
            .fact("storage_version", LAYOUT)
    }
    fn snapshot(&self, path: &str) -> Result<()> {
        if std::path::Path::new(path).exists() {
            return Err(Error::invalid("path", "the snapshot target already exists"));
        }
        self.conn.lock().execute("VACUUM INTO ?1", params![path]).map_err(db_err)?;
        Ok(())
    }
    fn ensure_index(&self, spec: &IndexSpec) -> Result<()> {
        spec.check()?;
        let collection = &spec.collection;
        if !plain_name(collection) {
            return Err(Error::internal("bad index name"));
        }
        let base = format!("{}_{}", collection.replace(['.', '-', ':'], "_"), spec.fields.iter().map(|f| f.replace('.', "_")).collect::<Vec<_>>().join("__"));
        let mut cols = vec![];
        for f in &spec.fields {
            if spec.unique {
                // a missing or null value is exempt from uniqueness: both parts are NULL
                cols.push(format!("NULLIF({}, 0)", field_rank(f)?));
            } else {
                cols.push(field_rank(f)?);
            }
            cols.push(path(f)?);
        }
        let cols = cols.join(", ");
        let sql = if spec.unique {
            format!("CREATE UNIQUE INDEX IF NOT EXISTS uq_{base} ON docs ({cols}) WHERE collection='{collection}' AND deleted=0")
        } else {
            // the index leads with the rank, so it serves the same expressions the queries use
            let old = if spec.fields.len() == 1 { format!("DROP INDEX IF EXISTS ix_{base}; ") } else { String::new() };
            format!("{old}CREATE INDEX IF NOT EXISTS ix2_{base} ON docs ({cols}) WHERE collection='{collection}'")
        };
        self.conn.lock().execute_batch(&sql).map_err(|e| match db_err(e) {
            c if c.category == Category::Conflict => Error::internal(format!("cannot make a unique index on {collection}: existing documents already share values")),
            other => other,
        })
    }
}
