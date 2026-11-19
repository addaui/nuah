// SPDX-License-Identifier: Apache-2.0
//! The Manifest ([ADR 0004]): everything a plugin says about itself without running any code.
//! Operations, hooks, listeners, Events, permissions and data are declared here; code only attaches a
//! handler to each declared name.
//!
//! [ADR 0004]: https://docs.addaui.com/nuah/adr/0004-manifest-declares-everything-names-are-namespaced/

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::db::IndexSpec;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    #[default]
    Native,
    /// Data only: runs no code (themes).
    Data,
}

/// Who can reach an instance ([ADR 0087]): the operator says it in the Profile, a plugin declares how far it is safe.
/// Ordered from narrowest to widest.
///
/// [ADR 0087]: https://docs.addaui.com/nuah/adr/0087-the-operator-says-who-can-reach-the-instance-plugins-declare-how-far-they-are-safe-and-going-past-it-is-allowed-once-acknowledged/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// Only the computer it runs on.
    #[default]
    ThisComputer,
    /// People on the same network.
    Network,
    /// Anyone on the internet.
    Internet,
}

impl Reach {
    /// What an address to listen on says by itself: loopback is this computer, anything else is a network. A program in a
    /// container listens off loopback whatever the host does with the port, so the operator's own answer (the Profile's
    /// `reach`) wins over this.
    pub fn of_listen(addr: &str) -> Reach {
        if addr.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback()) {
            Reach::ThisComputer
        } else {
            Reach::Network
        }
    }

    /// What the operator would call it, for messages.
    pub fn words(self) -> &'static str {
        match self {
            Reach::ThisComputer => "only this computer",
            Reach::Network => "people on your network",
            Reach::Internet => "anyone on the internet",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    #[default]
    Reference,
    Bronze,
    Silver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Adapter,
    Provider,
    Theme,
    /// Only font data: families a Face may draw with ([ADR 0090]).
    ///
    /// [ADR 0090]: https://docs.addaui.com/nuah/adr/0090-fonts-are-plugins-the-interface-has-one-font-per-language-pages-choose-from-a-short-curated-list-and-web-fonts-are-fetched-once-by-the-instance/
    Font,
    Feature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    #[default]
    Command,
    Query,
    /// Runs outside any transaction and may reach the network through the `net` Port ([ADR 0040]).
    ///
    /// [ADR 0040]: https://docs.addaui.com/nuah/adr/0040-outbound-operations-run-outside-the-transaction-and-reach-the-network-through-a-net-port/
    Outbound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Before,
    Around,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallPolicy {
    /// Exactly one provider, chosen by the operator.
    #[default]
    Single,
    /// Providers tried by priority; the first non-null answer wins.
    First,
    /// Every provider is called, each isolated from the others.
    Many,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Requires {
    pub plugins: Vec<String>,
    /// Core Ports used: db, auth, policy, clock, ids.
    pub ports: Vec<String>,
    pub extension_points: Vec<String>,
    /// Capabilities asked for: `emit`, `call`. Granted by the operator in the Profile ([ADR 0006]).
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionPointDef {
    #[serde(default)]
    pub policy: CallPolicy,
    /// The version of what the point promises (its methods, its `data`, and for a browser point the shared libraries and the
    /// module's API). Plugins require a range of it, never the plugin that defines it ([ADR 0095]).
    ///
    /// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
    #[serde(default = "one_zero_zero")]
    pub version: String,
    /// Its Providers are Browser modules, loaded and called in the browser by the plugin that defines the point.
    #[serde(default)]
    pub browser: bool,
    /// The JSON Schema of the static `data` a Provider may declare in its `provides` entry (`Null`: none).
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub description: String,
    pub input: Value,
    #[serde(default)]
    pub output: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Defines {
    pub extension_points: BTreeMap<String, ExtensionPointDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provide {
    pub extension_point: String,
    #[serde(default)]
    pub priority: i32,
    /// When true, a Profile that does not enable the plugin defining the point is fine: this plugin
    /// then simply offers nothing. Lets a plugin join something that may not be there (preferences).
    #[serde(default)]
    pub optional: bool,
    /// What the plugin declares to the point without running code (for the editor: its element kinds), checked against the
    /// point's `data` schema. A plugin with only browser code needs nothing else ([ADR 0095]).
    ///
    /// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDecl {
    pub collection: String,
    /// One field; or give `fields` for several.
    #[serde(default)]
    pub field: String,
    #[serde(default)]
    pub fields: Vec<String>,
    /// No two live documents may share the values (needs the `index.unique` feature).
    #[serde(default)]
    pub unique: bool,
}

impl IndexDecl {
    pub fn spec(&self) -> IndexSpec {
        let fields = if self.fields.is_empty() && !self.field.is_empty() { vec![self.field.clone()] } else { self.fields.clone() };
        IndexSpec { collection: self.collection.clone(), fields, unique: self.unique }
    }
}

/// What a plugin stores. Written as `{"schema": <the plugin's schema file>, "reads": {..}, "requires": [..], "prefers": [..]}`:
/// `collections`, `version`, `indexes` and `schemas` are worked out from the schema file ([ADR 0091]) and may not be
/// written beside it.
///
/// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, try_from = "RawDataDecl")]
pub struct DataDecl {
    /// The plugin's schema file, as written.
    pub schema: Option<Value>,
    /// Collections this plugin owns (the live Collections of the schema file).
    pub collections: Vec<String>,
    /// Read declarations: the fields of other plugins' Collections this plugin reads.
    pub reads: BTreeMap<String, Vec<String>>,
    /// The Data version.
    pub version: u32,
    /// The Upgrade floor.
    pub upgrades_from: Option<u32>,
    /// The Data versions that have an Upgrade step.
    pub steps: Vec<u32>,
    pub indexes: Vec<IndexDecl>,
    /// Features of the data Adapter the plugin cannot work without (see `nuah_kernel::feature`). The Core
    /// refuses to start when the Adapter lacks one.
    pub requires: Vec<String>,
    /// Features the plugin uses when there, and works without; a warning is logged when absent.
    pub prefers: Vec<String>,
    /// The strict Collection schema per owned Collection, checked by the kernel on every put.
    pub schemas: BTreeMap<String, Value>,
    /// The fields of each owned Collection the schema marks `"search": true`, for the text index ([ADR 0092]).
    ///
    /// [ADR 0092]: https://docs.addaui.com/nuah/adr/0092-one-index-of-entities-plugins-announce-them-in-their-events-the-data-port-searches-text-and-permissions-follow-the-events-audience/
    pub searches: BTreeMap<String, Vec<String>>,
}

/// `DataDecl` as written in a Manifest, before the schema file is unpacked.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawDataDecl {
    schema: Option<Value>,
    collections: Vec<String>,
    reads: BTreeMap<String, Vec<String>>,
    version: u32,
    indexes: Vec<IndexDecl>,
    requires: Vec<String>,
    prefers: Vec<String>,
    schemas: BTreeMap<String, Value>,
    upgrades_from: Option<u32>,
    steps: Vec<u32>,
}

impl TryFrom<RawDataDecl> for DataDecl {
    type Error = String;
    fn try_from(r: RawDataDecl) -> Result<DataDecl, String> {
        let mut d = DataDecl { schema: None, collections: r.collections, reads: r.reads, version: r.version, upgrades_from: r.upgrades_from, steps: r.steps, indexes: r.indexes, requires: r.requires, prefers: r.prefers, schemas: r.schemas, searches: BTreeMap::new() };
        let Some(schema) = r.schema else { return Ok(d) };
        if !d.collections.is_empty() || d.version != 0 || !d.indexes.is_empty() || !d.schemas.is_empty() || d.upgrades_from.is_some() || !d.steps.is_empty() {
            return Err("collections, version, indexes, upgrades_from and steps are written in the schema file only".into());
        }
        let file = crate::upgrade::SchemaFile::parse(&schema)?;
        d.collections = file.live_collections();
        d.version = file.version;
        d.upgrades_from = file.upgrades_from;
        d.steps = file.steps.clone();
        for c in &d.collections {
            let s = &file.collections[c];
            d.schemas.insert(c.clone(), crate::upgrade::strict(s));
            for ix in crate::upgrade::index_specs(c, s) {
                d.indexes.push(IndexDecl { collection: ix.collection, field: String::new(), fields: ix.fields, unique: ix.unique });
            }
            let fields = crate::upgrade::search_fields(s);
            if !fields.is_empty() {
                d.searches.insert(c.clone(), fields);
            }
        }
        if !d.searches.is_empty() && !d.requires.iter().any(|f| f == crate::db::feature::TEXT_SEARCH) {
            d.requires.push(crate::db::feature::TEXT_SEARCH.to_string());
        }
        d.schema = Some(schema);
        Ok(d)
    }
}

/// The thing an Operation acts on, so the policy can see it ([ADR 0005]).
///
/// [ADR 0005]: https://docs.addaui.com/nuah/adr/0005-policy-sees-the-resource-results-carry-what-you-may-do/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRef {
    /// Resource type, declared by the plugin that answers for it.
    #[serde(rename = "type")]
    pub kind: String,
    /// The input field holding the resource id.
    pub id: String,
}

/// Where in a result the Subject's "can" list goes: for every object found at `at` (an object, or an
/// array of objects; empty means the result itself) the kernel adds `can`, the permissions the Subject
/// holds on the resource whose id is in field `id` of that object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Affordances {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct OperationDecl {
    pub kind: OpKind,
    pub description: String,
    /// The Permission needed. Absent means any signed-in Subject.
    pub permission: Option<String>,
    /// Callable without signing in (sign-in itself).
    pub public: bool,
    pub resource: Option<ResourceRef>,
    pub affordances: Option<Affordances>,
    pub input: Value,
    pub output: Value,
    /// Reason codes this Operation may fail with (code -> English text), all under the plugin's namespaces.
    pub reasons: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PermissionDecl {
    pub description: String,
    /// Roles that hold it by default; the Profile may change this.
    pub default: Vec<String>,
}

/// What a Face does with the answer of a Slot item's Operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SlotResult {
    /// Nothing to show; the Face refreshes what it displays.
    #[default]
    None,
    /// The answer is a document the person saves as a file.
    Download,
}

/// An item a plugin puts into a [Slot](https://docs.addaui.com/nuah/references/glossary/#slot) a Face
/// offers ([ADR 0012], [ADR 0016]). It can only name an Operation.
///
/// [ADR 0012]: https://docs.addaui.com/nuah/adr/0012-slot-actions-name-operations-faces-declare-slots/
/// [ADR 0016]: https://docs.addaui.com/nuah/adr/0016-slot-items-are-declared-in-manifests-listed-by-describe-and-checked-against-each-face/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotFill {
    /// The Slot, named by the Face: `project.actions`.
    pub slot: String,
    /// Unique, under one of the plugin's namespaces.
    pub id: String,
    /// Language code to text; `en` is required.
    pub label: BTreeMap<String, String>,
    /// The Operation the item runs.
    pub operation: String,
    /// The Operation's input: a literal, or `$name.path` read from the Slot's context.
    #[serde(default)]
    pub args: BTreeMap<String, Value>,
    #[serde(default)]
    pub result: SlotResult,
    /// With `download`: the file name, a template such as `$project.name.json`.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDecl {
    pub stage: Stage,
    /// An Operation name, `ns.*` or `*`.
    pub on: String,
    #[serde(default)]
    pub order: i32,
    /// The name the code binds its handler under.
    pub handler: String,
    /// Reason codes this hook may refuse with, under the hook plugin's namespaces. They are added to
    /// the contract of every Operation the hook is attached to.
    #[serde(default)]
    pub reasons: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerDecl {
    pub id: String,
    /// Event kinds, `ns.*` allowed.
    pub on: Vec<String>,
    pub handler: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EventDecl {
    pub description: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub runtime: Runtime,
    #[serde(default)]
    pub quality: Quality,
    /// The prefixes this plugin owns; every name it contributes starts with one.
    #[serde(default)]
    pub namespaces: Vec<String>,
    #[serde(default)]
    pub requires: Requires,
    /// The core Port this plugin fills (an Adapter).
    #[serde(default)]
    pub fills: Option<String>,
    /// The widest reach this plugin is safe for ([`Reach`]); absent means no limit. An instance reachable by more people
    /// than this is a Problem until the operator accepts the risk ([ADR 0087]).
    ///
    /// [ADR 0087]: https://docs.addaui.com/nuah/adr/0087-the-operator-says-who-can-reach-the-instance-plugins-declare-how-far-they-are-safe-and-going-past-it-is-allowed-once-acknowledged/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_reach: Option<Reach>,
    #[serde(default)]
    pub defines: Defines,
    #[serde(default)]
    pub provides: Vec<Provide>,
    /// The Browser module this plugin ships for each browser Extension point it provides: point -> file, a relative path
    /// inside the plugin's own browser files (`editor/index.js`). Served at `/plugins/<scope>/<name>/<file>` ([ADR 0095]).
    ///
    /// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
    #[serde(default)]
    pub browser: BTreeMap<String, String>,
    #[serde(default)]
    pub data: DataDecl,
    #[serde(default)]
    pub operations: BTreeMap<String, OperationDecl>,
    #[serde(default)]
    pub permissions: BTreeMap<String, PermissionDecl>,
    #[serde(default)]
    pub hooks: Vec<HookDecl>,
    #[serde(default)]
    pub listeners: Vec<ListenerDecl>,
    #[serde(default)]
    pub events: BTreeMap<String, EventDecl>,
    /// Scopes (groups of Subjects) this plugin answers for.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Resource types this plugin answers for.
    #[serde(default)]
    pub resources: Vec<String>,
    #[serde(default)]
    pub settings_schema: Value,
    /// Items this plugin puts into Slots that Faces offer.
    #[serde(default)]
    pub slots: Vec<SlotFill>,
    /// A Theme's data for Faces.
    #[serde(default)]
    pub theme: Option<Value>,
    /// A font plugin's families for Faces (`fonts/1`, checked by the plugin that owns the data). Like a theme it is only data.
    #[serde(default)]
    pub fonts: Option<Value>,
    /// The name of the Face this plugin publishes (`web`, `console`). A projection that serves static
    /// Faces finds its files in `<faces dir>/<plugin id>` unless the Profile names another directory.
    #[serde(default)]
    pub face: Option<String>,
    /// Sites a Face may call straight from the browser because a feature of this plugin needs them (a provider whose terms forbid a
    /// proxy), as origins: `https://api.example.com`. A projection that sets a browser security policy allows exactly these and no
    /// other; see [`valid_connect_origin`].
    #[serde(default)]
    pub connect: Vec<String>,
    /// Sites a Face may load pictures and media from because a feature of this plugin shows them (a GIF provider's own files),
    /// as origins like `connect`. The browser loads from this server and from exactly these and no other ([ADR 0077]): a page cannot make a
    /// viewer's browser contact a site the operator did not allow.
    ///
    /// [ADR 0077]: https://docs.addaui.com/nuah/adr/0077-a-viewers-browser-loads-only-from-the-instance-and-the-sites-a-plugin-declares-previews-and-pictures-are-fetched-once-and-stored/
    #[serde(default)]
    pub images: Vec<String>,
    /// Sites the browser may load font files from, as origins like `connect` (`font-src`). Only for a font whose licence forbids the
    /// instance storing it; every other font is fetched once by the instance and served by it, so the browser contacts only this server
    /// ([ADR 0090]).
    ///
    /// [ADR 0090]: https://docs.addaui.com/nuah/adr/0090-fonts-are-plugins-the-interface-has-one-font-per-language-pages-choose-from-a-short-curated-list-and-web-fonts-are-fetched-once-by-the-instance/
    #[serde(default)]
    pub font_origins: Vec<String>,
}

/// Capabilities an Owner may allow a plugin without the operator: talking to the other plugins through Operations and events.
/// Every other Capability, and any added later, is privileged by default ([ADR 0080]).
///
/// [ADR 0080]: https://docs.addaui.com/nuah/adr/0080-two-kinds-of-plugin-platform-plugins-are-for-the-operator-feature-plugins-are-for-the-owner-and-the-line-is-what-the-plugin-provides/
pub const FEATURE_SAFE_CAPABILITIES: [&str; 2] = ["emit", "call"];

/// Who a plugin is for ([ADR 0080]).
///
/// [ADR 0080]: https://docs.addaui.com/nuah/adr/0080-two-kinds-of-plugin-platform-plugins-are-for-the-operator-feature-plugins-are-for-the-owner-and-the-line-is-what-the-plugin-provides/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginClass {
    Platform,
    Feature,
}

/// A plugin's class and the facts that decided it (empty for a feature).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Class {
    pub class: PluginClass,
    pub reasons: Vec<String>,
}

/// Who may change a setting: `bootstrap` (needed before the database opens) and `operator` are the operator's, in the console;
/// `owner` is the product Owner's, in the product's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTier {
    Bootstrap,
    Operator,
    Owner,
}

