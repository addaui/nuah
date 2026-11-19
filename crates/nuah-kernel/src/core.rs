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
    pub(crate) net: Arc<dyn Net>,
    pub(crate) plan: Plan,
    pub(crate) settings: BTreeMap<String, Value>,
    pub(crate) wake: watch::Sender<u64>,
    pub(crate) signals: std::sync::Mutex<crate::pulse::Signals>,
    pub(crate) profile_name: String,
    pub(crate) telemetry: crate::telemetry::Telemetry,
    pub(crate) shell: std::sync::RwLock<Option<Arc<dyn crate::plugin::ShellServices>>>,
    /// The enabled plugins' browser files, by (plugin, path), with the first 12 hex digits of each file's sha-256.
    pub(crate) browser: BTreeMap<(String, String), (&'static [u8], String)>,
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

    /// The program that runs this Core offers its services to plugins that were granted `host` ([ADR 0041]).
    ///
    /// [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/
    pub fn attach_shell(&self, shell: Arc<dyn crate::plugin::ShellServices>) {
        *self.inner.shell.write().unwrap_or_else(|e| e.into_inner()) = Some(shell);
    }

    /// Resolve, open the Adapters, record who owns which collection, migrate data, and be ready.
    pub fn start(dist: &Distribution) -> std::result::Result<Core, Vec<Problem>> {
        Core::start_watched(dist, None)
    }

    /// [`Core::start`], telling `watch` how far each data upgrade has come, so the program running the Core can show
    /// a "starting" page while it is not answering yet.
    pub fn start_watched(dist: &Distribution, watch: Option<crate::upgrade::OnUpgrade>) -> std::result::Result<Core, Vec<Problem>> {
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
                Ok(None) => problems.push(Problem { code: "adapter_filled_nothing".into(), plugins: vec![adapter.clone()], message: format!("{adapter} fills no {port}"), details: Default::default() }),
                Err(e) => problems.push(Problem { code: "adapter_failed".into(), plugins: vec![adapter.clone()], message: format!("{adapter} could not start: {}", e.message), details: Default::default() }),
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
                    Ok(Some(_)) => problems.push(Problem { code: "wrong_port_type".into(), plugins: vec![adapter.clone()], message: format!("{adapter} returned the wrong kind of port for {port}"), details: Default::default() }),
                    Ok(None) => problems.push(Problem { code: "adapter_filled_nothing".into(), plugins: vec![adapter.clone()], message: format!("{adapter} fills no {port}"), details: Default::default() }),
                    Err(e) => problems.push(Problem { code: "adapter_failed".into(), plugins: vec![adapter.clone()], message: format!("{adapter} could not start: {}", e.message), details: Default::default() }),
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
                        return Err(vec![Problem { code: "wrong_port_type".into(), plugins: vec![plan.ports[$port].clone()], message: format!("the adapter for {} returned the wrong kind of port", $port), details: Default::default() }])
                    }
                }
            };
        }
        let db: Arc<dyn Db> = if plan.ports.contains_key("db") { take!("db", Db) } else { Arc::new(crate::standins::NoDb) };
        let auth: Arc<dyn Auth> = if plan.ports.contains_key("auth") { take!("auth", Auth) } else { Arc::new(crate::standins::NoAuth) };
        let policy: Arc<dyn Policy> = if plan.ports.contains_key("policy") { take!("policy", Policy) } else { Arc::new(crate::standins::DenyAll) };
        let clock: Arc<dyn Clock> = if plan.ports.contains_key("clock") { take!("clock", Clock) } else { Arc::new(crate::standins::SystemTime) };
        let ids: Arc<dyn Ids> = if plan.ports.contains_key("ids") { take!("ids", Ids) } else { Arc::new(crate::standins::CounterIds::default()) };
        let net: Arc<dyn Net> = if plan.ports.contains_key("net") { take!("net", Net) } else { Arc::new(crate::standins::NoNet) };
        let (wake, _) = watch::channel(0u64);
        let settings = plan.settings.clone();
        let features = db.features();
        let browser = browser_files(&plan);
        let inner = Arc::new(Inner { db, features, auth, policy, clock, ids, net, plan, settings, wake, signals: Default::default(), profile_name: dist.profile.name.clone(), telemetry, shell: std::sync::RwLock::new(None), browser });
        if inner.plan.ports.contains_key("db") {
            if let Err(e) = inner.prepare_storage(watch) {
                if e.key == "kernel.upgrade_refused" {
                    let details = json!({"problems": e.params["upgrade"], "plugins": e.params["plugins"]});
                    let mut plugins: Vec<String> = e.params["upgrade"].as_array().into_iter().flatten().filter_map(|p| p["plugin"].as_str().map(String::from)).collect();
                    plugins.dedup();
                    return Err(vec![Problem { code: "upgrade_refused".into(), plugins, message: e.message, details }]);
                }
                let code = if e.category == Category::Unsupported { "unsupported" } else { "storage" };
                return Err(vec![Problem { code: code.into(), plugins: e.params["plugin"].as_str().map(|p| vec![p.to_string()]).unwrap_or_default(), message: e.message, details: Default::default() }]);
            }
            if let Err(e) = inner.init_listener_cursors() {
                return Err(vec![Problem { code: "storage".into(), plugins: vec![], message: e.message, details: Default::default() }]);
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

    /// One of an enabled plugin's browser files, with the hash its address carries; `None` when the plugin is not enabled or has no
    /// such file ([ADR 0095]).
    ///
    /// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
    pub fn browser_file(&self, plugin: &str, path: &str) -> Option<(&'static [u8], String)> {
        self.inner.browser.get(&(plugin.to_string(), path.to_string())).map(|(b, h)| (*b, h.clone()))
    }

    /// The Browser modules of the enabled plugins: which plugin, for which point, at which address (with the file's hash), and
    /// what the plugin declares to the point. Public: the files are served to anyone, like a Face's.
    pub fn browser_modules(&self) -> Vec<Value> {
        let inner = &*self.inner;
        let mut out = vec![];
        for (id, a) in &inner.plan.active {
            for (point, file) in &a.manifest.browser {
                let Some((_, hash)) = inner.browser.get(&(id.clone(), file.clone())) else { continue };
                let data = a.manifest.provides.iter().find(|p| &p.extension_point == point).and_then(|p| p.data.clone()).unwrap_or(Value::Null);
                out.push(json!({"plugin": id, "point": point, "file": file, "url": format!("/plugins/{}/{file}?v={hash}", id.trim_start_matches('@')), "data": data}));
            }
        }
        out
    }

    /// The origins the enabled plugins ask the browser to allow a Face to call (sorted, each once). One that is not a valid origin is left out.
    pub fn connect_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = self.inner.plan.active.values().flat_map(|a| a.manifest.connect.iter().cloned()).filter(|o| crate::manifest::valid_connect_origin(o)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The origins the enabled plugins ask the browser to load pictures and media from (sorted, each once), ADR 0077.
    pub fn image_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = self.inner.plan.active.values().flat_map(|a| a.manifest.images.iter().cloned()).filter(|o| crate::manifest::valid_connect_origin(o)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The origins the enabled plugins ask the browser to load font files from (sorted, each once), ADR 0090.
    pub fn font_origins(&self) -> Vec<String> {
        let mut out: Vec<String> = self.inner.plan.active.values().flat_map(|a| a.manifest.font_origins.iter().cloned()).filter(|o| crate::manifest::valid_connect_origin(o)).collect();
        out.sort();
        out.dedup();
        out
    }

    /// The font data of every enabled font plugin (`Manifest::fonts`), so a plugin that stores fonts knows which addresses it may fetch.
    pub fn font_data(&self) -> Vec<Value> {
        self.inner.plan.active.values().filter_map(|a| a.manifest.fonts.clone()).collect()
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

/// The work an upgrade of one plugin's data has to do.
struct Upgrade {
    to: u32,
    plan: crate::upgrade::UpgradePlan,
    /// The Data versions with an Upgrade step between `from` and `to`.
    stepped: Vec<u32>,
    /// The Collections the stored schema had.
    stored: Vec<String>,
    /// The new schema file, recorded when the upgrade is done.
    file: Value,
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
    fn prepare_storage(&self, watch: Option<crate::upgrade::OnUpgrade>) -> Result<()> {
        let mut have_of: BTreeMap<&String, u32> = BTreeMap::new();
        let mut upgrading = vec![];
        let mut refused: Vec<crate::upgrade::UpgradeProblem> = vec![];
        // for an Upgrade report: each plugin's name and versions, stored and new
        let mut versions = serde_json::Map::new();
        let mut work: BTreeMap<&String, Upgrade> = BTreeMap::new();
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
            let (mut stored, mut progress) = (None, None);
            self.db.read(&mut |r| {
                let meta = |k: String| r.get("_meta", &k).map(|d| d.filter(|d| !d.deleted).map(|d| d.body));
                have = meta(format!("version:{id}"))?.and_then(|b| b["version"].as_u64()).unwrap_or(0) as u32;
                stored = meta(format!("schema:{id}"))?;
                progress = meta(format!("upgrade:{id}"))?;
                for c in &data.collections {
                    if let Some(d) = meta(format!("collection:{c}"))? {
                        let owner = d["owner"].as_str().unwrap_or("");
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
            have_of.insert(id, have);
            let Some(raw) = &data.schema else {
                // a plugin with no schema file: only its Data version is recorded
                if have > data.version {
                    return Err(Error::internal(format!("the data of {id} is at version {have}, newer than the plugin's {}", data.version)));
                }
                if have > 0 && have < data.version {
                    upgrading.push(format!("{id} v{have} to v{}", data.version));
                }
                continue;
            };
            let new = crate::upgrade::SchemaFile::parse(raw).map_err(Error::internal)?;
            let name = if a.manifest.name.is_empty() { id.clone() } else { a.manifest.name.clone() };
            versions.insert(id.clone(), json!({"name": name, "from_release": stored.as_ref().map(|s| s["release"].clone()).unwrap_or(Value::Null), "to_release": a.manifest.version,
                "from_version": stored.as_ref().map(|s| s["version"].clone()).unwrap_or(json!(have)), "to_version": new.version}));
            let Some(stored) = stored else {
                if have > 0 {
                    refused.push(crate::upgrade::UpgradeProblem { plugin: id.clone(), collection: String::new(), path: String::new(), code: "data_without_schema".into(), params: json!({"stored": have}) });
                }
                continue;
            };
            let old = crate::upgrade::SchemaFile::parse(&stored["file"]).map_err(|e| Error::internal(format!("the stored schema of {id}: {e}")))?;
            if let Some(p) = &progress {
                if p["to"].as_u64() != Some(new.version as u64) {
                    refused.push(crate::upgrade::UpgradeProblem { plugin: id.clone(), collection: String::new(), path: String::new(), code: "interrupted_upgrade".into(), params: json!({"to": p["to"], "version": new.version}) });
                    continue;
                }
            }
            match crate::upgrade::plan(id, &old, &new) {
                Ok(plan) => {
                    let stepped = new.steps.iter().copied().filter(|s| *s > old.version && *s <= new.version).collect::<Vec<u32>>();
                    // with no Upgrade step to fix values up, every stored Document must fit after the declared changes
                    if old.version < new.version && stepped.is_empty() {
                        let found = self.dry_run(id, &old, &new, &plan, watch)?;
                        if !found.is_empty() {
                            refused.extend(found);
                            continue;
                        }
                    }
                    if old.version < new.version {
                        upgrading.push(format!("{id} v{} to v{}", old.version, new.version));
                        work.insert(id, Upgrade { to: new.version, plan, stepped, stored: old.live_collections(), file: raw.clone() });
                    }
                }
                Err(ps) => refused.extend(ps),
            }
        }
        if !refused.is_empty() {
            let message = refused.iter().map(|p| format!("{}: {}", p.plugin, p.message())).collect::<Vec<_>>().join("; ");
            let named: std::collections::BTreeSet<&str> = refused.iter().map(|p| p.plugin.as_str()).collect();
            versions.retain(|k, _| named.contains(k.as_str()));
            return Err(Error::new(Category::Invalid, "kernel.upgrade_refused", message).with_params(json!({"upgrade": refused, "plugins": versions})));
        }
        // before any change to data that already exists, let the Adapter take a copy
        if !upgrading.is_empty() {
            if let Some(f) = watch {
                f(&crate::upgrade::UpgradeProgress { phase: "copying".into(), plugin: String::new(), collection: String::new(), done: 0, total: 0, collection_number: 0, collections: 0 });
            }
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
            if let Some(u) = work.get(id) {
                self.upgrade(id, a, u, watch)?;
            }
            let data = &a.manifest.data;
            for ix in &data.indexes {
                self.db.ensure_index(&ix.spec())?;
            }
            if data.schema.is_some() {
                for c in &data.collections {
                    let keep: Vec<IndexSpec> = data.indexes.iter().map(|i| i.spec()).filter(|s| &s.collection == c).collect();
                    self.db.retain_indexes(c, &keep)?;
                    // the text index follows the schema too: new search fields fill it, none empties it
                    match data.searches.get(c) {
                        Some(fields) => self.db.ensure_search(c, fields)?,
                        None if self.features.has(feature::TEXT_SEARCH) => self.db.ensure_search(c, &[])?,
                        None => {}
                    }
                }
            }
            let (want, have) = (data.version, have_of[id]);
            if have == 0 {
                // new data: the plugin seeds what a first start needs, and the schema it was written under is recorded
                self.run_kernel(id, &mut |tx| {
                    a.plugin.init_data(tx)?;
                    let now = tx.now();
                    let w = tx.wr()?;
                    w.put("_meta", &format!("version:{id}"), "kernel", json!({"version": want}), Expect::Any, now)?;
                    if let Some(file) = &data.schema {
                        w.put("_meta", &format!("schema:{id}"), "kernel", json!({"version": want, "release": a.manifest.version, "file": file}), Expect::Any, now)?;
                    }
                    Ok(())
                })?;
            } else if data.schema.is_some() {
                // the release running now, so a later Upgrade report names the version the person really had
                let release = a.manifest.version.clone();
                self.run_kernel(id, &mut |tx| {
                    let now = tx.now();
                    let w = tx.wr()?;
                    let key = format!("schema:{id}");
                    if let Some(mut d) = w.get("_meta", &key)?.filter(|d| !d.deleted && d.body["release"] != json!(release)) {
                        d.body["release"] = json!(release);
                        w.put("_meta", &key, "kernel", d.body, Expect::Any, now)?;
                    }
                    Ok(())
                })?;
            } else if have < want {
                self.run_kernel(id, &mut |tx| {
                    let now = tx.now();
                    tx.wr()?.put("_meta", &format!("version:{id}"), "kernel", json!({"version": want}), Expect::Any, now)?;
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    /// Before an upgrade with no Upgrade step: apply the declared changes to every stored Document in memory and check
    /// each against the new Collection schema and the new unique indexes. Each Collection with misfits gives one problem
    /// with how many Documents break and one example, so the person sees it in the Upgrade report and nothing is written.
    fn dry_run(&self, id: &str, old: &crate::upgrade::SchemaFile, new: &crate::upgrade::SchemaFile, plan: &crate::upgrade::UpgradePlan, watch: Option<crate::upgrade::OnUpgrade>) -> Result<Vec<crate::upgrade::UpgradeProblem>> {
        use crate::upgrade::{apply, index_specs, strict, UpgradeProblem, BATCH};
        let mut out = vec![];
        let live = old.live_collections();
        for (number, from) in live.iter().cloned().enumerate() {
            let mut total = 0;
            self.db.read(&mut |r| {
                total = r.count(&from, &[])?;
                Ok(())
            })?;
            let mut checked = 0u64;
            let report = |done: u64| {
                if let Some(f) = watch {
                    f(&crate::upgrade::UpgradeProgress { phase: "checking".into(), plugin: id.to_string(), collection: from.clone(), done, total, collection_number: number + 1, collections: live.len() });
                }
            };
            report(0);
            let (to, ops) = match plan.collections.iter().find(|c| c.from == from) {
                Some(c) if c.removed => continue,
                Some(c) => (c.to.clone(), c.ops.clone()),
                None => (from.clone(), vec![]),
            };
            let Some(raw) = new.collections.get(&to) else { continue };
            let schema = strict(raw);
            let uniques: Vec<IndexSpec> = index_specs(&to, raw).into_iter().filter(|s| s.unique).collect();
            let mut seen: Vec<std::collections::BTreeSet<String>> = vec![Default::default(); uniques.len()];
            let (mut misfits, mut example, mut clashes, mut clash_example) = (0u64, None::<(String, String)>, vec![0u64; uniques.len()], vec![None::<String>; uniques.len()]);
            let mut after = String::new();
            loop {
                let mut batch: Vec<Doc> = vec![];
                self.db.read(&mut |r| {
                    batch = r.scan(&from, &after, BATCH)?.into_iter().take_while(|d| d.collection == from).collect();
                    Ok(())
                })?;
                let Some(last) = batch.last().map(|d| d.id.clone()) else { break };
                let alive: Vec<Doc> = batch.into_iter().filter(|d| !d.deleted).collect();
                checked += alive.len() as u64;
                for d in alive {
                    let mut b = d.body;
                    apply(&mut b, &ops);
                    if let Err(e) = crate::schema::validate(&schema, &b) {
                        misfits += 1;
                        example.get_or_insert((d.id.clone(), e.message.clone()));
                    }
                    for (n, u) in uniques.iter().enumerate() {
                        let vals: Vec<&Value> = u.fields.iter().map(|f| f.split('.').fold(&b, |v, k| &v[k])).collect();
                        if vals.iter().any(|v| v.is_null()) {
                            continue;
                        }
                        if !seen[n].insert(serde_json::to_string(&vals).unwrap_or_default()) {
                            clashes[n] += 1;
                            clash_example[n].get_or_insert(d.id.clone());
                        }
                    }
                }
                after = last;
                report(checked);
            }
            if let Some((doc, reason)) = example {
                out.push(UpgradeProblem { plugin: id.into(), collection: to.clone(), path: String::new(), code: "documents_do_not_fit".into(), params: json!({"count": misfits, "example": doc, "reason": reason}) });
            }
            for (n, u) in uniques.iter().enumerate() {
                if clashes[n] > 0 {
                    out.push(UpgradeProblem { plugin: id.into(), collection: to.clone(), path: u.fields.join(","), code: "unique_conflict".into(), params: json!({"count": clashes[n], "fields": u.fields, "example": clash_example[n]}) });
                }
            }
        }
        Ok(out)
    }

    /// Bring one plugin's stored Documents to its new schema ([ADR 0091]): every Collection with work, in name order,
    /// in batches of [`crate::upgrade::BATCH`], each batch one transaction together with the progress record, so a start
    /// that stops half-way continues where it stopped. The new schema and Data version are recorded last.
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    fn upgrade(&self, id: &str, a: &crate::resolver::ActivePlugin, u: &Upgrade, watch: Option<crate::upgrade::OnUpgrade>) -> Result<()> {
        use crate::upgrade::{apply, CollectionPlan, BATCH};
        let mut work = u.plan.collections.clone();
        if !u.stepped.is_empty() {
            for c in &u.stored {
                if !work.iter().any(|w| &w.from == c) {
                    work.push(CollectionPlan { from: c.clone(), to: c.clone(), removed: false, ops: vec![] });
                }
            }
        }
        work.sort_by(|x, y| x.from.cmp(&y.from));
        let key = format!("upgrade:{id}");
        let mut progress = json!({"to": u.to, "done": [], "collection": "", "after": ""});
        self.db.read(&mut |r| {
            if let Some(d) = r.get("_meta", &key)?.filter(|d| !d.deleted) {
                progress = d.body;
            }
            Ok(())
        })?;
        let stopped = |c: &str, e: Error| Error::internal(format!("upgrading {id} to version {} stopped in {c}: {}; the next start continues where it stopped", u.to, e.message));
        for (number, w) in work.iter().enumerate() {
            if progress["done"].as_array().is_some_and(|d| d.iter().any(|x| x == w.from.as_str())) {
                continue;
            }
            let resuming = progress["collection"] == w.from.as_str();
            let mut after = if resuming { progress["after"].as_str().unwrap_or("").to_string() } else { String::new() };
            // how many are done counts across an interruption; what has moved away is no longer counted where it was
            let mut done = if resuming { progress["count"].as_u64().unwrap_or(0) } else { 0 };
            let mut left = 0;
            self.db.read(&mut |r| {
                left = r.count(&w.from, &[])?;
                Ok(())
            })?;
            let total = if w.removed || w.to != w.from { left + done } else { left };
            let report = |done: u64| {
                if let Some(f) = watch {
                    f(&crate::upgrade::UpgradeProgress { phase: "upgrading".into(), plugin: id.to_string(), collection: w.from.clone(), done, total, collection_number: number + 1, collections: work.len() });
                }
            };
            report(done);
            loop {
                let mut batch: Vec<Doc> = vec![];
                self.db.read(&mut |r| {
                    batch = r.scan(&w.from, &after, BATCH)?.into_iter().take_while(|d| d.collection == w.from).collect();
                    Ok(())
                })?;
                let Some(last) = batch.last().map(|d| d.id.clone()) else { break };
                let live: Vec<(String, Value)> = batch.into_iter().filter(|d| !d.deleted).map(|d| (d.id, d.body)).collect();
                self.run_kernel(id, &mut |tx| {
                    let now = tx.now();
                    let mut items = live.clone();
                    if !w.removed {
                        for (_, b) in items.iter_mut() {
                            apply(b, &w.ops);
                        }
                        for s in &u.stepped {
                            a.plugin.upgrade(*s, &w.to, &mut items, tx)?;
                        }
                        if let Some((i, _)) = items.iter().find(|(i, _)| !live.iter().any(|(l, _)| l == i)) {
                            return Err(Error::internal(format!("an Upgrade step may change or remove the Documents of its batch but may not add any (it added {i}); put other Documents in another Collection with tx.put")));
                        }
                    }
                    let kept: std::collections::BTreeSet<&str> = if w.removed { Default::default() } else { items.iter().map(|(i, _)| i.as_str()).collect() };
                    // what moved to a new name, or what the step removed, goes from where it was
                    for (i, _) in &live {
                        if w.removed || w.to != w.from || !kept.contains(i.as_str()) {
                            tx.wr()?.delete(&w.from, i, Expect::Any, now)?;
                        }
                    }
                    if !w.removed {
                        for (i, b) in &items {
                            tx.put(&w.to, i, b.clone(), Expect::Any)?;
                        }
                    }
                    let mut p = progress.clone();
                    p["collection"] = json!(w.from);
                    p["after"] = json!(last);
                    p["count"] = json!(done + live.len() as u64);
                    tx.wr()?.put("_meta", &key, "kernel", p, Expect::Any, now)?;
                    Ok(())
                })
                .map_err(|e| stopped(&w.from, e))?;
                done += live.len() as u64;
                progress["collection"] = json!(w.from);
                progress["after"] = json!(last);
                progress["count"] = json!(done);
                after = last;
                report(done);
            }
            progress["done"].as_array_mut().expect("a list").push(json!(w.from));
            progress["collection"] = json!("");
            progress["after"] = json!("");
            progress["count"] = json!(0);
            self.run_kernel(id, &mut |tx| {
                let now = tx.now();
                tx.wr()?.put("_meta", &key, "kernel", progress.clone(), Expect::Any, now)?;
                Ok(())
            })?;
        }
        self.run_kernel(id, &mut |tx| {
            let now = tx.now();
            let wr = tx.wr()?;
            for w in work.iter().filter(|w| w.removed || w.to != w.from) {
                wr.delete("_meta", &format!("collection:{}", w.from), Expect::Any, now)?;
            }
            wr.put("_meta", &format!("schema:{id}"), "kernel", json!({"version": u.to, "release": a.manifest.version, "file": u.file}), Expect::Any, now)?;
            wr.put("_meta", &format!("version:{id}"), "kernel", json!({"version": u.to}), Expect::Any, now)?;
            wr.delete("_meta", &key, Expect::Any, now)?;
            Ok(())
        })?;
        // the indexes of a Collection that was renamed or removed go with its old name
        for w in work.iter().filter(|w| w.removed || w.to != w.from) {
            self.db.retain_indexes(&w.from, &[])?;
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
            let mut tx = Tx { inner: self, plugin: plugin.into(), who: &system, request: "kernel", h: Handle::W(w), events: &mut events, docs: &mut docs, signals: vec![], depth: 0 };
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

fn browser_files(plan: &Plan) -> BTreeMap<(String, String), (&'static [u8], String)> {
    use sha2::{Digest, Sha256};
    let mut out = BTreeMap::new();
    for (id, a) in &plan.active {
        for (path, bytes) in a.plugin.browser_files() {
            let hash = format!("{:x}", Sha256::digest(bytes))[..12].to_string();
            out.insert((id.clone(), path.to_string()), (*bytes, hash));
        }
    }
    out
}
