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
            "requires":{"capabilities":["emit"]},
            "operations":{"echo.say":{"kind":"query","input":{"type":"object","properties":{"text":{"type":"string"}}}},
                "echo.wave":{"kind":"command","input":{"type":"object"}}}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("echo.say", |tx, i| Ok(json!({"text": i["text"], "who": tx.who().subject})));
        b.op("echo.wave", |tx, _| {
            let me = tx.who().subject.clone();
            tx.signal("echo.here", Audience::subject(&me), json!({"hand": "up"}))?;
            Ok(json!({}))
        });
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(port: u16) -> (Core, Services) {
    let hash = AuthPassword::hash_password("pw");
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-password", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@test/echo", "@nuah/http"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/auth-password".into(), json!({"accounts": [{"username": "alice", "password_hash": hash, "roles": ["member"]}]}));
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    p.grants.insert("@test/echo".into(), vec!["emit".into()]);
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
    let (s, signed) = post(&base, "auth.sign_in", json!({"username": "alice", "password": "pw"}), None, None);
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
    let (s, body) = post(&base, "auth.sign_in", json!({"username": "alice", "password": "pw"}), None, Some("evil.example"));
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
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http"].iter().map(|s| s.to_string()).collect();
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

struct ConnectPlugin(&'static str, Vec<&'static str>);
impl Plugin for ConnectPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.0, "version": "1", "runtime": "data", "connect": self.1})).unwrap()
    }
}

#[test]
fn the_browser_may_call_only_the_sites_enabled_plugins_ask_for() {
    let port = free_port();
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http", "@test/gifs"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    // a path, a wildcard and plain http are refused; a valid origin is allowed once
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(Http::new()),
        std::sync::Arc::new(ConnectPlugin("@test/gifs", vec!["https://api.example.com", "https://api.example.com", "https://evil.example.com/path", "https://*.example.com", "http://plain.example.com", "https://x"])),
    ];
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(core.connect_origins(), vec!["https://api.example.com".to_string()]);
    let services = core.start_services().unwrap();
    let r = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap();
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("connect-src 'self' https://api.example.com;"), "{csp}");
    assert!(!csp.contains("evil") && !csp.contains('*'));
    services.stop();
}

struct ImagesPlugin(&'static str, Vec<&'static str>);
impl Plugin for ImagesPlugin {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.0, "version": "1", "runtime": "data", "images": self.1})).unwrap()
    }
}

#[test]
fn pictures_load_only_from_this_server_and_the_sites_enabled_plugins_declare() {
    let port = free_port();
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http", "@test/gifs"].iter().map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(Http::new()),
        std::sync::Arc::new(ImagesPlugin("@test/gifs", vec!["https://media.example.com", "https://media.example.com", "https://evil.example.com/p", "https://*.example.com", "http://plain.example.com"])),
    ];
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(core.image_origins(), vec!["https://media.example.com".to_string()]);
    let services = core.start_services().unwrap();
    let r = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap();
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("img-src 'self' data: blob: https://media.example.com;"), "{csp}");
    assert!(csp.contains("media-src 'self' blob: https://media.example.com;"), "{csp}");
    assert!(!csp.contains("evil") && !csp.contains('*') && !csp.contains(" https:;") && !csp.contains(" https: "), "{csp}");
    services.stop();
}

#[test]
fn without_a_declared_site_no_picture_loads_from_anywhere_but_this_server() {
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}")}));
    let r = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap();
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("img-src 'self' data: blob:;"), "{csp}");
    let img = csp.split(';').find(|d| d.trim_start().starts_with("img-src")).unwrap();
    assert!(!img.contains("https"), "{csp}");
    services.stop();
}

#[test]
fn without_a_plugin_asking_the_browser_calls_only_this_server() {
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}")}));
    let r = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap();
    assert!(r.headers().get("content-security-policy").unwrap().to_str().unwrap().contains("connect-src 'self';"));
    services.stop();
}

fn face_with_assets(tag: &str) -> String {
    let d = std::path::PathBuf::from(face_dir(tag, "<!doctype html><title>x</title>"));
    std::fs::create_dir_all(d.join("assets")).unwrap();
    std::fs::write(d.join("assets/app-3f9a1c.js"), "const answer = 42;\n".repeat(400)).unwrap();
    std::fs::write(d.join("assets/pic-77aa.png"), vec![0x89u8; 3000]).unwrap();
    d.to_string_lossy().to_string()
}

