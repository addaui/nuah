//! What a plugin is in code: a Manifest plus handlers attached to the names the Manifest declares,
//! and the [`Tx`] a handler works through.

use crate::core::Inner;
use crate::db::*;
use crate::error::{Category, Error, Result};
use crate::manifest::{CallPolicy, Manifest, OpKind};
use crate::ports::{Auth, Clock, Ids, Policy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub type OpFn = Arc<dyn Fn(&mut Tx, Value) -> Result<Value> + Send + Sync>;
pub type BeforeFn = Arc<dyn Fn(&mut Tx, &str, &mut Value) -> Result<()> + Send + Sync>;
pub type Next<'a> = &'a mut dyn FnMut(&mut Tx, Value) -> Result<Value>;
pub type AroundFn = Arc<dyn for<'a, 'b, 'c> Fn(&'a mut Tx<'b>, &str, Value, Next<'c>) -> Result<Value> + Send + Sync>;
pub type AfterFn = Arc<dyn Fn(&mut Tx, &str, &Value, &Value) -> Result<()> + Send + Sync>;
pub type ListenerFn = Arc<dyn Fn(&mut Tx, &Delivery) -> Result<()> + Send + Sync>;
pub type ScopeFn = Arc<dyn Fn(&dyn DbRead, &str, &str) -> Result<bool> + Send + Sync>;
pub type ResourceFn = Arc<dyn Fn(&dyn DbRead, &crate::ports::Identity, &str) -> Result<Option<Vec<String>>> + Send + Sync>;

/// An Event handed to a Listener, with where it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub seq: u64,
    pub at: Millis,
    pub actor: String,
    pub operation: String,
    pub event: Event,
}

/// The implementation a core-Port Adapter hands the kernel.
#[derive(Clone)]
pub enum PortImpl {
    Db(Arc<dyn Db>),
    Auth(Arc<dyn Auth>),
    Policy(Arc<dyn Policy>),
    Clock(Arc<dyn Clock>),
    Ids(Arc<dyn Ids>),
    Log(Arc<dyn crate::telemetry::LogAdapter>),
    Trace(Arc<dyn crate::telemetry::TraceAdapter>),
    Metric(Arc<dyn crate::telemetry::MetricAdapter>),
    Heartbeat(Arc<dyn crate::telemetry::HeartbeatAdapter>),
}

#[derive(Default)]
pub struct Bindings {
    pub(crate) ops: BTreeMap<String, OpFn>,
    pub(crate) before: BTreeMap<String, BeforeFn>,
    pub(crate) around: BTreeMap<String, AroundFn>,
    pub(crate) after: BTreeMap<String, AfterFn>,
    pub(crate) listeners: BTreeMap<String, ListenerFn>,
    pub(crate) scopes: BTreeMap<String, ScopeFn>,
    pub(crate) resources: BTreeMap<String, ResourceFn>,
    pub(crate) provides: BTreeMap<String, OpFn>,
}

impl Bindings {
    pub fn op<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Tx, Value) -> Result<Value> + Send + Sync + 'static,
    {
        self.ops.insert(name.into(), Arc::new(f));
    }
    pub fn before<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Tx, &str, &mut Value) -> Result<()> + Send + Sync + 'static,
    {
        self.before.insert(name.into(), Arc::new(f));
    }
    pub fn around<F>(&mut self, name: &str, f: F)
    where
        F: for<'a, 'b, 'c> Fn(&'a mut Tx<'b>, &str, Value, Next<'c>) -> Result<Value> + Send + Sync + 'static,
    {
        self.around.insert(name.into(), Arc::new(f));
    }
    pub fn after<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Tx, &str, &Value, &Value) -> Result<()> + Send + Sync + 'static,
    {
        self.after.insert(name.into(), Arc::new(f));
    }
    pub fn listener<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Tx, &Delivery) -> Result<()> + Send + Sync + 'static,
    {
        self.listeners.insert(name.into(), Arc::new(f));
    }
    pub fn scope<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&dyn DbRead, &str, &str) -> Result<bool> + Send + Sync + 'static,
    {
        self.scopes.insert(name.into(), Arc::new(f));
    }
    /// Answer for a resource type: the permissions this Identity holds on the resource, or `None` when
    /// it does not exist for them.
    pub fn resource<F>(&mut self, kind: &str, f: F)
    where
        F: Fn(&dyn DbRead, &crate::ports::Identity, &str) -> Result<Option<Vec<String>>> + Send + Sync + 'static,
    {
        self.resources.insert(kind.into(), Arc::new(f));
    }
    /// Fill an Extension point.
    pub fn provide<F>(&mut self, extension_point: &str, f: F)
    where
        F: Fn(&mut Tx, Value) -> Result<Value> + Send + Sync + 'static,
    {
        self.provides.insert(extension_point.into(), Arc::new(f));
    }
}

/// What a service gets when the Core starts it: the Link (a `Core` handle) and its own Settings.
pub struct ServiceContext {
    pub core: crate::core::Core,
    pub settings: Value,
}

