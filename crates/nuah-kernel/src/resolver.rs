// SPDX-License-Identifier: Apache-2.0
//! The Resolver: reads Manifests and a Profile and reports *every* problem at once, without starting
//! anything (no Adapter opened, no data touched). Problems carry their code and the plugins involved.

use crate::manifest::*;
use crate::plugin::{Bindings, Plugin};
use crate::ports::{cardinality, Cardinality, PORTS};
use crate::schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// One set of choices for a Distribution: which plugins are on, which Adapter fills each Port, the
/// Settings of each plugin, the Capabilities granted to each, and changes to default Permission grants.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// The contract hash this file was last saved against (like a lockfile). A different hash at start
    /// means a plugin upgrade changed the API under this file; the host warns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    pub enabled: Vec<String>,
    /// Port -> Adapter plugin id. Optional when exactly one enabled Adapter fills the Port.
    pub bindings: BTreeMap<String, String>,
    pub settings: BTreeMap<String, Value>,
    /// Plugin -> Capabilities the operator grants it ([ADR 0006]).
    ///
    /// [ADR 0006]: https://docs.addaui.com/nuah/adr/0006-the-operator-grants-capabilities-in-the-profile/
    pub grants: BTreeMap<String, Vec<String>>,
    /// Permission -> Roles, replacing the Manifest's default.
    pub permissions: BTreeMap<String, Vec<String>>,
    /// Levels, trace sampling and queue size of the telemetry Ports ([ADR 0032]).
    ///
    /// [ADR 0032]: https://docs.addaui.com/nuah/adr/0032-telemetry-is-four-optional-fan-out-ports-the-core-decides-what-to-record-adapters-decide-how-it-looks-and-where-it-goes/
    pub telemetry: crate::telemetry::TelemetrySettings,
}

/// Every plugin available to be switched on.
#[derive(Clone, Default)]
pub struct Catalog(pub Vec<Arc<dyn Plugin>>);

#[derive(Clone)]
pub struct Distribution {
    pub name: String,
    pub catalog: Catalog,
    pub profile: Profile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub plugins: Vec<String>,
    pub message: String,
}

fn problem(code: &str, plugins: &[&str], message: String) -> Problem {
    Problem { code: code.into(), plugins: plugins.iter().map(|p| p.to_string()).collect(), message }
}

#[derive(Debug, Clone)]
pub struct HookEntry {
    pub stage: Stage,
    pub on: String,
    pub order: i32,
    pub plugin: String,
    pub handler: String,
}

impl HookEntry {
    pub fn matches(&self, op: &str) -> bool {
        match self.on.strip_suffix(".*") {
            Some(prefix) => op.starts_with(&format!("{prefix}.")),
            None => self.on == "*" || self.on == op,
        }
    }
}

pub struct ActivePlugin {
    pub plugin: Arc<dyn Plugin>,
    pub manifest: Manifest,
    pub bindings: Bindings,
}

/// Everything the Resolver worked out.
#[derive(Default)]
pub struct Plan {
    /// Active plugins by id.
    pub active: BTreeMap<String, ActivePlugin>,
    /// Enabled but not running, with the reason.
    pub inactive: BTreeMap<String, String>,
    /// Port -> Adapter plugin id, for the Ports that take one Adapter.
    pub ports: BTreeMap<String, String>,
    /// Port -> every enabled Adapter, for the Ports that take several.
    pub many: BTreeMap<String, Vec<String>>,
    pub ops: BTreeMap<String, String>,
    pub hooks: Vec<HookEntry>,
    pub events: BTreeMap<String, String>,
    pub scopes: BTreeMap<String, String>,
    pub resources: BTreeMap<String, String>,
    pub ext_points: BTreeMap<String, (String, ExtensionPointDef)>,
    pub providers: BTreeMap<String, Vec<(i32, String)>>,
    pub table: BTreeMap<String, Vec<String>>,
    pub settings: BTreeMap<String, Value>,
    pub grants: BTreeMap<String, Vec<String>>,
    pub telemetry: crate::telemetry::TelemetrySettings,
    pub hash: String,
}

