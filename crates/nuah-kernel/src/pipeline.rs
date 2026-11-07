//! The call pipeline: authenticate, validate, authorize with the resource, replay check, hooks,
//! handler, change record, commit. One transaction per Command; Queries read one snapshot.

use crate::core::{Core, Inner};
use crate::db::*;
use crate::error::{Category, Error, Result};
use crate::manifest::*;
use crate::plugin::*;
use crate::ports::*;
use crate::schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// How long a remembered idempotency key is kept.
const IDEMPOTENCY_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// Change records kept for Pulse resume.
pub const CHANGE_RETENTION: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallRequest {
    /// A session or token; absent for public Operations and single-user setups.
    #[serde(default)]
    pub credential: Option<String>,
    pub op: String,
    #[serde(default)]
    pub input: Value,
    /// Makes a retry of a Command safe.
    #[serde(default)]
    pub key: Option<String>,
    /// The contract hash the Face was built for; a mismatch is `incompatible`.
    #[serde(default)]
    pub contract: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallResponse {
    pub output: Value,
    /// True when this is the remembered result of an earlier call with the same key.
    pub replayed: bool,
}

impl Core {
    /// Find out who a credential belongs to.
    pub fn authenticate(&self, credential: Option<&str>) -> Result<Identity> {
        let inner = &self.inner;
        let mut found = None;
        if let Some(c) = credential {
            let now = inner.clock.now();
            inner.db.read(&mut |r| {
                found = inner.auth.authenticate(r, c, now)?;
                Ok(())
            })?;
        }
        found.or_else(|| if credential.is_none() { inner.auth.implicit() } else { None }).ok_or_else(Error::unauthenticated)
    }

    /// The whole story of one call from outside.
    pub fn call(&self, req: CallRequest) -> Result<CallResponse> {
        if let Some(h) = &req.contract {
            if h != self.contract_hash() {
                return Err(Error::new(Category::Incompatible, "kernel.incompatible", "this Face was built for a different contract; reload it")
                    .with_params(json!({"expected": self.contract_hash(), "got": h})));
            }
        }
        let decl = self.inner.op_decl(&req.op).ok_or_else(|| Error::not_found(&format!("operation {}", req.op)))?;
        let who = if decl.public { Identity { subject: "anonymous".into(), display: "anonymous".into(), roles: vec![] } } else { self.authenticate(req.credential.as_deref())? };
        self.call_as(&who, &req.op, req.input, req.key.as_deref())
    }

    /// Run an Operation as an Identity the caller already trusts (in-process Faces, tests, Listeners).
    /// Every call is counted, timed, logged and (when a trace Adapter is on) traced; none of that can fail it.
    pub fn call_as(&self, who: &Identity, op: &str, input: Value, key: Option<&str>) -> Result<CallResponse> {
        let started = std::time::Instant::now();
        let start_ms = self.inner.clock.now();
        let mut request = String::new();
        let mut res = self.call_as_inner(who, op, input, key, &mut request);
        // An internal error carries the request id as its reference, so what the caller is told to quote finds the log line.
        if let Err(e) = &mut res {
            if e.category == Category::Internal && !request.is_empty() {
                match &mut e.params {
                    Value::Null => e.params = json!({"ref": request}),
                    Value::Object(o) => {
                        o.entry("ref").or_insert(json!(request));
                    }
                    _ => {}
                }
            }
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let outcome = match &res {
            Ok(_) => "ok".to_string(),
            Err(e) => serde_json::to_value(e.category).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "error".into()),
        };
        let m = &self.inner.telemetry.metrics;
        m.inc("nuah_operations_total", &[("operation", op), ("outcome", &outcome)], 1.0);
        m.observe("nuah_operation_duration_ms", &[("operation", op)], ms);
        let req = if request.is_empty() { None } else { Some(request.as_str()) };
        match &res {
            Ok(_) => self.log(crate::telemetry::Level::Debug, "nuah.kernel", "operation.ok", json!({"operation": op, "ms": ms, "request": req})),
            Err(e) if e.category == Category::Internal => self.log_error(crate::telemetry::Level::Error, "nuah.kernel", "operation.failed", json!({"operation": op, "ms": ms}), e, req),
            Err(e) => self.log_error(crate::telemetry::Level::Debug, "nuah.kernel", "operation.refused", json!({"operation": op, "ms": ms}), e, req),
        }
        if self.inner.telemetry.wants_spans() && req.is_some_and(|r| self.inner.telemetry.sampled(r)) {
            let r = request.clone();
            self.inner.telemetry.span(crate::telemetry::Span { trace: r.clone(), id: r, name: op.into(), target: "nuah.kernel".into(), start: start_ms, duration_ms: ms, ok: res.is_ok(), attributes: json!({"outcome": outcome}) });
        }
        res
    }

    fn call_as_inner(&self, who: &Identity, op: &str, input: Value, key: Option<&str>, request_out: &mut String) -> Result<CallResponse> {
        let inner = &*self.inner;
        let decl = inner.op_decl(op).ok_or_else(|| Error::not_found(&format!("operation {op}")))?;
        let request = inner.ids.new_id();
        *request_out = request.clone();
        let input = if input.is_null() { json!({}) } else { input };
        match decl.kind {
            OpKind::Query => {
                let mut out: Option<Value> = None;
                inner.db.read(&mut |r| {
                    let (mut events, mut docs) = (vec![], vec![]);
                    let mut tx = Tx { inner, plugin: String::new(), who, request: &request, h: Handle::R(r), events: &mut events, docs: &mut docs, depth: 0 };
                    out = Some(guarded(|| inner.execute(&mut tx, op, input.clone()))?);
                    Ok(())
                })?;
                Ok(CallResponse { output: out.unwrap_or(Value::Null), replayed: false })
            }
            OpKind::Command => {
                let mut res: Option<CallResponse> = None;
                let mut committed: Option<u64> = None;
                let idem_id = key.map(|k| idem_id(&who.subject, op, k));
                let input_hash = hash_value(&input);
                inner.db.write(&mut |w| {
                    res = None;
                    committed = None;
                    let now = inner.clock.now();
                    if let Some(id) = &idem_id {
                        // Authorization comes first, so a removed member cannot read an old result by replaying a key.
                        let probe = Tx { inner, plugin: String::new(), who, request: &request, h: Handle::R(&*w), events: &mut vec![], docs: &mut vec![], depth: 0 };
                        schema::validate(&decl.input, &input)?;
                        inner.authorize(&probe, decl, &input)?;
                        drop(probe);
                        if let Some(d) = w.get("_idem", id)? {
                            if !d.deleted && d.body["expires_at"].as_i64().unwrap_or(0) > now {
                                if d.body["input"].as_str() != Some(input_hash.as_str()) {
                                    return Err(Error::reason(Category::Conflict, "kernel.idempotency_key_reused", "this key was used with different input"));
                                }
                                res = Some(CallResponse { output: d.body["output"].clone(), replayed: true });
                                return Ok(());
                            }
                        }
                    }
                    let (mut events, mut docs) = (vec![], vec![]);
                    let output = {
                        let mut tx = Tx { inner, plugin: String::new(), who, request: &request, h: Handle::W(&mut *w), events: &mut events, docs: &mut docs, depth: 0 };
                        guarded(|| inner.execute(&mut tx, op, input.clone()))?
                    };
                    if let Some(id) = &idem_id {
                        let body = json!({"input": input_hash, "output": output, "expires_at": now + IDEMPOTENCY_TTL_MS});
                        w.put("_idem", id, "kernel", body, Expect::Any, now)?;
                    }
                    if !events.is_empty() || !docs.is_empty() {
                        let seq = w.append_change(ChangeRecord { seq: 0, at: now, actor: who.subject.clone(), request: request.clone(), operation: op.into(), docs: std::mem::take(&mut docs), events: std::mem::take(&mut events) })?;
                        if seq % 100 == 0 {
                            w.prune_changes(CHANGE_RETENTION)?;
                        }
                        committed = Some(seq);
                    }
                    res = Some(CallResponse { output, replayed: false });
                    Ok(())
                })?;
                if let Some(seq) = committed {
                    let _ = inner.wake.send(seq);
                }
                Ok(res.unwrap_or(CallResponse { output: Value::Null, replayed: false }))
            }
        }
    }
}

fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => Err(Error::internal("a plugin panicked")),
    }
}