/// A plugin's long-running part, for example a Projection listening on a socket. It runs in the Core's
/// process, on threads of its own ([ADR 0031]). The kernel stops it when the Core's services stop.
///
/// [ADR 0031]: https://docs.addaui.com/nuah/adr/0031-products-add-to-nuah-and-never-edit-it-ports-declare-cardinality-projections-are-plugins/
pub trait Service: Send {
    /// Stop and wait until it has. Called once; must not hang.
    fn stop(self: Box<Self>);
}

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    /// Attach handlers to the names the Manifest declares. Must not touch the outside world.
    fn bind(&self, _b: &mut Bindings) {}
    /// For an Adapter: build what fills the Port, from the plugin's Settings.
    fn fill(&self, _settings: &Value) -> Result<Option<PortImpl>> {
        Ok(None)
    }
    /// Start the plugin's long-running part. Called once the Core is ready, and only for a plugin whose
    /// Manifest asks for the capability `serve` (which the Profile must grant). Return `None` to run nothing.
    fn serve(&self, _ctx: ServiceContext) -> Result<Option<Box<dyn Service>>> {
        Ok(None)
    }
    /// Move this plugin's data from `from` to the Manifest's `data.version`, one step at a time.
    fn migrate(&self, _from: u32, _tx: &mut Tx) -> Result<()> {
        Ok(())
    }
}

pub(crate) enum Handle<'a> {
    R(&'a dyn DbRead),
    W(&'a mut dyn DbWrite),
}

/// What a plugin works through while it handles one request: the data it owns (and may read), the
/// Events it may emit, other plugins it may call. The kernel builds it; a plugin cannot forge one.
pub struct Tx<'a> {
    pub(crate) inner: &'a Inner,
    pub(crate) plugin: String,
    pub(crate) who: &'a crate::ports::Identity,
    pub(crate) request: &'a str,
    pub(crate) h: Handle<'a>,
    pub(crate) events: &'a mut Vec<Event>,
    pub(crate) docs: &'a mut Vec<DocRef>,
    pub(crate) depth: u32,
}

impl<'a> Tx<'a> {
    pub fn who(&self) -> &crate::ports::Identity {
        self.who
    }
    pub fn request_id(&self) -> &str {
        self.request
    }
    pub fn plugin_id(&self) -> &str {
        &self.plugin
    }
    pub fn now(&self) -> Millis {
        self.inner.clock.now()
    }
    pub fn new_id(&self) -> String {
        self.inner.ids.new_id()
    }
    /// Write a log record under this plugin's id, with the request id. The level is checked first.
    pub fn log(&self, level: crate::telemetry::Level, key: &str, fields: Value) {
        let t = &self.inner.telemetry;
        if t.enabled(level, &self.plugin) {
            t.log(crate::telemetry::record(self.inner.clock.now(), level, &self.plugin, key, fields, None, Some(self.request)));
        }
    }
    /// The Core's counters, gauges and histograms. Name a metric after the plugin (`tasks_created_total`).
    pub fn metrics(&self) -> &crate::telemetry::Metrics {
        &self.inner.telemetry.metrics
    }
    /// The Settings the operator gave this plugin in the Profile (null when none).
    pub fn settings(&self) -> Value {
        self.inner.settings.get(&self.plugin).cloned().unwrap_or(Value::Null)
    }
    pub fn is_query(&self) -> bool {
        matches!(self.h, Handle::R(_))
    }

    pub(crate) fn rd(&self) -> &dyn DbRead {
        match &self.h {
            Handle::R(r) => *r,
            Handle::W(w) => &**w,
        }
    }
    pub(crate) fn wr(&mut self) -> Result<&mut dyn DbWrite> {
        match &mut self.h {
            Handle::R(_) => Err(Error::internal("a Query may not write")),
            Handle::W(w) => Ok(&mut **w),
        }
    }

    fn may_read(&self, collection: &str) -> Result<()> {
        if self.plugin.is_empty() {
            return Ok(());
        }
        let m = self.inner.manifest(&self.plugin);
        let ok = m.is_some_and(|m| m.data.collections.iter().any(|c| c == collection) || m.data.reads.iter().any(|c| c == collection));
        if ok { Ok(()) } else { Err(Error::internal(format!("plugin {} did not declare collection {collection}", self.plugin))) }
    }
    fn may_write(&self, collection: &str) -> Result<()> {
        let ok = self.inner.manifest(&self.plugin).is_some_and(|m| m.data.collections.iter().any(|c| c == collection));
        if ok { Ok(()) } else { Err(Error::internal(format!("plugin {} does not own collection {collection}", self.plugin))) }
    }

