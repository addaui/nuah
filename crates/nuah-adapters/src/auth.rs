// SPDX-License-Identifier: Apache-2.0
//! Auth Adapters. Sign-in is an Operation ([ADR 0010]), so every Projection works the same way.
//!
//! [ADR 0010]: https://docs.addaui.com/nuah/adr/0010-sign-in-is-an-operation-sessions-expire/

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use nuah_kernel::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Local, single-user: everyone who can reach the Core is the admin. Refuse to use it on a network.
struct Local(Identity);
impl Auth for Local {
    fn name(&self) -> &str {
        "@nuah/auth-local"
    }
    fn authenticate(&self, _db: &dyn DbRead, credential: &str, _now: Millis) -> Result<Option<Identity>> {
        Ok(if credential == "local" { Some(self.0.clone()) } else { None })
    }
    fn implicit(&self) -> Option<Identity> {
        Some(self.0.clone())
    }
}

pub struct AuthLocal;
impl Plugin for AuthLocal {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id":"@nuah/auth-local","version":"0.2.0","name":"Single user, no sign-in","description":"Nobody signs in: you are always the administrator. Only for a Core that nobody else can reach.","fills":"auth","safe_reach":"this-computer",
            "settings_schema":{"type":"object","properties":{"name":{"type":"string","minLength":1}},"additionalProperties":false}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("me");
        Ok(Some(PortImpl::Auth(Arc::new(Local(Identity::new("local", name, &["admin"]))))))
    }
}

/// Open: the app has no sign-in (ADR 0076). Anyone who reaches the Core is the Owner; the Admin role is added by signing in with the instance's admin
/// password, which keeps the same subject, so what the Owner made stays theirs while the console is open. An unknown or expired session is the Owner, never nobody.
struct Open { owner_name: String }
const ADMIN_SESSIONS: &str = "auth.admin_sessions";
const OWNER: &str = "owner";

impl Open {
    fn owner(&self, admin: bool) -> Identity {
        let roles: &[&str] = if admin { &["member", "admin"] } else { &["member"] };
        Identity::new(OWNER, &self.owner_name, roles)
    }
}
impl Auth for Open {
    fn name(&self) -> &str {
        "@nuah/auth-open"
    }
    fn authenticate(&self, db: &dyn DbRead, credential: &str, now: Millis) -> Result<Option<Identity>> {
        let live = db.get(ADMIN_SESSIONS, &token_hash(credential))?.filter(|d| !d.deleted && d.body["expires_at"].as_i64().unwrap_or(0) > now);
        Ok(Some(self.owner(live.is_some())))
    }
    fn implicit(&self) -> Option<Identity> {
        Some(self.owner(false))
    }
}