/// The tier of one setting (one property of a `settings_schema`). A missing or misspelled tier is the operator's: the safe side.
pub fn settings_tier(property: &Value) -> SettingsTier {
    match property.get("x-tier").and_then(|t| t.as_str()) {
        Some("owner") => SettingsTier::Owner,
        Some("bootstrap") => SettingsTier::Bootstrap,
        _ => SettingsTier::Operator,
    }
}

/// An origin a plugin may ask the browser to allow: `https://` and a host name (a port is fine), no path, query, user, wildcard or space.
pub fn valid_connect_origin(o: &str) -> bool {
    let Some(host) = o.strip_prefix("https://") else { return false };
    !host.is_empty()
        && host.len() <= 100
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == ':')
        && !host.starts_with('.')
        && !host.starts_with('-')
        && host.contains('.')
}

/// Split a plugin id `@scope/name` ([ADR 0042]) into its scope and name; `None` when it is not one. Both parts are
/// lower case letters, digits and hyphens, and start with a letter.
///
/// [ADR 0042]: https://docs.addaui.com/nuah/adr/0042-the-host-becomes-the-shell-plugins-have-scoped-ids-the-identity-file-is-nuah-json-with-one-entry-per-plugin-and-settings-render-as-forms/
pub fn split_id(id: &str) -> Option<(&str, &str)> {
    let (scope, name) = id.strip_prefix('@')?.split_once('/')?;
    let ok = |s: &str| s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    (ok(scope) && ok(name)).then_some((scope, name))
}