/// A plain HTTP/1.1 request, headers as the server sent them (an HTTP client library hides `Content-Encoding` when it decodes for you).
fn raw_get(url: &str, accept: &str) -> RawResponse {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
    let mut s = std::net::TcpStream::connect(host).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\nAccept-Encoding: {accept}\r\nConnection: close\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let headers = head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    RawResponse { headers }
}
struct RawResponse { headers: Vec<(String, String)> }
impl RawResponse {
    fn header(&self, name: &str) -> Option<&str> { self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()) }
}

#[test]
fn face_files_are_compressed_and_hashed_files_are_cached_for_good() {
    let web = face_with_assets("cache");
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}"), "faces": {"/faces/web": web}}));
    let base = format!("http://127.0.0.1:{port}/faces/web");
    let js = format!("{base}/assets/app-3f9a1c.js");

    // text is compressed for a client that accepts it, in the best way it names, and not for one that does not
    let gz = raw_get(&js, "gzip");
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    assert!(gz.header("vary").is_some_and(|v| v.to_lowercase().contains("accept-encoding")));
    assert_eq!(raw_get(&js, "br, gzip").header("content-encoding"), Some("br"));
    assert_eq!(raw_get(&js, "identity").header("content-encoding"), None);
    // a picture is already compressed: left alone
    assert_eq!(raw_get(&format!("{base}/assets/pic-77aa.png"), "gzip").header("content-encoding"), None);

    // a file with a hash in its name never changes; the page that names it always may
    let forever = Some("public, max-age=31536000, immutable");
    assert_eq!(raw_get(&js, "identity").header("cache-control"), forever);
    assert_eq!(raw_get(&format!("{base}/assets/pic-77aa.png"), "identity").header("cache-control"), forever);
    assert_eq!(raw_get(&format!("{base}/"), "identity").header("cache-control"), Some("no-cache"));
    services.stop();
}

#[test]
fn the_link_and_the_event_stream_are_not_cached_or_compressed_by_the_face_rules() {
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}")}));
    let r = raw_get(&format!("http://127.0.0.1:{port}/healthz"), "gzip, br");
    assert_eq!(r.header("content-encoding"), None);
    assert!(!r.header("cache-control").is_some_and(|v| v.contains("immutable")));
    services.stop();
}

#[test]
fn only_a_file_with_a_hash_in_an_assets_folder_counts_as_hashed() {
    use nuah_http::is_hashed_asset as h;
    assert!(h("/faces/web/assets/index-DkH1kXDf.js") && h("/assets/react-CRL5HtpL.js") && h("/assets/noto-sans-arabic-CSfveiSG.woff2") && h("/a/assets/x-ab_12.css"));
    // no assets folder, no hash, too short a hash, or no name
    for p in ["/faces/web/index.html", "/faces/web/vendor-editor.js", "/assets/logo.png", "/assets/app-ab.js", "/assets/-abcd1234.js", "/assets/", "/assets/app-abcd1234", "/assets/a-b c d1.js"] {
        assert!(!h(p), "{p}");
    }
}

#[test]
fn a_configured_path_opens_a_face_so_a_browser_path_not_a_hash_route_reaches_it() {
    let (console, web) = (face_dir("fp-console", "CONSOLE"), face_dir("fp-web", "WEB"));
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}"), "face_paths": {"/share": "web", "tools/": "console"}, "faces": {"/faces/console": console, "/faces/web": web}}));
    let at = |p: &str| get_text(&format!("http://127.0.0.1:{port}{p}"));
    assert_eq!(at("/share"), (200, "WEB".to_string()));
    assert_eq!(at("/share/abc123"), (200, "WEB".to_string()), "any depth below it");
    assert_eq!(at("/share/a/b/c"), (200, "WEB".to_string()));
    assert_eq!(at("/tools/x").1, "CONSOLE", "a path may be written with or without its slashes");
    // the Face's own files and the Link are untouched
    assert_eq!(at("/faces/console/").1, "CONSOLE");
    assert_eq!(agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap().status().as_u16(), 200);
    services.stop();
}