pub struct AuthOpen;
impl Plugin for AuthOpen {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "@nuah/auth-open", "version": "0.1.0", "name": "No sign-in, settings password",
            "description": "The app has no sign-in: whoever reaches the Core is the Owner. The console has one: the instance's admin password gives a short Admin session. Whoever can reach this instance can use the Owner's data: keep it on a network you trust.",
            "fills": "auth", "safe_reach": "this-computer", "namespaces": ["auth"],
            "requires": {"ports": ["db", "clock", "ids"]},
            "data": {"schema": schema(include_str!("../schema/auth-open.json"))},
            "settings_schema": {"type": "object", "additionalProperties": false, "properties": {
                "owner_name": {"type": "string", "minLength": 1, "maxLength": 200, "default": "Owner", "title": "Owner's name", "description": "How the one person who uses the app is shown.", "x-tier": "operator", "x-apply": "restart"},
                "admin_password_hash": {"type": "string", "minLength": 1, "x-secret": true, "x-wizard-password": true, "title": "Admin password", "description": "The stored form of the password that opens the console (make it with `hash-password`). Without it nobody can be Admin.", "x-tier": "operator", "x-apply": "restart"},
                "admin_session_minutes": {"type": "integer", "minimum": 5, "maximum": 1440, "default": 60, "title": "Admin session (minutes)", "description": "How long the console stays open without signing in again.", "x-tier": "operator", "x-apply": "restart"}
            }},
            "operations": {
                "auth.sign_in": {"kind": "command", "public": true, "description": "Open the console: the admin password gives a short Admin session",
                    "input": {"type": "object", "required": ["password"], "properties": {"username": {"type": "string", "maxLength": 200}, "password": {"type": "string", "minLength": 1, "maxLength": 1000}}, "additionalProperties": false},
                    "output": {"type": "object", "required": ["token", "expires_at", "who"]},
                    "reasons": {"auth.bad_credentials": "The password is wrong"}},
                "auth.sign_out": {"kind": "command", "description": "End an Admin session you hold",
                    "input": {"type": "object", "required": ["token"], "properties": {"token": {"type": "string", "minLength": 1}}, "additionalProperties": false}, "output": {"type": "object"}},
                "auth.who": {"kind": "query", "description": "Who am I?", "output": {"type": "object", "required": ["subject"]}}
            }
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let name = s.get("owner_name").and_then(|n| n.as_str()).unwrap_or("Owner").to_string();
        Ok(Some(PortImpl::Auth(Arc::new(Open { owner_name: name }))))
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("auth.sign_in", |tx, input| {
            let password = input["password"].as_str().unwrap_or("");
            // always the same work, so timing does not say whether an admin password is set
            let stored = tx.settings()["admin_password_hash"].as_str().map(String::from);
            let set = stored.is_some();
            let hash = stored.unwrap_or_else(|| "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into());
            let ok = PasswordHash::new(&hash).map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()).unwrap_or(false) && set;
            if !ok {
                return Err(Error::reason(Category::Unauthenticated, "auth.bad_credentials", "The password is wrong"));
            }
            let name = tx.settings()["owner_name"].as_str().unwrap_or("Owner").to_string();
            let who = Identity::new(OWNER, &name, &["member", "admin"]);
            let token = format!("{}{}", tx.new_id(), tx.new_id());
            let ttl = tx.settings()["admin_session_minutes"].as_i64().unwrap_or(60) * 60_000;
            let expires = tx.now() + ttl;
            tx.put(ADMIN_SESSIONS, &token_hash(&token), json!({"expires_at": expires}), Expect::Any)?;
            Ok(json!({"token": token, "expires_at": expires, "who": who}))
        });
        b.op("auth.sign_out", |tx, input| {
            let id = token_hash(input["token"].as_str().unwrap_or(""));
            if tx.get(ADMIN_SESSIONS, &id)?.is_some() {
                tx.delete(ADMIN_SESSIONS, &id)?;
            }
            Ok(json!({}))
        });
        b.op("auth.who", |tx, _| Ok(serde_json::to_value(tx.who()).unwrap()));
    }
}

const SESSIONS: &str = "auth.sessions";
/// API keys ([ADR 0101]): long-lived credentials a person makes for an agent or a script; each acts as its owner. Kept by the hash
/// of the key (the key itself is shown once), with the owner and a name.
///
/// [ADR 0101]: https://docs.addaui.com/nuah/adr/0101-agents-reach-a-distribution-through-mcp-and-api-keys-a-key-acts-as-its-owner/
const API_KEYS: &str = "auth.api_keys";
/// Every API key starts with this, so a person (and a secret scanner) can tell one apart.
pub const API_KEY_PREFIX: &str = "nuah_";
const ACCOUNTS: &str = "auth.accounts";
const NAMES: &str = "auth.names";
const DEFAULT_TTL_MS: i64 = 12 * 60 * 60 * 1000;

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The username an account document holds. Accounts made before ADR 0089 keep it under `name`; both are read, `username` is written.
fn username_of(body: &Value) -> &str {
    body["username"].as_str().unwrap_or("")
}

/// A plugin's schema file, embedded in the program ([ADR 0091]).
///
/// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
fn schema(src: &str) -> Value {
    serde_json::from_str(src).expect("an embedded schema file is JSON")
}

