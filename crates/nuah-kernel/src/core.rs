// SPDX-License-Identifier: Apache-2.0
//! A Core: the kernel together with its active plugins and the Adapters bound to its Ports.

use crate::db::*;
use crate::error::{Category, Error, Result};
use crate::manifest::*;
use crate::plugin::*;
use crate::ports::*;
use crate::telemetry::{Check, ErrorInfo, Health, Level, Metrics, Snapshot};
use crate::telemetry::{HeartbeatAdapter, LogAdapter, MetricAdapter, TraceAdapter};
use crate::resolver::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::watch;

pub(crate) struct Inner {
    pub(crate) db: Arc<dyn Db>,
    pub(crate) features: Features,
    pub(crate) auth: Arc<dyn Auth>,
    pub(crate) policy: Arc<dyn Policy>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) ids: Arc<dyn Ids>,
    pub(crate) plan: Plan,
    pub(crate) settings: BTreeMap<String, Value>,
    pub(crate) wake: watch::Sender<u64>,
    pub(crate) profile_name: String,
    pub(crate) telemetry: crate::telemetry::Telemetry,
}

/// A running Core. Cheap to clone.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<Inner>,
}

impl Core {
    /// Run the Resolver alone: no Adapter is opened, no data is touched.
    pub fn check(dist: &Distribution) -> Vec<Problem> {
        resolve(dist).0
    }

    /// Like [`Core::check`], for a person editing a Profile somewhere other than where it will run: a
    /// `$env` or `$file` reference that does not resolve here is not a Problem yet (it is when the Core starts).
    pub fn check_for_setup(dist: &Distribution) -> Vec<Problem> {
        let all = resolve(dist).0;
        let unresolved: Vec<String> = all.iter().filter(|p| p.code == "bad_secret_ref").flat_map(|p| p.plugins.clone()).collect();
        all.into_iter().filter(|p| p.code != "bad_secret_ref" && !(p.code == "invalid_settings" && p.plugins.iter().any(|id| unresolved.contains(id)))).collect()
    }

    /// The contract hash this Distribution would have, without starting anything (for the lock in a saved Profile).
    pub fn contract_for(dist: &Distribution) -> String {
        resolve(dist).1.hash
    }