struct FontData(&'static str, Vec<&'static str>);
impl Plugin for FontData {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": self.0, "version": "1", "runtime": "data", "fonts": {"schema": "fonts/1", "families": []}, "font_origins": self.1})).unwrap()
    }
}

/// A store that answers `fonts.get` the way a font plugin does: base64 bytes for a key it holds.
struct FontStore;
impl Plugin for FontStore {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/font-store", "version": "1", "namespaces": ["fonts"],
            "operations": {"fonts.get": {"kind": "query", "public": true, "input": {"type": "object", "properties": {"key": {"type": "string"}}}}}})).unwrap()
    }
    fn bind(&self, b: &mut Bindings) {
        b.op("fonts.get", |_tx, i| match i["key"].as_str() {
            Some("abc123") => Ok(json!({"data": "d09GMgABAAA=", "type": "font/woff2"})),
            _ => Err(Error::not_found("font")),
        });
    }
}

fn start_with_fonts(port: u16, extra: Vec<std::sync::Arc<dyn Plugin>>, ids: &[&str]) -> (Core, Services) {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http"].iter().chain(ids.iter()).map(|s| s.to_string()).collect();
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let mut list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds),
        std::sync::Arc::new(Http::new()),
    ];
    list.extend(extra);
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let services = core.start_services().unwrap();
    (core, services)
}

#[test]
fn fonts_load_only_from_this_server_and_the_sites_a_font_plugin_declares() {
    let port = free_port();
    let (core, services) = start_with_fonts(port, vec![std::sync::Arc::new(FontData("@test/fonts-a", vec!["https://fonts.example.com", "https://*.example.com", "http://plain.example.com"]))], &["@test/fonts-a"]);
    assert_eq!(core.font_origins(), vec!["https://fonts.example.com".to_string()]);
    let r = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap();
    let csp = r.headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("font-src 'self' https://fonts.example.com;"), "{csp}");
    assert!(!csp.contains('*') && !csp.contains("plain.example.com"), "{csp}");
    services.stop();

    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}")}));
    let csp = agent().get(&format!("http://127.0.0.1:{port}/healthz")).call().unwrap().headers().get("content-security-policy").unwrap().to_str().unwrap().to_string();
    assert!(csp.contains("font-src 'self';"), "{csp}");
    services.stop();
}

#[test]
fn a_font_plugin_is_only_data_and_describe_lists_its_families() {
    let port = free_port();
    let (core, services) = start_with_fonts(port, vec![std::sync::Arc::new(FontData("@test/fonts-a", vec![]))], &["@test/fonts-a"]);
    let m = FontData("@test/fonts-a", vec![]).manifest();
    assert_eq!(m.kind(), PluginKind::Font);
    assert_eq!(m.role(), "font");
    let d = core.describe(&Identity::new("a", "A", &[]));
    assert_eq!(d["fonts"][0]["id"], "@test/fonts-a");
    assert_eq!(d["fonts"][0]["families"]["schema"], "fonts/1");
    services.stop();
}

#[test]
fn a_stored_font_is_served_to_anyone_and_cached_for_good() {
    let port = free_port();
    let (_core, services) = start_with_fonts(port, vec![std::sync::Arc::new(FontStore)], &["@test/font-store"]);
    let r = agent().get(&format!("http://127.0.0.1:{port}/fonts/abc123.woff2")).call().unwrap();
    assert_eq!(r.headers().get("content-type").unwrap(), "font/woff2");
    assert!(r.headers().get("cache-control").unwrap().to_str().unwrap().contains("immutable"));
    let mut body = r.into_body();
    assert_eq!(body.read_to_vec().unwrap(), vec![0x77, 0x4f, 0x46, 0x32, 0x00, 0x01, 0x00, 0x00]);
    for bad in ["nope.woff2", "abc123", "..%2Fx.woff2", "abc123.ttf"] {
        let r = agent().get(&format!("http://127.0.0.1:{port}/fonts/{bad}")).call();
        let status = match r { Ok(r) => r.status().as_u16(), Err(ureq::Error::StatusCode(c)) => c, Err(e) => panic!("{e}") };
        assert_eq!(status, 404, "{bad}");
    }
    services.stop();
}