impl Manifest {
    /// The owner part of the id (`nuah` for `@nuah/http`); empty when the id is not scoped.
    pub fn scope(&self) -> &str {
        split_id(&self.id).map_or("", |(s, _)| s)
    }

    pub fn parse(src: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(src).map_err(|e| format!("manifest does not parse: {e}"))?;
        for (point, file) in &m.browser {
            if !valid_browser_file(file) {
                return Err(format!("bad_browser_file: the Browser module for {point} must be a relative path of letters, digits, '.', '_', '-' and '/', without '..': {file:?}"));
            }
        }
        Ok(m)
    }

    /// The kind comes from what the Manifest says, never from a declared field.
    pub fn kind(&self) -> PluginKind {
        if self.fills.is_some() {
            PluginKind::Adapter
        } else if self.provides.iter().any(|p| !p.optional) {
            PluginKind::Provider
        } else if self.theme.is_some()
            && self.operations.is_empty()
            && self.hooks.is_empty()
            && self.listeners.is_empty()
        {
            PluginKind::Theme
        } else if self.fonts.is_some() && self.operations.is_empty() && self.hooks.is_empty() && self.listeners.is_empty() {
            PluginKind::Font
        } else {
            PluginKind::Feature
        }
    }

    /// What the plugin is to the rest of the system, in the words a person can use: an `adapter` fills a Port, a `projection` carries
    /// the Link over a transport (it runs a service), a `face` is something people use, a `provider` fills an extension point, a
    /// `theme` is only data, and everything else is a `feature`. Worked out from the Manifest, never declared.
    pub fn role(&self) -> &'static str {
        if self.fills.is_some() {
            "adapter"
        } else if self.requires.capabilities.iter().any(|c| c == "serve") {
            "projection"
        } else if self.face.is_some() {
            "face"
        } else {
            match self.kind() {
                PluginKind::Provider => "provider",
                PluginKind::Theme => "theme",
                PluginKind::Font => "font",
                _ => "feature",
            }
        }
    }

    /// What the plugin may do that a person should know before switching it on, in four kinds: it keeps data (`stores`), it
    /// opens a port (`listens`), it contacts the internet (`reaches out`), it can change how the instance runs (`changes settings`).
    /// Worked out from what the Manifest asks for, so a plugin cannot understate it.
    pub fn powers(&self) -> Vec<&'static str> {
        let cap = |c: &str| self.requires.capabilities.iter().any(|x| x == c);
        let mut out = vec![];
        if self.fills.as_deref() == Some("db") || !self.data.collections.is_empty() {
            out.push("stores");
        }
        if cap("serve") {
            out.push("listens");
        }
        if cap("net") || self.fills.as_deref() == Some("net") {
            out.push("reaches out");
        }
        if cap("shell") {
            out.push("changes settings");
        }
        out
    }

    /// The class of this plugin ([ADR 0080]): **platform** (makes the system run; the operator's, in the console) or **feature**
    /// (what a person sees or does; the Owner's too). It is worked out here from what the Manifest does, never from a field
    /// an author writes, and the reasons come with it so a Face can say why.
    ///
    /// A plugin is platform when any of these is true:
    /// - it fills a Port (`fills`);
    /// - it publishes a Face or runs a service (`face`, the capability `serve`);
    /// - it asks for a Capability outside [`FEATURE_SAFE_CAPABILITIES`] (`net`, `shell`, `serve` and anything added later);
    /// - it uses a Runtime not listed as feature-safe (the match below is exhaustive, so a new Runtime forces a decision);
    /// - one of its settings is in the `bootstrap` or `operator` tier, or has no tier (see [`settings_tier`]).
    ///
    /// Anything else is a feature. The only input an author writes is a setting's tier, and it cannot widen what the plugin may do:
    /// a Capability is granted in the Profile whatever the class is, and a plugin with only feature-safe Capabilities reaches
    /// nothing outside the Operations and events of the system.
    ///
    /// [ADR 0080]: https://docs.addaui.com/nuah/adr/0080-two-kinds-of-plugin-platform-plugins-are-for-the-operator-feature-plugins-are-for-the-owner-and-the-line-is-what-the-plugin-provides/
    pub fn class(&self) -> Class {
        let mut reasons: Vec<String> = vec![];
        if let Some(port) = &self.fills {
            reasons.push(format!("fills the port {port}"));
        }
        if let Some(face) = &self.face {
            reasons.push(format!("publishes the Face {face}"));
        }
        for c in &self.requires.capabilities {
            if !FEATURE_SAFE_CAPABILITIES.contains(&c.as_str()) {
                reasons.push(format!("needs the capability {c}"));
            }
        }
        match self.runtime {
            Runtime::Native | Runtime::Data => {}
        }
        if let Some(props) = self.settings_schema.get("properties").and_then(|p| p.as_object()) {
            for (name, p) in props {
                let tier = settings_tier(p);
                if tier != SettingsTier::Owner {
                    reasons.push(format!("the setting {name} is for the operator"));
                }
            }
        }
        Class { class: if reasons.is_empty() { PluginClass::Feature } else { PluginClass::Platform }, reasons }
    }

    pub fn owns_name(&self, name: &str) -> bool {
        self.namespaces.iter().any(|ns| name == ns || name.starts_with(&format!("{ns}.")))
    }

    /// A stable summary used for the contract hash.
    pub fn contract_part(&self) -> Value {
        let ops: BTreeMap<&String, Value> = self
            .operations
            .iter()
            .map(|(n, o)| {
                (n, json!({
                    "kind": o.kind, "permission": o.permission, "public": o.public,
                    "input": o.input, "output": o.output, "reasons": o.reasons.keys().collect::<Vec<_>>(),
                }))
            })
            .collect();
        let hooks: Vec<Value> = self.hooks.iter().map(|h| json!({"stage": h.stage, "on": h.on, "reasons": h.reasons.keys().collect::<Vec<_>>()})).collect();
        json!({ "operations": ops, "events": self.events.keys().collect::<Vec<_>>(), "hooks": hooks })
    }
}