fn idem_id(subject: &str, op: &str, key: &str) -> String {
    let mut h = Sha256::new();
    h.update(format!("{subject}\u{1}{op}\u{1}{key}").as_bytes());
    format!("{:x}", h.finalize())[..32].to_string()
}

fn hash_value(v: &Value) -> String {
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(v).unwrap_or_default());
    format!("{:x}", h.finalize())[..32].to_string()
}

impl Inner {
    pub(crate) fn grant_for(&self, r: &dyn DbRead, who: &Identity, kind: &str, id: &str) -> Result<Option<Grant>> {
        let plugin = self.plan.resources.get(kind).ok_or_else(|| Error::internal(format!("unknown resource type {kind}")))?;
        let f = self.plan.active[plugin].bindings.resources.get(kind).ok_or_else(|| Error::internal("resource handler missing"))?;
        Ok(f(r, who, id)?.map(|permissions| Grant { resource_type: kind.into(), resource_id: id.into(), permissions }))
    }

    pub(crate) fn authorize(&self, tx: &Tx, decl: &OperationDecl, input: &Value) -> Result<()> {
        if decl.public {
            return Ok(());
        }
        let grant = match &decl.resource {
            None => None,
            Some(r) => {
                let id = input.get(&r.id).and_then(|v| v.as_str()).ok_or_else(|| Error::invalid(&r.id, "is required"))?;
                Some(self.grant_for(tx.rd(), tx.who, &r.kind, id)?.ok_or_else(|| Error::not_found(&r.kind))?)
            }
        };
        if let Some(perm) = &decl.permission {
            if !self.policy.allows(tx.who, perm, grant.as_ref(), &self.plan.table) {
                return Err(Error::denied(perm));
            }
        }
        Ok(())
    }

