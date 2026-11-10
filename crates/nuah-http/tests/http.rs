// SPDX-License-Identifier: Apache-2.0
//! The `http` plugin on its own, with a one-operation plugin and no product around it (ADR 0031).

use nuah_adapters::*;
use nuah_http::Http;
use nuah_kernel::*;
use serde_json::{json, Value};
use std::time::Duration;

struct Echo;
impl Plugin for Echo {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id":"@test/echo","version":"1","namespaces":["echo"],
            "operations":{"echo.say":{"kind":"query","input":{"type":"object","properties":{"text":{"type":"string"}}}}}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("echo.say", |tx, i| Ok(json!({"text": i["text"], "who": tx.who().subject})));
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(port: u16) -> (Core, Services) {
    let hash = AuthPassword::hash_password("pw");
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@test/echo", "@nuah/http"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"name": "alice", "password_hash": hash, "roles": ["member"]}]}));
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let mut catalog = vec![];
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthPassword), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(Echo), std::sync::Arc::new(Http::new()),
    ];
    catalog.extend(list);
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(catalog), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let services = core.start_services().unwrap();
    (core, services)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(5))).build().into()
}

fn post(base: &str, op: &str, body: Value, bearer: Option<&str>, host: Option<&str>) -> (u16, Value) {
    let mut r = agent().post(&format!("{base}/link/call/{op}")).header("Content-Type", "application/json").header("X-Nuah-Face", "test");
    if let Some(t) = bearer {
        r = r.header("Authorization", &format!("Bearer {t}"));
    }
    if let Some(h) = host {
        r = r.header("Host", h);
    }
    let mut res = r.send(body.to_string()).unwrap();
    (res.status().as_u16(), res.body_mut().read_json().unwrap_or(Value::Null))
}

#[test]
fn a_product_less_core_is_served_over_http_and_stops() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let (_core, services) = start(port);
    assert_eq!(agent().get(&format!("{base}/healthz")).call().unwrap().status().as_u16(), 200);
    // before sign-in `describe` only says how to sign in
    let d: Value = agent().get(&format!("{base}/link/describe")).call().unwrap().body_mut().read_json().unwrap();
    assert!(d["operations"].get("echo.say").is_none(), "{d}");
    // an operation needs a session
    assert_eq!(post(&base, "echo.say", json!({"text": "hi"}), None, None).0, 401);
    let (s, signed) = post(&base, "auth.sign_in", json!({"name": "alice", "password": "pw"}), None, None);
    assert_eq!(s, 200, "{signed}");
    let token = signed["output"]["token"].as_str().unwrap().to_string();
    let (s, said) = post(&base, "echo.say", json!({"text": "hi"}), Some(&token), None);
    assert_eq!((s, said["output"]["text"].as_str()), (200, Some("hi")), "{said}");
    services.stop();
    assert!(agent().get(&format!("{base}/healthz")).call().is_err());
}

#[test]
fn the_guard_refuses_unknown_hosts_and_calls_without_the_face_header() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let (_core, services) = start(port);
    let (s, body) = post(&base, "auth.sign_in", json!({"name": "alice", "password": "pw"}), None, Some("evil.example"));
    assert_eq!(s, 421, "{body}");
    let mut res = agent().post(&format!("{base}/link/call/auth.sign_in")).header("Content-Type", "application/json").send("{}").unwrap();
    assert_eq!(res.status().as_u16(), 400, "{:?}", res.body_mut().read_to_string());
    services.stop();
}

#[test]
fn http_settings_say_where_they_live_and_how_a_change_applies() {
    let m = Http::new().manifest();
    let problems = setup::settings_annotation_problems(&m.settings_schema);
    assert!(problems.is_empty(), "{problems:?}");
}

#[test]
fn readiness_says_whether_the_core_can_read_its_data() {
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let (_core, services) = start(port);
    let mut r = agent().get(&format!("{base}/readyz")).call().unwrap();
    assert_eq!(r.status().as_u16(), 200);
    let v: Value = r.body_mut().read_json().unwrap();
    assert_eq!((v["status"].as_str(), v["ready"].as_bool()), (Some("ok"), Some(true)));
    assert!(v["checks"].as_array().unwrap().iter().any(|c| c["name"] == "db" && c["ok"] == true));
    services.stop();
}

fn start_with_http(http: Value) -> (Core, Services) {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "http"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/http".into(), http);
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds), std::sync::Arc::new(Http::new()),
    ];
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let services = core.start_services().unwrap();
    (core, services)
}

fn face_dir(tag: &str, marker: &str) -> String {
    let d = std::env::temp_dir().join(format!("nuah-root-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("index.html"), marker).unwrap();
    d.to_string_lossy().to_string()
}

fn get_text(url: &str) -> (u16, String) {
    let mut r = agent().get(url).call().unwrap();
    (r.status().as_u16(), r.body_mut().read_to_string().unwrap())
}

#[test]
fn the_root_path_shows_the_web_face_without_redirecting_unless_another_is_named() {
    let (console, web) = (face_dir("console", "CONSOLE"), face_dir("web", "WEB"));
    // `console` sorts before `web`, which used to send `/` to the console.
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}"), "faces": {"/faces/console": console, "/faces/web": web}}));
    assert_eq!(get_text(&format!("http://127.0.0.1:{port}/")), (200, "WEB".to_string()));
    assert_eq!(get_text(&format!("http://127.0.0.1:{port}/faces/console/")).1, "CONSOLE");
    services.stop();

    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}"), "root": "console", "faces": {"/faces/console": console, "/faces/web": web}}));
    assert_eq!(get_text(&format!("http://127.0.0.1:{port}/")), (200, "CONSOLE".to_string()));
    services.stop();
}

struct FacePlugin(&'static str, &'static str);
impl Plugin for FacePlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.0, "version": "1", "runtime": "data", "face": self.1})).unwrap()
    }
}

#[test]
fn a_plugin_that_publishes_a_face_is_hosted_from_the_faces_dir_under_its_id() {
    let base = std::env::temp_dir().join(format!("nuah-faces-{}", std::process::id()));
    for (id, marker) in [("test/my-site", "SITE"), ("test/my-admin", "ADMIN")] {
        std::fs::create_dir_all(base.join(id)).unwrap();
        std::fs::write(base.join(id).join("index.html"), marker).unwrap();
    }
    let other = face_dir("elsewhere", "ELSEWHERE");
    let port = free_port();
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http", "@test/my-site", "@test/my-admin"].iter().map(|s| s.to_string()).collect();
    // the admin Face is moved by the Profile; the site keeps the default place
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}"), "faces_dir": base.to_string_lossy(), "faces": {"/faces/console": other}}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(Http::new()), std::sync::Arc::new(FacePlugin("@test/my-site", "web")), std::sync::Arc::new(FacePlugin("@test/my-admin", "console")),
    ];
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let services = core.start_services().unwrap();
    assert_eq!(get_text(&format!("http://127.0.0.1:{port}/faces/web/")).1, "SITE");
    assert_eq!(get_text(&format!("http://127.0.0.1:{port}/faces/console/")).1, "ELSEWHERE");
    services.stop();
}
