// SPDX-License-Identifier: Apache-2.0
//! What fills a Port nobody needs ([ADR 0039]). A plugin that did not ask for a Port is refused by the
//! Resolver before it could reach one of these, so they are never a quiet fallback for a missing Adapter:
//! they only keep the kernel's own code from branching on "is there a database".
//!
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

use crate::db::*;
use crate::error::{Error, Result};
use crate::ports::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// No storage: reads see an empty database, writes are refused.
pub struct NoDb;

struct Empty;
impl DbRead for Empty {
    fn get(&self, _: &str, _: &str) -> Result<Option<Doc>> {
        Ok(None)
    }
    fn query(&self, _: &str, _: &Query) -> Result<Page> {
        Ok(Page { items: vec![], next: None })
    }
    fn count(&self, _: &str, _: &[Cond]) -> Result<u64> {
        Ok(0)
    }
    fn changes_after(&self, _: u64, _: usize) -> Result<Vec<ChangeRecord>> {
        Ok(vec![])
    }
    fn oldest_change(&self) -> Result<u64> {
        Ok(0)
    }
    fn latest_change(&self) -> Result<u64> {
        Ok(0)
    }
    fn scan(&self, _: &str, _: &str, _: usize) -> Result<Vec<Doc>> {
        Ok(vec![])
    }
}

impl Db for NoDb {
    fn name(&self) -> &str {
        "none"
    }
    fn features(&self) -> Features {
        Features::default()
    }
    fn read(&self, f: &mut dyn FnMut(&dyn DbRead) -> Result<()>) -> Result<()> {
        f(&Empty)
    }
    fn write(&self, _: &mut dyn FnMut(&mut dyn DbWrite) -> Result<()>) -> Result<()> {
        Err(Error::unsupported("db", "none"))
    }
    fn ensure_index(&self, _: &IndexSpec) -> Result<()> {
        Err(Error::unsupported("db", "none"))
    }
}

/// Nobody is ever signed in.
pub struct NoAuth;
impl Auth for NoAuth {
    fn name(&self) -> &str {
        "none"
    }
    fn authenticate(&self, _: &dyn DbRead, _: &str, _: Millis) -> Result<Option<Identity>> {
        Ok(None)
    }
}

/// Nothing is allowed.
pub struct DenyAll;
impl Policy for DenyAll {
    fn name(&self) -> &str {
        "none"
    }
    fn allows(&self, _: &Identity, _: &str, _: Option<&Grant>, _: &PermissionTable) -> bool {
        false
    }
}

pub struct SystemTime;
impl Clock for SystemTime {
    fn now(&self) -> Millis {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
    }
}

/// Ids made from the time and a counter: unique within a process run, not secret, not stable across runs.
#[derive(Default)]
pub struct CounterIds(AtomicU64);
impl Ids for CounterIds {
    fn new_id(&self) -> String {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        format!("{:x}{:04x}", t, self.0.fetch_add(1, Ordering::Relaxed) & 0xffff)
    }
}