#[test]
fn the_event_stream_carries_signals_without_an_id() {
    use std::io::{BufRead, BufReader, Write};
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let (_core, services) = start(port);
    let token = post(&base, "auth.sign_in", json!({"username": "alice", "password": "pw"}), None, None).1["output"]["token"].as_str().unwrap().to_string();
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(sock, "GET /pulse HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nX-Nuah-Face: test\r\nAccept: text/event-stream\r\n\r\n").unwrap();
    let mut lines = BufReader::new(sock.try_clone().unwrap()).lines();
    // wait until the stream is ready, then wave
    while !lines.next().unwrap().unwrap().contains("event: ready") {}
    let ready: Value = serde_json::from_str(lines.next().unwrap().unwrap().strip_prefix("data: ").unwrap()).unwrap();
    let me = ready["subject"].as_str().unwrap().to_string();
    assert_eq!(post(&base, "echo.wave", json!({}), Some(&token), None).0, 200);
    let mut block = vec![];
    loop {
        let l = lines.next().expect("the stream ended").expect("no Signal within 5 s");
        if l.contains("event: signal") {
            block.push(l);
            loop {
                let l = lines.next().unwrap().unwrap();
                if l.is_empty() { break; }
                block.push(l);
            }
            break;
        }
    }
    assert!(!block.iter().any(|l| l.starts_with("id:")), "a Signal never moves the cursor: {block:?}");
    let data = block.iter().find_map(|l| l.strip_prefix("data: ")).expect("data line");
    let v: Value = serde_json::from_str(data).unwrap();
    assert_eq!((v["kind"].as_str(), v["data"]["hand"].as_str(), v["actor"].as_str()), (Some("echo.here"), Some("up"), Some(me.as_str())), "{v}");
    drop(sock);
    services.stop();
}

#[test]
fn the_policy_lets_webassembly_compile_but_never_lets_a_script_eval() {
    let p = nuah_http::csp(&[], &[], &[]);
    assert!(p.contains("script-src 'self' 'wasm-unsafe-eval'"), "{p}");
    assert!(!p.contains("'unsafe-eval'") && !p.contains("'unsafe-inline'"), "{p}");
}

#[test]
fn a_faces_service_worker_may_look_after_the_whole_site_and_nothing_else_may() {
    let web = face_dir("sw", "WEB");
    std::fs::write(std::path::Path::new(&web).join("sw.js"), "// worker").unwrap();
    std::fs::write(std::path::Path::new(&web).join("other.js"), "// not a worker").unwrap();
    let port = free_port();
    let (_core, services) = start_with_http(json!({"listen": format!("127.0.0.1:{port}"), "faces": {"/faces/web": web}}));
    let r = raw_get(&format!("http://127.0.0.1:{port}/faces/web/sw.js"), "identity");
    assert_eq!(r.header("service-worker-allowed"), Some("/"));
    assert_eq!(raw_get(&format!("http://127.0.0.1:{port}/faces/web/other.js"), "identity").header("service-worker-allowed"), None);
    services.stop();
}

// ---- Browser modules (ADR 0095) ---------------------------------------------------------------------

struct Mod;
impl Plugin for Mod {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@test/mod", "version": "1.0.0", "namespaces": ["mod"],
            "defines": {"extension_points": {"mod.view": {"policy": "many", "browser": true, "input": {}}}},
            "provides": [{"extension_point": "mod.view"}], "browser": {"mod.view": "index.js"}})).unwrap()
    }
    fn browser_files(&self) -> &'static [(&'static str, &'static [u8])] {
        &[("index.js", b"export default function register() {}\n"), ("lib/py.wasm", b"\0asm")]
    }
}

