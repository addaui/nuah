//! Telemetry: levels, fan-out to every Adapter, a stalled Adapter never slows a command, nothing is lost silently.

mod common;
use common::*;
use nuah_adapters::log::{iso, json_line};
use nuah_adapters::*;
use nuah_kernel::*;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

type Seen<T> = Arc<Mutex<Vec<T>>>;

struct Collect {
    id: &'static str,
    seen: Seen<Record>,
    delay: Duration,
}
struct CollectAdapter {
    id: &'static str,
    seen: Seen<Record>,
    delay: Duration,
}
impl LogAdapter for CollectAdapter {
    fn name(&self) -> &str {
        self.id
    }
    fn min_level(&self) -> Level {
        Level::Trace
    }
    fn record(&self, r: &Record) {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        self.seen.lock().push(r.clone());
    }
}
impl Plugin for Collect {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.id, "version": "1.0.0", "fills": "log"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Log(Arc::new(CollectAdapter { id: self.id, seen: self.seen.clone(), delay: self.delay }))))
    }
}

struct Beat(Seen<String>);
impl HeartbeatAdapter for Beat {
    fn name(&self) -> &str {
        "beat"
    }
    fn interval_secs(&self) -> u64 {
        1
    }
    fn beat(&self, h: &Health) {
        self.0.lock().push(h.status.clone());
    }
}
struct BeatPlugin(Seen<String>);
impl Plugin for BeatPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "beat", "version": "1.0.0", "fills": "heartbeat"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Heartbeat(Arc::new(Beat(self.0.clone())))))
    }
}
struct Push(Seen<usize>);
impl MetricAdapter for Push {
    fn name(&self) -> &str {
        "push"
    }
    fn interval_secs(&self) -> u64 {
        1
    }
    fn export(&self, s: &Snapshot) {
        self.0.lock().push(s.series.len());
    }
}
struct PushPlugin(Seen<usize>);
impl Plugin for PushPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "push", "version": "1.0.0", "fills": "metric"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Metric(Arc::new(Push(self.0.clone())))))
    }
}

struct Rig {
    core: Core,
    logs: Vec<Seen<Record>>,
}

