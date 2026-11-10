// SPDX-License-Identifier: Apache-2.0
//! Editing the identity file: what the setup flavours ([`crate::Bootstrap`]) and the console share
//! ([ADR 0041]). A [`ProfileStore`] reads and writes one Profile file; a draft is saved only if it resolves.
//!
//! [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/

use nuah_adapters::AuthPassword;
use nuah_kernel::setup::{catalog_info, profile_from_json, restore_profile};
use nuah_kernel::{Catalog, Core, Distribution, ShellServices, Problem, Profile};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ProfileStore {
    pub product: String,
    pub catalog: Catalog,
    /// What a draft starts from when there is no file yet.
    pub starting_point: Profile,
    pub path: PathBuf,
    /// Told when a save has been written (setup waits on it) or a restart is asked for (the shell loop waits on it).
    pub saved: Arc<tokio::sync::Notify>,
    pub restart: Arc<tokio::sync::Notify>,
    /// What the console should tell the operator after a restart that went back to the earlier file.
    notice: std::sync::Mutex<Option<String>>,
}

impl ProfileStore {
    pub fn new(product: &str, catalog: Catalog, starting_point: Profile, path: PathBuf) -> Self {
        ProfileStore { product: product.into(), catalog, starting_point, path, saved: Arc::new(tokio::sync::Notify::new()), restart: Arc::new(tokio::sync::Notify::new()), notice: std::sync::Mutex::new(None) }
    }

    /// Remember a message for the next `state` (the shell sets it when a saved file did not start).
    pub fn set_notice(&self, text: Option<String>) {
        *self.notice.lock().unwrap_or_else(|e| e.into_inner()) = text;
    }

    pub fn dist(&self, profile: Profile) -> Distribution {
        Distribution { name: self.product.clone(), catalog: self.catalog.clone(), profile }
    }

    /// The Profile on disk, or why it could not be read.
    pub fn stored(&self) -> Result<Option<Profile>, String> {
        match std::fs::read_to_string(&self.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", self.path.display())),
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", self.path.display()))?;
                profile_from_json(&v).map(|mut p| {
                    nuah_kernel::migrate_ids(&mut p, &self.catalog);
                    Some(p)
                })
            }
        }
    }

    /// A draft from JSON, with the secrets it did not retype put back from the stored Profile.
    pub fn draft(&self, body: &Value) -> Result<Profile, String> {
        let mut p = profile_from_json(body)?;
        nuah_kernel::migrate_ids(&mut p, &self.catalog);
        let stored = self.stored().ok().flatten().unwrap_or_default();
        let dist = self.dist(p.clone());
        restore_profile(&mut p, &stored, &dist);
        Ok(p)
    }

    /// Lock the contract and write the draft if it has no Problems; otherwise the Problems, and nothing is written.
    pub fn commit(&self, mut p: Profile, mut problems: Vec<Problem>) -> Result<Vec<Problem>, String> {
        let dist = self.dist(p.clone());
        p.contract = Some(Core::contract_for(&dist));
        problems.extend(Core::check_for_setup(&dist));
        if !problems.is_empty() {
            return Ok(problems);
        }
        // The file as it was stays beside the new one, so a bad save is undone with a copy.
        if self.path.exists() {
            let _ = std::fs::copy(&self.path, self.path.with_extension("json.bak"));
        }
        write_profile(&self.path, &p).map_err(|e| format!("could not write {}: {e}", self.path.display()))?;
        self.saved.notify_one();
        Ok(vec![])
    }
}

impl ShellServices for ProfileStore {
    fn state(&self) -> Result<Value, String> {
        let (profile, load_error, exists) = match self.stored() {
            Ok(Some(p)) => (p, None, true),
            Ok(None) => (self.starting_point.clone(), None, false),
            Err(e) => (self.starting_point.clone(), Some(e), true),
        };
        let dist = self.dist(profile);
        let mut body = catalog_info(&dist);
        body["problems"] = json!(Core::check_for_setup(&dist));
        body["path"] = json!(self.path.display().to_string());
        body["file_exists"] = json!(exists);
        body["load_error"] = json!(load_error);
        body["notice"] = json!(self.notice.lock().unwrap_or_else(|e| e.into_inner()).clone());
        Ok(body)
    }
    fn check(&self, draft: &Value) -> Result<Vec<Problem>, String> {
        Ok(Core::check_for_setup(&self.dist(self.draft(draft)?)))
    }
    fn save(&self, draft: &Value) -> Result<Vec<Problem>, String> {
        self.commit(self.draft(draft)?, vec![])
    }
    fn restart(&self) {
        self.set_notice(None);
        self.restart.notify_one();
    }
}

/// The enabled plugin that keeps accounts and wants a first one made at setup (`x-first-account` on its
/// `accounts` setting names how the password is stored; `password-hash` is the one understood here).
pub fn first_account_plugin(catalog: &Catalog, p: &Profile) -> Option<String> {
    catalog.0.iter().map(|pl| pl.manifest()).find(|m| p.enabled.contains(&m.id) && m.settings_schema["properties"]["accounts"]["x-first-account"] == "password-hash").map(|m| m.id)
}

/// Does the Profile already hold accounts for that plugin (a hidden secret stands for accounts already saved)?
pub fn has_accounts(p: &Profile, plugin: &str) -> bool {
    match &p.settings.get(plugin).map(|v| &v["accounts"]) {
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

pub const MIN_PASSWORD: usize = 8;

/// The Problem for a first administrator that was not given properly, or None when it is fine.
pub fn first_account_problem(plugin: &str, name: &str, password: &str) -> Option<Problem> {
    (name.trim().is_empty() || password.chars().count() < MIN_PASSWORD)
        .then(|| Problem { code: "first_account_missing".into(), plugins: vec![plugin.into()], message: format!("Choose a name and a password of at least {MIN_PASSWORD} characters for the first administrator.") })
}

/// Put the first administrator, with the password as a hash, into the settings of the plugin that keeps accounts.
pub fn put_first_account(p: &mut Profile, plugin: &str, name: &str, password: &str) {
    let name = name.trim();
    let acc = json!([{"name": name, "display": name, "password_hash": AuthPassword::hash_password(password), "roles": ["admin"]}]);
    let slot = p.settings.entry(plugin.to_string()).or_insert_with(|| json!({}));
    if let Some(o) = slot.as_object_mut() {
        o.insert("accounts".into(), acc);
    }
}

/// Write to a sibling file and rename, so a crash never leaves half a Profile. A new file is
/// private to the owner; an existing one keeps its permissions.
pub fn write_profile(path: &Path, p: &Profile) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = f.set_permissions(meta.permissions());
    }
    f.write_all(serde_json::to_string_pretty(p).expect("a Profile serialises").as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}