    /// Resolve, open the Adapters, record who owns which collection, migrate data, and be ready.
    pub fn start(dist: &Distribution) -> std::result::Result<Core, Vec<Problem>> {
        let (mut problems, plan) = resolve(dist);
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut ports: BTreeMap<&str, PortImpl> = BTreeMap::new();
        for (port, adapter) in &plan.ports {
            let ap = &plan.active[adapter];
            let settings = plan.settings.get(adapter).cloned().unwrap_or_else(|| json!({}));
            match ap.plugin.fill(&settings) {
                Ok(Some(imp)) => {
                    ports.insert(port.as_str(), imp);
                }
                Ok(None) => problems.push(Problem { code: "adapter_filled_nothing".into(), plugins: vec![adapter.clone()], message: format!("{adapter} fills no {port}") }),
                Err(e) => problems.push(Problem { code: "adapter_failed".into(), plugins: vec![adapter.clone()], message: format!("{adapter} could not start: {}", e.message) }),
            }
        }
        // The Ports that take every enabled Adapter (telemetry), in plugin id order.
        let (mut logs, mut traces, mut metric, mut beats): (Vec<Arc<dyn LogAdapter>>, Vec<Arc<dyn TraceAdapter>>, Vec<Arc<dyn MetricAdapter>>, Vec<Arc<dyn HeartbeatAdapter>>) = (vec![], vec![], vec![], vec![]);
        for (port, adapters) in &plan.many {
            for adapter in adapters {
                let ap = &plan.active[adapter];
                let settings = plan.settings.get(adapter).cloned().unwrap_or_else(|| json!({}));
                match ap.plugin.fill(&settings) {
                    Ok(Some(PortImpl::Log(a))) if port == "log" => logs.push(a),
                    Ok(Some(PortImpl::Trace(a))) if port == "trace" => traces.push(a),
                    Ok(Some(PortImpl::Metric(a))) if port == "metric" => metric.push(a),
                    Ok(Some(PortImpl::Heartbeat(a))) if port == "heartbeat" => beats.push(a),
                    Ok(Some(_)) => problems.push(Problem { code: "wrong_port_type".into(), plugins: vec![adapter.clone()], message: format!("{adapter} returned the wrong kind of port for {port}") }),
                    Ok(None) => problems.push(Problem { code: "adapter_filled_nothing".into(), plugins: vec![adapter.clone()], message: format!("{adapter} fills no {port}") }),
                    Err(e) => problems.push(Problem { code: "adapter_failed".into(), plugins: vec![adapter.clone()], message: format!("{adapter} could not start: {}", e.message) }),
                }
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        let telemetry = crate::telemetry::Telemetry::new(&plan.telemetry, logs, traces, metric, beats);
        macro_rules! take {
            ($port:literal, $variant:ident) => {
                match ports.remove($port) {
                    Some(PortImpl::$variant(x)) => x,
                    _ => {
                        return Err(vec![Problem { code: "wrong_port_type".into(), plugins: vec![plan.ports[$port].clone()], message: format!("the adapter for {} returned the wrong kind of port", $port) }])
                    }
                }
            };
        }
        let db: Arc<dyn Db> = if plan.ports.contains_key("db") { take!("db", Db) } else { Arc::new(crate::standins::NoDb) };
        let auth: Arc<dyn Auth> = if plan.ports.contains_key("auth") { take!("auth", Auth) } else { Arc::new(crate::standins::NoAuth) };
        let policy: Arc<dyn Policy> = if plan.ports.contains_key("policy") { take!("policy", Policy) } else { Arc::new(crate::standins::DenyAll) };
        let clock: Arc<dyn Clock> = if plan.ports.contains_key("clock") { take!("clock", Clock) } else { Arc::new(crate::standins::SystemTime) };
        let ids: Arc<dyn Ids> = if plan.ports.contains_key("ids") { take!("ids", Ids) } else { Arc::new(crate::standins::CounterIds::default()) };
        let (wake, _) = watch::channel(0u64);
        let settings = plan.settings.clone();
        let features = db.features();
        let inner = Arc::new(Inner { db, features, auth, policy, clock, ids, plan, settings, wake, profile_name: dist.profile.name.clone(), telemetry });
        if inner.plan.ports.contains_key("db") {
            if let Err(e) = inner.prepare_storage() {
                let code = if e.category == Category::Unsupported { "unsupported" } else { "storage" };
                return Err(vec![Problem { code: code.into(), plugins: e.params["plugin"].as_str().map(|p| vec![p.to_string()]).unwrap_or_default(), message: e.message }]);
            }
            if let Err(e) = inner.init_listener_cursors() {
                return Err(vec![Problem { code: "storage".into(), plugins: vec![], message: e.message }]);
            }
        }
        let latest = {
            let mut l = 0;
            let _ = inner.db.read(&mut |r| {
                l = r.latest_change()?;
                Ok(())
            });
            l
        };
        let _ = inner.wake.send(latest);
        let core = Core { inner };
        core.log(Level::Info, "nuah.kernel", "core.started", json!({"profile": core.profile_name(), "contract": core.contract_hash(), "plugins": core.plugin_ids().len()}));
        Ok(core)
    }

    /// Write a log record. The level is checked first, so a record nobody wants costs one comparison.
    pub fn log(&self, level: Level, target: &str, key: &str, fields: Value) {
        if self.inner.telemetry.enabled(level, target) {
            self.inner.telemetry.log(crate::telemetry::record(self.inner.clock.now(), level, target, key, fields, None, None));
        }
    }
    /// Write a log record about an error; the reference is what the caller was shown.
    pub fn log_error(&self, level: Level, target: &str, key: &str, fields: Value, error: &Error, reference: Option<&str>) {
        if self.inner.telemetry.enabled(level, target) {
            self.inner.telemetry.log(crate::telemetry::record(self.inner.clock.now(), level, target, key, fields, Some(ErrorInfo::of(error, reference)), reference));
        }
    }
    pub fn log_enabled(&self, level: Level, target: &str) -> bool {
        self.inner.telemetry.enabled(level, target)
    }
    /// Change a level while running: for one target (a plugin id), or for all when `target` is `None`.
    pub fn set_log_level(&self, target: Option<&str>, level: Level) {
        self.inner.telemetry.set_level(target, level);
    }
    /// The Core's counters, gauges and histograms; plugins add their own through [`Core::metrics`].
    pub fn metrics(&self) -> &Metrics {
        &self.inner.telemetry.metrics
    }
    pub fn metrics_snapshot(&self) -> Snapshot {
        self.inner.telemetry.metrics.snapshot(self.inner.clock.now())
    }
    /// How the Core is doing: ready, and whether it can read its database.
    pub fn health(&self) -> Health {
        let mut db_ok = true;
        let mut detail = "reads".to_string();
        if let Err(e) = self.inner.db.read(&mut |r| r.latest_change().map(|_| ())) {
            db_ok = false;
            detail = e.message;
        }
        Health {
            status: if db_ok { "ok".into() } else { "degraded".into() },
            ready: db_ok,
            uptime_secs: self.inner.telemetry.started.elapsed().as_secs(),
            profile: self.profile_name().into(),
            contract: self.contract_hash().into(),
            checks: vec![Check { name: "db".into(), ok: db_ok, detail }],
        }
    }
    /// How many signals each telemetry Adapter has lost because its queue was full.
    pub fn telemetry_dropped(&self) -> Vec<(String, u64)> {
        self.inner.telemetry.dropped_total()
    }
    /// Give the telemetry Adapters up to `deadline` to write out what they hold.
    pub fn flush_telemetry(&self, deadline: std::time::Duration) {
        self.inner.telemetry.flush(deadline);
    }

    /// A signal that moves whenever something is committed; a Face waits on it instead of polling.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.wake.subscribe()
    }
    pub fn contract_hash(&self) -> &str {
        &self.inner.plan.hash
    }
    pub fn profile_name(&self) -> &str {
        &self.inner.profile_name
    }
    /// The Faces the enabled plugins publish, as (face name, plugin id), by plugin id.
    pub fn faces(&self) -> Vec<(String, String)> {
        self.inner.plan.active.iter().filter_map(|(id, a)| a.manifest.face.clone().map(|f| (f, id.clone()))).collect()
    }

    pub fn plugin_ids(&self) -> Vec<String> {
        self.inner.plan.active.keys().cloned().collect()
    }
    pub fn now(&self) -> Millis {
        self.inner.clock.now()
    }
}

/// The Core's running services, started by [`Core::start_services`]. Dropping it stops them, last started first.
pub struct Services {
    core: Core,
    done: bool,
    started: Vec<(String, Box<dyn Service>)>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    housekeeping: Option<std::thread::JoinHandle<()>>,
}

impl Services {
    /// Ids of the plugins whose service is running.
    pub fn running(&self) -> Vec<&str> {
        self.started.iter().map(|(id, _)| id.as_str()).collect()
    }
    /// Stop everything and wait.
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        while let Some((_, s)) = self.started.pop() {
            s.stop();
        }
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.housekeeping.take() {
            let _ = h.join();
        }
        self.core.log(Level::Info, "nuah.kernel", "core.stopped", json!({}));
        self.core.flush_telemetry(std::time::Duration::from_secs(2));
    }
}

