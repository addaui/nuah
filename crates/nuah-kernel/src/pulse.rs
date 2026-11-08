// SPDX-License-Identifier: Apache-2.0
//! Pulse (what one Subject sees of the Event log) and durable Listeners ([ADR 0003]).
//!
//! [ADR 0003]: https://docs.addaui.com/nuah/adr/0003-one-change-record-scoped-audience-durable-listeners/

use crate::core::{Core, Inner};
use crate::db::*;
use crate::error::{Error, Result};
use crate::plugin::*;
use crate::ports::Identity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SCAN: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseEvent {
    pub cursor: String,
    pub kind: String,
    pub data: Value,
    pub at: Millis,
    pub actor: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PulseBatch {
    pub events: Vec<PulseEvent>,
    /// Hand this back to continue. Opaque.
    pub cursor: String,
    /// The cursor is older than the log keeps: reload everything, then follow from `cursor`.
    pub reset: bool,
    /// There may be more right now.
    pub more: bool,
}

fn parse_cursor(c: &str) -> Result<u64> {
    c.strip_prefix('c').and_then(|n| n.parse().ok()).ok_or_else(|| Error::invalid("cursor", "is not a Pulse cursor"))
}

impl Inner {
    fn visible(&self, r: &dyn DbRead, a: &Audience, subject: &str) -> Result<bool> {
        match a {
            Audience::Subject { subject: s } => Ok(s == subject),
            Audience::Scope { scope, id } => {
                let Some(owner) = self.plan.scopes.get(scope) else { return Ok(false) };
                let f = &self.plan.active[owner].bindings.scopes[scope];
                f(r, id, subject)
            }
        }
    }
}

impl Core {
    /// The newest cursor: where a Face that has just loaded its state starts following.
    pub fn pulse_now(&self) -> Result<String> {
        let mut latest = 0;
        self.inner.db.read(&mut |r| {
            latest = r.latest_change()?;
            Ok(())
        })?;
        Ok(format!("c{latest}"))
    }

    /// The Events `who` may see after `after`, in commit order. Scopes are resolved now, so someone
    /// removed from a group stops receiving its Events.
    pub fn pulse(&self, who: &Identity, after: Option<&str>, limit: usize) -> Result<PulseBatch> {
        let inner = &*self.inner;
        let mut out: Option<PulseBatch> = None;
        let after_seq = match after {
            Some(c) => Some(parse_cursor(c)?),
            None => None,
        };
        inner.db.read(&mut |r| {
            let latest = r.latest_change()?;
            let Some(from) = after_seq else {
                out = Some(PulseBatch { events: vec![], cursor: format!("c{latest}"), reset: false, more: false });
                return Ok(());
            };
            let oldest = r.oldest_change()?;
            if oldest > 0 && from + 1 < oldest {
                out = Some(PulseBatch { events: vec![], cursor: format!("c{latest}"), reset: true, more: false });
                return Ok(());
            }
            let records = r.changes_after(from, SCAN)?;
            let mut events = vec![];
            let mut cursor = from;
            let mut more = false;
            for rec in &records {
                let mut mine = vec![];
                for ev in &rec.events {
                    if inner.visible(r, &ev.audience, &who.subject)? {
                        mine.push(PulseEvent { cursor: format!("c{}", rec.seq), kind: ev.kind.clone(), data: ev.data.clone(), at: rec.at, actor: rec.actor.clone(), operation: rec.operation.clone() });
                    }
                }
                if !events.is_empty() && events.len() + mine.len() > limit {
                    more = true;
                    break;
                }
                events.extend(mine);
                cursor = rec.seq;
            }
            if !more && records.len() == SCAN {
                more = true;
            }
            out = Some(PulseBatch { events, cursor: format!("c{cursor}"), reset: false, more });
            Ok(())
        })?;
        Ok(out.expect("pulse ran"))
    }

    /// Forget all but the newest `keep` change records. A Pulse cursor older than that gets a reset.
    pub fn prune_log(&self, keep: usize) -> Result<()> {
        self.inner.db.write(&mut |w| w.prune_changes(keep))
    }

    /// Deliver committed Events to every Listener that has not seen them yet, each in its own
    /// transaction with its own stored cursor. At least once: a Listener whose handler fails is retried
    /// from where it was. Returns how many deliveries succeeded; the first failure, if any, is returned
    /// after every Listener has had its turn.
    pub fn drain_listeners(&self) -> Result<usize> {
        let inner = &*self.inner;
        let mut delivered = 0;
        let mut first_err: Option<Error> = None;
        for (pid, a) in &inner.plan.active {
            for l in &a.manifest.listeners {
                loop {
                    match inner.run_listener(pid, &l.id, &l.on, &a.bindings.listeners[&l.id]) {
                        Ok(0) => break,
                        Ok(n) => delivered += n,
                        Err(e) => {
                            first_err.get_or_insert(e);
                            break;
                        }
                    }
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(delivered),
        }
    }
}

fn pattern_matches(patterns: &[String], kind: &str) -> bool {
    patterns.iter().any(|p| match p.strip_suffix(".*") {
        Some(prefix) => kind.starts_with(&format!("{prefix}.")),
        None => p == "*" || p == kind,
    })
}

impl Inner {
    /// Make sure every Listener has a cursor; a new one starts at "now".
    pub(crate) fn init_listener_cursors(&self) -> Result<()> {
        self.db.write(&mut |w| {
            let latest = w.latest_change()?;
            let now = self.clock.now();
            for (pid, a) in &self.plan.active {
                for l in &a.manifest.listeners {
                    let id = format!("{pid}:{}", l.id);
                    if w.get("_cursors", &id)?.is_none() {
                        w.put("_cursors", &id, "kernel", json!({"seq": latest}), Expect::Any, now)?;
                    }
                }
            }
            Ok(())
        })
    }

    /// One batch for one Listener. Returns the number of Events delivered (0 when caught up).
    fn run_listener(&self, plugin: &str, id: &str, on: &[String], f: &ListenerFn) -> Result<usize> {
        let cursor_id = format!("{plugin}:{id}");
        let who = Identity { subject: format!("listener:{cursor_id}"), display: cursor_id.clone(), roles: vec![] };
        let mut delivered = 0usize;
        let mut failure: Option<Error> = None;
        let mut committed: Option<u64> = None;
        let write = self.db.write(&mut |w| {
            delivered = 0;
            failure = None;
            committed = None;
            let from = w.get("_cursors", &cursor_id)?.and_then(|d| d.body["seq"].as_u64()).unwrap_or(0);
            let records = w.changes_after(from, 50)?;
            let Some(last) = records.last().map(|r| r.seq) else { return Ok(()) };
            let request = self.ids.new_id();
            let (mut events, mut docs) = (vec![], vec![]);
            {
                let mut tx = Tx { inner: self, plugin: plugin.into(), who: &who, request: &request, h: Handle::W(&mut *w), events: &mut events, docs: &mut docs, depth: 0 };
                for rec in &records {
                    for ev in rec.events.iter().filter(|e| pattern_matches(on, &e.kind)) {
                        let d = Delivery { seq: rec.seq, at: rec.at, actor: rec.actor.clone(), operation: rec.operation.clone(), event: ev.clone() };
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut tx, &d)));
                        match r {
                            Ok(Ok(())) => delivered += 1,
                            Ok(Err(e)) => {
                                failure = Some(e);
                                return Err(Error::internal("listener failed"));
                            }
                            Err(_) => {
                                failure = Some(Error::internal("a listener panicked"));
                                return Err(Error::internal("listener failed"));
                            }
                        }
                    }
                }
            }
            let now = self.clock.now();
            w.put("_cursors", &cursor_id, "kernel", json!({"seq": last}), Expect::Any, now)?;
            if !events.is_empty() || !docs.is_empty() {
                committed = Some(w.append_change(ChangeRecord { seq: 0, at: now, actor: who.subject.clone(), request, operation: format!("listener:{cursor_id}"), docs, events })?);
            }
            // Count batches that only skipped events as progress too, so the loop ends.
            if delivered == 0 {
                delivered = 0;
            }
            Ok(())
        });
        if let Some(e) = failure {
            return Err(e);
        }
        write?;
        if let Some(seq) = committed {
            let _ = self.wake.send(seq);
        }
        // Progress is made whenever the cursor moved, even if nothing matched.
        let mut more = 0usize;
        self.db.read(&mut |r| {
            let at = r.get("_cursors", &cursor_id)?.and_then(|d| d.body["seq"].as_u64()).unwrap_or(0);
            more = if r.latest_change()? > at { 1 } else { 0 };
            Ok(())
        })?;
        Ok(delivered.max(more))
    }
}
