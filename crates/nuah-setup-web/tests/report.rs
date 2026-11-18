// SPDX-License-Identifier: Apache-2.0
//! The Upgrade report (ADR 0091): what the page shows when a plugin's data cannot be upgraded, and starting without it.

use nuah_adapters::*;
use nuah_kernel::*;
use nuah_setup_web::{report_state, start_without};
use nuah_shell::edit::{write_profile, ProfileStore};
use serde_json::json;
use std::sync::Arc;

struct Bare(&'static str, &'static [&'static str]);
impl Plugin for Bare {
    fn manifest(&self) -> Manifest {
        let ns = self.0.trim_start_matches("@t/");
        serde_json::from_value(json!({"id": self.0, "version": "1.0.0", "name": ns, "namespaces": [ns], "requires": {"plugins": self.1}})).unwrap()
    }
}

fn store(name: &str) -> ProfileStore {
    let list: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(AdapterMemory), Arc::new(AuthLocal), Arc::new(PolicyRoles), Arc::new(StdClock), Arc::new(StdIds),
        Arc::new(Bare("@t/base", &[])), Arc::new(Bare("@t/mid", &["@t/base"])), Arc::new(Bare("@t/free", &[])),
    ];
    let mut p = Profile { name: "t".into(), ..Default::default() };
    p.enabled = ["@nuah/db-memory", "@nuah/auth-local", "@nuah/policy-roles", "@nuah/clock-system", "@nuah/ids-random", "@t/base", "@t/mid", "@t/free"].map(String::from).to_vec();
    let d = std::env::temp_dir().join(format!("nuah-report-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("nuah.json");
    write_profile(&path, &p).unwrap();
    ProfileStore::new("t", Catalog(list), Profile::default(), path)
}

fn refused() -> Problem {
    Problem {
        code: "upgrade_refused".into(),
        plugins: vec!["@t/base".into()],
        message: "@t/base: base.docs › \"level\" is missing".into(),
        details: json!({"problems": [{"plugin": "@t/base", "collection": "base.docs", "path": "level", "code": "field_missing", "params": {}}],
                        "plugins": {"@t/base": {"name": "base", "from_release": "0.3.0", "to_release": "0.4.0", "from_version": 1, "to_version": 2}}}),
    }
}

#[test]
fn report_lists_problems_versions_and_what_starting_without_takes_out() {
    let s = store("state");
    let state = report_state(&s, &refused()).unwrap();
    assert_eq!(state["product"], "t");
    assert_eq!(state["problems"][0]["path"], "level");
    assert_eq!(state["plugins"]["@t/base"]["to_release"], "0.4.0");
    assert_eq!(state["without"]["@t/base"], json!(["@t/mid"]));
}

#[test]
fn start_without_writes_the_profile_without_the_plugin_and_its_dependants() {
    let s = store("without");
    let removed = start_without(&s, &refused(), "@t/base").unwrap();
    assert_eq!(removed, vec!["@t/base".to_string(), "@t/mid".to_string()]);
    let p = s.stored().unwrap().unwrap();
    assert!(!p.enabled.iter().any(|x| x == "@t/base" || x == "@t/mid"));
    assert!(p.enabled.iter().any(|x| x == "@t/free"));
}

#[test]
fn start_without_refuses_a_plugin_the_report_does_not_name() {
    let s = store("other");
    assert!(start_without(&s, &refused(), "@t/free").is_err());
    assert!(s.stored().unwrap().unwrap().enabled.iter().any(|x| x == "@t/free"));
}
