//! Telemetry: what the Core records, and the four optional Ports that carry it away (log, trace,
//! metric, heartbeat). The Core decides *what* is recorded (levels, fields, errors); an Adapter
//! decides how it looks and where it goes.
//!
//! A signal never fails or slows the thing it describes: every Adapter runs behind its own bounded
//! queue and thread, owned by the kernel. When a queue is full the signal is dropped and counted.

use crate::db::Millis;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How much a record says, from rare to chatty. A record is kept when its level is at or below the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
            Level::Trace => "trace",
        }
    }
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "error" => Level::Error,
            "warn" | "warning" => Level::Warn,
            "info" => Level::Info,
            "debug" => Level::Debug,
            "trace" => Level::Trace,
            _ => return None,
        })
    }
}

/// The error part of a record: the same category and reason the Link returns, plus the reference a
/// caller can quote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub category: String,
    pub key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub message: String,
    /// What a caller quotes to find this record (the request id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

impl ErrorInfo {
    pub fn of(e: &Error, reference: Option<&str>) -> Self {
        ErrorInfo {
            category: serde_json::to_value(e.category).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(),
            key: e.key.clone(),
            reason: e.reason.clone(),
            message: e.message.clone(),
            reference: reference.map(String::from),
        }
    }
}

/// One log entry. The message is a key plus fields, so a destination can translate or reformat it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub time: Millis,
    pub level: Level,
    /// The plugin that wrote it, or `nuah.kernel`, `nuah.host`.
    pub target: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub fields: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

/// One finished unit of work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub trace: String,
    pub id: String,
    pub name: String,
    pub target: String,
    pub start: Millis,
    pub duration_ms: f64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub attributes: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Counter,
    Gauge,
    Histogram,
}

/// One labelled measurement at a moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    pub kind: Kind,
    pub labels: BTreeMap<String, String>,
    /// The value of a counter or gauge; the sum of a histogram.
    pub value: f64,
    /// Histograms only: (upper bound, cumulative count).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<(f64, u64)>,
    #[serde(default)]
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub time: Millis,
    pub series: Vec<Series>,
}

/// What the Core says about itself on a schedule, and on `/healthz`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// `ok` or `degraded`.
    pub status: String,
    pub ready: bool,
    pub uptime_secs: u64,
    pub profile: String,
    pub contract: String,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// A destination for log records. Called from one thread of the kernel's, so it may block.
pub trait LogAdapter: Send + Sync {
    fn name(&self) -> &str;
    /// Records chattier than this never reach the Adapter.
    fn min_level(&self) -> Level {
        Level::Info
    }
    fn record(&self, r: &Record);
    /// Everything received so far is written out (called on stop).
    fn flush(&self) {}
}

pub trait TraceAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn span(&self, s: &Span);
    fn flush(&self) {}
}

pub trait MetricAdapter: Send + Sync {
    fn name(&self) -> &str;
    /// How often the Core hands over a snapshot; `0` means the Adapter pulls (a scrape) and is never pushed to.
    fn interval_secs(&self) -> u64 {
        15
    }
    fn export(&self, s: &Snapshot);
    fn flush(&self) {}
}

pub trait HeartbeatAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn interval_secs(&self) -> u64 {
        30
    }
    fn beat(&self, h: &Health);
    fn flush(&self) {}
}

/// The `telemetry` part of a Profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetrySettings {
    /// Default level for every target (`info` when absent).
    pub level: Option<String>,
    /// Per-target levels, by plugin id (`http`: `debug`).
    pub levels: BTreeMap<String, String>,
    /// Share of operations that produce a trace span, 0 to 1 (1 when absent; only matters if a trace Adapter is on).
    pub trace_sample: Option<f64>,
    /// How many records an Adapter may have waiting (1024 when absent).
    pub queue: Option<usize>,
}

impl TelemetrySettings {
    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        if let Some(l) = &self.level {
            if Level::parse(l).is_none() {
                out.push(format!("telemetry.level `{l}` is not one of error, warn, info, debug, trace"));
            }
        }
        for (t, l) in &self.levels {
            if Level::parse(l).is_none() {
                out.push(format!("telemetry.levels.{t} `{l}` is not one of error, warn, info, debug, trace"));
            }
        }
        if let Some(s) = self.trace_sample {
            if !(0.0..=1.0).contains(&s) {
                out.push("telemetry.trace_sample must be between 0 and 1".into());
            }
        }
        if self.queue == Some(0) {
            out.push("telemetry.queue must be at least 1".into());
        }
        out
    }
}

