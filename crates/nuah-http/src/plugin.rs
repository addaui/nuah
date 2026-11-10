// SPDX-License-Identifier: Apache-2.0
//! The `http` plugin: the HTTP and SSE Projection as a plugin that runs as a service (ADR 0031).
//! It runs inside the Core's process on threads of its own: a multi-threaded tokio runtime for the
//! connections, while storage work goes to tokio's blocking pool and so on to the data Port.

use crate::{router_with, Config, RouteFn};
use axum::Router;
use nuah_kernel::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The Projection. A product builds it with its own routes and puts it in its catalog:
/// `Http::new().with_routes(|core| my_router(core))`.
#[derive(Default)]
pub struct Http {
    routes: Vec<RouteFn>,
}

impl Http {
    pub fn new() -> Self {
        Http::default()
    }
    /// Add routes of the product's own (file downloads, say). They sit behind the same guard as the Link.
    pub fn with_routes(mut self, f: impl Fn(Core) -> Router + Send + Sync + 'static) -> Self {
        self.routes.push(Arc::new(f));
        self
    }
}

struct Running {
    stop: tokio::sync::watch::Sender<bool>,
    thread: std::thread::JoinHandle<()>,
}

impl Service for Running {
    fn stop(self: Box<Self>) {
        let _ = self.stop.send(true);
        let _ = self.thread.join();
    }
}

impl Plugin for Http {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "@nuah/http", "version": "0.3.0", "name": "HTTP projection",
            "description": "Serves the Link (Operations, describe, Pulse) over HTTP and server-sent events, and static Faces",
            "namespaces": ["http"],
            "requires": {"capabilities": ["serve"]},
            "settings_schema": {"type": "object", "additionalProperties": false, "properties": {
                "listen": {"type": "string", "minLength": 3, "title": "Listen address", "description": "Where the web server listens: 127.0.0.1:3000 for this machine only, 0.0.0.0:3000 for every network.", "default": "127.0.0.1:3000", "x-ref": false, "x-tier": "bootstrap", "x-apply": "service"},
                "hosts": {"type": "array", "items": {"type": "string"}, "title": "Allowed host names", "description": "Names people use to reach this instance (behind a reverse proxy), one per line, for example madar.example.com.", "x-tier": "operator", "x-apply": "service"},
                "secure_cookie": {"type": "boolean", "title": "Secure cookie", "description": "Turn on when a proxy serves this over HTTPS, so the session cookie is only sent over HTTPS.", "x-tier": "operator", "x-apply": "service"},
                "root": {"type": "string", "description": "Which Face `/` shows, by name (for example web). Default: web, else the first.", "x-tier": "operator", "x-apply": "service"},
                "faces_dir": {"type": "string", "description": "Where plugins' Faces are: a plugin that publishes the Face named X is served from <faces_dir>/<scope>/<name> of its id. Default: faces (or NUAH_FACES)", "x-tier": "operator", "x-apply": "service"},
                "faces": {"type": "object", "additionalProperties": {"type": "string"}, "x-tier": "operator", "x-apply": "service"},
                "worker_threads": {"type": "integer", "minimum": 0, "maximum": 1024, "title": "Worker threads", "description": "0 uses every core.", "x-tier": "operator", "x-apply": "service"},
                "sign_in_failures_per_name": {"type": "integer", "minimum": 1, "x-tier": "operator", "x-apply": "service"},
                "sign_in_failures_overall": {"type": "integer", "minimum": 1, "x-tier": "operator", "x-apply": "service"},
                "throttle_window_secs": {"type": "integer", "minimum": 1, "x-tier": "operator", "x-apply": "service"},
                "pulse_streams_per_person": {"type": "integer", "minimum": 1, "x-tier": "operator", "x-apply": "service"},
                "pulse_streams_total": {"type": "integer", "minimum": 1, "x-tier": "operator", "x-apply": "service"}
            }}
        })).unwrap()
    }

    fn serve(&self, ctx: ServiceContext) -> Result<Option<Box<dyn Service>>> {
        let cfg = Config::from_settings(&ctx.settings).map_err(Error::internal)?;
        let routes = self.routes.clone();
        let core = ctx.core;
        let log = core.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<std::net::SocketAddr, String>>();
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let thread = std::thread::Builder::new()
            .name("nuah-http".into())
            .spawn(move || {
                let mut b = tokio::runtime::Builder::new_multi_thread();
                b.enable_all().thread_name("nuah-http-worker");
                if cfg.worker_threads > 0 {
                    b.worker_threads(cfg.worker_threads);
                }
                let rt = match b.build() {
                    Ok(rt) => rt,
                    Err(e) => return drop(ready_tx.send(Err(format!("cannot start the runtime: {e}")))),
                };
                rt.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(&cfg.listen).await {
                        Ok(l) => l,
                        Err(e) => return drop(ready_tx.send(Err(format!("cannot listen on {}: {e}", cfg.listen)))),
                    };
                    let _ = ready_tx.send(Ok(listener.local_addr().expect("bound address")));
                    let app = router_with(core.clone(), cfg, &routes, stop_rx.clone());
                    let mut graceful = stop_rx.clone();
                    let serving = axum::serve(listener, app).with_graceful_shutdown(async move {
                        let _ = graceful.wait_for(|down| *down).await;
                    });
                    // Streams close on the signal; anything still open after a few seconds is cut.
                    tokio::select! {
                        r = serving => { if let Err(e) = r { core.log(Level::Error, "@nuah/http", "http.stopped", json!({"error": e.to_string()})); } }
                        _ = async { let _ = stop_rx.wait_for(|down| *down).await; tokio::time::sleep(Duration::from_secs(5)).await } => {}
                    }
                });
            })
            .map_err(|e| Error::internal(format!("cannot start the http thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(addr)) => {
                log.log(Level::Info, "@nuah/http", "http.listening", json!({"addr": addr.to_string()}));
                Ok(Some(Box::new(Running { stop: stop_tx, thread })))
            }
            Ok(Err(msg)) => {
                let _ = thread.join();
                Err(Error::internal(msg))
            }
            Err(_) => Err(Error::internal("the http thread ended before it was ready")),
        }
    }
}