fn one_zero_zero() -> String {
    "1.0.0".into()
}

/// A Browser module's file: a relative path of `[A-Za-z0-9._/-]`, no empty part, no `.` or `..` part.
pub fn valid_browser_file(f: &str) -> bool {
    !f.is_empty()
        && f.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        && f.split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}

/// A requirement on an Extension point, `name` or `name@range` (`editor.element@^1`), split into the point and the range of its
/// version ([ADR 0095]). A range that does not parse is refused.
///
/// [ADR 0095]: https://docs.addaui.com/nuah/adr/0095-plugins-ship-browser-modules-for-another-plugins-host-and-require-an-extension-point-at-a-version-never-a-plugin-by-id/
pub fn point_requirement(s: &str) -> Result<(&str, Option<semver::VersionReq>), String> {
    match s.rsplit_once('@') {
        None => Ok((s, None)),
        Some((p, r)) => semver::VersionReq::parse(r).map(|v| (p, Some(v))).map_err(|e| format!("the requirement {s} has a version range that does not parse: {e}")),
    }
}

#[cfg(test)]
mod class_tests {
    use super::*;

    fn m(extra: Value) -> Manifest {
        let mut base = json!({"id": "@t/plugin", "version": "1.0.0", "namespaces": ["t"]});
        for (k, v) in extra.as_object().unwrap() {
            base[k] = v.clone();
        }
        Manifest::parse(&base.to_string()).unwrap()
    }

