// SPDX-License-Identifier: Apache-2.0
//! What a plugin is in code: a Manifest plus handlers attached to the names the Manifest declares,
//! and the [`Tx`] a handler works through.

use crate::core::Inner;
use crate::db::*;
use crate::error::{Category, Error, Result};
use crate::manifest::{CallPolicy, Manifest, OpKind};
use crate::ports::{Auth, Clock, Ids, Net, Policy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub type OpFn = Arc<dyn Fn(&mut Tx, Value) -> Result<Value> + Send + Sync>;
pub type OutboundFn = Arc<dyn Fn(&mut Outbound, Value) -> Result<Value> + Send + Sync>;
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
    Net(Arc<dyn Net>),
    Log(Arc<dyn crate::telemetry::LogAdapter>),
    Trace(Arc<dyn crate::telemetry::TraceAdapter>),
    Metric(Arc<dyn crate::telemetry::MetricAdapter>),
    Heartbeat(Arc<dyn crate::telemetry::HeartbeatAdapter>),
}

#[derive(Default)]
pub struct Bindings {
    pub(crate) ops: BTreeMap<String, OpFn>,
    pub(crate) outbound: BTreeMap<String, OutboundFn>,
    pub(crate) before: BTreeMap<String, BeforeFn>,
    pub(crate) around: BTreeMap<String, AroundFn>,
    pub(crate) after: BTreeMap<String, AfterFn>,
    pub(crate) listeners: BTreeMap<String, ListenerFn>,
    pub(crate) scopes: BTreeMap<String, ScopeFn>,
    pub(crate) resources: BTreeMap<String, ResourceFn>,
    pub(crate) provides: BTreeMap<String, OpFn>,
    pub(crate) disposers: Vec<Arc<dyn Fn() + Send + Sync>>,
}

impl Bindings {
    /// What this plugin closes when the Core stops (an open file, a connection, a thread it started). Run after the services
    /// have stopped, in the reverse of the order plugins are started, each with a time limit ([ADR 0135]).
    ///
    /// [ADR 0135]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=423f1889730140fa86c845229cc0f610
    pub fn on_stop<F>(&mut self, f: F)
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.disposers.push(Arc::new(f));
    }
    pub fn op<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Tx, Value) -> Result<Value> + Send + Sync + 'static,
    {
        self.ops.insert(name.into(), Arc::new(f));
    }
    /// The handler of an Operation declared with kind `outbound` ([ADR 0040]): it has no transaction.
    ///
    /// [ADR 0040]: https://docs.addaui.com/nuah/adr/0040-outbound-operations-run-outside-the-transaction-and-reach-the-network-through-a-net-port/
    pub fn outbound<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&mut Outbound, Value) -> Result<Value> + Send + Sync + 'static,
    {
        self.outbound.insert(name.into(), Arc::new(f));
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
    /// Answer whether a Subject is in a Scope (an Event's audience).
    ///
    /// The resolver gets the database whole, not filtered by the plugin's Read declaration (it answers with grants, never
    /// with data). Every field of another plugin's Collection it reads must still be named in the plugin's `data.reads`,
    /// so the owner cannot rename or remove it without the Resolver saying so ([ADR 0091]).
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    pub fn scope<F>(&mut self, name: &str, f: F)
    where
        F: Fn(&dyn DbRead, &str, &str) -> Result<bool> + Send + Sync + 'static,
    {
        self.scopes.insert(name.into(), Arc::new(f));
    }
    /// Answer for a resource type: the permissions this Identity holds on the resource, or `None` when
    /// it does not exist for them.
    ///
    /// The resolver gets the database whole, not filtered by the plugin's Read declaration (it answers with grants, never
    /// with data). Every field of another plugin's Collection it reads must still be named in the plugin's `data.reads`,
    /// so the owner cannot rename or remove it without the Resolver saying so ([ADR 0091]).
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
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
    /// How to ask this service whether it is well, from any thread, while it runs ([ADR 0135]). The Core lists the answer
    /// among its health checks, under the plugin's id, and reports `degraded` when it says not ok. The default is "no probe".
    ///
    /// [ADR 0135]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=423f1889730140fa86c845229cc0f610
    fn health_probe(&self) -> Option<HealthProbe> {
        None
    }
}

