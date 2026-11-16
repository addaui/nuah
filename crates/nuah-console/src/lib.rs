// SPDX-License-Identifier: Apache-2.0
//! The console plugin ([ADR 0033], [ADR 0039], [ADR 0041]): publishes the generic console Face (files in
//! `<faces dir>/nuah/console`, default `./faces/nuah/console`; it needs the `http` plugin to be served) and, when the
//! operator grants it the capability `shell`, lets an administrator read and change the identity file.
//!
//!   console.profile.get      the catalog and the stored Profile (secrets hidden), with its Problems
//!   console.profile.check    the Problems of a draft
//!   console.profile.save     write the draft if it has none; takes effect on restart
//!   console.restart          stop, read the file again, start
//!   console.auth.switch      choose how people sign in (ADR 0085): turns the chosen sign-in plugin on and the others off, makes the first administrator,
//!                            and keeps the Owner of an instance that had no sign-in as that account, so nothing they made is lost
//!
//! [ADR 0033]: https://docs.addaui.com/nuah/adr/0033-the-console-is-a-generic-face-and-an-admin-plugin-edits-the-profile-settings-have-tiers/
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
//! [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/

use nuah_kernel::{Bindings, Error, Manifest, Plugin};
use serde_json::{json, Value};

pub struct Console;

fn shell_error(message: String) -> Error {
    Error::internal(message)
}

/// The shell's services, or why the console cannot change anything here (the program running the Core attached none).
fn shell(cx: &nuah_kernel::Outbound) -> nuah_kernel::Result<std::sync::Arc<dyn nuah_kernel::ShellServices>> {
    cx.shell().map_err(|_| Error::reason(nuah_kernel::Category::Denied, "console.no_shell", "This instance does not let the console change its configuration (the program running it offers no shell services)"))
}

impl Plugin for Console {
    fn manifest(&self) -> Manifest {
        let draft = json!({"type":"object","required":["profile"],"properties":{"profile":{"type":"object"}},"additionalProperties":false});
        serde_json::from_value(json!({
            "id": "@nuah/console", "version": "0.3.0", "name": "Console", "namespaces": ["console"],
            "description": "A generic Face that shows the Core's Operations and health, and lets an administrator change the identity file",
            "requires": {"plugins": ["@nuah/http"], "capabilities": ["shell"]},
            "face": "console",
            "permissions": {"console.admin": {"description": "Read and change the identity file (plugins, grants, settings) and restart", "default": ["admin"]}},
            "operations": {
                "console.profile.get": {"kind": "outbound", "permission": "console.admin", "description": "The catalog and the stored Profile, secrets hidden.", "output": {"type":"object"}},
                "console.profile.check": {"kind": "outbound", "permission": "console.admin", "description": "The Problems of a draft Profile.", "input": draft, "output": {"type":"object","required":["problems"]}},
                "console.profile.save": {"kind": "outbound", "permission": "console.admin", "description": "Write the draft Profile if it has no Problems. It takes effect on restart.", "input": draft,
                    "reasons": {"console.problems": "The draft does not resolve; nothing was written", "console.no_shell": "The program running this instance offers the console no shell services"},
                    "output": {"type":"object","required":["saved"]}},
                "console.auth.switch": {"kind": "outbound", "permission": "console.admin", "description": "Turn on a way of signing in. For one with accounts, the first administrator is made now; when the instance had no sign-in, that person keeps everything the Owner made.",
                    "input": {"type":"object","required":["plugin"],"properties":{"plugin":{"type":"string"},"name":{"type":"string","maxLength":200},"display":{"type":"string","maxLength":200},"password":{"type":"string","maxLength":1000}},"additionalProperties":false},
                    "reasons": {"console.first_account": "Choose a name and a password of at least 8 characters for the first administrator", "console.problems": "The draft does not resolve; nothing was written", "console.no_shell": "The program running this instance offers the console no shell services"},
                    "output": {"type":"object","required":["saved","restart_required","adopts_owner"]}},
                "console.restart": {"kind": "outbound", "permission": "console.admin", "description": "Stop, read the identity file again and start.", "output": {"type":"object"}}
            }
        }))
        .expect("console manifest")
    }

