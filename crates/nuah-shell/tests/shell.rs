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
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"username": "root", "password_hash": AuthPassword::hash_password("first-password"), "roles": ["admin"]}]}));
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
    let out = c.call_as(&Identity::new("", "", &[]), "auth.sign_in", json!({"username": name, "password": pw}), None)?.output;
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
    core.call_as(&root, "auth.account.create", json!({"username": "sam", "password": "sam-secret-1", "roles": ["member"]}), None).unwrap();
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

#[test]
fn the_identity_file_is_nuah_json_unless_one_is_named() {
    let d = dir("identity");
    assert_eq!(nuah_shell::identity_path(None, Some(d.clone())), d.join("nuah.json"));
    assert_eq!(nuah_shell::identity_path(Some("x.json".into()), Some(d.clone())), std::path::PathBuf::from("x.json"));
}

#[test]
fn a_draft_is_checked_against_the_address_the_instance_listens_on_so_setup_and_console_see_what_start_would() {
    // the program listens off loopback (a container): a draft with local sign-in is a Problem now, not at the next start
    let d = dir("reach");
    let mut local = profile(false);
    local.enabled.retain(|e| e != "@nuah/auth-password");
    local.enabled.push("@nuah/auth-local".into());
    local.settings.remove("@nuah/auth-password");
    let mut cat = catalog().0;
    cat.push(Arc::new(AuthLocal));
    let store = ProfileStore::new("t", Catalog(cat), local.clone(), d.join("t.json"));
    assert!(store.check(&serde_json::to_value(&local).unwrap()).unwrap().iter().all(|p| p.code != "exceeds_reach"), "nothing says it is reachable yet");

    store.set_default_reach(Some(Reach::of_listen("0.0.0.0:3000")));
    let draft = serde_json::to_value(&local).unwrap();
    assert!(store.check(&draft).unwrap().iter().any(|p| p.code == "exceeds_reach"), "setup must see the problem before saving");

    // the operator accepts it: the same draft goes through, and nothing is forbidden
    let mut accepted = draft.clone();
    accepted["accepted_risks"] = json!(["@nuah/auth-local"]);
    assert!(store.check(&accepted).unwrap().iter().all(|p| p.code != "exceeds_reach"));
    assert_eq!(Reach::of_listen("127.0.0.1:3000"), Reach::ThisComputer);
    assert_eq!(Reach::of_listen("[::1]:3000"), Reach::ThisComputer);
    assert_eq!(Reach::of_listen("192.168.1.8:3000"), Reach::Network);
}

// ---- a password the person picks in the wizard for the settings area (ADR 0088) ---------------------

#[test]
fn a_settings_password_is_asked_for_hashed_and_stored_where_the_plugin_says() {
    use nuah_shell::edit::{has_settings_password, put_settings_password, settings_password_problem, settings_password_slot};
    let mut cat = catalog().0;
    cat.push(Arc::new(AuthOpen));
    let cat = Catalog(cat);
    let mut p = profile(false);
    p.enabled.retain(|e| e != "@nuah/auth-password");
    p.settings.remove("@nuah/auth-password");
    p.enabled.push("@nuah/auth-open".into());

    // the plugin names the setting that holds the hash; nothing is stored yet
    let (plugin, key) = settings_password_slot(&cat, &p).expect("auth-open asks for a settings password");
    assert_eq!((plugin.as_str(), key.as_str()), ("@nuah/auth-open", "admin_password_hash"));
    assert!(!has_settings_password(&p, &plugin, &key));

    // too short is a Problem, long enough is stored as a hash and never as the password
    assert!(settings_password_problem(&plugin, "short").is_some());
    assert!(settings_password_problem(&plugin, "long enough pw").is_none());
    put_settings_password(&mut p, &plugin, &key, "long enough pw");
    let stored = p.settings["@nuah/auth-open"]["admin_password_hash"].as_str().unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains("long enough pw"));
    assert!(has_settings_password(&p, &plugin, &key));

    // a plugin that does not ask, or one that is off, has no slot
    let off = profile(false);
    assert!(settings_password_slot(&cat, &off).is_none());
}

/// A plugin with nothing but an id and the plugins it requires.
struct Bare(&'static str, &'static [&'static str]);
impl Plugin for Bare {
    fn manifest(&self) -> Manifest {
        let ns = self.0.trim_start_matches("@t/");
        serde_json::from_value(json!({"id": self.0, "version": "1.0.0", "namespaces": [ns], "requires": {"plugins": self.1}})).unwrap()
    }
}

#[test]
fn starting_without_a_plugin_also_leaves_out_the_plugins_that_need_it() {
    let mut list = catalog().0;
    list.push(Arc::new(Bare("@t/base", &[])));
    list.push(Arc::new(Bare("@t/mid", &["@t/base"])));
    list.push(Arc::new(Bare("@t/top", &["@t/mid"])));
    list.push(Arc::new(Bare("@t/free", &[])));
    let mut p = profile(false);
    p.enabled.extend(["@t/base", "@t/mid", "@t/top", "@t/free"].map(String::from));
    let (q, removed) = nuah_shell::edit::without(&Catalog(list), &p, "@t/base");
    assert_eq!(removed, vec!["@t/mid".to_string(), "@t/top".to_string()]);
    assert!(!q.enabled.iter().any(|x| x.starts_with("@t/") && x != "@t/free"));
    assert!(q.enabled.contains(&"@t/free".to_string()));
}

/// A plugin that is only its Manifest.
struct RawM(Value);
impl Plugin for RawM {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(self.0.clone()).unwrap()
    }
}

#[test]
fn switching_off_the_plugin_that_defines_a_point_takes_out_the_plugins_that_need_it() {
    let mut list = catalog().0;
    list.push(Arc::new(RawM(json!({"id": "@t/editor", "version": "1.0.0", "namespaces": ["ed"],
        "defines": {"extension_points": {"ed.element": {"policy": "many", "version": "1.0.0", "input": {}}}}}))));
    list.push(Arc::new(RawM(json!({"id": "@t/el", "version": "1.0.0", "namespaces": ["el"], "requires": {"extension_points": ["ed.element@^1"]}}))));
    let mut p = profile(false);
    p.enabled.extend(["@t/editor", "@t/el"].map(String::from));
    let (q, removed) = nuah_shell::edit::without(&Catalog(list), &p, "@t/editor");
    assert_eq!(removed, vec!["@t/el".to_string()]);
    assert!(!q.enabled.contains(&"@t/el".to_string()));
}

#[test]
fn schema_check_prints_every_problem_and_says_whether_it_passes() {
    let old = json!({"version": 1, "collections": {"t.a": {"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}}}});
    let ok = json!({"version": 2, "collections": {"t.a": {"type": "object", "properties": {"c": {"type": "string", "renamed_from": ["a"]}, "b": {"type": "string"}}}}});
    let (text, pass) = nuah_shell::schema_check(&old, &ok);
    assert!(pass, "{text}");
    assert!(text.contains("t.a") && text.contains("rename a → c"), "{text}");
    let bad = json!({"version": 2, "collections": {"t.a": {"type": "object", "properties": {"b": {"type": "integer"}, "x": {"type": "string", "renamed_from": ["q"]}, "q": {"type": "string"}}}}});
    let (text, pass) = nuah_shell::schema_check(&old, &bad);
    assert!(!pass);
    for want in ["field_missing", "t.a › a", "type_changed", "name_reused"] {
        assert!(text.contains(want), "{want} missing from:\n{text}");
    }
    let (text, pass) = nuah_shell::schema_check(&old, &json!({"collections": {}}));
    assert!(!pass && text.contains("version"), "{text}");
}
