// SPDX-License-Identifier: Apache-2.0
//! What the host offers plugins and operators (ADR 0041): the console changes the identity file through the
//! `shell` capability, and a locked-out operator can set a password from the command line.

use nuah_adapters::*;
use nuah_shell::edit::{write_profile, ProfileStore};
use nuah_kernel::*;
use serde_json::{json, Value};
use std::sync::Arc;

fn catalog() -> Catalog {
    let list: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds),
        Arc::new(nuah_http::Http::new()), Arc::new(nuah_console::Console),
    ];
    Catalog(list)
}

fn profile(grant_host: bool) -> Profile {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http", "@nuah/console"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"name": "root", "password_hash": AuthPassword::hash_password("first-password"), "roles": ["admin"]}]}));
    p.settings.insert("@nuah/http".into(), json!({"listen": "127.0.0.1:0"}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    if grant_host {
        p.grants.insert("@nuah/console".into(), vec!["shell".into()]);
    }
    p
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nuah-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start(p: &Profile) -> Core {
    Core::start(&Distribution { name: "t".into(), catalog: catalog(), profile: p.clone() }).unwrap_or_else(|e| panic!("{e:?}"))
}

fn sign_in(c: &Core, name: &str, pw: &str) -> Result<Identity> {
    let out = c.call_as(&Identity::new("", "", &[]), "auth.sign_in", json!({"name": name, "password": pw}), None)?.output;
    Ok(serde_json::from_value(out["who"].clone()).unwrap())
}

#[test]
fn the_console_reads_and_changes_the_identity_file_and_asks_for_a_restart() {
    let d = dir("console");
    let path = d.join("t.json");
    let p = profile(true);
    write_profile(&path, &p).unwrap();
    let store = Arc::new(ProfileStore::new("t", catalog(), p.clone(), path.clone()));
    let core = start(&p);
    core.attach_shell(store.clone());
    let root = sign_in(&core, "root", "first-password").unwrap();

    // reading shows the catalog and hides the account hashes
    let got = core.call_as(&root, "console.profile.get", json!({}), None).unwrap().output;
    assert!(got["plugins"].as_array().unwrap().iter().any(|x| x["id"] == "@nuah/console"));
    assert!(!got.to_string().contains("$argon2"), "a password hash never reaches the browser");

    // a draft that changes a setting, with the hidden secret sent back as it came, is saved and the old file is kept
    let mut draft = got["profile"].clone();
    draft["plugins"]["@nuah/auth-password"]["settings"]["session_hours"] = json!(5);
    let saved = core.call_as(&root, "console.profile.save", json!({"profile": draft}), None).unwrap().output;
    assert_eq!(saved["restart_required"], true);
    let now: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(now["plugins"]["@nuah/auth-password"]["settings"]["session_hours"], 5);
    assert!(now["plugins"]["@nuah/auth-password"]["settings"]["accounts"][0]["password_hash"].as_str().unwrap().starts_with("$argon2"), "the secret the draft did not retype is kept");
    assert!(path.with_extension("json.bak").exists());
    assert!(now["contract"].is_string(), "the contract lock is written");

    // a draft that does not resolve writes nothing
    let before = std::fs::read_to_string(&path).unwrap();
    let mut bad = got["profile"].clone();
    bad["plugins"]["@test/no-such-plugin"] = json!({"enabled": true});
    let e = core.call_as(&root, "console.profile.save", json!({"profile": bad}), None).unwrap_err();
    assert_eq!(e.reason.as_deref(), Some("console.problems"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    // check reports the same Problems without writing
    let c = core.call_as(&root, "console.profile.check", json!({"profile": bad}), None).unwrap().output;
    assert!(!c["problems"].as_array().unwrap().is_empty());

    // restart is a request the host loop waits for
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    core.call_as(&root, "console.restart", json!({}), None).unwrap();
    rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(2), store.restart.notified()).await.expect("the host was asked to restart") });

    // only administrators may
    core.call_as(&root, "auth.account.create", json!({"name": "sam", "password": "sam-secret-1", "roles": ["member"]}), None).unwrap();
    let sam = sign_in(&core, "sam", "sam-secret-1").unwrap();
    assert_eq!(core.call_as(&sam, "console.profile.get", json!({}), None).unwrap_err().category, Category::Denied);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_console_needs_the_host_capability_granted_and_a_host_that_offers_it() {
    // not granted: the Resolver names the plugin, so the instance does not start half-trusted
    let problems = Core::check(&Distribution { name: "t".into(), catalog: catalog(), profile: profile(false) });
    assert!(problems.iter().any(|x| x.code == "capability_not_granted" && x.plugins == ["@nuah/console"] && x.message.contains("shell")), "{problems:?}");
    // granted, but the program running the Core attached no services (an embedding that is not the host): a clear reason, nothing written
    let core = start(&profile(true));
    let root = sign_in(&core, "root", "first-password").unwrap();
    for (op, input) in [("console.profile.get", json!({})), ("console.profile.save", json!({"profile": {}})), ("console.restart", json!({}))] {
        let e = core.call_as(&root, op, input, None).unwrap_err();
        assert_eq!(e.reason.as_deref(), Some("console.no_shell"), "{op}");
    }
}

#[test]
fn a_locked_out_operator_can_set_a_password_from_the_command_line() {
    let core = start(&profile(true));
    assert!(sign_in(&core, "root", "wrong").is_err());
    nuah_shell::reset_password(&core, "ROOT", "a-new-password").unwrap();
    assert!(sign_in(&core, "root", "first-password").is_err());
    assert!(sign_in(&core, "root", "a-new-password").is_ok());
    assert!(nuah_shell::reset_password(&core, "nobody", "a-new-password").unwrap_err().contains("no account"));
}

struct Tiny;
impl nuah_shell::Product for Tiny {
    fn name(&self) -> &str {
        "tiny"
    }
    fn catalog(&self) -> Catalog {
        catalog()
    }
    fn starting_point(&self, _home: &std::path::Path) -> Profile {
        Profile::default()
    }
}

#[test]
fn the_identity_file_is_nuah_json_and_an_old_product_named_file_is_carried_over() {
    let d = dir("identity");
    // nothing there: the new name, whatever the product is called
    assert_eq!(nuah_shell::identity_path(&Tiny, None, Some(d.clone())), d.join("nuah.json"));
    // an explicit file wins
    assert_eq!(nuah_shell::identity_path(&Tiny, Some("x.json".into()), Some(d.clone())), std::path::PathBuf::from("x.json"));
    // only the old name: it is copied, the new name is used from now on, and the old file is left alone
    std::fs::write(d.join("tiny.json"), "{\"name\":\"old\"}").unwrap();
    assert_eq!(nuah_shell::identity_path(&Tiny, None, Some(d.clone())), d.join("nuah.json"));
    assert_eq!(std::fs::read_to_string(d.join("nuah.json")).unwrap(), "{\"name\":\"old\"}");
    assert!(d.join("tiny.json").exists());
    // both exist: the new name wins and is not overwritten
    std::fs::write(d.join("nuah.json"), "{\"name\":\"new\"}").unwrap();
    assert_eq!(nuah_shell::identity_path(&Tiny, None, Some(d.clone())), d.join("nuah.json"));
    assert_eq!(std::fs::read_to_string(d.join("nuah.json")).unwrap(), "{\"name\":\"new\"}");
}
