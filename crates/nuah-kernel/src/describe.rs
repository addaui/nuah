// SPDX-License-Identifier: Apache-2.0
//! Self-description: one call tells a Face what exists and what the caller may do.

use crate::core::Core;
use crate::manifest::*;
use crate::ports::Identity;
use crate::resolver::ENVELOPE_VERSION;
use serde_json::{json, Map, Value};

impl Core {
    /// What a Face may know before signing in: how to sign in, and the contract version.
    pub fn describe_public(&self) -> Value {
        let inner = &*self.inner;
        let mut ops = Map::new();
        for (name, owner) in &inner.plan.ops {
            if let Some(d) = inner.manifest(owner).and_then(|m| m.operations.get(name)) {
                if d.public {
                    ops.insert(name.clone(), json!({"kind": d.kind, "description": d.description, "input": d.input, "output": d.output}));
                }
            }
        }
        json!({ "envelope": ENVELOPE_VERSION, "contract": inner.plan.hash, "operations": ops })
    }

    /// Everything a signed-in Face needs: plugins, Operations (with whether the caller's Roles allow
    /// them), Event kinds, Permissions, Themes.
    pub fn describe(&self, who: &Identity) -> Value {
        let inner = &*self.inner;
        let mut plugins = vec![];
        for (id, a) in &inner.plan.active {
            let m = &a.manifest;
            plugins.push(json!({"id": id, "name": m.name, "version": m.version, "kind": m.kind(), "quality": m.quality, "runtime": m.runtime, "description": m.description, "state": "active"}));
        }
        for (id, reason) in &inner.plan.inactive {
            plugins.push(json!({"id": id, "state": "inactive", "reason": reason}));
        }
        let mut ops = Map::new();
        for (name, owner) in &inner.plan.ops {
            let d = &inner.manifest(owner).unwrap().operations[name];
            let mut reasons = d.reasons.clone();
            for h in inner.plan.hooks.iter().filter(|h| h.matches(name)) {
                if let Some(hd) = inner.manifest(&h.plugin).and_then(|m| m.hooks.iter().find(|x| x.handler == h.handler)) {
                    reasons.extend(hd.reasons.clone());
                }
            }
            let allowed = match (&d.permission, d.resource.is_some()) {
                (Some(p), false) => inner.policy.allows(who, p, None, &inner.plan.table),
                _ => true, // decided per resource
            };
            ops.insert(name.clone(), json!({
                "plugin": owner, "kind": d.kind, "description": d.description, "permission": d.permission,
                "public": d.public, "resource": d.resource, "input": d.input, "output": d.output,
                "reasons": reasons, "allowed": allowed,
            }));
        }
        let mut events = Map::new();
        for (kind, owner) in &inner.plan.events {
            let d = &inner.manifest(owner).unwrap().events[kind];
            events.insert(kind.clone(), json!({"plugin": owner, "description": d.description, "data": d.data}));
        }
        let mut perms = Map::new();
        for (p, roles) in &inner.plan.table {
            perms.insert(p.clone(), json!({"roles": roles, "held": roles.iter().any(|r| who.has_role(r))}));
        }
        let themes: Vec<Value> = inner.plan.active.iter().filter_map(|(id, a)| a.manifest.theme.as_ref().map(|t| json!({"id": id, "name": a.manifest.name, "tokens": t}))).collect();
        let points: Map<String, Value> = inner.plan.ext_points.iter().map(|(n, (owner, d))| {
            let providers: Vec<&String> = inner.plan.providers.get(n).map(|v| v.iter().map(|(_, p)| p).collect()).unwrap_or_default();
            (n.clone(), json!({"owner": owner, "policy": d.policy, "providers": providers}))
        }).collect();
        // Slot items the caller can use: the Operation exists and the caller's Roles allow it.
        let mut slots: Vec<Value> = vec![];
        for (id, a) in &inner.plan.active {
            for f in &a.manifest.slots {
                if ops.get(&f.operation).and_then(|o| o["allowed"].as_bool()) == Some(true) {
                    slots.push(json!({
                        "slot": f.slot, "id": f.id, "plugin": id, "label": f.label, "operation": f.operation,
                        "args": f.args, "result": f.result, "file": f.file, "order": f.order,
                    }));
                }
            }
        }
        slots.sort_by(|a, b| (a["slot"].as_str(), a["order"].as_i64(), a["id"].as_str()).cmp(&(b["slot"].as_str(), b["order"].as_i64(), b["id"].as_str())));
        json!({
            "envelope": ENVELOPE_VERSION, "contract": inner.plan.hash, "profile": inner.profile_name,
            "who": who, "plugins": plugins, "operations": ops, "events": events,
            "permissions": perms, "themes": themes, "extension_points": points, "slots": slots,
        })
    }
}

#[allow(dead_code)]
fn _kind_is_used(m: &Manifest) -> PluginKind {
    m.kind()
}