    fn platform(extra: Value) -> Vec<String> {
        let c = m(extra).class();
        assert_eq!(c.class, PluginClass::Platform, "{c:?}");
        c.reasons
    }

    #[test]
    fn a_plugin_that_only_adds_operations_and_talks_through_emit_and_call_is_a_feature() {
        let c = m(json!({"requires": {"capabilities": ["emit", "call"]}, "operations": {"t.go": {"kind": "command", "input": {}, "output": {}}}})).class();
        assert_eq!(c, Class { class: PluginClass::Feature, reasons: vec![] });
    }

    #[test]
    fn filling_a_port_makes_a_plugin_platform() {
        assert_eq!(platform(json!({"fills": "db"})), vec!["fills the port db"]);
    }

    #[test]
    fn publishing_a_face_makes_a_plugin_platform() {
        assert_eq!(platform(json!({"face": "web"})), vec!["publishes the Face web"]);
    }

    #[test]
    fn every_capability_outside_the_safe_list_makes_a_plugin_platform_even_one_added_later() {
        for c in ["net", "shell", "serve", "storage", "something-new"] {
            assert_eq!(platform(json!({"requires": {"capabilities": [c]}})), vec![format!("needs the capability {c}")], "{c}");
        }
    }

    #[test]
    fn a_setting_for_the_operator_or_without_a_tier_makes_a_plugin_platform_and_one_for_the_owner_does_not() {
        let schema = |tier: Option<&str>| {
            let mut p = json!({"type": "string"});
            if let Some(t) = tier {
                p["x-tier"] = json!(t);
            }
            json!({"settings_schema": {"type": "object", "properties": {"key": p}}})
        };
        assert_eq!(platform(schema(Some("operator"))), vec!["the setting key is for the operator"]);
        assert_eq!(platform(schema(Some("bootstrap"))).len(), 1);
        assert_eq!(platform(schema(None)).len(), 1, "no tier is the safe side");
        assert_eq!(platform(schema(Some("typo"))).len(), 1, "a misspelt tier is the safe side");
        assert_eq!(m(schema(Some("owner"))).class().class, PluginClass::Feature);
    }