    fn bind(&self, b: &mut Bindings) {
        b.outbound("console.profile.get", |cx, _| shell(cx)?.state().map_err(shell_error));
        b.outbound("console.profile.check", |cx, i| {
            let problems = shell(cx)?.check(&i["profile"]).map_err(|m| Error::invalid("profile", &m))?;
            Ok(json!({"problems": problems}))
        });
        b.outbound("console.profile.save", |cx, i| {
            let problems = shell(cx)?.save(&i["profile"]).map_err(shell_error)?;
            if !problems.is_empty() {
                let first = problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join(" ");
                return Err(Error::reason(nuah_kernel::Category::Invalid, "console.problems", &first));
            }
            cx.log(nuah_kernel::Level::Info, "console.saved", json!({"by": cx.who().subject}));
            Ok(json!({"saved": true, "restart_required": true}))
        });
        b.outbound("console.auth.switch", |cx, i| {
            let sh = shell(cx)?;
            let state = sh.state().map_err(shell_error)?;
            let (new, adopts) = switch_auth(&state, &i)?;
            let problems = sh.save(&new).map_err(shell_error)?;
            if !problems.is_empty() {
                let first = problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join(" ");
                return Err(Error::reason(nuah_kernel::Category::Invalid, "console.problems", &first));
            }
            cx.log(nuah_kernel::Level::Warn, "console.auth_switched", json!({"by": cx.who().subject, "to": i["plugin"]}));
            Ok(json!({"saved": true, "restart_required": true, "adopts_owner": adopts}))
        });
        b.outbound("console.restart", |cx, _| {
            let shell_services = shell(cx)?;
            cx.log(nuah_kernel::Level::Warn, "console.restart", json!({"by": cx.who().subject}));
            shell_services.restart();
            Ok(Value::Object(Default::default()))
        });
    }
}

/// The sign-in plugin of the Owner of an instance with no sign-in: the account that takes over its subject keeps what the Owner made (ADR 0085).
const OPEN: &str = "@nuah/auth-open";
const OWNER_SUBJECT: &str = "owner";