/// `serve`: the plugin runs a long-lived service (a Projection, for one). See ADR 0031. `net`: an outbound Operation may use the `net` Port (ADR 0040).
const CAPABILITIES: [&str; 5] = ["emit", "call", "serve", "net", "host"];

/// The hash of the operation and schema set, so a Face can tell it was built for a different Core.
pub const ENVELOPE_VERSION: u32 = 1;

pub fn resolve(dist: &Distribution) -> (Vec<Problem>, Plan) {
    let mut ps: Vec<Problem> = vec![];
    let mut plan = Plan::default();
    // References to the environment or a file are replaced by their value; the Profile itself holds no secret.
    let resolved = crate::setup::resolve_profile_refs(&dist.profile, &mut ps);
    let p = &resolved;
    for m in p.telemetry.problems() {
        ps.push(problem("bad_telemetry", &[], m));
    }
    plan.telemetry = p.telemetry.clone();

    // 1. the catalog
    let mut catalog: BTreeMap<String, Arc<dyn Plugin>> = BTreeMap::new();
    for pl in &dist.catalog.0 {
        let id = pl.manifest().id;
        if catalog.insert(id.clone(), pl.clone()).is_some() {
            ps.push(problem("duplicate_plugin", &[&id], format!("two plugins in the catalog are called {id}")));
        }
    }

    // 2. what the profile enables
    let mut enabled: Vec<(String, Arc<dyn Plugin>, Manifest)> = vec![];
    let mut seen = BTreeSet::new();
    for id in &p.enabled {
        if !seen.insert(id.clone()) {
            continue;
        }
        match catalog.get(id) {
            None => ps.push(problem("unknown_plugin", &[id], format!("the profile enables {id}, which is not in the catalog"))),
            Some(pl) => {
                let mut m = pl.manifest();
                normalize(&mut m);
                enabled.push((id.clone(), pl.clone(), m));
            }
        }
    }
    for id in p.settings.keys().chain(p.grants.keys()) {
        if !seen.contains(id) {
            ps.push(problem("settings_for_disabled_plugin", &[id], format!("the profile configures {id}, which is not enabled")));
        }
    }
    let ids: BTreeSet<&str> = enabled.iter().map(|(i, _, _)| i.as_str()).collect();

    // 3. namespaces, collections and names each plugin owns
    let mut ns_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut coll_owner: BTreeMap<String, String> = BTreeMap::new();
    for (id, _, m) in &enabled {
        for ns in &m.namespaces {
            if let Some(other) = ns_owner.insert(ns.clone(), id.clone()) {
                ps.push(problem("namespace_clash", &[&other, id], format!("{other} and {id} both claim the namespace {ns}")));
            }
        }
        for c in &m.data.collections {
            if let Some(other) = coll_owner.insert(c.clone(), id.clone()) {
                ps.push(problem("collection_clash", &[&other, id], format!("{other} and {id} both own the collection {c}")));
            }
            if c.starts_with('_') {
                ps.push(problem("reserved_name", &[id], format!("{id}: collections starting with _ belong to the kernel ({c})")));
            }
        }
    }

    let mut resource_kinds: BTreeMap<String, String> = BTreeMap::new();
    let mut permission_owner: BTreeMap<String, String> = BTreeMap::new();
    let mut slot_item_owner: BTreeMap<String, String> = BTreeMap::new();

    for (id, _, m) in &enabled {
        let needs_ns = !m.operations.is_empty()
            || !m.events.is_empty()
            || !m.permissions.is_empty()
            || !m.data.collections.is_empty()
            || !m.scopes.is_empty()
            || !m.resources.is_empty()
            || !m.defines.extension_points.is_empty()
            || !m.slots.is_empty();
        if needs_ns && m.namespaces.is_empty() {
            ps.push(problem("no_namespace", &[id], format!("{id} contributes names but declares no namespace")));
        }
        let own = |what: &str, name: &str, ps: &mut Vec<Problem>| {
            if !m.owns_name(name) {
                ps.push(problem("name_outside_namespace", &[id], format!("{id}: {what} {name} is not under one of its namespaces {:?}", m.namespaces)));
            }
        };
        for c in &m.data.collections {
            own("collection", c, &mut ps);
        }
        for n in m.operations.keys() {
            own("operation", n, &mut ps);
            if let Some(other) = plan.ops.insert(n.clone(), id.clone()) {
                ps.push(problem("duplicate_operation", &[&other, id], format!("{other} and {id} both declare the operation {n}")));
            }
        }
        for f in &m.slots {
            own("slot item", &f.id, &mut ps);
            if let Some(other) = slot_item_owner.insert(f.id.clone(), id.clone()) {
                ps.push(problem("duplicate_slot_item", &[&other, id], format!("{other} and {id} both declare the slot item {}", f.id)));
            }
        }
        for n in m.events.keys() {
            own("event kind", n, &mut ps);
            if let Some(other) = plan.events.insert(n.clone(), id.clone()) {
                ps.push(problem("duplicate_event_kind", &[&other, id], format!("{other} and {id} both declare the event kind {n}")));
            }
        }
        for n in m.permissions.keys() {
            own("permission", n, &mut ps);
            permission_owner.insert(n.clone(), id.clone());
        }
        for n in &m.scopes {
            own("scope", n, &mut ps);
            plan.scopes.insert(n.clone(), id.clone());
        }
        for n in &m.resources {
            own("resource type", n, &mut ps);
            resource_kinds.insert(n.clone(), id.clone());
            plan.resources.insert(n.clone(), id.clone());
        }
        for (n, d) in &m.defines.extension_points {
            own("extension point", n, &mut ps);
            plan.ext_points.insert(n.clone(), (id.clone(), d.clone()));
        }
        for (op, d) in &m.operations {
            for r in d.reasons.keys() {
                own("reason code", r, &mut ps);
            }
            let _ = op;
        }
        for l in &m.listeners {
            own("listener", &l.id, &mut ps);
        }
        for h in &m.hooks {
            own("hook handler", &h.handler, &mut ps);
            for r in h.reasons.keys() {
                own("reason code", r, &mut ps);
            }
        }
    }

    // 4. references between declarations
    for (id, _, m) in &enabled {
        for (op, d) in &m.operations {
            if let Some(perm) = &d.permission {
                if !permission_owner.contains_key(perm) {
                    ps.push(problem("unknown_permission", &[id], format!("{op} needs permission {perm}, which no enabled plugin declares")));
                }
            }
            if let Some(r) = &d.resource {
                if !resource_kinds.contains_key(&r.kind) {
                    ps.push(problem("unknown_resource", &[id], format!("{op} acts on resource type {}, which no enabled plugin answers for", r.kind)));
                }
            }
            if let Some(a) = &d.affordances {
                if !resource_kinds.contains_key(&a.kind) {
                    ps.push(problem("unknown_resource", &[id], format!("{op} reports can-lists for resource type {}, which no enabled plugin answers for", a.kind)));
                }
            }
            if d.kind == OpKind::Outbound {
                if d.affordances.is_some() {
                    ps.push(problem("bad_operation", &[id], format!("{op} is outbound, so it cannot report can-lists (it holds no transaction to ask in)")));
                }
                for h in plan_hooks_on(&enabled, op) {
                    ps.push(problem("hook_on_outbound", &[id, &h.0], format!("{} hooks {op} at stage {:?}; an outbound Operation takes only `before` hooks", h.0, h.1)));
                }
            }
            if d.public && (d.permission.is_some() || d.resource.is_some()) {
                ps.push(problem("bad_operation", &[id], format!("{op} is public, so it cannot also need a permission or resource")));
            }
        }
        for dep in &m.requires.plugins {
            if !ids.contains(dep.as_str()) {
                ps.push(problem("missing_dependency", &[id, dep], format!("{id} needs {dep}, which is not enabled")));
            }
        }
        for port in &m.requires.ports {
            if !PORTS.contains(&port.as_str()) {
                ps.push(problem("unknown_port", &[id], format!("{id} requires the port {port}, which does not exist")));
            }
        }
        for c in &m.requires.capabilities {
            if !CAPABILITIES.contains(&c.as_str()) {
                ps.push(problem("unknown_capability", &[id], format!("{id} asks for the capability {c}, which does not exist")));
            } else if !p.grants.get(id).is_some_and(|g| g.contains(c)) {
                ps.push(problem("capability_not_granted", &[id], format!("{id} asks for the capability {c}; the profile does not grant it")));
            }
        }
        for g in p.grants.get(id).into_iter().flatten() {
            if !m.requires.capabilities.contains(g) {
                ps.push(problem("capability_not_requested", &[id], format!("the profile grants {id} the capability {g}, which its manifest never asked for")));
            }
        }
        for point in &m.requires.extension_points {
            match plan.ext_points.get(point) {
                None => ps.push(problem("unknown_extension_point", &[id], format!("{id} uses the extension point {point}, which no enabled plugin defines"))),
                Some((owner, _)) if !m.requires.plugins.contains(owner) && owner != id => {
                    ps.push(problem("missing_dependency", &[id, owner], format!("{id} uses {point}, so it must list {owner} in requires.plugins")))
                }
                _ => {}
            }
        }
        for pr in &m.provides {
            if !plan.ext_points.contains_key(&pr.extension_point) {
                if pr.optional {
                    continue;
                }
                ps.push(problem("unknown_extension_point", &[id], format!("{id} provides {}, which no enabled plugin defines", pr.extension_point)));
            } else {
                plan.providers.entry(pr.extension_point.clone()).or_default().push((pr.priority, id.clone()));
            }
        }
        for r in &m.data.reads {
            if !coll_owner.contains_key(r) {
                ps.push(problem("unknown_collection", &[id], format!("{id} reads the collection {r}, which no enabled plugin owns")));
            }
        }
        if !m.settings_schema.is_null() {
            let settings = p.settings.get(id).cloned().unwrap_or_else(|| json!({}));
            if let Err(e) = schema::validate(&m.settings_schema, &settings) {
                ps.push(problem("invalid_settings", &[id], format!("settings of {id}: {}", e.message)));
            }
        }
        if m.runtime == Runtime::Data && (!m.operations.is_empty() || !m.hooks.is_empty() || !m.listeners.is_empty()) {
            ps.push(problem("data_plugin_runs_code", &[id], format!("{id} is a data plugin but declares code")));
        }
    }
    for (point, provs) in &plan.providers {
        if plan.ext_points.get(point).is_some_and(|(_, d)| d.policy == CallPolicy::Single) && provs.len() > 1 {
            let names: Vec<&str> = provs.iter().map(|(_, i)| i.as_str()).collect();
            ps.push(problem("extension_point_ambiguous", &names, format!("{point} takes one provider, and {} are enabled", provs.len())));
        }
    }
    for (id, _, m) in &enabled {
        for point in &m.requires.extension_points {
            if plan.ext_points.get(point).is_some_and(|(_, d)| d.policy == CallPolicy::Single) && !plan.providers.contains_key(point) {
                ps.push(problem("extension_point_unfilled", &[id], format!("{id} needs {point}, and no enabled plugin provides it")));
            }
        }
    }

    // 4b. Slot items name an Operation and give it everything it needs
    for (id, _, m) in &enabled {
        for f in &m.slots {
            check_slot_item(id, f, &enabled, &mut ps);
        }
    }

    // 5. ports
    let mut fillers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, _, m) in &enabled {
        if let Some(port) = &m.fills {
            if !PORTS.contains(&port.as_str()) {
                ps.push(problem("unknown_port", &[id], format!("{id} fills the port {port}, which does not exist")));
            } else {
                fillers.entry(port.as_str()).or_default().push(id.as_str());
            }
        }
    }
    for (port, plugin) in &p.bindings {
        let ok = fillers.get(port.as_str()).is_some_and(|f| f.contains(&plugin.as_str()));
        if !PORTS.contains(&port.as_str()) {
            ps.push(problem("unknown_port", &[plugin], format!("the profile binds the port {port}, which does not exist")));
        } else if !ok {
            ps.push(problem("bad_binding", &[plugin], format!("the profile binds {port} to {plugin}, which is not an enabled adapter for it")));
        }
    }
    // A Port is needed only by the plugins that say so (or whose declarations imply it).
    let mut needers: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, _, m) in &enabled {
        for port in implied_ports(m) {
            needers.entry(port).or_default().push(id.as_str());
        }
    }
    for port in PORTS {
        if let Some(Cardinality::Many(_)) = cardinality(port) {
            if let Some(b) = p.bindings.get(port) {
                ps.push(problem("port_many_bound", &[b], format!("the port {port} feeds every enabled adapter; remove it from the profile's bindings")));
            }
            match fillers.get(port) {
                Some(f) => {
                    plan.many.insert(port.into(), f.iter().map(|s| s.to_string()).collect());
                }
                None => {
                    if let Some(who) = needers.get(port) {
                        ps.push(problem("port_unfilled", who, format!("{} need the port {port}, and no enabled adapter fills it", who.join(", "))))
                    }
                }
            }
            continue;
        }
        match (p.bindings.get(port), fillers.get(port)) {
            (Some(b), _) => {
                plan.ports.insert(port.into(), b.clone());
            }
            (None, None) => {
                if let Some(who) = needers.get(port) {
                    ps.push(problem("port_unfilled", who, format!("{} need the port {port}, and no enabled adapter fills it", who.join(", "))))
                }
            }
            (None, Some(f)) if f.len() == 1 => {
                plan.ports.insert(port.into(), f[0].to_string());
            }
            (None, Some(f)) => ps.push(problem("port_ambiguous", f, format!("{} adapters fill {port}; the profile must choose one", f.len()))),
        }
    }
    for (id, _, m) in &enabled {
        if let Some(port) = &m.fills {
            if plan.ports.get(port).is_some_and(|b| b != id) {
                plan.inactive.insert(id.clone(), format!("the port {port} went to {}", plan.ports[port]));
            }
        }
    }

    // 6. handlers named by manifests are bound, and nothing else is
    let mut bound: BTreeMap<String, Bindings> = BTreeMap::new();
    for (id, pl, m) in &enabled {
        if m.runtime == Runtime::Data {
            bound.insert(id.clone(), Bindings::default());
            continue;
        }
        let mut b = Bindings::default();
        pl.bind(&mut b);
        let miss = |ps: &mut Vec<Problem>, what: &str, name: &str| ps.push(problem("unbound_handler", &[id], format!("{id} declares {what} {name} but binds no handler for it")));
        let extra = |ps: &mut Vec<Problem>, what: &str, name: &str| ps.push(problem("extra_handler", &[id], format!("{id} binds a handler for {what} {name} that its manifest does not declare")));
        let outbound = |n: &String| m.operations.get(n).is_some_and(|d| d.kind == OpKind::Outbound);
        for n in m.operations.keys() {
            if !(if outbound(n) { b.outbound.contains_key(n) } else { b.ops.contains_key(n) }) {
                miss(&mut ps, "operation", n);
            }
        }
        for n in b.ops.keys() {
            if !m.operations.contains_key(n) || outbound(n) {
                extra(&mut ps, "operation", n);
            }
        }
        for n in b.outbound.keys() {
            if !outbound(n) {
                extra(&mut ps, "outbound operation", n);
            }
        }
        let hook_names: BTreeSet<&str> = m.hooks.iter().map(|h| h.handler.as_str()).collect();
        for h in &m.hooks {
            let found = match h.stage {
                Stage::Before => b.before.contains_key(&h.handler),
                Stage::Around => b.around.contains_key(&h.handler),
                Stage::After => b.after.contains_key(&h.handler),
            };
            if !found {
                miss(&mut ps, "hook", &h.handler);
            }
        }
        for n in b.before.keys().chain(b.around.keys()).chain(b.after.keys()) {
            if !hook_names.contains(n.as_str()) {
                extra(&mut ps, "hook", n);
            }
        }
        for l in &m.listeners {
            if !b.listeners.contains_key(&l.id) {
                miss(&mut ps, "listener", &l.id);
            }
        }
        for n in b.listeners.keys() {
            if !m.listeners.iter().any(|l| &l.id == n) {
                extra(&mut ps, "listener", n);
            }
        }
        for s in &m.scopes {
            if !b.scopes.contains_key(s) {
                miss(&mut ps, "scope", s);
            }
        }
        for s in b.scopes.keys() {
            if !m.scopes.contains(s) {
                extra(&mut ps, "scope", s);
            }
        }
        for r in &m.resources {
            if !b.resources.contains_key(r) {
                miss(&mut ps, "resource type", r);
            }
        }
        for r in b.resources.keys() {
            if !m.resources.contains(r) {
                extra(&mut ps, "resource type", r);
            }
        }
        for pr in &m.provides {
            if !b.provides.contains_key(&pr.extension_point) {
                miss(&mut ps, "extension point", &pr.extension_point);
            }
        }
        for n in b.provides.keys() {
            if !m.provides.iter().any(|x| &x.extension_point == n) {
                extra(&mut ps, "extension point", n);
            }
        }
        bound.insert(id.clone(), b);
    }

    // 7. hooks, permission table, settings, hash
    for (id, _, m) in &enabled {
        for h in &m.hooks {
            plan.hooks.push(HookEntry { stage: h.stage, on: h.on.clone(), order: h.order, plugin: id.clone(), handler: h.handler.clone() });
        }
        for (perm, d) in &m.permissions {
            plan.table.insert(perm.clone(), d.default.clone());
        }
        if let Some(s) = p.settings.get(id) {
            plan.settings.insert(id.clone(), s.clone());
        }
        if let Some(g) = p.grants.get(id) {
            plan.grants.insert(id.clone(), g.clone());
        }
    }
    for (perm, roles) in &p.permissions {
        if !plan.table.contains_key(perm) {
            ps.push(problem("unknown_permission", &[], format!("the profile changes the grants of {perm}, which no enabled plugin declares")));
        } else {
            plan.table.insert(perm.clone(), roles.clone());
        }
    }
    plan.hooks.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.plugin.cmp(&b.plugin)).then_with(|| a.handler.cmp(&b.handler)));

    let mut parts: BTreeMap<&str, Value> = BTreeMap::new();
    for (id, _, m) in &enabled {
        if !plan.inactive.contains_key(id) {
            parts.insert(id.as_str(), m.contract_part());
        }
    }
    let mut h = Sha256::new();
    h.update(format!("envelope:{ENVELOPE_VERSION};").as_bytes());
    h.update(serde_json::to_vec(&parts).unwrap_or_default());
    plan.hash = format!("{:x}", h.finalize())[..16].to_string();

    for (id, pl, m) in enabled {
        if plan.inactive.contains_key(&id) {
            continue;
        }
        let b = bound.remove(&id).unwrap_or_default();
        plan.active.insert(id, ActivePlugin { plugin: pl, manifest: m, bindings: b });
    }
    // Inactive plugins contribute nothing.
    plan.ops.retain(|_, pid| plan.active.contains_key(pid));
    plan.events.retain(|_, pid| plan.active.contains_key(pid));
    (ps, plan)
}

