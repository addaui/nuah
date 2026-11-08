// SPDX-License-Identifier: Apache-2.0
//! The PostgreSQL data Adapter.
//!
//! One Nuah process per database ([ADR 0036]): the Adapter holds a session advisory lock for the schema it uses
//! and refuses to start when another process holds it. Writes go through one connection, serializable, and are
//! retried when Postgres asks (serialization failure, deadlock); reads take a pooled connection and see one
//! snapshot (`REPEATABLE READ`). The change log is numbered by a counter row that a writer locks until it
//! commits, so numbers are handed out in commit order. Document bodies are `jsonb`; every comparison leads
//! with the value's type, so the rule is the Port's and not Postgres's (see `compare`).
//!
//! [ADR 0036]: https://docs.addaui.com/nuah/adr/0036-one-nuah-process-per-database-for-now-scaling-out-comes-before-the-hosted-service/

use nuah_kernel::*;
use parking_lot::{Condvar, Mutex};
use postgres::error::SqlState;
use postgres::types::ToSql;
use postgres::Client;
use serde_json::Value;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// The layout of the tables this Adapter creates (kept in `nuah_state`).
const LAYOUT: i64 = 1;
/// The class half of the advisory lock key; the other half is a hash of the schema name.
const LOCK_CLASS: i32 = 0x4e75_6168;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS nuah_docs (
  collection text COLLATE "C" NOT NULL, id text COLLATE "C" NOT NULL, rev bigint NOT NULL, changed_at bigint NOT NULL,
  deleted boolean NOT NULL, owner text NOT NULL, body jsonb NOT NULL, PRIMARY KEY (collection, id));
CREATE TABLE IF NOT EXISTS nuah_changes (seq bigint PRIMARY KEY, record jsonb NOT NULL);
CREATE TABLE IF NOT EXISTS nuah_state (key text PRIMARY KEY, value bigint NOT NULL);
INSERT INTO nuah_state (key, value) VALUES ('latest_change', 0) ON CONFLICT DO NOTHING;
"#;

fn pg_err(e: postgres::Error) -> Error {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        if code == &SqlState::UNIQUE_VIOLATION {
            return Error::conflict("kernel.unique", "another document already has the same value in a unique index");
        }
        if code == &SqlState::T_R_SERIALIZATION_FAILURE || code == &SqlState::T_R_DEADLOCK_DETECTED {
            return Error::conflict("kernel.retry", "the database asked to try again");
        }
        if code == &SqlState::UNTRANSLATABLE_CHARACTER || code.code() == "22P05" {
            return Error::invalid("body", "text may not contain the character U+0000");
        }
    }
    Error::internal(format!("postgres: {e}"))
}

fn json_err(e: serde_json::Error) -> Error {
    Error::internal(format!("stored json: {e}"))
}

struct Pool {
    idle: Mutex<Vec<Client>>,
    back: Condvar,
}

impl Pool {
    fn take(&self) -> Client {
        let mut idle = self.idle.lock();
        loop {
            if let Some(c) = idle.pop() {
                return c;
            }
            self.back.wait(&mut idle);
        }
    }
    fn give(&self, c: Client) {
        self.idle.lock().push(c);
        self.back.notify_one();
    }
}

pub struct PgDb {
    writer: Mutex<Client>,
    readers: Option<Pool>,
    /// Is there any unique index? Then every put runs under a savepoint, so a refused put does not
    /// poison the rest of the transaction (the Port lets a caller go on after a conflict).
    uniques: AtomicBool,
}

fn plain_ident(s: &str) -> bool {
    !s.is_empty() && s.len() <= 48 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 80 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | ':'))
}

/// TLS for connections whose URL asks for it (`sslmode=require`); the server's certificate is checked against
/// the public roots. `sslmode=disable` (or a server without TLS under the default `prefer`) stays plain.
fn tls() -> tokio_postgres_rustls::MakeRustlsConnect {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}

fn connect(url: &str, schema: Option<&str>) -> Result<Client> {
    let mut c = Client::connect(url, tls()).map_err(|e| Error::internal(format!("postgres: cannot connect: {e}")))?;
    if let Some(s) = schema {
        c.batch_execute(&format!("SET search_path TO {s}")).map_err(pg_err)?;
    }
    Ok(c)
}