/// The Profile with the chosen sign-in plugin on and the others off, and whether the first administrator takes over the Owner. Pure, so it is tested without a shell.
pub fn switch_auth(state: &Value, input: &Value) -> nuah_kernel::Result<(Value, bool)> {
    let plugin = input["plugin"].as_str().unwrap_or("");
    let catalog = state["plugins"].as_array().cloned().unwrap_or_default();
    let fills_auth = |p: &Value| p["fills"] == "auth";
    let target = catalog.iter().find(|p| p["id"] == plugin && fills_auth(p)).ok_or_else(|| Error::invalid("plugin", "that is not a way of signing in"))?;
    let mut profile = state["profile"].clone();
    if !profile["plugins"].is_object() {
        profile["plugins"] = json!({});
    }
    let was_open = profile["plugins"][OPEN]["enabled"] == true;
    for p in catalog.iter().filter(|p| fills_auth(p)) {
        let id = p["id"].as_str().unwrap_or("");
        let e = profile["plugins"].as_object_mut().expect("an object").entry(id.to_string()).or_insert_with(|| json!({}));
        e["enabled"] = json!(id == plugin);
    }
    let caps = target["requires"]["capabilities"].clone();
    if caps.as_array().is_some_and(|c| !c.is_empty()) {
        profile["plugins"][plugin]["grants"] = caps;
    }
    if let Some(b) = profile["bindings"].as_object_mut() {
        b.remove("auth");
    }
    let mut adopts = false;
    if target["settings_schema"]["properties"]["accounts"]["x-first-account"] == "password-hash" {
        let existing = &profile["plugins"][plugin]["settings"]["accounts"];
        let has = existing.is_object() || existing.as_array().is_some_and(|a| !a.is_empty());
        if !has {
            let (name, password) = (input["name"].as_str().unwrap_or("").trim(), input["password"].as_str().unwrap_or(""));
            if name.is_empty() || password.chars().count() < 8 {
                return Err(Error::reason(nuah_kernel::Category::Invalid, "console.first_account", "Choose a name and a password of at least 8 characters for the first administrator"));
            }
            adopts = was_open;
            let display = input["display"].as_str().filter(|d| !d.trim().is_empty()).unwrap_or(name);
            let mut account = json!({"name": name, "display": display, "password_hash": nuah_adapters::AuthPassword::hash_password(password), "roles": ["member", "admin"]});
            if adopts {
                account["subject"] = json!(OWNER_SUBJECT);
            }
            if !profile["plugins"][plugin]["settings"].is_object() {
                profile["plugins"][plugin]["settings"] = json!({});
            }
            profile["plugins"][plugin]["settings"]["accounts"] = json!([account]);
        }
    }
    Ok((profile, adopts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(open: bool) -> Value {
        json!({
            "plugins": [
                {"id": "@nuah/auth-open", "fills": "auth", "requires": {"capabilities": []}},
                {"id": "@nuah/auth-local", "fills": "auth", "requires": {"capabilities": []}},
                {"id": "@nuah/auth-password", "fills": "auth", "requires": {"capabilities": []}, "settings_schema": {"properties": {"accounts": {"x-first-account": "password-hash"}}}},
                {"id": "@nuah/db-memory", "fills": "db", "requires": {"capabilities": []}}
            ],
            "profile": {"plugins": {"@nuah/auth-open": {"enabled": open}, "@nuah/db-memory": {"enabled": true}}, "bindings": {"auth": "@nuah/auth-open", "db": "@nuah/db-memory"}}
        })
    }

    #[test]
    fn switching_from_no_sign_in_makes_the_first_administrator_the_owner_so_nothing_is_lost() {
        let (p, adopts) = switch_auth(&state(true), &json!({"plugin": "@nuah/auth-password", "name": "Sam", "password": "long enough pw"})).unwrap();
        assert!(adopts);
        assert_eq!(p["plugins"]["@nuah/auth-open"]["enabled"], false);
        assert_eq!(p["plugins"]["@nuah/auth-password"]["enabled"], true);
        let a = &p["plugins"]["@nuah/auth-password"]["settings"]["accounts"][0];
        assert_eq!((a["name"].clone(), a["subject"].clone(), a["roles"].clone()), (json!("Sam"), json!("owner"), json!(["member", "admin"])));
        assert!(a["password_hash"].as_str().unwrap().starts_with("$argon2"), "the password is stored hashed");
        assert!(!p.to_string().contains("long enough pw"));
        assert!(p["bindings"].get("auth").is_none(), "the old choice of Adapter goes");
        assert_eq!(p["plugins"]["@nuah/db-memory"]["enabled"], true, "nothing else changes");
    }

    #[test]
    fn an_instance_that_already_had_accounts_is_not_given_another_first_administrator() {
        let mut s = state(false);
        s["profile"]["plugins"]["@nuah/auth-password"] = json!({"enabled": false, "settings": {"accounts": {"$secret": true}}});
        let (p, adopts) = switch_auth(&s, &json!({"plugin": "@nuah/auth-password"})).unwrap();
        assert!(!adopts);
        assert_eq!(p["plugins"]["@nuah/auth-password"]["settings"]["accounts"], json!({"$secret": true}));
        assert_eq!(p["plugins"]["@nuah/auth-password"]["enabled"], true);
    }

    #[test]
    fn a_name_and_a_password_of_eight_characters_are_needed_and_only_sign_in_plugins_can_be_chosen() {
        let e = switch_auth(&state(true), &json!({"plugin": "@nuah/auth-password", "name": "Sam", "password": "short"})).unwrap_err();
        assert_eq!(e.reason.as_deref(), Some("console.first_account"));
        assert!(switch_auth(&state(true), &json!({"plugin": "@nuah/db-memory"})).is_err());
        assert!(switch_auth(&state(true), &json!({"plugin": "@nobody/x"})).is_err());
        // a sign-in plugin with no accounts of its own needs nothing more
        let (p, adopts) = switch_auth(&state(true), &json!({"plugin": "@nuah/auth-local"})).unwrap();
        assert!(!adopts && p["plugins"]["@nuah/auth-local"]["enabled"] == true);
    }
}
