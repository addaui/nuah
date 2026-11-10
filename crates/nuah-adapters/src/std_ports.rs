// SPDX-License-Identifier: Apache-2.0
//! Clock and id Adapters: the system ones, and fixed ones for tests and replay.

use nuah_kernel::*;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Millis {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    }
}

/// A clock that only moves when told to.
#[derive(Default)]
pub struct FixedClock(pub AtomicI64);
impl FixedClock {
    pub fn at(ms: i64) -> Arc<Self> {
        Arc::new(FixedClock(AtomicI64::new(ms)))
    }
    pub fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}
impl Clock for FixedClock {
    fn now(&self) -> Millis {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct RandomIds;
impl Ids for RandomIds {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().simple().to_string()
    }
}

/// Ids n1, n2, n3 ... : predictable, for tests.
#[derive(Default)]
pub struct SequentialIds(AtomicU64);
impl Ids for SequentialIds {
    fn new_id(&self) -> String {
        format!("n{}", self.0.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

fn simple_adapter(id: &str, port: &str, desc: &str) -> Manifest {
    serde_json::from_value(json!({"id": id, "version": "0.2.0", "name": id, "description": desc, "fills": port})).unwrap()
}

macro_rules! adapter {
    ($name:ident, $id:literal, $port:literal, $desc:literal, |$s:ident| $make:expr) => {
        pub struct $name;
        impl Plugin for $name {
            fn manifest(&self) -> Manifest {
                simple_adapter($id, $port, $desc)
            }
            fn fill(&self, $s: &Value) -> Result<Option<PortImpl>> {
                Ok(Some($make))
            }
        }
    };
}

adapter!(StdClock, "@nuah/clock-system", "clock", "The system clock", |_s| PortImpl::Clock(Arc::new(SystemClock)));
adapter!(StdIds, "@nuah/ids-random", "ids", "Random ids", |_s| PortImpl::Ids(Arc::new(RandomIds)));

/// Test adapters hand out shared handles so a test can move time and see ids.
pub struct TestClock(pub Arc<FixedClock>);
impl Plugin for TestClock {
    fn manifest(&self) -> Manifest {
        simple_adapter("@nuah/clock-fixed", "clock", "A clock that only moves when told to")
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Clock(self.0.clone())))
    }
}
pub struct TestIds;
impl Plugin for TestIds {
    fn manifest(&self) -> Manifest {
        simple_adapter("@nuah/ids-sequential", "ids", "Predictable ids")
    }
    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Ids(Arc::new(SequentialIds::default()))))
    }
}

/// Keeps one shared `Mutex` so the macro-generated types stay trivially `Send + Sync`.
#[allow(dead_code)]
type Unused = Mutex<()>;