/// Fill in defaults so the rest of the kernel can rely on them.
/// `$name` or `$name.path.to.field`: where a Slot item's argument comes from.
fn is_context_ref(s: &str) -> bool {
    s.strip_prefix('$').is_some_and(|r| !r.is_empty() && r.split('.').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')))
}

fn check_slot_item(plugin: &str, f: &SlotFill, enabled: &[(String, Arc<dyn Plugin>, Manifest)], ps: &mut Vec<Problem>) {
    let bad = |ps: &mut Vec<Problem>, why: String| ps.push(problem("bad_slot", &[plugin], format!("{plugin}: slot item {}: {why}", f.id)));
    let named = |s: &str| !s.is_empty() && s.split('.').all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
    if !named(&f.slot) {
        bad(ps, format!("the slot name {:?} must be lowercase words joined by dots", f.slot));
    }
    if f.label.get("en").map_or(true, |l| l.trim().is_empty()) {
        bad(ps, "needs an English label (`label.en`)".into());
    }
    for (name, value) in &f.args {
        if let Some(r) = value.as_str().filter(|r| r.starts_with('$')) {
            if !is_context_ref(r) {
                bad(ps, format!("argument {name} reads {r:?}, which is not `$name` or `$name.path`"));
            }
        }
    }
    if f.result == SlotResult::Download && f.file.as_deref().map_or(true, |t| t.trim().is_empty()) {
        bad(ps, "a download needs a `file` name".into());
    }
    let Some(op) = enabled.iter().find_map(|(_, _, m)| m.operations.get(&f.operation)) else {
        bad(ps, format!("names the operation {}, which no enabled plugin declares", f.operation));
        return;
    };
    // The item must give the Operation what it requires, and nothing it does not know.
    let required: Vec<&str> = op.input["required"].as_array().map(|r| r.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    for r in required {
        if !f.args.contains_key(r) {
            bad(ps, format!("{} requires {r}, which the item does not provide", f.operation));
        }
    }
    if let Some(props) = op.input["properties"].as_object() {
        for name in f.args.keys() {
            if !props.contains_key(name) {
                bad(ps, format!("{} has no input called {name}", f.operation));
            }
        }
    }
}

fn normalize(m: &mut Manifest) {
    for d in m.operations.values_mut() {
        if d.input.is_null() {
            d.input = json!({"type":"object","additionalProperties":false});
        }
    }
}

/// The Ports a plugin needs: the ones it lists in `requires.ports`, plus the ones its declarations imply
/// (owning collections needs `db`; an operation that is not public needs `auth` and `policy`).
pub(crate) fn implied_ports(m: &Manifest) -> BTreeSet<&'static str> {
    let mut out: BTreeSet<&'static str> = BTreeSet::new();
    for want in &m.requires.ports {
        if let Some(name) = PORTS.iter().find(|p| *p == want) {
            out.insert(name);
        }
    }
    if !m.data.collections.is_empty() || !m.data.reads.is_empty() || !m.data.requires.is_empty() || !m.listeners.is_empty() {
        out.insert("db");
    }
    if m.requires.capabilities.iter().any(|c| c == "net") {
        out.insert("net");
    }
    if m.operations.values().any(|d| !d.public) {
        out.insert("auth");
        out.insert("policy");
    }
    out
}

/// The (plugin, stage) of every `around` or `after` hook that names the Operation exactly. A wildcard hook
/// (`notes.*`, `*`) simply does not apply to an outbound Operation, which has only `before` hooks.
fn plan_hooks_on(enabled: &[(String, Arc<dyn Plugin>, Manifest)], op: &str) -> Vec<(String, Stage)> {
    let mut v = vec![];
    for (id, _, m) in enabled {
        for h in &m.hooks {
            if h.stage != Stage::Before && h.on == op {
                v.push((id.clone(), h.stage));
            }
        }
    }
    v
}
