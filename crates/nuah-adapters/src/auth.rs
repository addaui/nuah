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
            "id":"@nuah/auth-local","version":"0.2.0","name":"Local sign-in","description":"One person, always signed in as admin. Only for a Core that nobody else can reach.","fills":"auth",
            "settings_schema":{"type":"object","properties":{"name":{"type":"string","minLength":1}},"additionalProperties":false}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("me");
        Ok(Some(PortImpl::Auth(Arc::new(Local(Identity::new("local", name, &["admin"]))))))
    }
}

const SESSIONS: &str = "auth.sessions";
const ACCOUNTS: &str = "auth.accounts";
const NAMES: &str = "auth.names";
const DEFAULT_TTL_MS: i64 = 12 * 60 * 60 * 1000;

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The person an account document describes. The document's id is the `subject`: an opaque id made when the
/// account was created, kept when the login name changes, never reused ([ADR 0039]).
///
/// [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
fn identity_of(subject: &str, body: &Value) -> Identity {
    Identity {
        subject: subject.into(),
        display: body["display"].as_str().filter(|d| !d.is_empty()).or_else(|| body["name"].as_str()).unwrap_or(subject).into(),
        roles: body["roles"].as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default(),
    }
}

/// Accounts and sessions are data the Adapter owns, so they live wherever the data Adapter keeps data.
struct Password;
impl Auth for Password {
    fn name(&self) -> &str {
        "@nuah/auth-password"
    }
    fn authenticate(&self, db: &dyn DbRead, credential: &str, now: Millis) -> Result<Option<Identity>> {
        let Some(d) = db.get(SESSIONS, &token_hash(credential))?.filter(|d| !d.deleted) else { return Ok(None) };
        if d.body["expires_at"].as_i64().unwrap_or(0) <= now {
            return Ok(None);
        }
        // Who they are comes from the account as it is now, so a changed role or a disabled account applies at once.
        let Some(subject) = d.body["subject"].as_str() else { return Ok(None) };
        let Some(acc) = db.get(ACCOUNTS, subject)?.filter(|a| !a.deleted && a.body["disabled"] != true) else { return Ok(None) };
        Ok(Some(identity_of(subject, &acc.body)))
    }
    fn find(&self, db: &dyn DbRead, name: &str) -> Result<Option<Identity>> {
        let Some(subject) = db.get(NAMES, &name_key(name))?.filter(|d| !d.deleted).and_then(|d| d.body["subject"].as_str().map(String::from)) else { return Ok(None) };
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

fn create_account(tx: &mut Tx, name: &str, display: &str, password_hash: &str, roles: &[String]) -> Result<String> {
    let taken = || Error::reason(Category::Conflict, "auth.name_taken", "That name is already used");
    let key = name_key(name);
    if key.is_empty() || tx.get(NAMES, &key)?.is_some() {
        return Err(taken());
    }
    let subject = tx.new_id();
    let now = tx.now();
    tx.put(NAMES, &key, json!({"subject": subject}), Expect::Absent).map_err(|_| taken())?;
    tx.put(ACCOUNTS, &subject, json!({"name": name.trim(), "display": display, "password_hash": password_hash, "roles": roles, "disabled": false, "created_at": now}), Expect::Absent)?;
    Ok(subject)
}

fn roles_of(v: &Value) -> Vec<String> {
    v.as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn public_account(subject: &str, body: &Value) -> Value {
    json!({"subject": subject, "name": body["name"], "display": body["display"], "roles": body["roles"], "disabled": body["disabled"], "created_at": body["created_at"]})
}

/// Would this change leave nobody able to manage accounts?
fn another_admin_remains(tx: &Tx, except: &str) -> Result<bool> {
    let admins = tx.query(ACCOUNTS, &Query::new().has("roles", "admin").eq("disabled", false).limit(2))?;
    Ok(admins.items.iter().any(|d| d.id != except))
}

impl Plugin for AuthPassword {
    fn manifest(&self) -> Manifest {
        let name_in = json!({"type":"string","minLength":1,"maxLength":200});
        let pw_in = json!({"type":"string","minLength":8,"maxLength":1000});
        serde_json::from_value(json!({
            "id": "@nuah/auth-password", "version": "0.3.0", "name": "Password sign-in",
            "description": "Accounts with passwords, kept in the database; sign-in gives an expiring session",
            "fills": "auth", "namespaces": ["auth"],
            "requires": {"ports": ["db", "clock", "ids"]},
            "data": {"collections": [SESSIONS, ACCOUNTS, NAMES], "version": 1, "indexes": [{"collection": ACCOUNTS, "field": "disabled"}]},
            "permissions": {"auth.manage": {"description": "Create accounts, change their roles and passwords, disable them", "default": ["admin"]}},
            "settings_schema": {"type":"object","properties":{
                "session_hours": {"type":"integer","minimum":1,"maximum":720,"title":"Session length (hours)","description":"How long someone stays signed in without signing in again.","default":24},
                "accounts": {"x-secret":true,"x-first-account":"password-hash","description":"Accounts to create the first time the data is set up; after that they are managed in the database","type":"array","items":{"type":"object","required":["name","password_hash"],
                    "properties":{"name":{"type":"string","minLength":1},"display":{"type":"string"},
                    "password_hash":{"type":"string","minLength":1},"roles":{"type":"array","items":{"type":"string"}}},
                    "additionalProperties":false}}},
                "additionalProperties": false},
            "operations": {
                "auth.sign_in": {"kind":"command","public":true,"description":"Exchange a name and password for a session",
                    "input":{"type":"object","required":["name","password"],"properties":{"name":{"type":"string","minLength":1,"maxLength":200},"password":{"type":"string","minLength":1,"maxLength":1000}},"additionalProperties":false},
                    "output":{"type":"object","required":["token","expires_at","who"]},
                    "reasons":{"auth.bad_credentials":"The name or password is wrong"}},
                "auth.refresh": {"kind":"command","description":"Extend a session you hold",
                    "input":{"type":"object","required":["token"],"properties":{"token":{"type":"string","minLength":1}},"additionalProperties":false},
                    "output":{"type":"object","required":["expires_at"]}},
                "auth.sign_out": {"kind":"command","description":"End a session you hold",
                    "input":{"type":"object","required":["token"],"properties":{"token":{"type":"string","minLength":1}},"additionalProperties":false},
                    "output":{"type":"object"}},
                "auth.who": {"kind":"query","description":"Who am I?","output":{"type":"object","required":["subject"]}},
                "auth.account.list": {"kind":"query","permission":"auth.manage","description":"Every account, without passwords"},
                "auth.account.create": {"kind":"command","permission":"auth.manage","description":"Create an account",
                    "input":{"type":"object","required":["name","password"],"properties":{"name":name_in,"display":{"type":"string","maxLength":200},"password":pw_in,"roles":{"type":"array","items":{"type":"string"}}},"additionalProperties":false},
                    "output":{"type":"object","required":["subject"]},
                    "reasons":{"auth.name_taken":"That name is already used"}},
                "auth.account.update": {"kind":"command","permission":"auth.manage","description":"Rename an account, change its display name or roles, disable or enable it",
                    "input":{"type":"object","required":["subject"],"properties":{"subject":{"type":"string","minLength":1},"name":name_in,"display":{"type":"string","maxLength":200},"roles":{"type":"array","items":{"type":"string"}},"disabled":{"type":"boolean"}},"additionalProperties":false},
                    "reasons":{"auth.name_taken":"That name is already used","auth.last_admin":"At least one enabled administrator must remain","auth.no_account":"There is no such account"}},
                "auth.account.set_password": {"kind":"command","permission":"auth.manage","description":"Set the password of an account",
                    "input":{"type":"object","required":["subject","password"],"properties":{"subject":{"type":"string","minLength":1},"password":pw_in},"additionalProperties":false},
                    "reasons":{"auth.no_account":"There is no such account"}}
            }
        })).unwrap()
    }

    fn migrate(&self, from: u32, tx: &mut Tx) -> Result<()> {
        if from == 0 {
            // The first time this data is set up, the accounts the Profile names become accounts in the database.
            for a in tx.settings()["accounts"].as_array().cloned().unwrap_or_default() {
                let name = a["name"].as_str().unwrap_or("");
                let roles = if a["roles"].is_array() { roles_of(&a["roles"]) } else { vec!["member".into()] };
                create_account(tx, name, a["display"].as_str().unwrap_or(name), a["password_hash"].as_str().unwrap_or(""), &roles)?;
            }
        }
        Ok(())
    }

    fn bind(&self, b: &mut Bindings) {
        b.op("auth.sign_in", |tx, input| {
            let name = input["name"].as_str().unwrap_or("");
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
                return Err(Error::reason(Category::Unauthenticated, "auth.bad_credentials", "The name or password is wrong"));
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
        b.op("auth.account.list", |tx, _| {
            let page = tx.query(ACCOUNTS, &Query::new().limit(1000))?;
            let mut accounts: Vec<Value> = page.items.iter().map(|d| public_account(&d.id, &d.body)).collect();
            accounts.sort_by_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());
            Ok(json!({"accounts": accounts}))
        });
        b.op("auth.account.create", |tx, input| {
            let name = input["name"].as_str().unwrap_or("");
            let roles = if input["roles"].is_array() { roles_of(&input["roles"]) } else { vec!["member".into()] };
            let hash = AuthPassword::hash_password(input["password"].as_str().unwrap_or(""));
            let subject = create_account(tx, name, input["display"].as_str().unwrap_or(name), &hash, &roles)?;
            Ok(json!({"subject": subject}))
        });
        b.op("auth.account.update", |tx, input| {
            let subject = input["subject"].as_str().unwrap_or("");
            let Some(d) = tx.get(ACCOUNTS, subject)? else { return Err(Error::reason(Category::NotFound, "auth.no_account", "There is no such account")) };
            let mut body = d.body.clone();
            if let Some(n) = input["name"].as_str().filter(|n| name_key(n) != name_key(body["name"].as_str().unwrap_or(""))) {
                tx.put(NAMES, &name_key(n), json!({"subject": subject}), Expect::Absent).map_err(|_| Error::reason(Category::Conflict, "auth.name_taken", "That name is already used"))?;
                tx.delete(NAMES, &name_key(body["name"].as_str().unwrap_or("")))?;
                body["name"] = json!(n.trim());
            } else if let Some(n) = input["name"].as_str() {
                body["name"] = json!(n.trim());
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
