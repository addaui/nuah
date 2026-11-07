//! The host: the one `main` a product built on Nuah needs ([ADR 0031]). A product describes itself with
//! [`Product`] and calls [`run`]; the host reads a Profile, starts the Core, starts its services (the
//! Projections the Profile enables, and the Core's own upkeep) and stops them on SIGINT or SIGTERM.
//!
//!   <product> hash-password                        read a password on stdin, print its stored form
//!   <product> serve --profile profile.json         run a Profile (what the wizard saves)
//!   <product> serve --db a.db --accounts b.json    the shortcut: the product's server Profile
//!   <product> setup --profile profile.json         choose plugins and settings in a browser
//!
//! [ADR 0031]: https://docs.addaui.com/nuah/adr/0031-products-add-to-nuah-and-never-edit-it-ports-declare-cardinality-projections-are-plugins/

pub mod setup;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use nuah_adapters::AuthPassword;
use nuah_kernel::{Catalog, Core, Distribution, Profile};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

/// What a product tells the host about itself. Everything product-specific is here; the host has no
/// idea what a task is.
pub trait Product {
    /// The product's name (`madar`): the program name, the Distribution name.
    fn name(&self) -> &str;
    /// Every plugin available, including the Projections (`nuah_http::Http`) with whatever routes the
    /// product adds. `with` holds the `--with NAME` switches the operator gave.
    fn catalog(&self, with: &[String]) -> Catalog;
    /// The Profile for `serve --db FILE --accounts FILE`: several people with passwords on one database.
    fn server_profile(&self, db: &str, accounts: Value) -> Profile;
    /// What the setup wizard starts from when there is no Profile file yet.
    fn starting_point(&self) -> Profile;
}

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    HashPassword,
    /// Choose plugins and settings in a browser and save a Profile (loopback only; ADR 0014).
    Setup {
        /// The Profile file to edit (created if missing).
        #[arg(long)]
        profile: PathBuf,
        #[arg(long, default_value = "127.0.0.1:3100")]
        listen: String,
        /// Directory of the wizard (hosted at /faces/setup).
        #[arg(long)]
        face: Option<PathBuf>,
    },
    Serve {
        /// Run a Profile file (what the wizard saves) instead of --db and --accounts.
        #[arg(long, env = "NUAH_PROFILE", conflicts_with_all = ["db", "accounts"])]
        profile: Option<PathBuf>,
        #[arg(long, env = "NUAH_DB", requires = "accounts")]
        db: Option<String>,
        #[arg(long, env = "NUAH_ACCOUNTS", requires = "db")]
        accounts: Option<String>,
        /// Where the http plugin listens. Overrides the Profile; default 127.0.0.1:3000.
        #[arg(long, env = "NUAH_LISTEN")]
        listen: Option<String>,
        /// Extra Host names to answer to (behind a reverse proxy). Comma separated in the environment.
        #[arg(long = "allow-host", env = "NUAH_ALLOW_HOSTS", value_delimiter = ',')]
        allow_host: Vec<String>,
        /// The proxy terminates TLS: mark the session cookie Secure.
        #[arg(long, env = "NUAH_SECURE_COOKIE")]
        secure_cookie: bool,
        /// Directory of the product's web face (hosted at /faces/web).
        #[arg(long)]
        web: Option<PathBuf>,
        /// Directory of the generated console (hosted at /faces/console).
        #[arg(long)]
        console: Option<PathBuf>,
        /// Another static Face, `/faces/NAME=DIRECTORY`.
        #[arg(long = "face")]
        faces: Vec<String>,
        /// A switch the product understands (Madar: `previews`).
        #[arg(long = "with", env = "NUAH_WITH", value_delimiter = ',')]
        with: Vec<String>,
    },
    /// Exit 0 if a running host answers on /healthz (for container health checks; no curl needed).
    Healthcheck {
        #[arg(long, env = "NUAH_LISTEN", default_value = "127.0.0.1:3000")]
        listen: String,
    },
}