fn start_mod(on: bool) -> (Core, Services, u16) {
    let port = free_port();
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@nuah/http"].iter().map(|s| s.to_string()).collect();
    if on {
        p.enabled.push("@test/mod".into());
    }
    p.settings.insert("@nuah/http".into(), json!({"listen": format!("127.0.0.1:{port}")}));
    p.grants.insert("@nuah/http".into(), vec!["serve".into()]);
    let list: Vec<std::sync::Arc<dyn Plugin>> = vec![
        std::sync::Arc::new(AdapterMemory), std::sync::Arc::new(AuthLocal), std::sync::Arc::new(PolicyRoles), std::sync::Arc::new(StdClock), std::sync::Arc::new(StdIds), std::sync::Arc::new(Http::new()), std::sync::Arc::new(Mod),
    ];
    let core = Core::start(&Distribution { name: "t".into(), catalog: Catalog(list), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    let services = core.start_services().unwrap();
    (core, services, port)
}

/// Status, lower-cased headers and body of a GET sent as written (no path clean-up by a client).
fn get_raw(port: u16, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    get_raw_with(port, path, "")
}
/// The same, with one more header line (`Name: value\r\n`).
fn get_raw_with(port: u16, path: &str, extra: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAccept-Encoding: identity\r\n{extra}Connection: close\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect();
    (status, headers, buf[end + 4..].to_vec())
}
fn hdr<'a>(h: &'a [(String, String)], n: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
}

#[test]
fn browser_module_served_with_hash_cache() {
    let (core, services, port) = start_mod(true);
    let url = core.describe_public()["browser"][0]["url"].as_str().unwrap().to_string();
    let (st, h, body) = get_raw(port, &url);
    assert_eq!(st, 200);
    assert_eq!(hdr(&h, "content-type"), Some("text/javascript; charset=utf-8"));
    assert_eq!(hdr(&h, "cache-control"), Some("public, max-age=31536000, immutable"));
    assert_eq!(body, b"export default function register() {}\n");
    let (st, h, _) = get_raw(port, "/plugins/test/mod/lib/py.wasm");
    assert_eq!((st, hdr(&h, "content-type")), (200, Some("application/wasm")));
    services.stop();
}

#[test]
fn browser_module_wrong_hash_not_cached() {
    let (_core, services, port) = start_mod(true);
    let (st, h, _) = get_raw(port, "/plugins/test/mod/index.js?v=000000000000");
    assert_eq!((st, hdr(&h, "cache-control")), (200, Some("no-cache")));
    let (st, h, _) = get_raw(port, "/plugins/test/mod/index.js");
    assert_eq!((st, hdr(&h, "cache-control")), (200, Some("no-cache")));
    services.stop();
}

#[test]
fn browser_file_without_hash_is_revalidated_not_downloaded_again() {
    // files a module loads by relative path (chunks, a runtime's .wasm) carry no ?v: the browser asks, and gets 304 while unchanged
    let (_core, services, port) = start_mod(true);
    let (st, h, body) = get_raw(port, "/plugins/test/mod/lib/py.wasm");
    assert_eq!(st, 200);
    assert!(!body.is_empty());
    let etag = hdr(&h, "etag").expect("an ETag").to_string();
    let (st, h2, body) = get_raw_with(port, "/plugins/test/mod/lib/py.wasm", &format!("If-None-Match: {etag}\r\n"));
    assert_eq!(st, 304);
    assert!(body.is_empty());
    assert_eq!(hdr(&h2, "etag"), Some(etag.as_str()));
    let (st, _, _) = get_raw_with(port, "/plugins/test/mod/lib/py.wasm", "If-None-Match: \"something-else\"\r\n");
    assert_eq!(st, 200);
    services.stop();
}

#[test]
fn browser_module_of_disabled_plugin_404() {
    let (_core, services, port) = start_mod(false);
    assert_eq!(get_raw(port, "/plugins/test/mod/index.js").0, 404);
    services.stop();
}

#[test]
fn browser_module_path_traversal_404() {
    let (_core, services, port) = start_mod(true);
    assert_eq!(get_raw(port, "/plugins/test/mod/../mod/index.js").0, 404);
    assert_eq!(get_raw(port, "/plugins/test/mod/lib/../index.js").0, 404);
    assert_eq!(get_raw(port, "/plugins/test/mod/nothing.js").0, 404);
    services.stop();
}

#[test]
fn csp_unchanged_by_browser_modules() {
    let (_core, services, port) = start_mod(true);
    let (_, h, _) = get_raw(port, "/plugins/test/mod/index.js");
    assert_eq!(hdr(&h, "content-security-policy").map(String::from), Some(nuah_http::csp(&[], &[], &[])));
    services.stop();
}