impl Drop for Services {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The Core's own upkeep: deliver committed Events to Listeners, trim the change log.
fn housekeeping(core: Core, stop: Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering::SeqCst;
    use std::time::{Duration, Instant};
    let mut wake = core.subscribe();
    let mut rounds = 0u64;
    while !stop.load(SeqCst) {
        rounds += 1;
        if let Err(e) = core.drain_listeners() {
            core.log_error(Level::Error, "nuah.kernel", "listener.failed", json!({}), &e, None);
        }
        core.inner.telemetry.tick(core.inner.clock.now(), || core.health());
        if rounds % 600 == 0 {
            let _ = core.prune_log(crate::pipeline::CHANGE_RETENTION);
        }
        // A commit wakes the loop at once; once a second covers another process writing the same database.
        let started = Instant::now();
        while !stop.load(SeqCst) && started.elapsed() < Duration::from_secs(1) {
            match wake.has_changed() {
                Ok(true) => {
                    wake.borrow_and_update();
                    break;
                }
                Ok(false) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => return,
            }
        }
    }
}

impl Core {
    /// Start the Core's upkeep and the service of every plugin that asked for `serve`, in plugin id
    /// order. If one cannot start, the ones already running are stopped and the failure is returned.
    pub fn start_services(&self) -> Result<Services> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (c, s) = (self.clone(), stop.clone());
        let housekeeping = std::thread::Builder::new().name("nuah-housekeeping".into()).spawn(move || housekeeping(c, s)).map_err(|e| Error::internal(format!("cannot start housekeeping: {e}")))?;
        let mut services = Services { core: self.clone(), done: false, started: vec![], stop, housekeeping: Some(housekeeping) };
        for (id, a) in &self.inner.plan.active {
            if !a.manifest.requires.capabilities.iter().any(|c| c == "serve") {
                continue;
            }
            let settings = self.inner.settings.get(id).cloned().unwrap_or(Value::Null);
            match a.plugin.serve(ServiceContext { core: self.clone(), settings }) {
                Ok(Some(svc)) => services.started.push((id.clone(), svc)),
                Ok(None) => {}
                Err(e) => return Err(Error::internal(format!("{id} could not start its service: {}", e.message))),
            }
        }
        Ok(services)
    }
}

