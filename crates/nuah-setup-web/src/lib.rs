// SPDX-License-Identifier: Apache-2.0
//! First-run setup as a web wizard ([ADR 0014], [ADR 0039]): a [`Bootstrap`] a product picks when its
//! distribution has a browser. It serves the wizard Face from `<faces dir>/nuah/setup-web` (default
//! `./faces/nuah/setup-web`; `--faces` or `NUAH_FACES` moves the directory), guarded by a one-time code printed on the terminal.
//!
//! [ADR 0014]: https://docs.addaui.com/nuah/adr/0014-setup-wizard-is-a-separate-mode-with-a-setup-code-and-no-secrets/
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

mod report;
mod starting;
mod wizard;

use nuah_shell::edit::ProfileStore;
use nuah_shell::{wait_for_stop, Bootstrap, SetupContext};
use std::path::PathBuf;

pub use report::{report_router, report_state, start_without, ReportConfig};
pub use starting::{starting_router, Starting};
pub use wizard::{setup_router, SetupConfig};

pub struct SetupWeb {
    faces_dir: Option<PathBuf>,
}

impl SetupWeb {
    /// The wizard Face is where the shell says the Faces are (`./faces/nuah/setup-web` by default).
    pub fn new() -> Self {
        SetupWeb { faces_dir: None }
    }
    /// The wizard Face is in `<dir>/nuah/setup-web`, whatever the shell says.
    pub fn with_faces_dir(dir: impl Into<PathBuf>) -> Self {
        SetupWeb { faces_dir: Some(dir.into()) }
    }
}

impl Default for SetupWeb {
    fn default() -> Self {
        SetupWeb::new()
    }
}

impl Bootstrap for SetupWeb {
    fn run(&self, ctx: SetupContext) -> Result<(), String> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(serve(self, ctx))
    }
    fn report(&self, ctx: SetupContext, refused: nuah_kernel::Problem) -> Result<(), String> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
        rt.block_on(serve_report(self, ctx, refused))
    }

    fn starting(&self, ctx: SetupContext, progress: std::sync::Arc<std::sync::Mutex<Option<nuah_kernel::upgrade::UpgradeProgress>>>) -> Option<Box<dyn FnOnce()>> {
        let listen = ctx.listen.clone().unwrap_or_else(|| "127.0.0.1:3000".into());
        let face = self.faces_dir.clone().unwrap_or_else(|| ctx.faces_dir.clone()).join("nuah").join("setup-web");
        if !face.is_dir() {
            eprintln!("the setup Face, which shows the upgrade's progress, is not at {}", face.display());
            return None;
        }
        // bound here, before returning, so the page is there by the time the caller goes on
        let listener = match std::net::TcpListener::bind(&listen).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cannot show the upgrade's progress on {listen}: {e}");
                return None;
            }
        };
        let mut server = nuah_http::Config::default();
        server.hosts.extend(ctx.allow_hosts.iter().filter(|h| !h.is_empty()).map(|h| h.to_lowercase()));
        server.faces.push(("/faces/setup".to_string(), face));
        let names = ctx.catalog.0.iter().map(|p| p.manifest()).map(|m| (m.id.clone(), serde_json::json!(if m.name.is_empty() { m.id.clone() } else { m.name.clone() }))).collect();
        let app = starting_router(Starting { product: ctx.product.clone(), names, progress }, server);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(async move {
                let Ok(listener) = tokio::net::TcpListener::from_std(listener) else { return };
                // not a graceful stop: a page waiting on an answer simply tries again, and finds the Core
                tokio::select! { _ = axum::serve(listener, app) => {}, _ = stopped => {} }
            });
        });
        eprintln!("upgrading data before the server starts; the progress is on http://{listen}/");
        Some(Box::new(move || {
            let _ = stop.send(());
            let _ = thread.join();
        }))
    }
}

/// Serve the Upgrade report until the operator starts without a plugin (then return, so the shell starts the Core again)
/// or stops the process.
async fn serve_report(me: &SetupWeb, ctx: SetupContext, refused: nuah_kernel::Problem) -> Result<(), String> {
    let listen = ctx.listen.clone().unwrap_or_else(|| "127.0.0.1:3000".into());
    let addr: std::net::SocketAddr = listen.parse().map_err(|_| format!("{listen} is not an address like 127.0.0.1:3000"))?;
    let code = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let mut server = nuah_http::Config::default();
    server.hosts.extend(ctx.allow_hosts.iter().filter(|h| !h.is_empty()).map(|h| h.to_lowercase()));
    let face = me.faces_dir.clone().unwrap_or_else(|| ctx.faces_dir.clone()).join("nuah").join("setup-web");
    if !face.is_dir() {
        return Err(format!("the setup Face, which shows the upgrade report, is not at {}", face.display()));
    }
    server.faces.push(("/faces/setup".to_string(), face));
    let store = std::sync::Arc::new(ProfileStore::new(&ctx.product, ctx.catalog, ctx.starting_point, ctx.identity_file));
    let saved = store.saved.clone();
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    eprintln!("the upgrade was refused and nothing was changed; the report is on http://{listen}/\nsetup code: {code}   (type it into the page; it changes every start)");
    axum::serve(listener, report_router(ReportConfig { store, refused, code, server }))
        .with_graceful_shutdown(async move {
            tokio::select! { _ = saved.notified() => {}, _ = wait_for_stop() => std::process::exit(1) }
        })
        .await
        .map_err(|e| e.to_string())
}

async fn serve(me: &SetupWeb, ctx: SetupContext) -> Result<(), String> {
    let listen = ctx.listen.clone().unwrap_or_else(|| if ctx.until_saved { "127.0.0.1:3000".into() } else { "127.0.0.1:3100".into() });
    let addr: std::net::SocketAddr = listen.parse().map_err(|_| format!("{listen} is not an address like 127.0.0.1:3100"))?;
    // Off loopback only while the first run waits for its file; the one-time code is what protects it.
    if !addr.ip().is_loopback() && !ctx.until_saved {
        return Err("setup listens on loopback only (127.0.0.1 or [::1])".into());
    }
    let code = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let mut server = nuah_http::Config::default();
    if ctx.until_saved {
        server.hosts.extend(ctx.allow_hosts.iter().filter(|h| !h.is_empty()).map(|h| h.to_lowercase()));
    }
    let face = me.faces_dir.clone().unwrap_or_else(|| ctx.faces_dir.clone()).join("nuah").join("setup-web");
    if face.is_dir() {
        server.faces.push(("/faces/setup".to_string(), face));
    } else {
        return Err(format!("the wizard Face is not at {}; put the setup-web Face there, or point --faces / NUAH_FACES at the directory that has it", face.display()));
    }
    let store = std::sync::Arc::new(ProfileStore::new(&ctx.product, ctx.catalog, ctx.starting_point, ctx.identity_file));
    store.set_default_reach(Some(nuah_kernel::Reach::of_listen(&listen)));
    let saved = store.saved.clone();
    let cfg = SetupConfig { store, code: code.clone(), server };
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {listen}: {e}"))?;
    eprintln!("setup on http://{listen}/\nsetup code: {code}   (type it into the wizard; it changes every start)");
    let until_saved = ctx.until_saved;
    axum::serve(listener, setup_router(cfg))
        .with_graceful_shutdown(async move {
            if until_saved {
                tokio::select! { _ = saved.notified() => {}, _ = wait_for_stop() => std::process::exit(0) }
            } else {
                wait_for_stop().await
            }
        })
        .await
        .map_err(|e| e.to_string())
}
