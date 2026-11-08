// SPDX-License-Identifier: Apache-2.0
//! The setup wizard's server mode ([ADR 0014]): serves `/setup/*` and the wizard Face before any
//! Core exists. It starts no plugin and opens no database; it only reads manifests, asks the
//! Resolver, and writes a Profile file that resolves.
//!
//!   GET  /setup/state    the Catalog, the current Profile (secrets hidden), the current Problems
//!   POST /setup/check    Problems of a draft Profile
//!   POST /setup/save     write the draft only if it has no Problems
//!
//! Every request needs the one-time code in `X-Nuah-Setup`.
//!
//! [ADR 0014]: https://docs.addaui.com/nuah/adr/0014-setup-wizard-is-a-separate-mode-with-a-setup-code-and-no-secrets/

use nuah_http::{guard, header_str, refuse, Config};
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nuah_kernel::setup::{catalog_info, profile_from_json, redact_profile, restore_profile};
use nuah_kernel::{Catalog, Core, Distribution, Profile};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub struct SetupConfig {
    pub distribution: String,
    pub catalog: Catalog,
    /// What the wizard starts from when there is no file yet.
    pub starting_point: Profile,
    pub profile_path: PathBuf,
    /// The one-time code the operator types into the wizard.
    pub code: String,
    /// `/faces/setup` and so on; the server config's hosts apply.
    pub server: Config,
}

#[derive(Clone)]
struct Setup(Arc<SetupConfig>);

impl Setup {
    fn dist(&self, profile: Profile) -> Distribution {
        Distribution { name: self.0.distribution.clone(), catalog: self.0.catalog.clone(), profile }
    }

    /// The Profile on disk, or why it could not be read.
    fn stored(&self) -> Result<Option<Profile>, String> {
        match std::fs::read_to_string(&self.0.profile_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read {}: {e}", self.0.profile_path.display())),
            Ok(text) => {
                let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", self.0.profile_path.display()))?;
                profile_from_json(&v).map(Some)
            }
        }
    }
}

/// Equal-time comparison, so the code cannot be guessed from response timing.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn need_code(State(s): State<Setup>, req: Request, next: Next) -> Response {
    if !header_str(req.headers(), "x-nuah-setup").is_some_and(|c| same(c.trim(), &s.0.code)) {
        return refuse(StatusCode::UNAUTHORIZED, "unauthenticated", "setup.bad_code", "Enter the setup code the server printed when it started.");
    }
    next.run(req).await
}

async fn state(State(s): State<Setup>) -> Response {
    let (profile, load_error, exists) = match s.stored() {
        Ok(Some(p)) => (p, None, true),
        Ok(None) => (s.0.starting_point.clone(), None, false),
        Err(e) => (s.0.starting_point.clone(), Some(e), true),
    };
    let dist = s.dist(profile);
    let mut body = catalog_info(&dist);
    body["problems"] = json!(Core::check_for_setup(&dist));
    body["path"] = json!(s.0.profile_path.display().to_string());
    body["file_exists"] = json!(exists);
    body["load_error"] = json!(load_error);
    Json(body).into_response()
}

/// Read a draft from the body and put back the secrets it did not retype.
fn draft(s: &Setup, body: &Value) -> Result<Profile, Response> {
    let mut p = profile_from_json(body).map_err(|m| refuse(StatusCode::BAD_REQUEST, "invalid", "setup.bad_profile", &m))?;
    let stored = s.stored().ok().flatten().unwrap_or_default();
    let dist = s.dist(p.clone());
    restore_profile(&mut p, &stored, &dist);
    Ok(p)
}

async fn check(State(s): State<Setup>, Json(body): Json<Value>) -> Response {
    match draft(&s, &body) {
        Err(r) => r,
        Ok(p) => Json(json!({"problems": Core::check_for_setup(&s.dist(p))})).into_response(),
    }
}

async fn save(State(s): State<Setup>, Json(body): Json<Value>) -> Response {
    let p = match draft(&s, &body) {
        Err(r) => return r,
        Ok(p) => p,
    };
    let dist = s.dist(p.clone());
    let problems = Core::check_for_setup(&dist);
    if !problems.is_empty() {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"ok": false, "problems": problems}))).into_response();
    }
    match write_profile(&s.0.profile_path, &p) {
        Ok(()) => Json(json!({"ok": true, "saved": s.0.profile_path.display().to_string(), "profile": redact_profile(&dist)})).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "setup.write_failed", &format!("Could not write the file: {e}")),
    }
}

/// Write to a sibling file and rename, so a crash never leaves half a Profile. A new file is
/// private to the owner; an existing one keeps its permissions.
fn write_profile(path: &std::path::Path, p: &Profile) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = f.set_permissions(meta.permissions());
    }
    f.write_all(serde_json::to_string_pretty(p).expect("a Profile serialises").as_bytes())?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn setup_router(cfg: SetupConfig) -> Router {
    let server = Arc::new(Config { hosts: cfg.server.hosts.clone(), secure_cookie: false, faces: cfg.server.faces.clone(), ..Config::default() });
    let faces = cfg.server.faces.clone();
    let s = Setup(Arc::new(cfg));
    let api = Router::new()
        .route("/setup/state", get(state))
        .route("/setup/check", post(check))
        .route("/setup/save", post(save))
        .layer(middleware::from_fn_with_state(s.clone(), need_code))
        .with_state(s);
    let mut r = api.route("/healthz", get(|| async { Json(json!({"ok": true})) }));
    for (prefix, dir) in &faces {
        r = r.nest_service(prefix, tower_http::services::ServeDir::new(dir));
    }
    if let Some((prefix, _)) = faces.first() {
        let to = format!("{prefix}/");
        r = r.route("/", get(move || async move { axum::response::Redirect::temporary(&to) }));
    }
    r.layer(middleware::from_fn_with_state(server, guard))
}