    /// Validate, authorize, run hooks and the handler, check the result, add the can-lists.
    pub(crate) fn execute(&self, tx: &mut Tx, op: &str, mut input: Value) -> Result<Value> {
        let owner = self.plan.ops.get(op).ok_or_else(|| Error::not_found(&format!("operation {op}")))?.clone();
        let decl = self.manifest(&owner).and_then(|m| m.operations.get(op)).ok_or_else(|| Error::internal("operation vanished"))?;
        if input.is_null() {
            input = json!({});
        }
        schema::validate(&decl.input, &input)?;
        self.authorize(tx, decl, &input)?;
        let is_query = decl.kind == OpKind::Query;

        for h in self.plan.hooks.iter().filter(|h| h.stage == Stage::Before && h.matches(op)) {
            let f = self.plan.active[&h.plugin].bindings.before[&h.handler].clone();
            let saved = std::mem::replace(&mut tx.plugin, h.plugin.clone());
            let r = f(tx, op, &mut input);
            tx.plugin = saved;
            r?;
            schema::validate(&decl.input, &input)?;
        }

        let arounds: Vec<(String, AroundFn)> = self
            .plan
            .hooks
            .iter()
            .filter(|h| h.stage == Stage::Around && h.matches(op))
            .map(|h| (h.plugin.clone(), self.plan.active[&h.plugin].bindings.around[&h.handler].clone()))
            .collect();
        let handler = (owner.clone(), self.plan.active[&owner].bindings.ops[op].clone());
        let ran_input = input.clone();
        let output = self.run_chain(tx, op, &arounds, &handler, input)?;

        for h in self.plan.hooks.iter().filter(|h| h.stage == Stage::After && h.matches(op)) {
            let f = self.plan.active[&h.plugin].bindings.after[&h.handler].clone();
            let saved = std::mem::replace(&mut tx.plugin, h.plugin.clone());
            let r = f(tx, op, &ran_input, &output);
            tx.plugin = saved;
            r?;
        }
        if !decl.output.is_null() {
            schema::validate(&decl.output, &output).map_err(|e| Error::internal(format!("{op} returned the wrong shape: {}", e.message)))?;
        }
        let mut output = output;
        // Nested calls get the can-list too: a plugin deciding what its caller may do reads it.
        if let Some(a) = &decl.affordances {
            self.add_can(tx, a, &mut output)?;
        }
        let _ = is_query;
        Ok(output)
    }

    fn run_chain(&self, tx: &mut Tx, op: &str, arounds: &[(String, AroundFn)], handler: &(String, OpFn), input: Value) -> Result<Value> {
        match arounds.split_first() {
            None => {
                let saved = std::mem::replace(&mut tx.plugin, handler.0.clone());
                let r = (handler.1)(tx, input);
                tx.plugin = saved;
                r
            }
            Some(((pid, f), rest)) => {
                let saved = std::mem::replace(&mut tx.plugin, pid.clone());
                let nested = !tx.is_query();
                if nested {
                    tx.wr()?.begin_nested()?;
                }
                let (ev, dc) = (tx.events.len(), tx.docs.len());
                let mut used = false;
                let mut next = |tx: &mut Tx, inp: Value| -> Result<Value> {
                    if used {
                        return Err(Error::internal("an around hook called next twice"));
                    }
                    used = true;
                    let p = tx.plugin.clone();
                    let r = self.run_chain(tx, op, rest, handler, inp);
                    tx.plugin = p;
                    r
                };
                let r = f(tx, op, input, &mut next);
                tx.plugin = saved;
                if nested {
                    let keep = r.is_ok();
                    tx.wr()?.end_nested(keep)?;
                    if !keep {
                        tx.events.truncate(ev);
                        tx.docs.truncate(dc);
                    }
                }
                r
            }
        }
    }

    fn add_can(&self, tx: &Tx, a: &Affordances, out: &mut Value) -> Result<()> {
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        let mut targets: Vec<&mut Value> = vec![];
        let mut cur = &mut *out;
        if !a.at.is_empty() {
            for part in a.at.split('.') {
                match cur.get_mut(part) {
                    Some(v) => cur = v,
                    None => return Ok(()),
                }
            }
        }
        match cur {
            Value::Array(items) => targets.extend(items.iter_mut()),
            v @ Value::Object(_) => targets.push(v),
            _ => {}
        }
        for obj in targets {
            let Some(id) = obj.get(&a.id).and_then(|v| v.as_str()).map(|s| s.to_string()) else { continue };
            if !cache.contains_key(&id) {
                let can = match self.grant_for(tx.rd(), tx.who, &a.kind, &id)? {
                    None => vec![],
                    Some(g) => g.permissions.iter().filter(|p| self.policy.allows(tx.who, p, Some(&g), &self.plan.table)).cloned().collect(),
                };
                cache.insert(id.clone(), can);
            }
            obj["can"] = json!(cache[&id]);
        }
        Ok(())
    }
}
