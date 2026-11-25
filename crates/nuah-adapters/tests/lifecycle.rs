// SPDX-License-Identifier: Apache-2.0
//! A plugin's service can report its health, what a plugin registered to close is closed when the Core stops, and a stop
//! waits for calls already running while refusing new ones from outside ([ADR 0135]).
//!
//! [ADR 0135]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=423f1889730140fa86c845229cc0f610
mod common;
use common::*;
use nuah_adapters::*;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Probe(Arc<AtomicBool>);
impl Service for Probe {
    fn stop(self: Box<Self>) {}
    fn health_probe(&self) -> Option<HealthProbe> {
        let well = self.0.clone();
        Some(Arc::new(move || (well.load(Ordering::SeqCst), if well.load(Ordering::SeqCst) { "fine".into() } else { "cannot reach its upstream".into() })))
    }
}

struct Lifecycle {
    id: &'static str,
    well: Arc<AtomicBool>,
    closed: Arc<Mutex<Vec<String>>>,
}
impl Plugin for Lifecycle {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": self.id, "version": "1", "namespaces": [self.id.trim_start_matches("@test/")],
            "requires": {"capabilities": ["serve"]},
            "permissions": {format!("{}.use", self.id.trim_start_matches("@test/")): {"default": ["member"]}},
            "operations": {format!("{0}.slow", self.id.trim_start_matches("@test/")): {"kind": "query", "permission": format!("{0}.use", self.id.trim_start_matches("@test/"))}}
        }))
        .unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        let name = format!("{}.slow", self.id.trim_start_matches("@test/"));
        b.op(&name, |_, _| {
            std::thread::sleep(Duration::from_millis(400));
            Ok(json!({"done": true}))
        });
        let (id, closed) = (self.id.to_string(), self.closed.clone());
        b.on_stop(move || closed.lock().unwrap().push(id.clone()));
    }
    fn serve(&self, _ctx: ServiceContext) -> Result<Option<Box<dyn Service>>> {
        Ok(Some(Box::new(Probe(self.well.clone()))))
    }
}

fn world(well: &Arc<AtomicBool>, closed: &Arc<Mutex<Vec<String>>>) -> Core {
    let clock = FixedClock::at(1_000_000);
    let seen = Arc::new(parking_lot::Mutex::new(vec![]));
    let mut cat = catalog(&clock, &seen);
    cat.0.push(Arc::new(Lifecycle { id: "@test/aaa", well: well.clone(), closed: closed.clone() }));
    cat.0.push(Arc::new(Lifecycle { id: "@test/bbb", well: Arc::new(AtomicBool::new(true)), closed: closed.clone() }));
    let mut p = profile(&["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-fixed", "@nuah/ids-sequential", "@test/aaa", "@test/bbb"]);
    p.grants.insert("@test/aaa".into(), vec!["serve".into()]);
    p.grants.insert("@test/bbb".into(), vec!["serve".into()]);
    Core::start(&Distribution { name: "test".into(), catalog: cat, profile: p }).unwrap_or_else(|p| panic!("start failed: {p:#?}"))
}

#[test]
fn a_service_that_is_not_well_makes_the_core_degraded_and_says_which() {
    let (well, closed) = (Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(vec![])));
    let core = world(&well, &closed);
    let services = core.start_services().unwrap();
    let h = core.health();
    assert_eq!(h.status, "ok");
    assert!(h.checks.iter().any(|c| c.name == "@test/aaa" && c.ok), "{:?}", h.checks);
    well.store(false, Ordering::SeqCst);
    let h = core.health();
    assert_eq!(h.status, "degraded");
    assert!(h.ready, "the Core can still serve; the check names what is wrong");
    let bad = h.checks.iter().find(|c| !c.ok).unwrap();
    assert_eq!((bad.name.as_str(), bad.detail.as_str()), ("@test/aaa", "cannot reach its upstream"));
    services.stop();
}

#[test]
fn what_a_plugin_registered_to_close_is_closed_last_plugin_first() {
    let (well, closed) = (Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(vec![])));
    let core = world(&well, &closed);
    core.start_services().unwrap().stop();
    assert_eq!(*closed.lock().unwrap(), ["@test/bbb", "@test/aaa"]);
}

#[test]
fn a_stop_waits_for_a_call_already_running_and_refuses_new_ones_from_outside() {
    let (well, closed) = (Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(vec![])));
    let core = world(&well, &closed);
    let services = core.start_services().unwrap();
    let who = person("alice", &["member"]);
    let (c2, w2) = (core.clone(), who.clone());
    let running = std::thread::spawn(move || c2.call_as(&w2, "aaa.slow", json!({}), None).map(|r| r.output));
    std::thread::sleep(Duration::from_millis(100));
    let stopped_at = Instant::now();
    let c3 = core.clone();
    let stopper = std::thread::spawn(move || services.stop());
    std::thread::sleep(Duration::from_millis(50));
    let refused = core.call(CallRequest { op: "aaa.slow".into(), input: Value::Null, credential: None, key: None, contract: None });
    assert_eq!(refused.unwrap_err().reason.as_deref(), Some("kernel.draining"), "a new call from outside is refused while stopping");
    stopper.join().unwrap();
    assert!(stopped_at.elapsed() >= Duration::from_millis(250), "the stop waited for the running call: {:?}", stopped_at.elapsed());
    assert_eq!(running.join().unwrap().unwrap(), json!({"done": true}), "the running call finished");
    let _ = c3;
}