/// Answers whether a service is well, with a short detail.
pub type HealthProbe = Arc<dyn Fn() -> (bool, String) + Send + Sync>;

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;
    /// The built files of this plugin's Browser modules, by the path the Manifest's `browser` map names (`index.js`) and
    /// any file they load. Served at `/plugins/<scope>/<name>/<path>` while the plugin is enabled ([ADR 0095]).
    ///
    /// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
    fn browser_files(&self) -> &'static [(&'static str, &'static [u8])] {
        &[]
    }
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
    /// Called once, when this plugin's data is new (nothing stored yet): put what a first start needs (seed accounts
    /// from the Settings, for example). Not called on an upgrade.
    fn init_data(&self, _tx: &mut Tx) -> Result<()> {
        Ok(())
    }
    /// An Upgrade step ([ADR 0091]): called for every batch of every Collection when the data moves past a Data version
    /// listed in `steps`, after the kernel applied the declared renames, removals and defaults. `batch` holds the
    /// Documents (id, body) under their new Collection name `collection`; change bodies in place, remove an entry to
    /// delete that Document, push one to create it. The kernel writes the batch afterwards and checks each body against
    /// the new schema. The work for one batch and the runner's progress commit together.
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    fn upgrade(&self, _to: u32, _collection: &str, _batch: &mut Vec<(String, Value)>, _tx: &mut Tx) -> Result<()> {
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
    /// Signals sent in this transaction: handed to Pulse after the commit, never stored.
    pub(crate) signals: Vec<Event>,
    pub(crate) depth: u32,
    /// Values this plugin read or wrote in fields its schema marks `x-secret`: kept out of Events, Signals and logs.
    pub(crate) secrets: std::sync::Mutex<Vec<String>>,
}

