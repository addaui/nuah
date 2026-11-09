// SPDX-License-Identifier: Apache-2.0
//! The fixed Ports: db, auth, policy, clock, ids, net (one Adapter each) and the telemetry Ports log, trace, metric,
//! heartbeat (every enabled Adapter). A Port needs an Adapter only when an enabled plugin needs the Port
//! ([ADR 0039]); the kernel fills an unneeded one with an inert stand-in.
//!
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

use crate::db::{DbRead, Millis};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Who is making a request, as verified by the auth Adapter.
///
/// `subject` is the one key every other plugin stores about a person. The auth Port's contract ([ADR 0039]):
/// it is opaque, made once when the person is first known, never reused and never changed when their login
/// name or display name does. An Adapter that takes over from another (OIDC after passwords) maps its own
/// identity onto the subject already in use, so what other plugins stored keeps pointing at the same person.
///
/// [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub subject: String,
    pub display: String,
    pub roles: Vec<String>,
}

impl Identity {
    pub fn new(subject: &str, display: &str, roles: &[&str]) -> Self {
        Identity { subject: subject.into(), display: display.into(), roles: roles.iter().map(|r| r.to_string()).collect() }
    }
    pub fn has_role(&self, r: &str) -> bool {
        self.roles.iter().any(|x| x == r)
    }
}

pub trait Auth: Send + Sync {
    fn name(&self) -> &str;
    /// Who does this credential belong to? `None` means nobody (unknown, expired). Sessions live in
    /// data the Adapter owns, so it reads through the handle it is given.
    fn authenticate(&self, db: &dyn DbRead, credential: &str, now: Millis) -> Result<Option<Identity>>;
    /// Find a person by the name they sign in with, so another plugin can refer to them (share a project with
    /// "bob") without reading the Adapter's data. The answer carries the stable `subject`. An Adapter with no
    /// such names (a single-user one) answers `None`.
    fn find(&self, _db: &dyn DbRead, _name: &str) -> Result<Option<Identity>> {
        Ok(None)
    }
    /// May an anonymous caller be treated as signed in without a credential (single-user setups)?
    fn implicit(&self) -> Option<Identity> {
        None
    }
}

/// What the Subject holds on the resource being acted on, as answered by the plugin that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub resource_type: String,
    pub resource_id: String,
    pub permissions: Vec<String>,
}

/// Permission -> Roles holding it by default, built from manifests and the Profile.
pub type PermissionTable = BTreeMap<String, Vec<String>>;

pub trait Policy: Send + Sync {
    fn name(&self) -> &str;
    /// May `who` use `permission`, given what they hold on the resource (if there is one)?
    fn allows(&self, who: &Identity, permission: &str, grant: Option<&Grant>, table: &PermissionTable) -> bool;
}

pub trait Clock: Send + Sync {
    fn now(&self) -> Millis;
}

pub trait Ids: Send + Sync {
    fn new_id(&self) -> String;
}

/// A request to the outside world ([ADR 0040]). Which addresses may be reached, and how, is the Adapter's rule.
///
/// [ADR 0040]: https://docs.addaui.com/nuah/adr/0040-outbound-operations-run-outside-the-transaction-and-reach-the-network-through-a-net-port/
#[derive(Debug, Clone)]
pub struct NetRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// The answer's body is cut after this many bytes.
    pub max_bytes: usize,
    pub timeout_ms: u64,
}

impl NetRequest {
    pub fn get(url: &str) -> Self {
        NetRequest { method: "GET".into(), url: url.into(), headers: vec![], body: None, max_bytes: 1 << 20, timeout_ms: 5000 }
    }
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct NetResponse {
    pub status: u16,
    /// Where the answer came from after redirects.
    pub url: String,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The body was longer than `max_bytes`.
    pub truncated: bool,
}

impl NetResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

pub trait Net: Send + Sync {
    fn name(&self) -> &str;
    /// Blocks until there is an answer or the time limit passes. A status of 400 or more is an answer, not an error.
    fn fetch(&self, req: &NetRequest) -> Result<NetResponse>;
}

/// How many Adapters fill one Port ([ADR 0031]). Whether a Port may have several is a property of the
/// Port, not a choice of the Profile.
///
/// [ADR 0031]: https://docs.addaui.com/nuah/adr/0031-products-add-to-nuah-and-never-edit-it-ports-declare-cardinality-projections-are-plugins/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    /// Exactly one Adapter; the Profile names it when several are enabled.
    Single,
    /// Every enabled Adapter is active; the routing rule says how a signal reaches them.
    Many(Routing),
}

/// How a `many` Port hands what it receives to its Adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Routing {
    /// Every Adapter receives everything and applies its own filter (the telemetry Ports).
    Broadcast,
}

#[derive(Debug, Clone, Copy)]
pub struct PortSpec {
    pub name: &'static str,
    pub cardinality: Cardinality,
}

const fn spec(name: &'static str, cardinality: Cardinality) -> PortSpec {
    PortSpec { name, cardinality }
}

pub const PORT_SPECS: [PortSpec; 10] = [
    spec("db", Cardinality::Single),
    spec("auth", Cardinality::Single),
    spec("policy", Cardinality::Single),
    spec("clock", Cardinality::Single),
    spec("ids", Cardinality::Single),
    spec("net", Cardinality::Single),
    spec("log", Cardinality::Many(Routing::Broadcast)),
    spec("trace", Cardinality::Many(Routing::Broadcast)),
    spec("metric", Cardinality::Many(Routing::Broadcast)),
    spec("heartbeat", Cardinality::Many(Routing::Broadcast)),
];

pub const PORTS: [&str; 10] = ["db", "auth", "policy", "clock", "ids", "net", "log", "trace", "metric", "heartbeat"];

/// How many Adapters fill this Port; `None` for a name that is not a Port.
pub fn cardinality(port: &str) -> Option<Cardinality> {
    PORT_SPECS.iter().find(|s| s.name == port).map(|s| s.cardinality)
}
