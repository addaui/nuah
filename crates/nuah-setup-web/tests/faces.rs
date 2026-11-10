// SPDX-License-Identifier: Apache-2.0
//! The console and the wizard each carry the same settings-form script (ADR 0042); they must not drift apart,
//! and every element id a Face's script asks for must exist in its page.

use std::path::PathBuf;

fn faces() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../faces/nuah")
}

fn read(p: &str) -> String {
    std::fs::read_to_string(faces().join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
}

#[test]
fn the_two_faces_share_one_settings_form_script() {
    assert_eq!(read("console/form.js"), read("setup-web/form.js"), "copy faces/nuah/console/form.js over faces/nuah/setup-web/form.js");
}

fn ids_asked_for(js: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = js;
    while let Some(i) = rest.find("$(\"").or_else(|| rest.find("$('")) {
        let after = &rest[i + 3..];
        let end = after.find(['"', '\'']).unwrap_or(0);
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

#[test]
fn every_id_a_face_script_asks_for_is_in_its_page() {
    for face in ["console", "setup-web"] {
        let html = read(&format!("{face}/index.html"));
        let js = read(&format!("{face}/app.js"));
        // ids the script makes itself (dialog fields, per plugin switches) are built with a prefix or concatenation
        for id in ids_asked_for(&js) {
            let made_by_script = js.contains(&format!("id: '{id}'")) || js.contains(&format!("id: \"{id}\""));
            if id.is_empty() || made_by_script || id.starts_with("tab-") || id.starts_with("d-") {
                continue;
            }
            assert!(html.contains(&format!("id=\"{id}\"")), "{face}: the script asks for #{id}, which the page does not have");
        }
        for t in ["overview", "config", "accounts", "ops", "live"].iter().filter(|_| face == "console") {
            assert!(html.contains(&format!("id=\"tab-{t}\"")), "console: no panel for the {t} tab");
        }
    }
}