fn rig(adapters: &[(&'static str, Duration)], tweak: impl FnOnce(&mut Profile), extra: Vec<Arc<dyn Plugin>>, extra_ids: &[&str]) -> Rig {
    let clock = FixedClock::at(1_000_000);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut catalog = catalog(&clock, &seen);
    let mut enabled = vec!["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a"];
    let mut logs = vec![];
    for (id, delay) in adapters {
        let s: Seen<Record> = Arc::new(Mutex::new(vec![]));
        catalog.0.push(Arc::new(Collect { id, seen: s.clone(), delay: *delay }));
        enabled.push(id);
        logs.push(s);
    }
    catalog.0.extend(extra);
    enabled.extend_from_slice(extra_ids);
    let mut profile = profile(&enabled);
    tweak(&mut profile);
    let core = Core::start(&Distribution { name: "t".into(), catalog, profile }).unwrap_or_else(|p| panic!("{p:#?}"));
    Rig { core, logs }
}

fn create_book(core: &Core, title: &str) -> Result<CallResponse> {
    core.call_as(&person("ann", &["member"]), "notes.book.create", json!({"title": title}), None)
}

#[test]
fn every_enabled_log_adapter_receives_every_record_and_levels_filter() {
    let r = rig(&[("log-a", Duration::ZERO), ("log-b", Duration::ZERO)], |p| p.telemetry.level = Some("debug".into()), vec![], &[]);
    create_book(&r.core, "one").unwrap();
    r.core.flush_telemetry(Duration::from_secs(2));
    for seen in &r.logs {
        let got = seen.lock();
        assert!(got.iter().any(|x| x.key == "core.started" && x.level == Level::Info), "both Adapters see the start");
        assert!(got.iter().any(|x| x.key == "operation.ok" && x.fields["operation"] == "notes.book.create"), "and the call");
    }
    // a level can change while running, for one target
    r.core.set_log_level(Some("nuah.kernel"), Level::Warn);
    let before = r.logs[0].lock().len();
    create_book(&r.core, "two").unwrap();
    r.core.flush_telemetry(Duration::from_secs(2));
    assert_eq!(r.logs[0].lock().len(), before, "debug records of that target stopped");
}

#[test]
fn the_default_level_hides_chatty_records() {
    let r = rig(&[("log-a", Duration::ZERO)], |_| {}, vec![], &[]);
    create_book(&r.core, "one").unwrap();
    r.core.flush_telemetry(Duration::from_secs(2));
    assert!(r.logs[0].lock().iter().all(|x| x.level <= Level::Info));
    assert!(!r.logs[0].lock().iter().any(|x| x.key == "operation.ok"));
}

#[test]
fn a_refusal_is_logged_with_the_reference_the_caller_can_quote() {
    let r = rig(&[("log-a", Duration::ZERO)], |p| p.telemetry.level = Some("debug".into()), vec![], &[]);
    let e = r.core.call_as(&person("ann", &["member"]), "notes.book.create", json!({"title": ""}), None).unwrap_err();
    assert_eq!(e.category, Category::Invalid);
    r.core.flush_telemetry(Duration::from_secs(2));
    let got = r.logs[0].lock();
    let rec = got.iter().find(|x| x.key == "operation.refused").expect("the refusal is logged");
    let info = rec.error.as_ref().expect("with its error");
    assert_eq!(info.category, "invalid");
    assert_eq!(info.reference, rec.request, "the reference is the request id");
    assert!(rec.fields.get("input").is_none(), "inputs are never recorded");
}

#[test]
fn a_stalled_adapter_never_slows_a_command_and_what_it_loses_is_counted() {
    let r = rig(&[("log-slow", Duration::from_millis(150))], |p| {
        p.telemetry.level = Some("debug".into());
        p.telemetry.queue = Some(4);
    }, vec![], &[]);
    let t = Instant::now();
    for i in 0..40 {
        create_book(&r.core, &format!("b{i}")).unwrap();
    }
    assert!(t.elapsed() < Duration::from_secs(2), "40 commands took {:?}: the Adapter held them up", t.elapsed());
    let lost = r.core.telemetry_dropped().into_iter().find(|(n, _)| n == "log-slow").map(|x| x.1).unwrap_or(0);
    assert!(lost > 0, "records the full queue refused are counted");
}

#[test]
fn operations_are_counted_and_timed() {
    let r = rig(&[], |_| {}, vec![], &[]);
    create_book(&r.core, "one").unwrap();
    let _ = r.core.call_as(&person("ann", &["member"]), "notes.book.create", json!({"title": ""}), None);
    let snap = r.core.metrics_snapshot();
    let find = |name: &str, outcome: Option<&str>| snap.series.iter().find(|s| s.name == name && outcome.map_or(true, |o| s.labels.get("outcome").map(|x| x.as_str()) == Some(o)) && s.labels.get("operation").map(|x| x.as_str()) == Some("notes.book.create"));
    assert_eq!(find("nuah_operations_total", Some("ok")).unwrap().value, 1.0);
    assert_eq!(find("nuah_operations_total", Some("invalid")).unwrap().value, 1.0);
    let h = find("nuah_operation_duration_ms", None).unwrap();
    assert_eq!(h.count, 2);
    assert_eq!(h.buckets.last().unwrap().1, 2, "the last bucket holds everything");
}

#[test]
fn a_port_that_feeds_every_adapter_cannot_be_bound_to_one() {
    let clock = FixedClock::at(1);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Collect { id: "log-a", seen: Arc::new(Mutex::new(vec![])), delay: Duration::ZERO }));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a", "log-a"]);
    p.bindings.insert("log".into(), "log-a".into());
    let problems = Core::check(&Distribution { name: "t".into(), catalog: cat, profile: p });
    assert!(problems.iter().any(|x| x.code == "port_many_bound"), "{problems:#?}");
}