    #[test]
    fn a_wizard_can_tell_what_a_plugin_is_and_what_it_may_do_from_the_manifest_alone() {
        let r = |extra: Value| m(extra).role();
        assert_eq!(r(json!({"fills": "db"})), "adapter");
        assert_eq!(r(json!({"requires": {"capabilities": ["serve"]}})), "projection");
        assert_eq!(r(json!({"face": "console"})), "face");
        assert_eq!(r(json!({"provides": [{"extension_point": "files.store"}]})), "provider");
        assert_eq!(r(json!({"theme": {"colors": {}}})), "theme");
        assert_eq!(r(json!({})), "feature");
        let may = |extra: Value| m(extra).powers();
        assert_eq!(may(json!({"fills": "db"})), vec!["stores"]);
        assert_eq!(may(json!({"data": {"collections": ["x.docs"], "version": 1}})), vec!["stores"]);
        assert_eq!(may(json!({"requires": {"capabilities": ["serve"]}})), vec!["listens"]);
        assert_eq!(may(json!({"requires": {"capabilities": ["net"]}})), vec!["reaches out"]);
        assert_eq!(may(json!({"fills": "net"})), vec!["reaches out"]);
        assert_eq!(may(json!({"requires": {"capabilities": ["shell"]}})), vec!["changes settings"]);
        assert!(may(json!({"requires": {"capabilities": ["emit", "call"]}})).is_empty(), "the safe capabilities are not a power worth a tag");
    }