// ---- the queue every Adapter sits behind ---------------------------------------------------------------

enum Msg<T> {
    Item(T),
    Flush(SyncSender<()>),
}

/// A bounded queue and the thread that drains it into one Adapter.
pub(crate) struct Dispatcher<T: Send + 'static> {
    pub(crate) name: String,
    tx: SyncSender<Msg<T>>,
    dropped: Arc<AtomicU64>,
}

impl<T: Send + 'static> Dispatcher<T> {
    pub(crate) fn start(name: &str, cap: usize, mut handle: impl FnMut(T) + Send + 'static, mut flush: impl FnMut() + Send + 'static) -> Self {
        let (tx, rx): (SyncSender<Msg<T>>, Receiver<Msg<T>>) = sync_channel(cap.max(1));
        let spawned = std::thread::Builder::new().name(format!("nuah-telemetry-{name}")).spawn(move || {
            while let Ok(m) = rx.recv() {
                match m {
                    Msg::Item(t) => handle(t),
                    Msg::Flush(done) => {
                        flush();
                        let _ = done.send(());
                    }
                }
            }
            flush();
        });
        // If a thread cannot be made, the Adapter simply never receives anything.
        let _ = spawned;
        Dispatcher { name: name.into(), tx, dropped: Arc::new(AtomicU64::new(0)) }
    }

    /// Never blocks. A full queue drops the signal and counts it.
    pub(crate) fn send(&self, t: T) {
        if let Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) = self.tx.try_send(Msg::Item(t)) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait (up to `deadline`) until everything queued so far has been handled and flushed.
    pub(crate) fn flush(&self, deadline: Duration) {
        let start = Instant::now();
        let (done, wait) = sync_channel(1);
        let mut msg = Msg::Flush(done);
        loop {
            match self.tx.try_send(msg) {
                Ok(()) => break,
                Err(TrySendError::Full(m)) if start.elapsed() < deadline => {
                    msg = m;
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
        let _ = wait.recv_timeout(deadline.saturating_sub(start.elapsed()));
    }
}

// ---- metrics the Core keeps ------------------------------------------------------------------------------

const BUCKETS: [f64; 12] = [1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0];

#[derive(Clone)]
struct Cell {
    kind: Kind,
    value: f64,
    buckets: [u64; 12],
    count: u64,
}

/// The Core's counters, gauges and histograms. Cheap to update; read as a [`Snapshot`].
#[derive(Default)]
pub struct Metrics {
    cells: Mutex<BTreeMap<(String, Vec<(String, String)>), Cell>>,
}

fn key(name: &str, labels: &[(&str, &str)]) -> (String, Vec<(String, String)>) {
    let mut l: Vec<(String, String)> = labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    l.sort();
    (name.into(), l)
}

impl Metrics {
    fn cell(&self, name: &str, labels: &[(&str, &str)], kind: Kind, f: impl FnOnce(&mut Cell)) {
        let mut g = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.entry(key(name, labels)).or_insert(Cell { kind, value: 0.0, buckets: [0; 12], count: 0 });
        f(c);
    }
    /// Add to a counter.
    pub fn inc(&self, name: &str, labels: &[(&str, &str)], by: f64) {
        self.cell(name, labels, Kind::Counter, |c| c.value += by);
    }
    /// Set a gauge.
    pub fn set(&self, name: &str, labels: &[(&str, &str)], v: f64) {
        self.cell(name, labels, Kind::Gauge, |c| c.value = v);
    }
    /// Record one observation (milliseconds, by convention) in a histogram.
    pub fn observe(&self, name: &str, labels: &[(&str, &str)], v: f64) {
        self.cell(name, labels, Kind::Histogram, |c| {
            c.value += v;
            c.count += 1;
            for (i, b) in BUCKETS.iter().enumerate() {
                if v <= *b {
                    c.buckets[i] += 1;
                }
            }
        });
    }
    pub fn snapshot(&self, time: Millis) -> Snapshot {
        let g = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let series = g
            .iter()
            .map(|((name, labels), c)| Series {
                name: name.clone(),
                kind: c.kind,
                labels: labels.iter().cloned().collect(),
                value: c.value,
                buckets: if c.kind == Kind::Histogram { BUCKETS.iter().cloned().zip(c.buckets.iter().cloned()).collect() } else { vec![] },
                count: c.count,
            })
            .collect();
        Snapshot { time, series }
    }
}

// ---- the Core's telemetry --------------------------------------------------------------------------------

pub(crate) struct Telemetry {
    levels: RwLock<(Level, BTreeMap<String, Level>)>,
    logs: Vec<(Dispatcher<Arc<Record>>, Level)>,
    traces: Vec<Dispatcher<Arc<Span>>>,
    sample: f64,
    metric_out: Vec<(Dispatcher<Arc<Snapshot>>, u64)>,
    beat_out: Vec<(Dispatcher<Arc<Health>>, u64)>,
    last_push: Mutex<(Instant, Instant)>,
    pub(crate) metrics: Metrics,
    pub(crate) started: Instant,
}

impl Telemetry {
    pub(crate) fn new(
        s: &TelemetrySettings,
        logs: Vec<Arc<dyn LogAdapter>>,
        traces: Vec<Arc<dyn TraceAdapter>>,
        metric: Vec<Arc<dyn MetricAdapter>>,
        beats: Vec<Arc<dyn HeartbeatAdapter>>,
    ) -> Self {
        let cap = s.queue.unwrap_or(1024);
        let default = s.level.as_deref().and_then(Level::parse).unwrap_or(Level::Info);
        let targets = s.levels.iter().filter_map(|(t, l)| Level::parse(l).map(|l| (t.clone(), l))).collect();
        let logs = logs
            .into_iter()
            .map(|a| {
                let min = a.min_level();
                let (a2, a3) = (a.clone(), a.clone());
                (Dispatcher::start(a.name(), cap, move |r: Arc<Record>| a2.record(&r), move || a3.flush()), min)
            })
            .collect();
        let traces = traces
            .into_iter()
            .map(|a| {
                let (a2, a3) = (a.clone(), a.clone());
                Dispatcher::start(a.name(), cap, move |s: Arc<Span>| a2.span(&s), move || a3.flush())
            })
            .collect();
        let metric_out = metric
            .into_iter()
            .map(|a| {
                let (a2, a3, secs) = (a.clone(), a.clone(), a.interval_secs());
                (Dispatcher::start(a.name(), 2, move |s: Arc<Snapshot>| a2.export(&s), move || a3.flush()), secs)
            })
            .collect();
        let beat_out = beats
            .into_iter()
            .map(|a| {
                let (a2, a3, secs) = (a.clone(), a.clone(), a.interval_secs());
                (Dispatcher::start(a.name(), 2, move |h: Arc<Health>| a2.beat(&h), move || a3.flush()), secs)
            })
            .collect();
        let now = Instant::now();
        Telemetry {
            levels: RwLock::new((default, targets)),
            logs,
            traces,
            sample: s.trace_sample.unwrap_or(1.0),
            metric_out,
            beat_out,
            last_push: Mutex::new((now, now)),
            metrics: Metrics::default(),
            started: now,
        }
    }

    pub(crate) fn enabled(&self, level: Level, target: &str) -> bool {
        let g = self.levels.read().unwrap_or_else(|e| e.into_inner());
        level <= g.1.get(target).copied().unwrap_or(g.0)
    }

    pub(crate) fn set_level(&self, target: Option<&str>, level: Level) {
        let mut g = self.levels.write().unwrap_or_else(|e| e.into_inner());
        match target {
            Some(t) => {
                g.1.insert(t.into(), level);
            }
            None => g.0 = level,
        }
    }

    pub(crate) fn log(&self, rec: Record) {
        if !self.enabled(rec.level, &rec.target) {
            return;
        }
        if self.logs.is_empty() {
            // Nothing is enabled: errors must still never be silent.
            eprintln!("{}", fallback_line(&rec));
            return;
        }
        let rec = Arc::new(rec);
        for (d, min) in &self.logs {
            if rec.level <= *min {
                d.send(rec.clone());
            }
        }
    }

    pub(crate) fn wants_spans(&self) -> bool {
        !self.traces.is_empty() && self.sample > 0.0
    }

    pub(crate) fn sampled(&self, request: &str) -> bool {
        if self.sample >= 1.0 {
            return true;
        }
        let h = request.bytes().fold(14695981039346656037u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211));
        (h % 10_000) as f64 / 10_000.0 < self.sample
    }

    pub(crate) fn span(&self, s: Span) {
        let s = Arc::new(s);
        for d in &self.traces {
            d.send(s.clone());
        }
    }

    pub(crate) fn dropped_total(&self) -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = vec![];
        for (d, _) in &self.logs {
            v.push((d.name.clone(), d.dropped()));
        }
        for d in &self.traces {
            v.push((d.name.clone(), d.dropped()));
        }
        for (d, _) in &self.metric_out {
            v.push((d.name.clone(), d.dropped()));
        }
        for (d, _) in &self.beat_out {
            v.push((d.name.clone(), d.dropped()));
        }
        v
    }

    /// Hand snapshots and heartbeats to the Adapters whose interval has passed. Called by housekeeping.
    pub(crate) fn tick(&self, now_ms: Millis, health: impl FnOnce() -> Health) {
        for (name, n) in self.dropped_total() {
            self.metrics.set("nuah_telemetry_dropped_total", &[("adapter", &name)], n as f64);
        }
        self.metrics.set("nuah_uptime_seconds", &[], self.started.elapsed().as_secs_f64());
        let mut last = self.last_push.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if !self.metric_out.is_empty() {
            let due = self.metric_out.iter().any(|(_, secs)| *secs > 0 && now.duration_since(last.0).as_secs() >= *secs);
            if due {
                last.0 = now;
                let snap = Arc::new(self.metrics.snapshot(now_ms));
                for (d, secs) in &self.metric_out {
                    if *secs > 0 {
                        d.send(snap.clone());
                    }
                }
            }
        }
        if !self.beat_out.is_empty() {
            let due = self.beat_out.iter().any(|(_, secs)| *secs > 0 && now.duration_since(last.1).as_secs() >= *secs);
            if due {
                last.1 = now;
                let h = Arc::new(health());
                for (d, secs) in &self.beat_out {
                    if *secs > 0 {
                        d.send(h.clone());
                    }
                }
            }
        }
    }

    /// Give every Adapter up to `deadline` to write out what it has.
    pub(crate) fn flush(&self, deadline: Duration) {
        let start = Instant::now();
        let left = || deadline.saturating_sub(start.elapsed());
        for (d, _) in &self.logs {
            d.flush(left());
        }
        for d in &self.traces {
            d.flush(left());
        }
        for (d, _) in &self.metric_out {
            d.flush(left());
        }
        for (d, _) in &self.beat_out {
            d.flush(left());
        }
    }
}

fn fallback_line(r: &Record) -> String {
    let mut s = format!("{} [{}] {}", r.level.as_str().to_uppercase(), r.target, r.key);
    if !r.fields.is_null() {
        s.push(' ');
        s.push_str(&r.fields.to_string());
    }
    if let Some(e) = &r.error {
        s.push_str(&format!(" error={} ({})", e.message, e.category));
        if let Some(rf) = &e.reference {
            s.push_str(&format!(" ref={rf}"));
        }
    }
    s
}

pub(crate) fn record(time: Millis, level: Level, target: &str, key: &str, fields: Value, error: Option<ErrorInfo>, request: Option<&str>) -> Record {
    Record { time, level, target: target.into(), key: key.into(), fields, error, request: request.map(String::from) }
}

/// A health snapshot with no checks beyond readiness; the Core adds its own.
pub fn health_json(h: &Health) -> Value {
    json!({"status": h.status, "ready": h.ready, "uptime_secs": h.uptime_secs, "profile": h.profile, "contract": h.contract,
           "checks": h.checks.iter().map(|c| json!({"name": c.name, "ok": c.ok, "detail": c.detail})).collect::<Vec<_>>()})
}