/// Parse the command line and run. Never returns normally: it exits when the product stops.
pub fn run(product: impl Product) -> ! {
    let name: &'static str = Box::leak(product.name().to_string().into_boxed_str());
    let matches = Args::command().name(name).get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    match args.cmd {
        Cmd::HashPassword => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).expect("read stdin");
            println!("{}", AuthPassword::hash_password(s.trim_end_matches(['\r', '\n'])));
        }
        Cmd::Healthcheck { listen } => {
            use std::io::Write;
            let ok = (|| -> std::io::Result<bool> {
                let addr = listen.replace("0.0.0.0", "127.0.0.1");
                let mut c = std::net::TcpStream::connect_timeout(&addr.parse().map_err(|_| std::io::ErrorKind::InvalidInput)?, std::time::Duration::from_secs(3))?;
                c.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
                c.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
                let mut head = String::new();
                let _ = c.take(64).read_to_string(&mut head);
                Ok(head.starts_with("HTTP/1.1 200"))
            })()
            .unwrap_or(false);
            std::process::exit(if ok { 0 } else { 1 });
        }
        Cmd::Setup { profile, listen, face } => runtime().block_on(setup_mode(&product, profile, listen, face)),
        Cmd::Serve { profile, db, accounts, listen, allow_host, secure_cookie, web, console, faces, with } => {
            let mut profile = match (profile, db, accounts) {
                (Some(path), _, _) => {
                    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| die(&format!("cannot read {}: {e}", path.display())));
                    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("{} is not JSON: {e}", path.display())));
                    nuah_kernel::setup::profile_from_json(&v).unwrap_or_else(|e| die(&e))
                }
                (None, Some(db), Some(accounts)) => {
                    let accounts: Value = serde_json::from_str(&std::fs::read_to_string(&accounts).unwrap_or_else(|e| die(&format!("cannot read {accounts}: {e}"))))
                        .unwrap_or_else(|e| die(&format!("{accounts} is not JSON: {e}")));
                    product.server_profile(&db, accounts)
                }
                _ => die("give --profile FILE, or both --db and --accounts"),
            };
            let mut face_list: Vec<(String, String)> = vec![];
            face_list.extend(web.map(|d| ("/faces/web".to_string(), d.display().to_string())));
            face_list.extend(console.map(|d| ("/faces/console".to_string(), d.display().to_string())));
            for f in &faces {
                let (prefix, dir) = f.split_once('=').unwrap_or_else(|| die(&format!("--face {f}: write /faces/NAME=DIRECTORY")));
                face_list.push((prefix.to_string(), dir.to_string()));
            }
            with_http(&mut profile, HttpFlags { listen, hosts: allow_host, secure_cookie, faces: face_list });
            let listen_at = profile.settings["http"]["listen"].as_str().unwrap_or("127.0.0.1:3000").to_string();
            if profile.enabled.iter().any(|p| p == "auth-local") && !listen_at.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback()) {
                die("the profile uses auth-local (everyone is admin); listen on 127.0.0.1 or use auth-password");
            }
            let dist = Distribution { name: product.name().to_string(), catalog: product.catalog(&with), profile };
            let core = Core::start(&dist).unwrap_or_else(|problems| {
                for p in &problems {
                    eprintln!("{}: {}", p.code, p.message);
                }
                die("the profile does not resolve")
            });
            let services = core.start_services().unwrap_or_else(|e| die(&e.message));
            eprintln!("{} running (contract {}); services: {}", product.name(), core.contract_hash(), services.running().join(", "));
            runtime().block_on(wait_for_stop());
            eprintln!("stopping…");
            services.stop();
        }
    }
    std::process::exit(0)
}

/// What the command line says about the `http` plugin; each piece overrides the Profile only when given.
pub struct HttpFlags {
    pub listen: Option<String>,
    pub hosts: Vec<String>,
    pub secure_cookie: bool,
    pub faces: Vec<(String, String)>,
}

/// Make sure the Profile runs the `http` plugin, granted `serve`, with the command line's choices on top of
/// whatever the Profile already says.
pub fn with_http(profile: &mut Profile, f: HttpFlags) {
    if !profile.enabled.iter().any(|p| p == "http") {
        profile.enabled.push("http".into());
    }
    let grant = profile.grants.entry("http".into()).or_default();
    if !grant.iter().any(|g| g == "serve") {
        grant.push("serve".into());
    }
    let s = profile.settings.entry("http".into()).or_insert_with(|| json!({}));
    if !s.is_object() {
        *s = json!({});
    }
    let o = s.as_object_mut().unwrap();
    if let Some(l) = f.listen {
        o.insert("listen".into(), json!(l));
    } else {
        o.entry("listen").or_insert(json!("127.0.0.1:3000"));
    }
    if !f.hosts.is_empty() {
        let mut hosts: Vec<Value> = o.get("hosts").and_then(|h| h.as_array()).cloned().unwrap_or_default();
        hosts.extend(f.hosts.iter().map(|h| json!(h.to_lowercase())));
        o.insert("hosts".into(), json!(hosts));
    }
    if f.secure_cookie {
        o.insert("secure_cookie".into(), json!(true));
    }
    if !f.faces.is_empty() {
        let faces = o.entry("faces").or_insert(json!({}));
        if let Some(m) = faces.as_object_mut() {
            for (prefix, dir) in f.faces {
                m.insert(prefix, json!(dir));
            }
        }
    }
}

async fn setup_mode(product: &impl Product, profile: PathBuf, listen: String, face: Option<PathBuf>) {
    let addr: std::net::SocketAddr = listen.parse().unwrap_or_else(|_| die(&format!("{listen} is not an address like 127.0.0.1:3100")));
    if !addr.ip().is_loopback() {
        die("setup listens on loopback only (127.0.0.1 or [::1])");
    }
    let code = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let mut server = nuah_http::Config::default();
    server.faces.extend(face.map(|d| ("/faces/setup".to_string(), d)));
    let cfg = setup::SetupConfig { distribution: product.name().into(), catalog: product.catalog(&[]), starting_point: product.starting_point(), profile_path: profile, code: code.clone(), server };
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(&format!("cannot listen on {listen}: {e}")));
    eprintln!("setup on http://{listen}/\nsetup code: {code}   (type it into the wizard; it changes every start)");
    axum::serve(listener, setup::setup_router(cfg)).with_graceful_shutdown(wait_for_stop()).await.expect("serve");
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime")
}

/// Resolves on Ctrl-C everywhere, and on SIGTERM on Unix (what `docker stop` and systemd send).
async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
