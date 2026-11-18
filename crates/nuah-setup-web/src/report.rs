// SPDX-License-Identifier: Apache-2.0
//! The Upgrade report ([ADR 0091]): when a plugin's stored data cannot be upgraded to its new schema, the Core does not
//! start and nothing is written. The shell hands the refusal to the product's bootstrap, and this serves only the setup
//! Face's report view, guarded by a one-time code printed on the terminal:
//!
//!   GET  /upgrade/state     the problems, each plugin's versions, and what starting without each plugin takes out
//!   POST /upgrade/without   {plugin}: write the identity file without it (and the plugins that need it), then the
//!                           shell starts the Core again
//!
//! [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use nuah_http::{guard, header_str, refuse, Config};
use nuah_kernel::Problem;
use nuah_shell::edit::{without, ProfileStore};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// What the report view shows: the product, every Upgrade problem, each named plugin's name and versions, and for each
/// named plugin the other plugins that "start without it" would also switch off.
pub fn report_state(store: &ProfileStore, refused: &Problem) -> Result<Value, String> {
    let profile = store.stored()?.ok_or_else(|| format!("{} does not exist", store.path.display()))?;
    let mut without_map = Map::new();
    let mut mentioned: Vec<String> = refused.plugins.clone();
    for p in &refused.plugins {
        let also = without(&store.catalog, &profile, p).1;
        mentioned.extend(also.iter().cloned());
        without_map.insert(p.clone(), json!(also));
    }
    // what people call each plugin the report mentions
    let mut names = Map::new();
    for m in store.catalog.0.iter().map(|pl| pl.manifest()).filter(|m| mentioned.contains(&m.id)) {
        names.insert(m.id.clone(), json!(if m.name.is_empty() { m.id.clone() } else { m.name.clone() }));
    }
    Ok(json!({
        "product": store.product,
        "problems": refused.details["problems"],
        "plugins": refused.details["plugins"],
        "without": without_map,
        "names": names,
    }))
}

/// Write the identity file with `plugin` switched off, and the plugins that need it. Only a plugin the report names
/// may be left out this way. Returns every plugin switched off, `plugin` first.
pub fn start_without(store: &ProfileStore, refused: &Problem, plugin: &str) -> Result<Vec<String>, String> {
    if !refused.plugins.iter().any(|p| p == plugin) {
        return Err(format!("{plugin} is not one of the plugins whose upgrade was refused"));
    }
    let profile = store.stored()?.ok_or_else(|| format!("{} does not exist", store.path.display()))?;
    let (next, also) = without(&store.catalog, &profile, plugin);
    let problems = store.commit(next, vec![])?;
    if !problems.is_empty() {
        return Err(problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join("; "));
    }
    Ok(std::iter::once(plugin.to_string()).chain(also).collect())
}

pub struct ReportConfig {
    pub store: Arc<ProfileStore>,
    pub refused: Problem,
    /// The one-time code the operator types into the page.
    pub code: String,
    /// `/faces/setup` and so on; the server config's hosts apply.
    pub server: Config,
}

#[derive(Clone)]
struct Report(Arc<ReportConfig>);

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn need_code(State(s): State<Report>, req: Request, next: Next) -> Response {
    if !header_str(req.headers(), "x-nuah-setup").is_some_and(|c| same(c.trim(), &s.0.code)) {
        return refuse(StatusCode::UNAUTHORIZED, "unauthenticated", "setup.bad_code", "Enter the setup code the server printed when it started.");
    }
    next.run(req).await
}

async fn state(State(s): State<Report>) -> Response {
    match report_state(&s.0.store, &s.0.refused) {
        Ok(body) => Json(body).into_response(),
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal", "upgrade.state_failed", &e),
    }
}

async fn start_without_it(State(s): State<Report>, Json(body): Json<Value>) -> Response {
    let plugin = body["plugin"].as_str().unwrap_or("");
    match start_without(&s.0.store, &s.0.refused, plugin) {
        Ok(off) => Json(json!({"ok": true, "off": off})).into_response(),
        Err(e) => refuse(StatusCode::UNPROCESSABLE_ENTITY, "invalid", "upgrade.cannot_start_without", &e),
    }
}

pub fn report_router(cfg: ReportConfig) -> Router {
    let server = Arc::new(Config { hosts: cfg.server.hosts.clone(), secure_cookie: false, faces: cfg.server.faces.clone(), ..Config::default() });
    let faces = cfg.server.faces.clone();
    let s = Report(Arc::new(cfg));
    let api = Router::new()
        .route("/upgrade/state", get(state))
        .route("/upgrade/without", post(start_without_it))
        .layer(middleware::from_fn_with_state(s.clone(), need_code))
        .with_state(s);
    // a health check says the Core is not running, so a container is not reported healthy
    let mut r = api.route("/healthz", get(|| async { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"ok": false, "upgrade_refused": true}))) }));
    for (prefix, dir) in &faces {
        r = r.nest_service(prefix, tower_http::services::ServeDir::new(dir));
    }
    if let Some((prefix, _)) = faces.first() {
        let to = format!("{prefix}/");
        r = r.route("/", get(move || async move { axum::response::Redirect::temporary(&to) }));
    }
    r.layer(middleware::from_fn_with_state(server, guard))
}