#[test]
fn bad_telemetry_settings_are_problems() {
    let clock = FixedClock::at(1);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a"]);
    p.telemetry.level = Some("loud".into());
    p.telemetry.trace_sample = Some(2.0);
    let problems = Core::check(&Distribution { name: "t".into(), catalog: catalog(&clock, &seen), profile: p });
    assert_eq!(problems.iter().filter(|x| x.code == "bad_telemetry").count(), 2, "{problems:#?}");
    let v = setup::profile_from_json(&json!({"name": "x", "telemetry": {"level": "info", "levels": {"http": "debug"}}})).unwrap();
    assert_eq!(v.telemetry.levels["http"], "debug");
    assert!(setup::profile_from_json(&json!({"name": "x", "telemetry": {"lvl": "info"}})).is_err(), "typos are errors");
}

#[test]
fn metrics_and_heartbeats_are_pushed_on_their_interval_and_stop_flushes() {
    let (beats, pushes): (Seen<String>, Seen<usize>) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
    let r = rig(&[], |_| {}, vec![Arc::new(BeatPlugin(beats.clone())), Arc::new(PushPlugin(pushes.clone()))], &["beat", "push"]);
    let services = r.core.start_services().unwrap();
    create_book(&r.core, "one").unwrap();
    let t = Instant::now();
    while (beats.lock().is_empty() || pushes.lock().is_empty()) && t.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(100));
    }
    services.stop();
    assert_eq!(beats.lock().first().map(|s| s.as_str()), Some("ok"));
    assert!(pushes.lock().first().is_some_and(|n| *n > 0), "the snapshot holds the Core's own series");
}

#[test]
fn the_health_snapshot_reads_the_database() {
    let r = rig(&[], |_| {}, vec![], &[]);
    let h = r.core.health();
    assert!(h.ready && h.status == "ok");
    assert!(h.checks.iter().any(|c| c.name == "db" && c.ok));
    assert_eq!(h.contract, r.core.contract_hash());
}

// ---- the first-party Adapters --------------------------------------------------------------------------

#[test]
fn time_is_written_in_utc_iso() {
    assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso(1_000_000_000_123), "2001-09-09T01:46:40.123Z");
    assert_eq!(iso(1_782_000_000_000), "2026-06-21T00:00:00.000Z");
}

#[test]
fn a_record_is_one_json_line() {
    let r = Record { time: 0, level: Level::Warn, target: "x".into(), key: "k".into(), fields: json!({"a": 1}), error: None, request: Some("q".into()) };
    let line = json_line(&r);
    assert!(!line.contains('\n'));
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!((v["level"].as_str(), v["target"].as_str(), v["fields"]["a"].as_i64(), v["request"].as_str()), (Some("warn"), Some("x"), Some(1), Some("q")));
}