impl Inner {
    pub(crate) fn manifest(&self, plugin: &str) -> Option<&Manifest> {
        self.plan.active.get(plugin).map(|a| &a.manifest)
    }
    pub(crate) fn op_decl(&self, op: &str) -> Option<&OperationDecl> {
        let owner = self.plan.ops.get(op)?;
        self.manifest(owner)?.operations.get(op)
    }
    pub(crate) fn check_capability(&self, plugin: &str, cap: &str) -> Result<()> {
        if self.plan.grants.get(plugin).is_some_and(|g| g.iter().any(|c| c == cap)) {
            Ok(())
        } else {
            Err(Error::internal(format!("plugin {plugin} was not granted the capability {cap}")))
        }
    }
    pub(crate) fn event_decl(&self, kind: &str, plugin: &str) -> Result<&EventDecl> {
        match self.plan.events.get(kind) {
            Some(owner) if owner == plugin => Ok(&self.manifest(owner).unwrap().events[kind]),
            Some(owner) => Err(Error::internal(format!("event kind {kind} belongs to {owner}, not {plugin}"))),
            None => Err(Error::internal(format!("event kind {kind} is not declared"))),
        }
    }
    pub(crate) fn extension_def(&self, point: &str) -> Option<ExtensionPointDef> {
        self.plan.ext_points.get(point).map(|(_, d)| d.clone())
    }
    pub(crate) fn providers_of(&self, point: &str) -> Vec<(String, OpFn)> {
        let mut provs = self.plan.providers.get(point).cloned().unwrap_or_default();
        provs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        provs
            .into_iter()
            .filter_map(|(_, id)| self.plan.active.get(&id).and_then(|a| a.bindings.provides.get(point).cloned()).map(|f| (id, f)))
            .collect()
    }