    #[test]
    fn the_reasons_name_every_fact_not_only_the_first() {
        let r = platform(json!({"fills": "net", "requires": {"capabilities": ["net"]}}));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn nothing_a_plugin_writes_in_its_manifest_can_name_its_own_class() {
        // there is no `class` or `kind` field to write: a manifest that tries is read, and the field ignored
        let c = m(json!({"fills": "db", "class": "feature", "kind": "feature"})).class();
        assert_eq!(c.class, PluginClass::Platform);
    }

    #[test]
    fn the_settings_annotation_check_accepts_the_owner_tier() {
        let schema = json!({"properties": {"a": {"x-tier": "owner", "x-apply": "live"}}});
        assert!(crate::setup::settings_annotation_problems(&schema).is_empty());
    }
}

#[cfg(test)]
mod data_schema_tests {
    use super::*;

    fn parse(data: Value) -> Result<Manifest, String> {
        Manifest::parse(&json!({"id": "@t/p", "version": "1.0.0", "namespaces": ["t"], "data": data}).to_string())
    }

    #[test]
    fn the_schema_file_fills_collections_version_indexes_and_strict_schemas() {
        let m = parse(json!({"schema": {"version": 3, "collections": {
            "t.tasks": {"type": "object", "properties": {"project": {"type": "string", "index": true}}},
            "t.gone": {"removed": true}
        }}})).unwrap();
        assert_eq!(m.data.collections, vec!["t.tasks".to_string()]);
        assert_eq!(m.data.version, 3);
        assert_eq!(m.data.indexes.iter().map(|i| i.spec()).collect::<Vec<_>>(), vec![crate::db::IndexSpec { collection: "t.tasks".into(), fields: vec!["project".into()], unique: false }]);
        assert_eq!(m.data.schemas["t.tasks"]["additionalProperties"], json!(false));
        assert!(m.data.schema.is_some());
    }