#[test]
fn the_file_log_appends_json_lines_and_rotates() {
    let dir = std::env::temp_dir().join(format!("nuah-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("app.log");
    let Some(PortImpl::Log(a)) = LogFile.fill(&json!({"path": path.display().to_string(), "max_bytes": 1024, "keep": 2})).unwrap() else { panic!("a log adapter") };
    let rec = |i: usize| Record { time: 0, level: Level::Info, target: "t".into(), key: format!("line{i}"), fields: json!({"pad": "x".repeat(100)}), error: None, request: None };
    for i in 0..30 {
        a.record(&rec(i));
    }
    a.flush();
    assert!(dir.join("app.log.1").exists() && dir.join("app.log.2").exists(), "rotated");
    assert!(!dir.join("app.log.3").exists(), "only `keep` files are kept");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.lines().all(|l| serde_json::from_str::<Value>(l).is_ok()));
    assert!(text.contains("line29"), "the newest line is in the live file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_secret_can_be_a_reference_to_the_environment_or_a_file() {
    let dir = std::env::temp_dir().join(format!("nuah-ref-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("secret");
    std::fs::write(&f, "from-a-file\n").unwrap();
    assert_eq!(setup::resolve_refs(&json!({"a": {"$file": f.display().to_string()}, "b": [{"$env": "PATH"}]})).unwrap()["a"], "from-a-file");
    assert!(setup::resolve_refs(&json!({"$env": "NUAH_SURELY_NOT_SET_9f3"})).is_err());
    assert!(setup::is_ref(&json!({"$env": "X"})) && !setup::is_ref(&json!({"$env": "X", "other": 1})) && !setup::is_ref(&json!({"$secret": true})));
    // the Resolver reports an unresolved reference as a Problem and goes on
    let clock = FixedClock::at(1);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(LogFile));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a", "log-file"]);
    p.settings.insert("log-file".into(), json!({"path": {"$env": "NUAH_SURELY_NOT_SET_9f3"}}));
    let problems = Core::check(&Distribution { name: "t".into(), catalog: cat.clone(), profile: p.clone() });
    assert!(problems.iter().any(|x| x.code == "bad_secret_ref" && x.plugins == ["log-file"]), "{problems:#?}");
    p.settings.insert("log-file".into(), json!({"path": {"$file": f.display().to_string()}}));
    assert!(Core::check(&Distribution { name: "t".into(), catalog: cat, profile: p }).is_empty(), "a reference passes the settings schema once resolved");
    // a reference stays visible in the wizard; only real secrets are hidden
    assert_eq!(setup::redact(&json!({"$env": "X"}), &json!({"x-secret": true})), json!({"$env": "X"}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_services_stopping_twice_says_so_once() {
    let r = rig(&[("log-a", Duration::ZERO)], |_| {}, vec![], &[]);
    let s = r.core.start_services().unwrap();
    s.stop();
    r.core.flush_telemetry(Duration::from_secs(1));
    assert_eq!(r.logs[0].lock().iter().filter(|x| x.key == "core.stopped").count(), 1);
}

#[test]
fn first_party_settings_say_where_they_live_and_how_a_change_applies() {
    for p in [Arc::new(LogConsole) as Arc<dyn Plugin>, Arc::new(LogFile), Arc::new(AdapterSqlite)] {
        let m = p.manifest();
        let problems = setup::settings_annotation_problems(&m.settings_schema);
        assert!(problems.is_empty(), "{}: {problems:?}", m.id);
    }
}

struct SlowInterval(Seen<usize>, u64, &'static str);
impl MetricAdapter for SlowInterval {
    fn name(&self) -> &str {
        self.2
    }
    fn interval_secs(&self) -> u64 {
        self.1
    }
    fn export(&self, s: &Snapshot) {
        self.0.lock().push(s.series.len());
    }
}
struct SlowIntervalPlugin(Seen<usize>, u64, &'static str);
impl Plugin for SlowIntervalPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.2, "version": "1.0.0", "fills": "metric"})).unwrap()
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Metric(Arc::new(SlowInterval(self.0.clone(), self.1, self.2)))))
    }
}

#[test]
fn each_metric_adapter_has_its_own_interval() {
    let (fast, slow): (Seen<usize>, Seen<usize>) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
    let r = rig(&[], |_| {}, vec![Arc::new(SlowIntervalPlugin(fast.clone(), 1, "fast")), Arc::new(SlowIntervalPlugin(slow.clone(), 3600, "slow"))], &["fast", "slow"]);
    let services = r.core.start_services().unwrap();
    let t = Instant::now();
    while fast.lock().is_empty() && t.elapsed() < Duration::from_secs(6) {
        std::thread::sleep(Duration::from_millis(100));
    }
    services.stop();
    assert!(!fast.lock().is_empty());
    assert!(slow.lock().is_empty(), "the hourly Adapter is not handed a snapshot every second");
}

#[test]
fn setup_does_not_insist_on_a_reference_that_only_resolves_where_the_core_runs() {
    let clock = FixedClock::at(1);
    let seen = Arc::new(Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(LogFile));
    let mut p = profile(&["db-memory", "auth-local", "policy-roles", "clock-fixed", "ids-sequential", "notes", "tell", "tell-a", "log-file"]);
    p.settings.insert("log-file".into(), json!({"path": {"$env": "NUAH_SURELY_NOT_SET_9f3"}}));
    let d = Distribution { name: "t".into(), catalog: cat, profile: p };
    assert!(!Core::check(&d).is_empty(), "starting still fails");
    assert!(Core::check_for_setup(&d).is_empty(), "saving a Profile in setup does not");
}

#[cfg(unix)]
#[test]
fn the_log_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("nuah-logmode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("x.log");
    let _ = LogFile.fill(&json!({"path": path.display().to_string()})).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = std::fs::remove_dir_all(&dir);
}