    /// Record collection ownership, make indexes, and bring each plugin's data to its declared version.
    /// Everything that can be refused is checked first, before anything is written, so a start that fails
    /// leaves the data as it was.
    fn prepare_storage(&self) -> Result<()> {
        let mut have_of: BTreeMap<&String, u32> = BTreeMap::new();
        let mut upgrading = vec![];
        for (id, a) in &self.plan.active {
            let data = &a.manifest.data;
            for f in &data.requires {
                if !self.features.has(f) {
                    return Err(Error::new(
                        Category::Unsupported,
                        "kernel.unsupported",
                        format!("the plugin {id} requires the data feature {f}, which the data Adapter {} does not provide", self.db.name()),
                    )
                    .with_params(json!({"plugin": id, "feature": f, "adapter": self.db.name()})));
                }
            }
            for f in data.prefers.iter().filter(|f| !self.features.has(f)) {
                let t = &self.telemetry;
                if t.enabled(Level::Warn, "nuah.kernel") {
                    t.log(crate::telemetry::record(self.clock.now(), Level::Warn, "nuah.kernel", "data.feature_missing", json!({"plugin": id, "feature": f, "adapter": self.db.name()}), None, None));
                }
            }
            for ix in &data.indexes {
                let spec = ix.spec();
                spec.check()?;
                if !data.collections.contains(&spec.collection) {
                    return Err(Error::internal(format!("the plugin {id} declares an index on {}, which it does not own", spec.collection)));
                }
                if spec.unique && !self.features.has(feature::INDEX_UNIQUE) {
                    return Err(Error::unsupported(feature::INDEX_UNIQUE, self.db.name()));
                }
                if spec.fields.len() > 1 && !self.features.has(feature::INDEX_COMPOSITE) {
                    return Err(Error::unsupported(feature::INDEX_COMPOSITE, self.db.name()));
                }
            }
            let mut have = 0u32;
            let mut foreign = None;
            self.db.read(&mut |r| {
                have = r.get("_meta", &format!("version:{id}"))?.filter(|d| !d.deleted).and_then(|d| d.body["version"].as_u64()).unwrap_or(0) as u32;
                for c in &data.collections {
                    if let Some(d) = r.get("_meta", &format!("collection:{c}"))?.filter(|d| !d.deleted) {
                        let owner = d.body["owner"].as_str().unwrap_or("");
                        if owner != id {
                            foreign = Some(format!("the collection {c} belongs to the plugin {owner}, not {id}"));
                        }
                    }
                }
                Ok(())
            })?;
            if let Some(m) = foreign {
                return Err(Error::internal(m));
            }
            if have > data.version {
                return Err(Error::internal(format!("the data of {id} is at version {have}, newer than the plugin's {}", data.version)));
            }
            if have > 0 && have < data.version {
                upgrading.push(format!("{id} v{have} to v{}", data.version));
            }
            have_of.insert(id, have);
        }
        // before any migration of data that already exists, let the Adapter take a copy
        if !upgrading.is_empty() {
            self.db.before_migration(&upgrading.join(", "))?;
        }
        for (id, a) in &self.plan.active {
            // ownership is claimed once, in the database
            self.db.write(&mut |w| {
                let now = self.clock.now();
                for c in &a.manifest.data.collections {
                    let key = format!("collection:{c}");
                    match w.get("_meta", &key)?.filter(|d| !d.deleted) {
                        Some(d) if d.body["owner"].as_str() != Some(id.as_str()) => {
                            return Err(Error::internal(format!("the collection {c} belongs to the plugin {}, not {id}", d.body["owner"].as_str().unwrap_or(""))));
                        }
                        Some(_) => {}
                        None => {
                            w.put("_meta", &key, "kernel", json!({"owner": id}), Expect::Any, now)?;
                        }
                    }
                }
                Ok(())
            })?;
            for ix in &a.manifest.data.indexes {
                self.db.ensure_index(&ix.spec())?;
            }
            // migrations, one step at a time, each in its own transaction
            let (want, have) = (a.manifest.data.version, have_of[id]);
            for from in have..want {
                self.run_kernel(id, &mut |tx| {
                    a.plugin.migrate(from, tx)?;
                    let now = tx.now();
                    tx.wr()?.put("_meta", &format!("version:{id}"), "kernel", json!({"version": from + 1}), Expect::Any, now)?;
                    Ok(())
                })?;
            }
            if want == 0 && have == 0 {
                self.run_kernel(id, &mut |tx| {
                    let now = tx.now();
                    tx.wr()?.put("_meta", &format!("version:{id}"), "kernel", json!({"version": 0}), Expect::Any, now)?;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    /// Run kernel-internal work as a plugin, in a write transaction, with no Subject.
    pub(crate) fn run_kernel(&self, plugin: &str, f: &mut dyn FnMut(&mut Tx) -> Result<()>) -> Result<()> {
        let system = Identity { subject: "kernel".into(), display: "kernel".into(), roles: vec![] };
        let mut out = Ok(());
        self.db.write(&mut |w| {
            let mut events = vec![];
            let mut docs = vec![];
            let mut tx = Tx { inner: self, plugin: plugin.into(), who: &system, request: "kernel", h: Handle::W(w), events: &mut events, docs: &mut docs, depth: 0 };
            out = f(&mut tx);
            if out.is_err() {
                return Err(Error::internal("rolled back"));
            }
            Ok(())
        })
        .or_else(|e| if out.is_err() { Ok(()) } else { Err(e) })?;
        out
    }
}