    #[test]
    fn search_fields_come_from_the_schema_and_need_the_text_index() {
        let m = parse(json!({"schema": {"version": 1, "collections": {
            "t.notes": {"type": "object", "properties": {"title": {"type": "string", "search": true}, "tags": {"type": "array", "items": {"type": "string"}, "search": true},
                "old": {"removed": true, "search": true}, "n": {"type": "integer"}}},
            "t.plain": {"type": "object", "properties": {"a": {"type": "string"}}}
        }}})).unwrap();
        assert_eq!(m.data.searches.get("t.notes"), Some(&vec!["tags".to_string(), "title".to_string()]));
        assert!(!m.data.searches.contains_key("t.plain"));
        assert!(m.data.requires.contains(&crate::db::feature::TEXT_SEARCH.to_string()), "an Adapter without a text index cannot run it");
        let plain = parse(json!({"schema": {"version": 1, "collections": {"t.plain": {"type": "object", "properties": {"a": {"type": "string"}}}}}})).unwrap();
        assert!(plain.data.requires.is_empty());
    }

    #[test]
    fn hand_written_collections_beside_schema_is_refused() {
        let e = parse(json!({"schema": {"version": 1, "collections": {"t.a": {"type": "object"}}}, "collections": ["t.a"]})).unwrap_err();
        assert!(e.contains("schema file"), "{e}");
        assert!(parse(json!({"schema": {"version": 1, "collections": {}}, "version": 2})).is_err());
    }

    #[test]
    fn a_bad_schema_file_is_refused_when_the_manifest_is_read() {
        assert!(parse(json!({"schema": {"collections": {}}})).is_err());
    }

    #[test]
    fn reads_name_the_fields_of_each_collection() {
        let m = parse(json!({"reads": {"o.pages": ["title", "body"]}})).unwrap();
        assert_eq!(m.data.reads["o.pages"], vec!["title".to_string(), "body".to_string()]);
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    #[test]
    fn requirement_with_range() {
        let (p, r) = point_requirement("editor.element@^1").unwrap();
        assert_eq!(p, "editor.element");
        assert!(r.unwrap().matches(&semver::Version::parse("1.4.0").unwrap()));
        let (p, r) = point_requirement("notify.channel").unwrap();
        assert_eq!((p, r.is_none()), ("notify.channel", true));
        assert!(point_requirement("x@not-a-range").is_err());
    }

    #[test]
    fn browser_fields_parse() {
        let m = Manifest::parse(&json!({"id":"@t/p","version":"1.0.0","namespaces":["t"],
          "browser":{"t.view":"view/index.js"},
          "defines":{"extension_points":{"t.view":{"policy":"many","browser":true,"version":"1.2.0","input":{},"data":{"type":"object"}}}},
          "provides":[{"extension_point":"t.view","data":{"kinds":[]}}]}).to_string()).unwrap();
        assert_eq!(m.browser["t.view"], "view/index.js");
        let d = &m.defines.extension_points["t.view"];
        assert!(d.browser);
        assert_eq!(d.version, "1.2.0");
        assert_eq!(d.data, json!({"type":"object"}));
        assert_eq!(m.provides[0].data, Some(json!({"kinds":[]})));
    }

    #[test]
    fn point_version_defaults_to_one() {
        let m = Manifest::parse(&json!({"id":"@t/p","version":"1.0.0","namespaces":["t"],
          "defines":{"extension_points":{"t.x":{"input":{}}}}}).to_string()).unwrap();
        assert_eq!(m.defines.extension_points["t.x"].version, "1.0.0");
        assert!(!m.defines.extension_points["t.x"].browser);
    }

    #[test]
    fn bad_browser_file_refused() {
        for f in ["../x.js", "/abs.js", "a b.js", "a/../../b.js", ""] {
            let e = Manifest::parse(&json!({"id":"@t/p","version":"1.0.0","browser":{"t.v": f}}).to_string()).unwrap_err();
            assert!(e.contains("bad_browser_file"), "{f}: {e}");
        }
    }
}