/// The body field the Core keeps the person in for a `per_subject` Collection. A plugin can neither set nor see it.
const SUBJECT: &str = "_subject";

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
            let fields = self.redacted(fields);
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
    /// Does the data Adapter provide this feature (see [`feature`](crate::feature))? For a plugin that
    /// `prefers` a feature and takes another path without it.
    /// The font data of every enabled font plugin (ADR 0090), for an operation that describes fonts to a reader.
    pub fn font_data(&self) -> Vec<Value> {
        self.inner.plan.active.values().filter_map(|a| a.manifest.fonts.clone()).collect()
    }
    pub fn has_feature(&self, name: &str) -> bool {
        self.inner.features.has(name)
    }
    /// Who signs in under this name, with the stable `subject` other plugins store ([`Auth::find`](crate::ports::Auth::find)).
    pub fn find_person(&self, name: &str) -> Result<Option<crate::ports::Identity>> {
        self.inner.auth.find(self.rd(), name)
    }
    /// Does this instance have accounts people sign in to (see [`Auth::has_accounts`](crate::ports::Auth::has_accounts))?
    pub fn has_accounts(&self) -> bool {
        self.inner.auth.has_accounts()
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

    /// For a `per_subject` Collection, the signed-in person; `None` for any other. A caller with no subject is refused.
    fn subject_for(&self, collection: &str) -> Result<Option<String>> {
        if self.plugin.is_empty() || !self.inner.plan.active.values().any(|a| a.manifest.data.per_subject.contains(collection)) {
            return Ok(None);
        }
        let subject = &self.who().subject;
        if subject.is_empty() {
            return Err(Error::unauthenticated());
        }
        Ok(Some(subject.clone()))
    }
    /// Conditions and ordering may not name the reserved field.
    fn refuse_subject_paths<'p>(&self, mut paths: impl Iterator<Item = &'p str>) -> Result<()> {
        if paths.any(|f| f.split('.').next() == Some(SUBJECT)) {
            return Err(Error::internal(format!("{SUBJECT} is kept by the Core; a plugin may not name it")));
        }
        Ok(())
    }
    /// Take the person out of a document a plugin is about to see; `None` if the document is another person's.
    fn mine(&self, subject: &Option<String>, mut d: Doc) -> Option<Doc> {
        let Some(s) = subject else { return Some(d) };
        let o = d.body.as_object_mut()?;
        (o.remove(SUBJECT).and_then(|v| v.as_str().map(String::from)).as_deref() == Some(s.as_str())).then_some(d)
    }
    fn with_subject(subject: &Option<String>, conds: &[Cond]) -> Vec<Cond> {
        let mut v = conds.to_vec();
        if let Some(s) = subject {
            v.push(Cond { field: SUBJECT.into(), op: Cmp::Eq, value: Value::String(s.clone()) });
        }
        v
    }
    /// Remember the values of the `x-secret` fields of a document of `collection`.
    fn note_secrets(&self, collection: &str, body: &Value) {
        let Some(fields) = self.inner.plan.active.values().find_map(|a| a.manifest.data.secrets.get(collection)) else { return };
        let mut seen = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        for f in fields {
            if let Some(v) = body.get(f).and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
                if !seen.iter().any(|x| x == v) {
                    seen.push(v.to_string());
                }
            }
        }
    }
    /// `data` with every remembered secret replaced, for an Event, a Signal or a log line.
    fn redacted(&self, data: Value) -> Value {
        let seen = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        if seen.is_empty() {
            return data;
        }
        fn go(v: Value, secrets: &[String]) -> Value {
            match v {
                Value::String(mut s) => {
                    for x in secrets {
                        if s.contains(x.as_str()) {
                            s = s.replace(x.as_str(), "[secret]");
                        }
                    }
                    Value::String(s)
                }
                Value::Array(a) => Value::Array(a.into_iter().map(|x| go(x, secrets)).collect()),
                Value::Object(o) => Value::Object(o.into_iter().map(|(k, x)| (k, go(x, secrets))).collect()),
                other => other,
            }
        }
        go(data, &seen)
    }
    fn seen(&self, collection: &str, d: Doc) -> Doc {
        self.note_secrets(collection, &d.body);
        d
    }

    fn may_read(&self, collection: &str) -> Result<()> {
        if self.plugin.is_empty() {
            return Ok(());
        }
        let m = self.inner.manifest(&self.plugin);
        let ok = m.is_some_and(|m| m.data.collections.iter().any(|c| c == collection) || m.data.reads.contains_key(collection));
        if ok { Ok(()) } else { Err(Error::internal(format!("plugin {} did not declare collection {collection}", self.plugin))) }
    }
    fn may_write(&self, collection: &str) -> Result<()> {
        let ok = self.inner.manifest(&self.plugin).is_some_and(|m| m.data.collections.iter().any(|c| c == collection));
        if ok { Ok(()) } else { Err(Error::internal(format!("plugin {} does not own collection {collection}", self.plugin))) }
    }

    /// The fields this plugin declared it reads from another plugin's Collection (its Read declaration), or `None`
    /// for a Collection it owns (or kernel work), which it sees whole ([ADR 0091]).
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    fn read_fields(&self, collection: &str) -> Option<&Vec<String>> {
        let m = self.inner.manifest(&self.plugin)?;
        if m.data.collections.iter().any(|c| c == collection) {
            return None;
        }
        m.data.reads.get(collection)
    }
    /// A reader may filter and order only by fields it declared.
    fn check_read_paths<'p>(&self, collection: &str, paths: impl Iterator<Item = &'p str>) -> Result<()> {
        if let Some(fields) = self.read_fields(collection) {
            for f in paths {
                let top = f.split('.').next().unwrap_or("");
                if !fields.iter().any(|x| x == top) {
                    return Err(Error::internal(format!("plugin {} reads {collection} by the field {f}, which its Read declaration does not name", self.plugin)));
                }
            }
        }
        Ok(())
    }
    fn only_declared(&self, collection: &str, mut d: Doc) -> Doc {
        if let (Some(fields), Some(o)) = (self.read_fields(collection), d.body.as_object_mut()) {
            o.retain(|k, _| fields.iter().any(|f| f == k));
        }
        d
    }
    /// A live document, or `None`. From another plugin's Collection, only the declared fields.
    pub fn get(&self, collection: &str, id: &str) -> Result<Option<Doc>> {
        self.may_read(collection)?;
        let subject = self.subject_for(collection)?;
        Ok(self.rd().get(collection, id)?.filter(|d| !d.deleted).and_then(|d| self.mine(&subject, d)).map(|d| self.seen(collection, self.only_declared(collection, d))))
    }
    pub fn query(&self, collection: &str, q: &Query) -> Result<Page> {
        self.may_read(collection)?;
        for c in &q.conds {
            if !valid_field(&c.field) {
                return Err(Error::internal(format!("bad field path {}", c.field)));
            }
        }
        self.check_read_paths(collection, q.conds.iter().map(|c| c.field.as_str()).chain(q.order.iter().map(|(f, _)| f.as_str())))?;
        let subject = self.subject_for(collection)?;
        let mut q = q.clone();
        if subject.is_some() {
            self.refuse_subject_paths(q.conds.iter().map(|c| c.field.as_str()).chain(q.order.iter().map(|(f, _)| f.as_str())))?;
            q.conds = Self::with_subject(&subject, &q.conds);
        }
        let mut page = self.rd().query(collection, &q)?;
        page.items = page.items.into_iter().filter_map(|d| self.mine(&subject, d)).map(|d| self.seen(collection, self.only_declared(collection, d))).collect();
        Ok(page)
    }
    pub fn count(&self, collection: &str, conds: &[Cond]) -> Result<u64> {
        self.may_read(collection)?;
        self.check_read_paths(collection, conds.iter().map(|c| c.field.as_str()))?;
        let subject = self.subject_for(collection)?;
        if subject.is_some() {
            self.refuse_subject_paths(conds.iter().map(|c| c.field.as_str()))?;
        }
        self.rd().count(collection, &Self::with_subject(&subject, conds))
    }
    /// Live documents of a Collection this plugin owns whose search fields (`"search": true` in its schema) hold
    /// every word of `q`, folded as the index folds text, and that meet `conds`; newest first, at most `limit`
    /// ([ADR 0092]). A query with no words finds nothing.
    ///
    /// [ADR 0092]: https://docs.addaui.com/nuah/adr/0092-one-index-of-entities-plugins-announce-them-in-their-events-the-data-port-searches-text-and-permissions-follow-the-events-audience/
    pub fn search(&self, collection: &str, q: &str, conds: &[Cond], limit: usize) -> Result<Vec<Doc>> {
        let owned = self.inner.manifest(&self.plugin).is_some_and(|m| m.data.searches.contains_key(collection));
        if !owned {
            return Err(Error::internal(format!("plugin {} searches {collection}, which is not one of its Collections with search fields", self.plugin)));
        }
        for c in conds {
            if !valid_field(&c.field) {
                return Err(Error::internal(format!("bad field path {}", c.field)));
            }
        }
        let words = crate::text::words(q);
        if words.is_empty() {
            return Ok(vec![]);
        }
        let subject = self.subject_for(collection)?;
        if subject.is_some() {
            self.refuse_subject_paths(conds.iter().map(|c| c.field.as_str()))?;
        }
        let found = self.rd().search(collection, &words, &Self::with_subject(&subject, conds), limit)?;
        Ok(found.into_iter().filter_map(|d| self.mine(&subject, d)).map(|d| self.seen(collection, d)).collect())
    }
    /// Would the caller receive an Event addressed to `audience`? Resolved now, as Pulse resolves it: a Scope asks the
    /// plugin that declares it; an unknown Scope holds no one.
    pub fn may_see(&self, audience: &Audience) -> Result<bool> {
        self.inner.visible(self.rd(), audience, &self.who().subject)
    }
    /// Create or replace a document in a collection this plugin owns.
    pub fn put(&mut self, collection: &str, id: &str, body: Value, expect: Expect) -> Result<Doc> {
        self.may_write(collection)?;
        if !body.is_object() {
            return Err(Error::internal("documents are JSON objects"));
        }
        if let Some(schema) = self.inner.manifest(&self.plugin).and_then(|m| m.data.schemas.get(collection)) {
            crate::schema::validate(schema, &body).map_err(|e| Error::internal(format!("a document of {collection} breaks the schema the plugin declared: {}", e.message)))?;
        }
        let now = self.now();
        let owner = self.plugin.clone();
        self.note_secrets(collection, &body);
        let subject = self.subject_for(collection)?;
        let mut body = body;
        if let Some(s) = &subject {
            if let Some(old) = self.rd().get(collection, id)?.filter(|d| !d.deleted) {
                if old.body.get(SUBJECT).and_then(|v| v.as_str()) != Some(s.as_str()) {
                    return Err(Error::denied("this document belongs to another person"));
                }
            }
            body[SUBJECT] = Value::String(s.clone());
        }
        let doc = self.wr()?.put(collection, id, &owner, body, expect, now)?;
        let kind = if doc.rev == 1 { ChangeKind::Created } else { ChangeKind::Updated };
        self.docs.push(DocRef { collection: collection.into(), id: id.into(), rev: doc.rev, kind });
        self.mine(&subject, doc).ok_or_else(|| Error::internal("a stored document lost its person"))
    }
    /// Create with a fresh id; fails if the id exists.
    pub fn create(&mut self, collection: &str, body: Value) -> Result<Doc> {
        let id = self.new_id();
        self.put(collection, &id, body, Expect::Absent)
    }
    pub fn delete(&mut self, collection: &str, id: &str) -> Result<bool> {
        self.may_write(collection)?;
        let now = self.now();
        if let Some(s) = self.subject_for(collection)? {
            let theirs = self.rd().get(collection, id)?.filter(|d| !d.deleted).is_some_and(|d| d.body.get(SUBJECT).and_then(|v| v.as_str()) != Some(s.as_str()));
            if theirs {
                return Ok(false);
            }
        }
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
        let data = self.redacted(data);
        self.events.push(Event { kind: kind.into(), audience, data });
        Ok(())
    }

    /// Tell an Audience something that matters only now (a [Signal](https://docs.addaui.com/nuah/references/glossary/#signal), such
    /// as Presence): Pulse delivers it to whoever is following when the Command commits, and it is never stored. The kind is in one of
    /// the plugin's namespaces; the data is at most 16 kB.
    pub fn signal(&mut self, kind: &str, audience: Audience, data: Value) -> Result<()> {
        if self.is_query() {
            return Err(Error::internal("a Query may not send Signals"));
        }
        self.inner.check_capability(&self.plugin, "emit")?;
        let owned = self.inner.plan.active.get(&self.plugin).is_some_and(|a| a.manifest.namespaces.iter().any(|n| kind.starts_with(&format!("{n}."))));
        if !owned {
            return Err(Error::internal(format!("signal {kind} is outside the namespaces of {}", self.plugin)));
        }
        if serde_json::to_vec(&data).map(|b| b.len()).unwrap_or(usize::MAX) > crate::pulse::SIGNAL_MAX {
            return Err(Error::internal(format!("signal {kind} is larger than 16 kB")));
        }
        let data = self.redacted(data);
        self.signals.push(Event { kind: kind.into(), audience, data });
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
        if decl.kind == OpKind::Outbound {
            return Err(Error::internal("an outbound Operation holds no transaction, so it cannot be called from inside one"));
        }
        let caller = std::mem::replace(&mut self.plugin, String::new());
        self.depth += 1;
        let out = self.inner.execute(self, op, input);
        self.depth -= 1;
        self.plugin = caller;
        out
    }

    /// What the enabled Providers of a point declared (ADR 0095): each one's plugin, its `provides` data and, for a browser point, the
    /// address of its Browser module (with the file's hash), in the order the point calls them. Only for a point this plugin defines
    /// or requires.
    pub fn provided(&mut self, point: &str) -> Result<Vec<Value>> {
        let m = self.inner.manifest(&self.plugin).ok_or_else(|| Error::internal("no manifest for the calling plugin"))?;
        let defines = m.defines.extension_points.contains_key(point);
        let requires = m.requires.extension_points.iter().any(|p| crate::manifest::point_requirement(p).map(|(n, _)| n).unwrap_or(p) == point);
        if !defines && !requires {
            return Err(Error::internal(format!("plugin {} neither defines nor requires {point}", self.plugin)));
        }
        let mut provs = self.inner.plan.providers.get(point).cloned().unwrap_or_default();
        provs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(provs
            .into_iter()
            .filter_map(|(_, id)| {
                let pm = self.inner.manifest(&id)?;
                let data = pm.provides.iter().find(|p| p.extension_point == point).and_then(|p| p.data.clone()).unwrap_or(Value::Null);
                let module = pm.browser.get(point).and_then(|f| self.inner.browser.get(&(id.clone(), f.clone())).map(|(_, h)| format!("/plugins/{}/{f}?v={h}", id.trim_start_matches('@'))));
                Some(json!({"plugin": id, "data": data, "module": module}))
            })
            .collect())
    }

    /// Call whatever fills an Extension point this plugin requires, following the point's policy.
    pub fn extension_call(&mut self, point: &str, input: Value) -> Result<Value> {
        let allowed = self.inner.manifest(&self.plugin).is_some_and(|m| m.requires.extension_points.iter().any(|p| crate::manifest::point_requirement(p).map(|(n, _)| n).unwrap_or(p) == point));
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

/// What an `outbound` Operation's handler works with ([ADR 0040]): who is asking, the plugin's settings, the
/// clock, the `net` Port, and other Operations as the same person. There is no database handle: reading and
/// writing data is done by calling Operations, each in a transaction of its own.
///
/// [ADR 0040]: https://docs.addaui.com/nuah/adr/0040-outbound-operations-run-outside-the-transaction-and-reach-the-network-through-a-net-port/
pub struct Outbound<'a> {
    pub(crate) core: &'a crate::core::Core,
    pub(crate) plugin: String,
    pub(crate) who: &'a crate::ports::Identity,
    pub(crate) request: &'a str,
    pub(crate) correlation: Option<&'a str>,
}

