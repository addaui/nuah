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
        "auth-local"
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
            "id":"auth-local","version":"0.2.0","name":"Local sign-in","description":"One person, always signed in as admin. Only for a Core that nobody else can reach.","fills":"auth",
            "settings_schema":{"type":"object","properties":{"name":{"type":"string","minLength":1}},"additionalProperties":false}
        })).unwrap()
    }
    fn fill(&self, s: &Value) -> Result<Option<PortImpl>> {
        let name = s.get("name").and_then(|n| n.as_str()).unwrap_or("me");
        Ok(Some(PortImpl::Auth(Arc::new(Local(Identity::new("local", name, &["admin"]))))))
    }
}

const SESSIONS: &str = "auth.sessions";
const DEFAULT_TTL_MS: i64 = 12 * 60 * 60 * 1000;

fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// Accounts named in the Profile; sessions are stored data the Adapter owns.
struct Password;
impl Auth for Password {
    fn name(&self) -> &str {
        "auth-password"
    }
    fn authenticate(&self, db: &dyn DbRead, credential: &str, now: Millis) -> Result<Option<Identity>> {
        let Some(d) = db.get(SESSIONS, &token_hash(credential))?.filter(|d| !d.deleted) else { return Ok(None) };
        if d.body["expires_at"].as_i64().unwrap_or(0) <= now {
            return Ok(None);
        }
        Ok(serde_json::from_value(d.body["who"].clone()).ok())
    }
}

pub struct AuthPassword;

impl AuthPassword {
    /// Make the stored form of a password for a Profile's `accounts`.
    pub fn hash_password(password: &str) -> String {
        let salt = SaltString::generate(&mut rand_core::OsRng);
        Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
    }
}

impl Plugin for AuthPassword {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "auth-password", "version": "0.2.0", "name": "Password sign-in",
            "description": "Accounts named in the Profile; sign-in gives an expiring session",
            "fills": "auth", "namespaces": ["auth"],
            "requires": {"ports": ["clock", "ids"]},
            "data": {"collections": [SESSIONS]},
            "settings_schema": {"type":"object","required":["accounts"],"properties":{
                "session_hours": {"type":"integer","minimum":1,"maximum":720},
                "accounts": {"x-secret":true,"type":"array","minItems":1,"items":{"type":"object","required":["name","password_hash"],
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
                "auth.who": {"kind":"query","description":"Who am I?","output":{"type":"object","required":["subject"]}}
            }
        })).unwrap()
    }

    fn bind(&self, b: &mut Bindings) {
        b.op("auth.sign_in", |tx, input| {
            let name = input["name"].as_str().unwrap_or("");
            let password = input["password"].as_str().unwrap_or("");
            let settings = tx.settings();
            let account = settings["accounts"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name)).cloned();
            // Always do the same work, so timing does not say whether the name exists.
            let stored = account.as_ref().and_then(|a| a["password_hash"].as_str()).unwrap_or("$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").to_string();
            let ok = PasswordHash::new(&stored).map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()).unwrap_or(false);
            let Some(acc) = account.filter(|_| ok) else {
                return Err(Error::reason(Category::Unauthenticated, "auth.bad_credentials", "The name or password is wrong"));
            };
            let roles: Vec<String> = acc["roles"].as_array().map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_else(|| vec!["member".into()]);
            let who = Identity { subject: name.into(), display: acc["display"].as_str().unwrap_or(name).into(), roles };
            let token = format!("{}{}", tx.new_id(), tx.new_id());
            let ttl = settings["session_hours"].as_i64().map(|h| h * 3_600_000).unwrap_or(DEFAULT_TTL_MS);
            let expires = tx.now() + ttl;
            tx.put(SESSIONS, &token_hash(&token), json!({"who": who, "expires_at": expires, "subject": who.subject}), Expect::Any)?;
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
    }

    fn fill(&self, _s: &Value) -> Result<Option<PortImpl>> {
        Ok(Some(PortImpl::Auth(Arc::new(Password))))
    }
}