impl PgDb {
    /// `url`: a libpq-style URL (`postgres://user:password@host/db`). `schema`: keep the tables in this schema
    /// (created when missing) instead of the connection's default. `readers`: pooled read connections (0: reads
    /// share the writer).
    pub fn open(url: &str, schema: Option<&str>, readers: usize) -> Result<Self> {
        if let Some(s) = schema {
            if !plain_ident(s) {
                return Err(Error::internal("the schema name may hold letters, digits and _ only"));
            }
            let mut c = connect(url, None)?;
            c.batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {s}")).map_err(pg_err)?;
        }
        let mut w = connect(url, schema)?;
        let got: bool = w
            .query_one("SELECT pg_try_advisory_lock($1, hashtext(current_schema()))", &[&LOCK_CLASS])
            .map_err(pg_err)?
            .get(0);
        if !got {
            return Err(Error::internal("another Nuah process is already using this database (schema); one process per database"));
        }
        w.batch_execute(DDL).map_err(pg_err)?;
        let layout: Option<i64> = w.query_opt("SELECT value FROM nuah_state WHERE key='layout'", &[]).map_err(pg_err)?.map(|r| r.get(0));
        if layout.is_some_and(|l| l > LAYOUT) {
            return Err(Error::internal(format!("this database has layout {}, newer than the {LAYOUT} this version of the Postgres Adapter understands; update Nuah", layout.unwrap())));
        }
        w.execute("INSERT INTO nuah_state (key, value) VALUES ('layout', $1) ON CONFLICT (key) DO UPDATE SET value=EXCLUDED.value", &[&LAYOUT]).map_err(pg_err)?;
        let has_unique = w
            .query_one("SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname=current_schema() AND tablename='nuah_docs' AND indexname LIKE 'nuah\\_uq\\_%')", &[])
            .map_err(pg_err)?
            .get::<_, bool>(0);
        let readers = if readers == 0 {
            None
        } else {
            let mut idle = Vec::with_capacity(readers);
            for _ in 0..readers {
                idle.push(connect(url, schema)?);
            }
            Some(Pool { idle: Mutex::new(idle), back: Condvar::new() })
        };
        Ok(PgDb { writer: Mutex::new(w), readers, uniques: AtomicBool::new(has_unique) })
    }
}

// ---- SQL pieces ---------------------------------------------------------------------------------

/// Bound values, all passed as text and cast in the SQL (`$1::text::jsonb`).
#[derive(Default)]
struct Binds(Vec<String>);

impl Binds {
    fn add(&mut self, s: String) -> usize {
        self.0.push(s);
        self.0.len()
    }
    fn refs(&self) -> Vec<&(dyn ToSql + Sync)> {
        self.0.iter().map(|s| s as &(dyn ToSql + Sync)).collect()
    }
}