/// The one rule for a username ([ADR 0089]): 2 to 32 letters, numbers, dots, dashes or underscores. Checked when one is made or changed, never at sign-in,
/// so an older username that does not fit keeps working.
///
/// [ADR 0089]: https://docs.addaui.com/nuah/adr/0089-what-a-person-types-to-sign-in-is-called-username-everywhere-and-has-one-rule/
pub fn valid_username(username: &str) -> bool {
    let t = username.trim();
    (2..=32).contains(&t.chars().count()) && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn bad_username() -> Error {
    Error::reason(Category::Invalid, "auth.bad_username", "A username is 2 to 32 letters, numbers, dots, dashes or underscores")
}

/// The person an account document describes. The document's id is the `subject`: an opaque id made when the
/// account was created, kept when the login name changes, never reused ([ADR 0039]).
///
/// [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
fn identity_of(subject: &str, body: &Value) -> Identity {
    Identity {
        subject: subject.into(),
        display: body["display"].as_str().filter(|d| !d.is_empty()).or_else(|| Some(username_of(body)).filter(|n| !n.is_empty())).unwrap_or(subject).into(),
        roles: body["roles"].as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
        via: None,
        limits: None,
    }
}

/// Accounts and sessions are data the Adapter owns, so they live wherever the data Adapter keeps data.
struct Password;
impl Auth for Password {
    fn name(&self) -> &str {
        "@nuah/auth-password"
    }
    fn authenticate(&self, db: &dyn DbRead, credential: &str, now: Millis) -> Result<Option<Identity>> {
        // an API key acts as its owner, as the owner's account is now (a disabled account's keys stop at once)
        if credential.starts_with(API_KEY_PREFIX) {
            let Some(k) = db.get(API_KEYS, &token_hash(credential))?.filter(|d| !d.deleted) else { return Ok(None) };
            let Some(subject) = k.body["subject"].as_str() else { return Ok(None) };
            // an expired key is no key (ADR 0132)
            if k.body["expires_at"].as_i64().is_some_and(|e| e <= now) {
                return Ok(None);
            }
            let (via, limits) = (k.body["id"].as_str().map(String::from), k.body.get("limits").filter(|l| !l.is_null()).cloned());
            return Ok(db.get(ACCOUNTS, subject)?.filter(|a| !a.deleted && a.body["disabled"] != true).map(|a| Identity { via, limits, ..identity_of(subject, &a.body) }));
        }
        let Some(d) = db.get(SESSIONS, &token_hash(credential))?.filter(|d| !d.deleted) else { return Ok(None) };
        if d.body["expires_at"].as_i64().unwrap_or(0) <= now {
            return Ok(None);
        }
        // Who they are comes from the account as it is now, so a changed role or a disabled account applies at once.
        let Some(subject) = d.body["subject"].as_str() else { return Ok(None) };
        let Some(acc) = db.get(ACCOUNTS, subject)?.filter(|a| !a.deleted && a.body["disabled"] != true) else { return Ok(None) };
        Ok(Some(identity_of(subject, &acc.body)))
    }
    fn has_accounts(&self) -> bool {
        true
    }
    fn find(&self, db: &dyn DbRead, username: &str) -> Result<Option<Identity>> {
        let Some(subject) = db.get(NAMES, &name_key(username))?.filter(|d| !d.deleted).and_then(|d| d.body["subject"].as_str().map(String::from)) else { return Ok(None) };
        Ok(db.get(ACCOUNTS, &subject)?.filter(|a| !a.deleted && a.body["disabled"] != true).map(|a| identity_of(&subject, &a.body)))
    }
}

pub struct AuthPassword;

impl AuthPassword {
    /// Make the stored form of a password (for a seed account in the Profile, or an account created by hand).
    pub fn hash_password(password: &str) -> String {
        let salt = SaltString::generate(&mut rand_core::OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
    }
}

/// `subject`: a person who already has data under an id (the Owner of an instance that had no sign-in, whose id is `owner`) keeps it, so everything
/// they made stays theirs. Only a seed account in the Profile may name one, and never one that exists.
fn create_account(tx: &mut Tx, username: &str, display: &str, password_hash: &str, roles: &[String], subject: Option<&str>) -> Result<String> {
    let taken = || Error::reason(Category::Conflict, "auth.username_taken", "That username is already used");
    let key = name_key(username);
    if key.is_empty() || tx.get(NAMES, &key)?.is_some() {
        return Err(taken());
    }
    let subject = match subject {
        Some(s) if !s.is_empty() => {
            if tx.get(ACCOUNTS, s)?.is_some() {
                return Err(taken());
            }
            s.to_string()
        }
        _ => tx.new_id(),
    };
    let now = tx.now();
    tx.put(NAMES, &key, json!({"subject": subject}), Expect::Absent).map_err(|_| taken())?;
    tx.put(ACCOUNTS, &subject, json!({"username": username.trim(), "display": display, "password_hash": password_hash, "roles": roles, "disabled": false, "created_at": now}), Expect::Absent)?;
    Ok(subject)
}

fn roles_of(v: &Value) -> Vec<String> {
    v.as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn public_account(subject: &str, body: &Value) -> Value {
    json!({"subject": subject, "username": username_of(body), "display": body["display"], "roles": body["roles"], "disabled": body["disabled"], "created_at": body["created_at"]})
}

/// Would this change leave nobody able to manage accounts?
fn another_admin_remains(tx: &Tx, except: &str) -> Result<bool> {
    let admins = tx.query(ACCOUNTS, &Query::new().has("roles", "admin").eq("disabled", false).limit(2))?;
    Ok(admins.items.iter().any(|d| d.id != except))
}

impl Plugin for AuthPassword {
    fn manifest(&self) -> Manifest {
        let username_in = json!({"type":"string","minLength":1,"maxLength":200,"description":"2 to 32 letters, numbers, dots, dashes or underscores"});
        let pw_in = json!({"type":"string","minLength":8,"maxLength":1000});
        serde_json::from_value(json!({
            "id": "@nuah/auth-password", "version": "0.3.0", "name": "Password sign-in",
            "description": "Accounts with passwords, kept in the database; sign-in gives an expiring session",
            "fills": "auth", "namespaces": ["auth"],
            "requires": {"ports": ["db", "clock", "ids"]},
            "data": {"schema": schema(include_str!("../schema/auth-password.json"))},
            "permissions": {"auth.manage": {"description": "Create accounts, change their roles and passwords, disable them", "default": ["admin"]}},
            "settings_schema": {"type":"object","properties":{
                "session_hours": {"type":"integer","minimum":1,"maximum":720,"title":"Session length (hours)","description":"How long someone stays signed in without signing in again.","default":24},
                "accounts": {"x-secret":true,"x-first-account":"password-hash","description":"Accounts to create the first time the data is set up; after that they are managed in the database","type":"array","items":{"type":"object","required":["password_hash"],
                    "properties":{"username":{"type":"string","minLength":1},"display":{"type":"string"},
                    "password_hash":{"type":"string","minLength":1},"roles":{"type":"array","items":{"type":"string"}},
                    "subject":{"type":"string","minLength":1,"maxLength":100,"description":"Keep the id a person already has (the Owner of an instance that had no sign-in is `owner`), so what they made stays theirs"}},
                    "additionalProperties":false}}},
                "additionalProperties": false},
            "operations": {
                "auth.sign_in": {"kind":"command","public":true,"description":"Exchange a username and password for a session",
                    "input":{"type":"object","required":["username","password"],"properties":{"username":{"type":"string","minLength":1,"maxLength":200},"password":{"type":"string","minLength":1,"maxLength":1000}},"additionalProperties":false},
                    "output":{"type":"object","required":["token","expires_at","who"]},
                    "reasons":{"auth.bad_credentials":"The username or password is wrong"}},
                "auth.refresh": {"kind":"command","description":"Extend a session you hold",
                    "input":{"type":"object","required":["token"],"properties":{"token":{"type":"string","minLength":1}},"additionalProperties":false},
                    "output":{"type":"object","required":["expires_at"]}},
                "auth.sign_out": {"kind":"command","description":"End a session you hold",
                    "input":{"type":"object","required":["token"],"properties":{"token":{"type":"string","minLength":1}},"additionalProperties":false},
                    "output":{"type":"object"}},
                "auth.who": {"kind":"query","description":"Who am I?","output":{"type":"object","required":["subject"]}},
                "auth.keys.create": {"kind":"command","description":"Make an API key that acts as you (for an agent or a script); the key is shown this once",
                    "input":{"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":1,"maxLength":100},"permissions":{"type":"array","maxItems":100,"items":{"type":"string","minLength":1,"maxLength":80},"description":"Limit the key to Operations that need one of these permissions"},"expires_in_days":{"type":"integer","minimum":1,"maximum":3650,"description":"The key stops working after this many days"}},"additionalProperties":false},
                    "output":{"type":"object","required":["id","key","name","created_at"]},
                    "reasons":{"auth.key_cannot_mint":"A limited API key cannot make or revoke API keys"}},
                "auth.keys.list": {"kind":"query","description":"Your API keys, by name (never the keys themselves)",
                    "output":{"type":"object","required":["keys"],"properties":{"keys":{"type":"array","items":{"type":"object","required":["id","name","created_at"]}}}}},
                "auth.keys.revoke": {"kind":"command","description":"Revoke one of your API keys; whatever used it is signed out at once",
                    "input":{"type":"object","required":["id"],"properties":{"id":{"type":"string","minLength":1}},"additionalProperties":false},
                    "output":{"type":"object"},
                    "reasons":{"auth.no_key":"You have no API key with that id"}},
                "auth.password.change": {"kind":"command","description":"Change your own password by giving the current one",
                    "input":{"type":"object","required":["current","new"],"properties":{"current":{"type":"string","minLength":1,"maxLength":1000},"new":pw_in},"additionalProperties":false},
                    "output":{"type":"object"},
                    "reasons":{"auth.bad_credentials":"The current password is wrong"}},
                "auth.account.list": {"kind":"query","permission":"auth.manage","description":"Every account, without passwords",
                    "output":{"type":"object","required":["accounts"],"properties":{"accounts":{"type":"array","items":{"type":"object","required":["subject","username"]}}}}},
                "auth.account.create": {"kind":"command","permission":"auth.manage","description":"Create an account",
                    "input":{"type":"object","required":["username","password"],"properties":{"username":username_in,"display":{"type":"string","maxLength":200},"password":pw_in,"roles":{"type":"array","items":{"type":"string"}}},"additionalProperties":false},
                    "output":{"type":"object","required":["subject"]},
                    "reasons":{"auth.username_taken":"That username is already used","auth.bad_username":"A username is 2 to 32 letters, numbers, dots, dashes or underscores"}},
                "auth.account.update": {"kind":"command","permission":"auth.manage","description":"Rename an account, change its display name or roles, disable or enable it",
                    "input":{"type":"object","required":["subject"],"properties":{"subject":{"type":"string","minLength":1},"username":username_in,"display":{"type":"string","maxLength":200},"roles":{"type":"array","items":{"type":"string"}},"disabled":{"type":"boolean"}},"additionalProperties":false},
                    "output":{"type":"object"},
                    "reasons":{"auth.username_taken":"That username is already used","auth.bad_username":"A username is 2 to 32 letters, numbers, dots, dashes or underscores","auth.last_admin":"At least one enabled administrator must remain","auth.no_account":"There is no such account"}},
                "auth.account.set_password": {"kind":"command","permission":"auth.manage","description":"Set the password of an account",
                    "input":{"type":"object","required":["subject","password"],"properties":{"subject":{"type":"string","minLength":1},"password":pw_in},"additionalProperties":false},
                    "output":{"type":"object"},
                    "reasons":{"auth.no_account":"There is no such account"}}
            }
        })).unwrap()
    }

    fn init_data(&self, tx: &mut Tx) -> Result<()> {
        // The first time this data is set up, the accounts the Profile names become accounts in the database.
        for a in tx.settings()["accounts"].as_array().cloned().unwrap_or_default() {
            let name = username_of(&a);
            let roles = if a["roles"].is_array() { roles_of(&a["roles"]) } else { vec!["member".into()] };
            create_account(tx, name, a["display"].as_str().unwrap_or(name), a["password_hash"].as_str().unwrap_or(""), &roles, a["subject"].as_str())?;
        }
        Ok(())
    }

    fn bind(&self, b: &mut Bindings) {
        b.op("auth.sign_in", |tx, input| {
            let name = input["username"].as_str().unwrap_or("");
            let password = input["password"].as_str().unwrap_or("");
            let subject = tx.get(NAMES, &name_key(name))?.and_then(|d| d.body["subject"].as_str().map(String::from));
            let account = match &subject {
                Some(s) => tx.get(ACCOUNTS, s)?.filter(|a| a.body["disabled"] != true),
                None => None,
            };
            // Always do the same work, so timing does not say whether the name exists.
            let stored = account.as_ref().and_then(|a| a.body["password_hash"].as_str()).unwrap_or("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").to_string();
            let ok = PasswordHash::new(&stored).map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()).unwrap_or(false);
            let (Some(acc), Some(subject)) = (account.filter(|_| ok), subject) else {
                return Err(Error::reason(Category::Unauthenticated, "auth.bad_credentials", "The username or password is wrong"));
            };
            let who = identity_of(&subject, &acc.body);
            let token = format!("{}{}", tx.new_id(), tx.new_id());
            let ttl = tx.settings()["session_hours"].as_i64().map(|h| h * 3_600_000).unwrap_or(DEFAULT_TTL_MS);
            let expires = tx.now() + ttl;
            tx.put(SESSIONS, &token_hash(&token), json!({"expires_at": expires, "subject": who.subject}), Expect::Any)?;
            Ok(json!({"token": token, "expires_at": expires, "who": who}))
        });
        b.op("auth.refresh", |tx, input| {
            let id = token_hash(input["token"].as_str().unwrap_or(""));
            let Some(d) = tx.get(SESSIONS, &id)? else { return Err(Error::unauthenticated()) };
            if d.body["subject"] != tx.who().subject.as_str() || d.body["expires_at"].as_i64().unwrap_or(0) <= tx.now() {
                return Err(Error::unauthenticated());
            }
            let ttl = tx.settings()["session_hours"].as_i64().map(|h| h * 3_600_000).unwrap_or(DEFAULT_TTL_MS);
            let expires = tx.now() + ttl;
            let mut body = d.body.clone();
            body["expires_at"] = json!(expires);
            tx.put(SESSIONS, &id, body, Expect::Rev(d.rev))?;
            Ok(json!({"expires_at": expires}))
        });
        b.op("auth.sign_out", |tx, input| {
            let id = token_hash(input["token"].as_str().unwrap_or(""));
            if let Some(d) = tx.get(SESSIONS, &id)? {
                if d.body["subject"] == tx.who().subject.as_str() {
                    tx.delete(SESSIONS, &id)?;
                }
            }
            Ok(json!({}))
        });
        b.op("auth.who", |tx, _| Ok(serde_json::to_value(tx.who()).unwrap()));
        b.op("auth.keys.create", |tx, input| {
            let who = tx.who().clone();
            if who.subject.is_empty() {
                return Err(Error::new(Category::Unauthenticated, "kernel.unauthenticated", "sign in first"));
            }
            let key = format!("{API_KEY_PREFIX}{}{}", tx.new_id(), tx.new_id());
            // a limited key cannot make keys: it would otherwise make itself an unlimited one (ADR 0132). A key with no limits
            // acts as its person in this too, as before.
            if who.limits.is_some() {
                return Err(Error::new(Category::Denied, "auth.key_cannot_mint", "a limited API key cannot make API keys; sign in as the person").with_reason("auth.key_cannot_mint"));
            }
            let (id, name, now) = (tx.new_id(), input["name"].as_str().unwrap_or("").trim().to_string(), tx.now());
            let mut doc = json!({"id": id, "subject": who.subject, "name": name, "created_at": now});
            if let Some(list) = input.get("permissions").and_then(|p| p.as_array()) {
                let names: Vec<&str> = list.iter().filter_map(|p| p.as_str()).map(str::trim).filter(|p| !p.is_empty() && p.len() <= 80).collect();
                if names.is_empty() || names.len() != list.len() {
                    return Err(Error::invalid("permissions", "is a list of permission names, at least one"));
                }
                doc["limits"] = json!({"permissions": names});
            }
            if let Some(days) = input.get("expires_in_days").and_then(|d| d.as_i64()) {
                doc["expires_at"] = json!(now + days * 86_400_000);
                // an end date is a limit too: such a key may not make a key without one
                if doc.get("limits").is_none() {
                    doc["limits"] = json!({});
                }
            }
            tx.put(API_KEYS, &token_hash(&key), doc.clone(), Expect::Absent)?;
            Ok(json!({"id": id, "key": key, "name": name, "created_at": now, "permissions": doc["limits"]["permissions"], "expires_at": doc.get("expires_at")}))
        });
        b.op("auth.keys.list", |tx, _| {
            let subject = tx.who().subject.clone();
            let page = tx.query(API_KEYS, &Query::new().eq("subject", subject).limit(500))?;
            let mut keys: Vec<Value> = page.items.iter().map(|d| json!({"id": d.body["id"], "name": d.body["name"], "created_at": d.body["created_at"], "permissions": d.body["limits"]["permissions"], "expires_at": d.body["expires_at"]})).collect();
            keys.sort_by_key(|k| k["created_at"].as_i64().unwrap_or(0));
            Ok(json!({"keys": keys}))
        });
        b.op("auth.keys.revoke", |tx, input| {
            if tx.who().limits.is_some() {
                return Err(Error::new(Category::Denied, "auth.key_cannot_mint", "a limited API key cannot manage API keys; sign in as the person").with_reason("auth.key_cannot_mint"));
            }
            let subject = tx.who().subject.clone();
            let page = tx.query(API_KEYS, &Query::new().eq("id", input["id"].as_str().unwrap_or("")).eq("subject", subject).limit(1))?;
            let Some(d) = page.items.first() else { return Err(Error::reason(Category::NotFound, "auth.no_key", "You have no API key with that id")) };
            tx.delete(API_KEYS, &d.id)?;
            Ok(json!({}))
        });
        b.op("auth.account.list", |tx, _| {
            let page = tx.query(ACCOUNTS, &Query::new().limit(1000))?;
            let mut accounts: Vec<Value> = page.items.iter().map(|d| public_account(&d.id, &d.body)).collect();
            accounts.sort_by_key(|a| a["username"].as_str().unwrap_or("").to_lowercase());
            Ok(json!({"accounts": accounts}))
        });
        b.op("auth.account.create", |tx, input| {
            let name = input["username"].as_str().unwrap_or("");
            if !valid_username(name) {
                return Err(bad_username());
            }
            let roles = if input["roles"].is_array() { roles_of(&input["roles"]) } else { vec!["member".into()] };
            let hash = AuthPassword::hash_password(input["password"].as_str().unwrap_or(""));
            let subject = create_account(tx, name, input["display"].as_str().unwrap_or(name), &hash, &roles, None)?;
            Ok(json!({"subject": subject}))
        });
        b.op("auth.account.update", |tx, input| {
            let subject = input["subject"].as_str().unwrap_or("");
            let Some(d) = tx.get(ACCOUNTS, subject)? else { return Err(Error::reason(Category::NotFound, "auth.no_account", "There is no such account")) };
            let mut body = d.body.clone();
            if let Some(n) = input["username"].as_str() {
                let current = username_of(&body).to_string();
                if name_key(n) != name_key(&current) {
                    if !valid_username(n) {
                        return Err(bad_username());
                    }
                    tx.put(NAMES, &name_key(n), json!({"subject": subject}), Expect::Absent).map_err(|_| Error::reason(Category::Conflict, "auth.username_taken", "That username is already used"))?;
                    tx.delete(NAMES, &name_key(&current))?;
                }
                body["username"] = json!(n.trim());
            }
            if let Some(v) = input["display"].as_str() {
                body["display"] = json!(v);
            }
            if input["roles"].is_array() {
                body["roles"] = json!(roles_of(&input["roles"]));
            }
            if let Some(v) = input["disabled"].as_bool() {
                body["disabled"] = json!(v);
            }
            let still_admin = body["disabled"] != true && roles_of(&body["roles"]).iter().any(|r| r == "admin");
            let was_admin = d.body["disabled"] != true && roles_of(&d.body["roles"]).iter().any(|r| r == "admin");
            if was_admin && !still_admin && !another_admin_remains(tx, subject)? {
                return Err(Error::reason(Category::Conflict, "auth.last_admin", "At least one enabled administrator must remain"));
            }
            tx.put(ACCOUNTS, subject, body, Expect::Rev(d.rev))?;
            Ok(json!({}))
        });
        b.op("auth.password.change", |tx, input| {
            // Only the caller's own account: there is no way to name another one.
            let subject = tx.who().subject.clone();
            let Some(d) = tx.get(ACCOUNTS, &subject)?.filter(|a| a.body["disabled"] != true) else { return Err(Error::unauthenticated()) };
            let stored = d.body["password_hash"].as_str().unwrap_or("").to_string();
            let ok = PasswordHash::new(&stored).map(|h| Argon2::default().verify_password(input["current"].as_str().unwrap_or("").as_bytes(), &h).is_ok()).unwrap_or(false);
            if !ok {
                return Err(Error::reason(Category::Unauthenticated, "auth.bad_credentials", "The current password is wrong"));
            }
            let mut body = d.body.clone();
            body["password_hash"] = json!(AuthPassword::hash_password(input["new"].as_str().unwrap_or("")));
            tx.put(ACCOUNTS, &subject, body, Expect::Rev(d.rev))?;
            Ok(json!({}))
        });
        b.op("auth.account.set_password", |tx, input| {
            let subject = input["subject"].as_str().unwrap_or("");
            let Some(d) = tx.get(ACCOUNTS, subject)? else { return Err(Error::reason(Category::NotFound, "auth.no_account", "There is no such account")) };
            let mut body = d.body.clone();
            body["password_hash"] = json!(AuthPassword::hash_password(input["password"].as_str().unwrap_or("")));
            tx.put(ACCOUNTS, subject, body, Expect::Rev(d.rev))?;
            Ok(json!({}))
        });
    }

    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Auth(Arc::new(Password))))
    }
}
