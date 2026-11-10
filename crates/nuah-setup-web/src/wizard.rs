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
use nuah_shell::edit::{first_account_plugin, first_account_problem, has_accounts, put_first_account, ProfileStore};
use nuah_kernel::setup::redact_profile;
use nuah_kernel::{Core, ShellServices, Problem, Profile};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct SetupConfig {
    /// The identity file and the catalog it is checked against; `saved` is told when a Profile has been written,
    /// so a host that started setup because no file existed can go on to serve it.
    pub store: Arc<ProfileStore>,
    /// The one-time code the operator types into the wizard.
    pub code: String,
    /// `/faces/setup` and so on; the server config's hosts apply.
    pub server: Config,
}

#[derive(Clone)]
struct Setup(Arc<SetupConfig>);

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
    match s.0.store.state() {
        Ok(body) => Json(body).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "setup.state_failed", &e),
    }
}

/// Read a draft from the body and put back the secrets it did not retype. `first_account` (name, password) is
/// not part of a Profile: it becomes the first administrator in the settings of the plugin that keeps accounts,
/// as a password hash, when `hash` is true.
fn draft(s: &Setup, body: &Value, hash: bool) -> Result<(Profile, Vec<Problem>), Response> {
    let mut body = body.clone();
    let first = body.as_object_mut().and_then(|o| o.remove("first_account"));
    let mut p = s.0.store.draft(&body).map_err(|m| refuse(StatusCode::BAD_REQUEST, "invalid", "setup.bad_profile", &m))?;
    let mut problems = vec![];
    if let Some(plugin) = first_account_plugin(&s.0.store.catalog, &p) {
        let name = first.as_ref().and_then(|f| f["name"].as_str()).unwrap_or("").to_string();
        let password = first.as_ref().and_then(|f| f["password"].as_str()).unwrap_or("").to_string();
        if !has_accounts(&p, &plugin) {
            match first_account_problem(&plugin, &name, &password) {
                Some(problem) => problems.push(problem),
                None if hash => put_first_account(&mut p, &plugin, &name, &password),
                None => {}
            }
        }
    }
    Ok((p, problems))
}

async fn check(State(s): State<Setup>, Json(body): Json<Value>) -> Response {
    match draft(&s, &body, false) {
        Err(r) => r,
        Ok((p, mut problems)) => {
            problems.extend(Core::check_for_setup(&s.0.store.dist(p)));
            Json(json!({"problems": problems})).into_response()
        }
    }
}

async fn save(State(s): State<Setup>, Json(body): Json<Value>) -> Response {
    let (p, problems) = match draft(&s, &body, true) {
        Err(r) => return r,
        Ok(x) => x,
    };
    let shown = redact_profile(&s.0.store.dist(p.clone()));
    match s.0.store.commit(p, problems) {
        Ok(problems) if problems.is_empty() => Json(json!({"ok": true, "saved": s.0.store.path.display().to_string(), "profile": shown})).into_response(),
        Ok(problems) => (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"ok": false, "problems": problems}))).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "setup.write_failed", &e),
    }
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
