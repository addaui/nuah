// SPDX-License-Identifier: Apache-2.0
//! The console is a plain script Face: every element id its script asks for must exist in its page. The setup Face is a built
//! React app (`faces-src/setup`, committed under `faces/nuah/setup-web`): its page must load its own files and, because the server's
//! Content-Security-Policy forbids them, carry no inline script and no inline style.

use std::path::PathBuf;

fn faces() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../faces/nuah")
}

fn read(p: &str) -> String {
    std::fs::read_to_string(faces().join(p)).unwrap_or_else(|e| panic!("{p}: {e}"))
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
fn every_id_the_console_script_asks_for_is_in_its_page() {
    let html = read("console/index.html");
    let js = read("console/app.js");
    // ids the script makes itself (dialog fields, per plugin switches) are built with a prefix or concatenation
    for id in ids_asked_for(&js) {
        let made_by_script = js.contains(&format!("id: '{id}'")) || js.contains(&format!("id: \"{id}\""));
        if id.is_empty() || made_by_script || id.starts_with("tab-") || id.starts_with("d-") {
            continue;
        }
        assert!(html.contains(&format!("id=\"{id}\"")), "console: the script asks for #{id}, which the page does not have");
    }
    for t in ["overview", "config", "accounts", "ops", "live"] {
        assert!(html.contains(&format!("id=\"tab-{t}\"")), "console: no panel for the {t} tab");
    }
}

#[test]
fn the_built_setup_face_loads_its_own_files_and_has_nothing_inline() {
    let html = read("setup-web/index.html");
    assert!(html.contains("<div id=\"root\">"), "the page has the place the app mounts");
    for tag in ["src=\"/faces/setup/assets/", "href=\"/faces/setup/assets/"] {
        assert!(html.contains(tag), "the page loads its files from /faces/setup/assets/ ({tag})");
    }
    // each file the page names is there
    for part in html.split('"').filter(|p| p.starts_with("/faces/setup/assets/")) {
        let file = part.trim_start_matches("/faces/setup/");
        assert!(faces().join("setup-web").join(file).is_file(), "{file} is named by the page but missing: run `bun run build` in faces-src/setup");
    }
    assert!(!html.contains("<style"), "no inline style: the server's policy forbids it");
    assert!(!html.contains("style=\""), "no inline style attribute");
    let scripts = html.matches("<script").count();
    assert_eq!(scripts, html.matches("<script type=\"module\" crossorigin src=").count(), "every script is a file, none inline");
}