fn parts(field: &str) -> Result<String> {
    if !valid_field(field) {
        return Err(Error::internal(format!("bad field path {field}")));
    }
    Ok(field.split('.').collect::<Vec<_>>().join(","))
}
/// The field as jsonb (SQL NULL when missing).
fn jval(field: &str) -> Result<String> {
    Ok(format!("(body #> '{{{}}}')", parts(field)?))
}
/// The field as text, for strings.
fn jtext(field: &str) -> Result<String> {
    Ok(format!("(body #>> '{{{}}}')", parts(field)?))
}
/// The rank of a value in the Port's ordering, from its jsonb type; missing is rank 0 like null.
fn rank_of(j: &str) -> String {
    format!("(CASE jsonb_typeof({j}) WHEN 'boolean' THEN 1 WHEN 'number' THEN 2 WHEN 'string' THEN 3 WHEN 'array' THEN 4 WHEN 'object' THEN 5 ELSE 0 END)")
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

fn eq_sql(j: &str, v: &Value, b: &mut Binds) -> String {
    if v.is_null() {
        return format!("{}=0", rank_of(j));
    }
    let n = b.add(v.to_string());
    format!("({j} = ${n}::text::jsonb)")
}

fn ordered_sql(field: &str, op: Cmp, v: &Value, b: &mut Binds) -> Result<String> {
    let (sym, below, above) = match op {
        Cmp::Lt => ("<", true, false),
        Cmp::Lte => ("<=", true, false),
        Cmp::Gt => (">", false, true),
        _ => (">=", false, true),
    };
    let (j, jt) = (jval(field)?, jtext(field)?);
    let (r, rv) = (rank_of(&j), value_rank(v));
    let across = if below { format!("{r}<{rv}") } else if above { format!("{r}>{rv}") } else { "false".into() };
    let within = match v {
        Value::Null => "false".to_string(),
        _ => {
            let n = b.add(v.to_string());
            match v {
                Value::Bool(_) => format!("(CASE WHEN jsonb_typeof({j})='boolean' THEN ({j})::boolean {sym} (${n}::text)::boolean ELSE false END)"),
                Value::Number(_) => format!("(CASE WHEN jsonb_typeof({j})='number' THEN ({j})::numeric {sym} (${n}::text)::numeric ELSE false END)"),
                Value::String(_) => {
                    // the bound text is JSON (a quoted string); take the plain text out of it
                    format!("(CASE WHEN jsonb_typeof({j})='string' THEN ({jt} COLLATE \"C\") {sym} ((${n}::text)::jsonb #>> '{{}}' COLLATE \"C\") ELSE false END)")
                }
                _ => {
                    let t = if v.is_array() { "array" } else { "object" };
                    format!("(CASE WHEN jsonb_typeof({j})='{t}' THEN {j} {sym} ${n}::text::jsonb ELSE false END)")
                }
            }
        }
    };
    Ok(format!("({r}>0 AND ({across} OR ({r}={rv} AND {within})))"))
}

fn where_sql(collection: &str, conds: &[Cond], b: &mut Binds) -> Result<String> {
    let c = b.add(collection.to_string());
    let mut sql = format!("collection=${c} AND NOT deleted");
    for k in conds {
        let j = jval(&k.field)?;
        match k.op {
            Cmp::Eq => sql += &format!(" AND {}", eq_sql(&j, &k.value, b)),
            Cmp::Ne => {
                if k.value.is_null() {
                    sql += &format!(" AND {}<>0", rank_of(&j));
                } else {
                    let n = b.add(k.value.to_string());
                    sql += &format!(" AND ({j} IS DISTINCT FROM ${n}::text::jsonb)");
                }
            }
            Cmp::In => {
                let list = k.value.as_array().cloned().unwrap_or_default();
                let mut ors: Vec<String> = list.iter().map(|v| eq_sql(&j, v, b)).collect();
                if ors.is_empty() {
                    ors.push("false".into());
                }
                sql += &format!(" AND ({})", ors.join(" OR "));
            }
            Cmp::Has => {
                let n = b.add(k.value.to_string());
                sql += &format!(" AND (jsonb_typeof({j})='array' AND EXISTS (SELECT 1 FROM jsonb_array_elements({j}) AS e(v) WHERE e.v = ${n}::text::jsonb))");
            }
            Cmp::Lt | Cmp::Lte | Cmp::Gt | Cmp::Gte => sql += &format!(" AND {}", ordered_sql(&k.field, k.op, &k.value, b)?),
        }
    }
    Ok(sql)
}

fn order_sql(field: &str, desc: bool) -> Result<String> {
    let (j, jt) = (jval(field)?, jtext(field)?);
    let d = if desc { "DESC" } else { "ASC" };
    // one column per type; within a rank only one of them is not NULL, so the others tie
    Ok(format!(
        " ORDER BY {r} {d}, (CASE WHEN jsonb_typeof({j})='boolean' THEN ({j})::boolean END) {d}, (CASE WHEN jsonb_typeof({j})='number' THEN ({j})::numeric END) {d}, \
         (CASE WHEN jsonb_typeof({j})='string' THEN {jt} COLLATE \"C\" END) {d}, (CASE WHEN jsonb_typeof({j}) IN ('array','object') THEN {j} END) {d}, id",
        r = rank_of(&j)
    ))
}

const COLS: &str = "collection,id,rev,changed_at,deleted,owner,body::text";

fn row_doc(r: &postgres::Row) -> Result<Doc> {
    let body: String = r.get(6);
    Ok(Doc {
        collection: r.get(0),
        id: r.get(1),
        rev: r.get::<_, i64>(2) as u64,
        changed_at: r.get(3),
        deleted: r.get(4),
        owner: r.get(5),
        body: serde_json::from_str(&body).map_err(json_err)?,
    })
}

// ---- the handles --------------------------------------------------------------------------------

struct Conn<'a> {
    c: RefCell<&'a mut Client>,
    nested: u32,
    savepoint_puts: bool,
}

