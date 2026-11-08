// SPDX-License-Identifier: Apache-2.0
//! The Manifest ([ADR 0004]): everything a plugin says about itself without running any code.
//! Operations, hooks, listeners, Events, permissions and data are declared here; code only attaches a
//! handler to each declared name.
//!
//! [ADR 0004]: https://docs.addaui.com/nuah/adr/0004-manifest-declares-everything-names-are-namespaced/

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Runtime {
    #[default]
    Native,
    /// Data only: runs no code (themes).
    Data,
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
    Feature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    #[default]
    Command,
    Query,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexDecl {
    pub collection: String,
    pub field: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DataDecl {
    /// Collections this plugin owns.
    pub collections: Vec<String>,
    /// Other plugins' collections it may read.
    pub reads: Vec<String>,
    pub version: u32,
    pub indexes: Vec<IndexDecl>,
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
    #[serde(default)]
    pub defines: Defines,
    #[serde(default)]
    pub provides: Vec<Provide>,
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
}

impl Manifest {
    pub fn parse(src: &str) -> Result<Manifest, String> {
        serde_json::from_str(src).map_err(|e| format!("manifest does not parse: {e}"))
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
        } else {
            PluginKind::Feature
        }
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
