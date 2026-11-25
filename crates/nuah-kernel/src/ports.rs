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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub subject: String,
    pub display: String,
    pub roles: Vec<String>,
    /// The API key this call came in through, when it did ([ADR 0132]): the audit shows a key acting for a person.
    ///
    /// [ADR 0132]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=dbc2140764d64b149f93a6011782bd54
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// What the key may do, as the auth Adapter stored it. The Core reads `permissions` (a list of permission names) and
    /// refuses an Operation outside it; the product may read the rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<serde_json::Value>,
}

impl Identity {
    pub fn new(subject: &str, display: &str, roles: &[&str]) -> Self {
        Identity { subject: subject.into(), display: display.into(), roles: roles.iter().map(|r| r.to_string()).collect(), via: None, limits: None }
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
    /// Does this Adapter keep accounts that people sign in to? A plugin that offers "anyone signed in" or "these people" needs to know, because
    /// on an instance without accounts those choices mean nothing.
    fn has_accounts(&self) -> bool {
        false
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
    /// Like `fetch`, but for an answer with a status of 200 to 299 the body is handed to `on_chunk` piece by piece as it arrives
    /// instead of being collected; `on_chunk` returns whether to go on (false closes the connection), and the response then has
    /// an empty body. Any other status is returned whole, as `fetch` does, so a refusal can be read. The time limit covers the
    /// whole stream and `max_bytes` counts the streamed bytes. The default delivers the whole body as one piece, so an Adapter
    /// that cannot stream still works ([ADR 0134]).
    ///
    /// [ADR 0134]: https://madar.addaui.com/#/p/c732d1c1f2a8405da02ddc3cf964557c?tab=knowledge&page=82c570e033cd4320b15a65255dc11955
    fn fetch_stream(&self, req: &NetRequest, on_chunk: &mut dyn FnMut(&[u8]) -> bool) -> Result<NetResponse> {
        let mut r = self.fetch(req)?;
        if (200..300).contains(&r.status) {
            let body = std::mem::take(&mut r.body);
            if !body.is_empty() {
                on_chunk(&body);
            }
        }
        Ok(r)
    }
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
