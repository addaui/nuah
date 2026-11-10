// SPDX-License-Identifier: Apache-2.0
//! The terminal setup writes the same identity file the web wizard does (ADR 0041).

use nuah_adapters::*;
use nuah_host::{Bootstrap, SetupContext};
use nuah_kernel::*;
use nuah_setup_cli::SetupCli;
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::Arc;

fn catalog() -> Catalog {
    let list: Vec<Arc<dyn Plugin>> = vec![Arc::new(AdapterMemory), Arc::new(AuthPassword), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds), Arc::new(nuah_http::Http::new())];
    Catalog(list)
}

fn start(file: &std::path::Path) -> SetupContext {
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["db-memory", "auth-password", "policy-roles", "clock-system", "ids-random", "http"].iter().map(|s| s.to_string()).collect();
    p.grants.insert("http".into(), vec!["serve".into()]);
    SetupContext { product: "t".into(), catalog: catalog(), starting_point: p, identity_file: file.to_path_buf(), listen: None, allow_hosts: vec![], until_saved: true }
}

fn run(file: &std::path::Path, answers: &str) -> std::result::Result<String, String> {
    let out = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    struct W(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for W {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let cli = SetupCli::with_io(Cursor::new(answers.to_string().into_bytes()), W(out.clone()));
    cli.run(start(file)).map(|_| String::from_utf8_lossy(&out.lock().unwrap()).to_string())
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nuah-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_terminal_asks_for_the_first_administrator_and_writes_a_profile_that_resolves() {
    let d = dir("ok");
    let file = d.join("t.json");
    // keep the plugins; listen elsewhere; a short password is refused; the two entries must match
    run(&file, "\n127.0.0.1:4100\nada\nshort\nlong-enough-1\nnot-the-same\nlong-enough-1\nlong-enough-1\n").unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(saved["settings"]["http"]["listen"], "127.0.0.1:4100");
    let acc = &saved["settings"]["auth-password"]["accounts"][0];
    assert_eq!(acc["name"], "ada");
    assert_eq!(acc["roles"], json!(["admin"]));
    assert!(acc["password_hash"].as_str().unwrap().starts_with("$argon2"), "the password is stored hashed");
    assert!(!std::fs::read_to_string(&file).unwrap().contains("long-enough-1"));
    assert!(saved["contract"].is_string());
    // and the file starts, with that account
    let p = nuah_kernel::setup::profile_from_json(&saved).unwrap();
    let core = Core::start(&Distribution { name: "t".into(), catalog: catalog(), profile: p }).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(core.call_as(&Identity::new("", "", &[]), "auth.sign_in", json!({"name": "ada", "password": "long-enough-1"}), None).is_ok());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn choices_that_do_not_resolve_write_nothing() {
    let d = dir("bad");
    let file = d.join("t.json");
    // change plugins: turn the policy off, which auth-password needs
    let answers = "y\nn\ny\nn\ny\ny\ny\n127.0.0.1:4100\nada\nlong-enough-1\nlong-enough-1\n";
    let e = run(&file, answers).unwrap_err();
    assert!(e.contains("nothing was written"), "{e}");
    assert!(!file.exists());
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn running_out_of_input_is_an_error_not_a_hang() {
    let d = dir("eof");
    let e = run(&d.join("t.json"), "").unwrap_err();
    assert!(e.contains("cut short"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}
