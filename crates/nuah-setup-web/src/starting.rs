// SPDX-License-Identifier: Apache-2.0
//! The "starting" page ([ADR 0091], point 6): while the Core upgrades data at start it is not answering, so this serves
//! the setup Face's starting view on the Core's address, and stops (freeing the address) once the upgrade is done.
//!
//!   GET /upgrade/progress   which plugin and Collection is being upgraded, and how far it has come (no code: it holds
//!                           no data, only names and counts)
//!   GET /healthz            503, so a container is not reported ready
//!   any other GET           leads to the page; anything else is answered 503
//!
//! [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use nuah_http::{guard, Config};
use nuah_kernel::upgrade::UpgradeProgress;
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex};

/// What the starting view needs: the product, what people call each plugin, and the progress as the upgrade runs.
#[derive(Clone)]
pub struct Starting {
    pub product: String,
    pub names: Map<String, Value>,
    pub progress: Arc<Mutex<Option<UpgradeProgress>>>,
}

async fn progress(State(s): State<Starting>) -> Json<Value> {
    let now = s.progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut v = now.map(|p| serde_json::to_value(p).unwrap_or_default()).unwrap_or_else(|| json!({}));
    v["product"] = json!(s.product);
    let name = v["plugin"].as_str().and_then(|p| s.names.get(p)).cloned().unwrap_or_else(|| v["plugin"].clone());
    v["name"] = name;
    Json(v)
}

async fn elsewhere(req: Request) -> Response {
    if req.method() == Method::GET {
        Redirect::temporary("/faces/setup/").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": {"category": "unavailable", "reason": "upgrade.starting", "message": "The server is upgrading its data and will answer in a moment."}}))).into_response()
    }
}

pub fn starting_router(state: Starting, server: Config) -> Router {
    let faces = server.faces.clone();
    let server = Arc::new(Config { hosts: server.hosts.clone(), secure_cookie: false, faces: faces.clone(), ..Config::default() });
    let mut r = Router::new()
        .route("/upgrade/progress", get(progress))
        .route("/healthz", get(|| async { (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"ok": false, "starting": true}))) }))
        .with_state(state);
    for (prefix, dir) in &faces {
        r = r.nest_service(prefix, tower_http::services::ServeDir::new(dir));
    }
    r.fallback(elsewhere).layer(middleware::from_fn_with_state(server, guard))
}