impl Conn<'_> {
    fn rows(&self, sql: &str, b: &Binds) -> Result<Vec<postgres::Row>> {
        self.c.borrow_mut().query(sql, &b.refs()).map_err(pg_err)
    }
    fn exec(&self, sql: &str, p: &[&(dyn ToSql + Sync)]) -> Result<u64> {
        self.c.borrow_mut().execute(sql, p).map_err(pg_err)
    }
}

impl DbRead for Conn<'_> {
    fn get(&self, collection: &str, id: &str) -> Result<Option<Doc>> {
        let rows = self.c.borrow_mut().query(&format!("SELECT {COLS} FROM nuah_docs WHERE collection=$1 AND id=$2"), &[&collection, &id]).map_err(pg_err)?;
        rows.first().map(row_doc).transpose()
    }
    fn query(&self, collection: &str, q: &Query) -> Result<Page> {
        let mut b = Binds::default();
        let w = where_sql(collection, &q.conds, &mut b)?;
        let order = match &q.order {
            Some((f, desc)) => order_sql(f, *desc)?,
            None => " ORDER BY id".into(),
        };
        let sql = format!("SELECT {COLS} FROM nuah_docs WHERE {w}{order} LIMIT {} OFFSET {}", q.limit + 1, q.offset);
        let mut items = self.rows(&sql, &b)?.iter().map(row_doc).collect::<Result<Vec<_>>>()?;
        let next = if items.len() > q.limit { items.truncate(q.limit); Some(q.offset + q.limit) } else { None };
        Ok(Page { items, next })
    }
    fn count(&self, collection: &str, conds: &[Cond]) -> Result<u64> {
        let mut b = Binds::default();
        let w = where_sql(collection, conds, &mut b)?;
        let rows = self.rows(&format!("SELECT count(*) FROM nuah_docs WHERE {w}"), &b)?;
        Ok(rows[0].get::<_, i64>(0) as u64)
    }
    fn changes_after(&self, after: u64, limit: usize) -> Result<Vec<ChangeRecord>> {
        let rows = self.c.borrow_mut().query("SELECT seq, record::text FROM nuah_changes WHERE seq > $1 ORDER BY seq LIMIT $2", &[&(after as i64), &(limit as i64)]).map_err(pg_err)?;
        rows.iter()
            .map(|r| {
                let mut rec: ChangeRecord = serde_json::from_str(&r.get::<_, String>(1)).map_err(json_err)?;
                rec.seq = r.get::<_, i64>(0) as u64;
                Ok(rec)
            })
            .collect()
    }
    fn oldest_change(&self) -> Result<u64> {
        let r = self.c.borrow_mut().query_one("SELECT COALESCE(MIN(seq), 0) FROM nuah_changes", &[]).map_err(pg_err)?;
        Ok(r.get::<_, i64>(0) as u64)
    }
    fn latest_change(&self) -> Result<u64> {
        let r = self.c.borrow_mut().query_one("SELECT value FROM nuah_state WHERE key='latest_change'", &[]).map_err(pg_err)?;
        Ok(r.get::<_, i64>(0) as u64)
    }
    fn scan(&self, ac: &str, ai: &str, limit: usize) -> Result<Vec<Doc>> {
        let rows = self.c.borrow_mut().query(&format!("SELECT {COLS} FROM nuah_docs WHERE (collection,id) > ($1,$2) ORDER BY collection,id LIMIT $3"), &[&ac, &ai, &(limit as i64)]).map_err(pg_err)?;
        rows.iter().map(row_doc).collect()
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

const UPSERT: &str = "INSERT INTO nuah_docs (collection,id,rev,changed_at,deleted,owner,body) VALUES ($1,$2,$3,$4,$5,$6,$7::text::jsonb)
  ON CONFLICT (collection,id) DO UPDATE SET rev=EXCLUDED.rev, changed_at=EXCLUDED.changed_at, deleted=EXCLUDED.deleted, owner=EXCLUDED.owner, body=EXCLUDED.body";

impl DbWrite for Conn<'_> {
    fn put(&mut self, collection: &str, id: &str, owner: &str, body: Value, expect: Expect, now: Millis) -> Result<Doc> {
        let prev = self.get(collection, id)?;
        check_expect(&prev, expect)?;
        let rev = prev.map(|p| p.rev + 1).unwrap_or(1);
        let text = body.to_string();
        let args: [&(dyn ToSql + Sync); 7] = [&collection, &id, &(rev as i64), &now, &false, &owner, &text];
        if self.savepoint_puts {
            self.c.borrow_mut().batch_execute("SAVEPOINT nuah_put").map_err(pg_err)?;
            match self.exec(UPSERT, &args) {
                Ok(_) => self.c.borrow_mut().batch_execute("RELEASE nuah_put").map_err(pg_err)?,
                Err(e) => {
                    self.c.borrow_mut().batch_execute("ROLLBACK TO nuah_put; RELEASE nuah_put").map_err(pg_err)?;
                    return Err(e);
                }
            }
        } else {
            self.exec(UPSERT, &args)?;
        }
        Ok(Doc { id: id.into(), collection: collection.into(), rev, changed_at: now, deleted: false, owner: owner.into(), body })
    }
    fn delete(&mut self, collection: &str, id: &str, expect: Expect, now: Millis) -> Result<Option<Doc>> {
        let prev = self.get(collection, id)?;
        check_expect(&prev, expect)?;
        match prev {
            Some(p) if !p.deleted => {
                self.exec("UPDATE nuah_docs SET rev=$3, changed_at=$4, deleted=true, body='{}'::jsonb WHERE collection=$1 AND id=$2", &[&collection, &id, &((p.rev + 1) as i64), &now])?;
                Ok(Some(Doc { rev: p.rev + 1, changed_at: now, deleted: true, body: serde_json::json!({}), ..p }))
            }
            _ => Ok(None),
        }
    }
    fn append_change(&mut self, mut rec: ChangeRecord) -> Result<u64> {
        // the counter row stays locked until this transaction ends, so writers number their records one after
        // another, in the order they commit
        let seq: i64 = self.c.borrow_mut().query_one("UPDATE nuah_state SET value = value + 1 WHERE key='latest_change' RETURNING value", &[]).map_err(pg_err)?.get(0);
        rec.seq = seq as u64;
        let text = serde_json::to_string(&rec).map_err(json_err)?;
        self.exec("INSERT INTO nuah_changes (seq, record) VALUES ($1, $2::text::jsonb)", &[&seq, &text])?;
        Ok(seq as u64)
    }
    fn prune_changes(&mut self, keep: usize) -> Result<()> {
        let latest = self.latest_change()? as i64;
        self.exec("DELETE FROM nuah_changes WHERE seq <= $1", &[&(latest - keep as i64)])?;
        Ok(())
    }
    fn restore(&mut self, d: Doc) -> Result<()> {
        let text = d.body.to_string();
        self.exec(UPSERT, &[&d.collection, &d.id, &(d.rev as i64), &d.changed_at, &d.deleted, &d.owner, &text])?;
        Ok(())
    }
    fn restore_change(&mut self, rec: ChangeRecord) -> Result<()> {
        let (seq, text) = (rec.seq as i64, serde_json::to_string(&rec).map_err(json_err)?);
        self.exec("INSERT INTO nuah_changes (seq, record) VALUES ($1, $2::text::jsonb)", &[&seq, &text])?;
        self.restore_latest_change(rec.seq)
    }
    fn restore_latest_change(&mut self, n: u64) -> Result<()> {
        self.exec("UPDATE nuah_state SET value = GREATEST(value, $1) WHERE key='latest_change'", &[&(n as i64)])?;
        Ok(())
    }
    fn begin_nested(&mut self) -> Result<()> {
        self.nested += 1;
        self.c.borrow_mut().batch_execute(&format!("SAVEPOINT n{}", self.nested)).map_err(pg_err)
    }
    fn end_nested(&mut self, keep: bool) -> Result<()> {
        if self.nested == 0 {
            return Err(Error::internal("no nested transaction open"));
        }
        let n = self.nested;
        self.nested -= 1;
        let sql = if keep { format!("RELEASE n{n}") } else { format!("ROLLBACK TO n{n}; RELEASE n{n}") };
        self.c.borrow_mut().batch_execute(&sql).map_err(pg_err)
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

impl Db for PgDb {
    fn name(&self) -> &str {
        "postgres"
    }
    fn features(&self) -> Features {
        Features::default().with(feature::INDEX_UNIQUE, Support::Native).with(feature::INDEX_COMPOSITE, Support::Native).fact("storage_version", LAYOUT)
    }
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()> {
        let run = |c: &mut Client, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>| {
            c.batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").map_err(pg_err)?;
            let r = f(&Conn { c: RefCell::new(&mut *c), nested: 0, savepoint_puts: false });
            let _ = c.batch_execute("ROLLBACK");
            r
        };
        if let Some(pool) = &self.readers {
            let mut c = pool.take();
            let r = run(&mut c, f);
            pool.give(c);
            return r;
        }
        run(&mut self.writer.lock(), f)
    }
    fn write(&self, f: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()> {
        const TRIES: u32 = 5;
        for attempt in 1..=TRIES {
            let mut guard = self.writer.lock();
            guard.batch_execute("BEGIN ISOLATION LEVEL SERIALIZABLE").map_err(pg_err)?;
            let r = f(&mut Conn { c: RefCell::new(&mut guard), nested: 0, savepoint_puts: self.uniques.load(Ordering::Relaxed) });
            match r {
                Ok(()) => match guard.batch_execute("COMMIT") {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        let e = pg_err(e);
                        if e.key == "kernel.retry" && attempt < TRIES {
                            continue;
                        }
                        return Err(e);
                    }
                },
                Err(e) => {
                    let _ = guard.batch_execute("ROLLBACK");
                    if e.key == "kernel.retry" && attempt < TRIES {
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(Error::conflict("kernel.retry", "the database kept asking to try again"))
    }
    fn ensure_index(&self, spec: &IndexSpec) -> Result<()> {
        spec.check()?;
        if !plain_name(&spec.collection) {
            return Err(Error::internal("bad index name"));
        }
        let key = format!("{}|{}|{}", spec.collection, spec.fields.join(","), spec.unique);
        let name = format!("nuah_{}_{:016x}", if spec.unique { "uq" } else { "ix" }, fnv(&key));
        let mut cols = vec![];
        for f in &spec.fields {
            let j = jval(f)?;
            // a JSON null is exempt from uniqueness like a missing value
            cols.push(if spec.unique { format!("(NULLIF({j}, 'null'::jsonb))") } else { j });
        }
        let cols = cols.join(", ");
        let c = &spec.collection;
        let sql = if spec.unique {
            format!("CREATE UNIQUE INDEX IF NOT EXISTS {name} ON nuah_docs ({cols}) WHERE collection='{c}' AND NOT deleted")
        } else {
            format!("CREATE INDEX IF NOT EXISTS {name} ON nuah_docs ({cols}) WHERE collection='{c}'")
        };
        let mut w = self.writer.lock();
        w.batch_execute(&sql).map_err(|e| match pg_err(e) {
            e if e.reason.as_deref() == Some("kernel.unique") => Error::internal(format!("cannot make a unique index on {c}: existing documents already share values")),
            other => other,
        })?;
        if spec.unique {
            self.uniques.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}