    /// A live document, or `None`.
    pub fn get(&self, collection: &str, id: &str) -> Result<Option<Doc>> {
        self.may_read(collection)?;
        Ok(self.rd().get(collection, id)?.filter(|d| !d.deleted))
    }
    pub fn query(&self, collection: &str, q: &Query) -> Result<Page> {
        self.may_read(collection)?;
        for c in &q.conds {
            if !valid_field(&c.field) {
                return Err(Error::internal(format!("bad field path {}", c.field)));
            }
        }
        self.rd().query(collection, q)
    }
    pub fn count(&self, collection: &str, conds: &[Cond]) -> Result<u64> {
        self.may_read(collection)?;
        self.rd().count(collection, conds)
    }
    /// Create or replace a document in a collection this plugin owns.
    pub fn put(&mut self, collection: &str, id: &str, body: Value, expect: Expect) -> Result<Doc> {
        self.may_write(collection)?;
        if !body.is_object() {
            return Err(Error::internal("documents are JSON objects"));
        }
        let now = self.now();
        let owner = self.plugin.clone();
        let doc = self.wr()?.put(collection, id, &owner, body, expect, now)?;
        let kind = if doc.rev == 1 { ChangeKind::Created } else { ChangeKind::Updated };
        self.docs.push(DocRef { collection: collection.into(), id: id.into(), rev: doc.rev, kind });
        Ok(doc)
    }
    /// Create with a fresh id; fails if the id exists.
    pub fn create(&mut self, collection: &str, body: Value) -> Result<Doc> {
        let id = self.new_id();
        self.put(collection, &id, body, Expect::Absent)
    }
    pub fn delete(&mut self, collection: &str, id: &str) -> Result<bool> {
        self.may_write(collection)?;
        let now = self.now();
        let tomb = self.wr()?.delete(collection, id, Expect::Any, now)?;
        if let Some(t) = &tomb {
            self.docs.push(DocRef { collection: collection.into(), id: id.into(), rev: t.rev, kind: ChangeKind::Deleted });
        }
        Ok(tomb.is_some())
    }

    /// Record that something happened, in the same commit as the work that caused it.
    pub fn emit(&mut self, kind: &str, audience: Audience, data: Value) -> Result<()> {
        if self.is_query() {
            return Err(Error::internal("a Query may not emit Events"));
        }
        self.inner.check_capability(&self.plugin, "emit")?;
        let decl = self.inner.event_decl(kind, &self.plugin)?;
        if !decl.data.is_null() {
            crate::schema::validate(&decl.data, &data).map_err(|e| Error::internal(format!("event {kind}: {}", e.message)))?;
        }
        self.events.push(Event { kind: kind.into(), audience, data });
        Ok(())
    }

    /// Call another plugin's Operation as the same Subject: same permission check, same transaction.
    pub fn call(&mut self, op: &str, input: Value) -> Result<Value> {
        self.inner.check_capability(&self.plugin, "call")?;
        if self.depth >= 8 {
            return Err(Error::internal("operations call each other too deeply"));
        }
        let decl = self.inner.op_decl(op).ok_or_else(|| Error::internal(format!("unknown operation {op}")))?;
        if self.is_query() && decl.kind == OpKind::Command {
            return Err(Error::internal("a Query may not call a Command"));
        }
        let caller = std::mem::replace(&mut self.plugin, String::new());
        self.depth += 1;
        let out = self.inner.execute(self, op, input);
        self.depth -= 1;
        self.plugin = caller;
        out
    }

    /// Call whatever fills an Extension point this plugin requires, following the point's policy.
    pub fn extension_call(&mut self, point: &str, input: Value) -> Result<Value> {
        let allowed = self.inner.manifest(&self.plugin).is_some_and(|m| m.requires.extension_points.iter().any(|p| p == point));
        if !allowed {
            return Err(Error::internal(format!("plugin {} did not declare it uses {point}", self.plugin)));
        }
        let def = self.inner.extension_def(point).ok_or_else(|| Error::internal(format!("unknown extension point {point}")))?;
        crate::schema::validate(&def.input, &input).map_err(|e| Error::internal(format!("{point}: {}", e.message)))?;
        let providers = self.inner.providers_of(point);
        let caller = std::mem::take(&mut self.plugin);
        let mut results: Vec<Value> = vec![];
        let mut answer = Value::Null;
        for (pid, f) in providers {
            self.plugin = pid.clone();
            let nested = !self.is_query();
            if nested {
                self.wr()?.begin_nested()?;
            }
            let r = f(self, input.clone());
            let ok = r.is_ok();
            if nested {
                self.wr()?.end_nested(ok)?;
            }
            match (def.policy, r) {
                (CallPolicy::Single, Ok(v)) => {
                    answer = v;
                    break;
                }
                (CallPolicy::Single, Err(e)) => {
                    self.plugin = caller;
                    return Err(e);
                }
                (CallPolicy::First, Ok(v)) if !v.is_null() => {
                    answer = v;
                    break;
                }
                (CallPolicy::First, _) => {}
                (CallPolicy::Many, Ok(v)) => results.push(json!({ "provider": pid, "ok": true, "output": v })),
                (CallPolicy::Many, Err(e)) => results.push(json!({ "provider": pid, "ok": false, "error": e.message })),
            }
        }
        self.plugin = caller;
        Ok(if def.policy == CallPolicy::Many { Value::Array(results) } else { answer })
    }

    /// Make a conflict out of a plain failure of a stale-state check.
    pub fn stale(reason: &str, what: &str) -> Error {
        Error::new(Category::Conflict, reason, format!("{what} changed since you read it")).with_reason(reason)
    }
}
