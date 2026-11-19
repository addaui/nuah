// SPDX-License-Identifier: Apache-2.0
//! The "starting" page (ADR 0091, point 6): while the Core upgrades data at start it is not answering, so the setup
//! Face says how far the upgrade has come, on the address the Core will use, and gives the address back when it is done.

use nuah_kernel::upgrade::UpgradeProgress;
use nuah_kernel::*;
use nuah_setup_web::SetupWeb;
use nuah_shell::{Bootstrap, SetupContext};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

struct Named;
impl Plugin for Named {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({"id": "@t/tasks", "version": "1.0.0", "name": "Tasks", "namespaces": ["tasks"]})).unwrap()
    }
}

fn get(addr: &str, path: &str) -> (u16, String) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    (status, out.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default())
}

#[test]
fn the_starting_page_says_how_far_the_upgrade_is_and_frees_the_address_when_stopped() {
    let addr = { let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().to_string() };
    let faces = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../faces");
    let ctx = SetupContext {
        product: "madar".into(), catalog: Catalog(vec![Arc::new(Named)]), starting_point: Profile::default(),
        identity_file: std::env::temp_dir().join("nuah-starting-unused.json"), listen: Some(addr.clone()), allow_hosts: vec![],
        faces_dir: faces, until_saved: true,
    };
    let progress = Arc::new(Mutex::new(Some(UpgradeProgress { phase: "upgrading".into(), plugin: "@t/tasks".into(), collection: "tasks.tasks".into(), done: 500, total: 2000, collection_number: 1, collections: 2 })));
    let stop = SetupWeb::new().starting(ctx, progress.clone()).expect("the setup Face can show the page");

    let (status, body) = get(&addr, "/upgrade/progress");
    assert_eq!(status, 200);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["product"].as_str(), v["name"].as_str(), v["done"].as_u64(), v["total"].as_u64(), v["collections"].as_u64()), (Some("madar"), Some("Tasks"), Some(500), Some(2000), Some(2)));

    progress.lock().unwrap().as_mut().unwrap().done = 1500;
    let v: Value = serde_json::from_str(&get(&addr, "/upgrade/progress").1).unwrap();
    assert_eq!(v["done"], 1500, "each request reads the progress as it is now");

    assert_eq!(get(&addr, "/healthz").0, 503, "a health check does not call the instance ready");
    let (status, _) = get(&addr, "/faces/madar/tasks");
    assert!(status == 307 || status == 303, "any other address leads to the page, got {status}");

    stop();
    std::net::TcpListener::bind(&addr).expect("the address is free for the Core once the page stops");
}