/// What the program running the Core offers to plugins ([ADR 0041]): the state of the identity file and the
/// power to change it. Reached only from an outbound Operation, by a plugin granted `shell`. The kernel knows
/// nothing about files; the shell implements this.
///
/// [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/
pub trait ShellServices: Send + Sync {
    /// The catalog and the stored Profile (secrets hidden), and the Problems of the stored Profile.
    fn state(&self) -> std::result::Result<Value, String>;
    /// The Problems of a draft Profile (secrets the draft did not retype are kept from the stored one).
    fn check(&self, draft: &Value) -> std::result::Result<Vec<crate::resolver::Problem>, String>;
    /// Save the draft if it has no Problems; otherwise return them and write nothing. Empty means saved.
    fn save(&self, draft: &Value) -> std::result::Result<Vec<crate::resolver::Problem>, String>;
    /// Stop the services, read the identity file again and start a new Core. Returns at once.
    fn restart(&self);
}

impl<'a> Outbound<'a> {
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
        self.core.inner.clock.now()
    }
    pub fn new_id(&self) -> String {
        self.core.inner.ids.new_id()
    }
    /// The Settings the operator gave this plugin in the Profile (null when none).
    pub fn settings(&self) -> Value {
        self.core.inner.settings.get(&self.plugin).cloned().unwrap_or(Value::Null)
    }
    pub fn log(&self, level: crate::telemetry::Level, key: &str, fields: Value) {
        let t = &self.core.inner.telemetry;
        if t.enabled(level, &self.plugin) {
            t.log(crate::telemetry::record(self.now(), level, &self.plugin, key, fields, None, Some(self.request)));
        }
    }
    /// The network. Needs the capability `net`, granted in the Profile.
    pub fn net(&self) -> Result<&dyn Net> {
        self.core.inner.check_capability(&self.plugin, "net")?;
        Ok(&*self.core.inner.net)
    }
    /// Call an Operation as this plugin itself, not as the person asking: the Subject is the plugin's id and its only Role is
    /// `plugin:<id>`, so a Permission whose default Roles are exactly that is held by this plugin and by nobody else. For writes a plugin
    /// must make after checking something a person cannot be trusted to (a file it just fetched). Needs the capability `call`.
    pub fn call_as_self(&self, op: &str, input: Value) -> Result<Value> {
        self.core.inner.check_capability(&self.plugin, "call")?;
        let me = crate::ports::Identity::new(&self.plugin, &self.plugin, &[&format!("plugin:{}", self.plugin)]);
        self.core.call_as_with(&me, op, input, None, self.correlation).map(|r| r.output)
    }
    /// The font data of every enabled font plugin: what an outbound Operation that stores fonts may fetch (ADR 0090).
    pub fn font_data(&self) -> Vec<Value> {
        self.core.font_data()
    }
    /// The shell's services. Needs the capability `shell`, granted in the Profile.
    pub fn shell(&self) -> Result<Arc<dyn ShellServices>> {
        self.core.inner.check_capability(&self.plugin, "shell")?;
        self.core.inner.shell.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| Error::unsupported("shell", "none"))
    }
    /// Call an Operation as the same Subject, with the same checks as from outside, in a transaction of its own.
    pub fn call(&self, op: &str, input: Value) -> Result<Value> {
        self.core.inner.check_capability(&self.plugin, "call")?;
        self.core.call_as_with(self.who, op, input, None, self.correlation).map(|r| r.output)
    }
}
