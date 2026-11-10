// SPDX-License-Identifier: Apache-2.0
//! The shell: the one `main` a product built on Nuah needs ([ADR 0031]). A product describes itself with
//! [`Product`] and calls [`run`]; the shell reads a Profile, starts the Core, starts its services (the
//! Projections the Profile enables, and the Core's own upkeep) and stops them on SIGINT or SIGTERM.
//!
//!   <product> serve                                run the identity file (`nuah.json` in $NUAH_HOME or the working
//!                                                  directory); with no file, the product's bootstrap runs first and writes it
//!   <product> serve --profile profile.json         run another Profile file
//!   <product> serve --db a.db --accounts b.json    the shortcut: the product's server Profile
//!   <product> reset-password <name>               set a new password (read on stdin) for a locked-out account
//!   <product> hash-password                        read a password on stdin, print its stored form
//!   <product> setup --profile profile.json         change a Profile with the product's bootstrap
//!
//! [ADR 0031]: https://docs.addaui.com/nuah/adr/0031-products-add-to-nuah-and-never-edit-it-ports-declare-cardinality-projections-are-plugins/

pub mod edit;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use nuah_adapters::AuthPassword;
use nuah_kernel::{Catalog, Core, Distribution, Level, Profile};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;

/// What a product tells the host about itself. Everything product-specific is here; the shell has no
/// idea what a task is, and does not know whether the product has a web face, a database or a terminal.
pub trait Product {
    /// The product's name (`madar`): the program name, the Distribution name.
    fn name(&self) -> &str;
    /// Every plugin available, including the Projections (`nuah_http::Http`) with whatever routes the product adds.
    fn catalog(&self) -> Catalog;
    /// What setup starts from when there is no Profile file yet: the product's defaults. `home` is the directory
    /// of the identity file, for defaults that live next to it (a database file).
    fn starting_point(&self, home: &std::path::Path) -> Profile;
    /// How a first run (no identity file) gets one: a web wizard, terminal prompts, whatever the product picks
    /// ([ADR 0039]). `None` means the product cannot set itself up, and `serve` without a file says so and stops.
    ///
    /// [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
    fn bootstrap(&self) -> Option<Box<dyn Bootstrap>> {
        None
    }
}

/// Writes the identity file when there is none (or changes one). It runs before any Core exists, so it
/// reads manifests and asks the Resolver, and starts no plugin.
pub trait Bootstrap {
    /// Returns once a Profile that resolves has been saved to `ctx.identity_file`. With `ctx.until_saved`
    /// false (the `setup` command) it may keep running until the operator stops it.
    fn run(&self, ctx: SetupContext) -> Result<(), String>;
}

pub struct SetupContext {
    pub product: String,
    pub catalog: Catalog,
    pub starting_point: Profile,
    /// Where the Profile is written (and read from, when it exists).
    pub identity_file: PathBuf,
    /// `--listen` / `NUAH_LISTEN`, for a bootstrap that listens.
    pub listen: Option<String>,
    /// `--allow-host` / `NUAH_ALLOW_HOSTS`, for a bootstrap that listens.
    pub allow_hosts: Vec<String>,
    /// Where the Faces are (`--faces` / `NUAH_FACES`, default `./faces`).
    pub faces_dir: PathBuf,
    /// True when `serve` found no identity file and will run the one saved; false for the `setup` command.
    pub until_saved: bool,
}

#[derive(Parser)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    HashPassword,
    /// Create or change a Profile with the product's bootstrap.
    Setup {
        /// The Profile file to edit (created if missing).
        #[arg(long)]
        profile: PathBuf,
        #[arg(long, env = "NUAH_LISTEN")]
        listen: Option<String>,
        #[arg(long = "allow-host", env = "NUAH_ALLOW_HOSTS", value_delimiter = ',')]
        allow_host: Vec<String>,
        /// The directory holding the Faces (`<dir>/nuah/console`, `<dir>/nuah/setup-web`, ...). Default: `faces`.
        #[arg(long, env = "NUAH_FACES")]
        faces: Option<PathBuf>,
    },
    Serve {
        /// Run this Profile file instead of the identity file.
        #[arg(long, env = "NUAH_PROFILE")]
        profile: Option<PathBuf>,
        /// The directory holding the identity file `nuah.json` (default: the working directory).
        #[arg(long, env = "NUAH_HOME")]
        home: Option<PathBuf>,
        /// Where the http plugin listens (when the Profile enables it). Overrides the Profile; default 127.0.0.1:3000.
        #[arg(long, env = "NUAH_LISTEN")]
        listen: Option<String>,
        /// Extra Host names to answer to (behind a reverse proxy). Comma separated in the environment.
        #[arg(long = "allow-host", env = "NUAH_ALLOW_HOSTS", value_delimiter = ',')]
        allow_host: Vec<String>,
        /// The proxy terminates TLS: mark the session cookie Secure.
        #[arg(long, env = "NUAH_SECURE_COOKIE")]
        secure_cookie: bool,
        /// Records chattier than this are not written (error, warn, info, debug, trace). Overrides the Profile.
        #[arg(long, env = "NUAH_LOG_LEVEL")]
        log_level: Option<String>,
        /// The directory holding the Faces (`<dir>/nuah/console`, `<dir>/nuah/setup-web`, ...). Default: `faces`.
        #[arg(long, env = "NUAH_FACES")]
        faces: Option<PathBuf>,
    },
    /// Set a new password for an account, for an operator locked out of the instance: reads the password on
    /// stdin, starts the Core without its services and calls the auth plugin's `auth.account.set_password`.
    /// Run it with the instance stopped (one process per database, ADR 0036).
    ResetPassword {
        /// The account's login name.
        name: String,
        #[arg(long, env = "NUAH_PROFILE")]
        profile: Option<PathBuf>,
        #[arg(long, env = "NUAH_HOME")]
        home: Option<PathBuf>,
    },
    /// Exit 0 if a running shell answers on /healthz (for container health checks; no curl needed).
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
        Cmd::Setup { profile, listen, allow_host, faces } => {
            let Some(bootstrap) = product.bootstrap() else { die("this product has no setup") };
            bootstrap.run(context(&product, profile, listen, allow_host, faces, false)).unwrap_or_else(|e| die(&e));
        }
        Cmd::Serve { profile, home, listen, allow_host, secure_cookie, log_level, faces } => {
            let path = identity_path(profile, home.clone());
            if !path.exists() {
                // No identity file: the product's bootstrap writes one, then this process goes on to run it.
                let Some(bootstrap) = product.bootstrap() else {
                    die(&format!("{} does not exist, and {} has no setup to create it; write the file or run it with --profile", path.display(), product.name()))
                };
                eprintln!("no {} found: starting setup", path.display());
                bootstrap.run(context(&product, path.clone(), listen.clone(), allow_host.clone(), faces.clone(), true)).unwrap_or_else(|e| die(&e));
            }
            let flags = ServeFlags { listen, allow_host, secure_cookie, log_level, faces };
            let store = std::sync::Arc::new(edit::ProfileStore::new(product.name(), product.catalog(), product.starting_point(&home_of(&path)), path.clone()));
            let mut restarted = false;
            loop {
                let (core, file_changed) = match start_core(&product, &path, &flags) {
                    Ok(x) => x,
                    Err(problems) if restarted => {
                        // A saved change did not start: put the file as it was back, once, and run that.
                        for p in &problems {
                            eprintln!("{}: {}", p.code, p.message);
                        }
                        store.set_notice(Some(format!("The saved configuration did not start ({}), so the earlier file is running. Fix the problem and save again.", problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join("; "))));
                        let bak = path.with_extension("json.bak");
                        if std::fs::copy(&bak, &path).is_err() {
                            die("the saved profile does not resolve, and there is no earlier file to go back to");
                        }
                        eprintln!("the saved profile does not resolve: went back to the earlier file");
                        restarted = false;
                        continue;
                    }
                    Err(problems) => {
                        for p in &problems {
                            eprintln!("{}: {}", p.code, p.message);
                        }
                        die("the profile does not resolve")
                    }
                };
                let source = path.display().to_string();
                let writable = std::fs::OpenOptions::new().append(true).open(&source).is_ok();
                core.log(Level::Info, "nuah.shell", "shell.identity", json!({"source": source, "writable": writable}));
                if let Some(locked) = file_changed {
                    core.log(Level::Warn, "nuah.shell", "shell.contract_changed", json!({"locked": locked, "now": core.contract_hash(), "hint": "a plugin upgrade changed the API this file was saved against"}));
                }
                core.attach_shell(store.clone());
                let services = core.start_services().unwrap_or_else(|e| die(&e.message));
                core.log(Level::Info, "nuah.shell", "shell.running", json!({"product": product.name(), "contract": core.contract_hash(), "services": services.running()}));
                let for_panics = core.clone();
                let previous = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    // Queued, never waited for: a panicking thread must not stall on a slow log. The message can hold data, so only the place is recorded.
                    for_panics.log(Level::Error, "nuah.shell", "shell.panic", json!({"where": info.location().map(|l| l.to_string())}));
                    previous(info);
                }));
                let again = runtime().block_on(async {
                    tokio::select! {
                        _ = wait_for_stop() => false,
                        _ = store.restart.notified() => {
                            // The answer to the request that asked for the restart is on its way out.
                            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                            true
                        }
                    }
                });
                core.log(Level::Info, "nuah.shell", if again { "shell.restarting" } else { "shell.stopping" }, json!({}));
                services.stop();
                let _ = std::panic::take_hook();
                if !again {
                    break;
                }
                restarted = true;
            }
        }
        Cmd::ResetPassword { name, profile, home } => {
            let path = identity_path(profile, home);
            if !path.exists() {
                die(&format!("{} does not exist", path.display()));
            }
            let mut pw = String::new();
            std::io::stdin().read_to_string(&mut pw).expect("read stdin");
            let pw = pw.trim_end_matches(['\r', '\n']);
            let core = start_core(&product, &path, &ServeFlags::default()).unwrap_or_else(|problems| {
                for p in &problems {
                    eprintln!("{}: {}", p.code, p.message);
                }
                die("the profile does not resolve")
            }).0;
            reset_password(&core, &name, pw).unwrap_or_else(|e| die(&e));
            eprintln!("password changed for {name}");
        }
    }
    std::process::exit(0)
}

/// What the command line says about the `http` plugin; each piece overrides the Profile only when given, and
/// only when the Profile enables `http` (a product with no web server has nothing to apply them to).
pub struct HttpFlags {
    pub listen: Option<String>,
    /// Where the Faces are; overrides the Profile's `faces_dir` when given.
    pub faces_dir: Option<PathBuf>,
    pub hosts: Vec<String>,
    pub secure_cookie: bool,
}

pub fn apply_http_flags(profile: &mut Profile, f: HttpFlags) {
    if !profile.enabled.iter().any(|p| p == "@nuah/http") {
        return;
    }
    let s = profile.settings.entry("@nuah/http".into()).or_insert_with(|| json!({}));
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
    if let Some(d) = f.faces_dir {
        o.insert("faces_dir".into(), json!(d.display().to_string()));
    }
}

/// What the command line says about a run, applied to the Profile each time the Core is (re)started.
#[derive(Default)]
struct ServeFlags {
    listen: Option<String>,
    allow_host: Vec<String>,
    secure_cookie: bool,
    log_level: Option<String>,
    faces: Option<PathBuf>,
}

/// Read the file, apply the flags, resolve and start a Core (no services). Also returns the contract the file
/// was locked to when that differs from the contract now.
fn start_core(product: &impl Product, path: &std::path::Path, f: &ServeFlags) -> Result<(Core, Option<String>), Vec<nuah_kernel::Problem>> {
    let mut profile = read_profile(path);
    // The lock is checked against the file as it was saved, before this run's command-line changes.
    let changed = profile.contract.clone().filter(|c| *c != Core::contract_for(&Distribution { name: product.name().to_string(), catalog: product.catalog(), profile: profile.clone() }));
    apply_http_flags(&mut profile, HttpFlags { listen: f.listen.clone(), faces_dir: f.faces.clone(), hosts: f.allow_host.clone(), secure_cookie: f.secure_cookie });
    if let Some(level) = &f.log_level {
        profile.telemetry.level = Some(level.clone());
    }
    let listen_at = profile.settings["@nuah/http"]["listen"].as_str().unwrap_or("127.0.0.1:3000").to_string();
    if profile.enabled.iter().any(|p| p == "@nuah/auth-local") && profile.enabled.iter().any(|p| p == "@nuah/http") && !listen_at.parse::<std::net::SocketAddr>().is_ok_and(|a| a.ip().is_loopback()) {
        // a Problem, not a stop, so a restart after a console save can go back to the earlier file
        return Err(vec![nuah_kernel::Problem { code: "auth_local_off_loopback".into(), plugins: vec!["@nuah/auth-local".into()], message: "the profile uses auth-local (everyone is admin); listen on 127.0.0.1 or use auth-password".into() }]);
    }
    Core::start(&Distribution { name: product.name().to_string(), catalog: product.catalog(), profile }).map(|c| (c, changed))
}

fn home_of(identity_file: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(identity_file.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."))).unwrap_or_else(|_| PathBuf::from("."))
}

/// Find the account by login name and give it a new password, as a built-in administrator.
pub fn reset_password(core: &Core, name: &str, password: &str) -> Result<(), String> {
    let admin = nuah_kernel::Identity::new("shell-recovery", "Recovery", &["admin"]);
    let list = core.call_as(&admin, "auth.account.list", json!({}), None).map_err(|e| match e.category {
        nuah_kernel::Category::NotFound | nuah_kernel::Category::Unsupported => "this instance's auth plugin has no account management (auth.account.*)".to_string(),
        _ => e.message,
    })?;
    let want = name.trim().to_lowercase();
    let subject = list.output["accounts"].as_array().into_iter().flatten().find(|a| a["name"].as_str().is_some_and(|n| n.to_lowercase() == want)).and_then(|a| a["subject"].as_str()).map(String::from).ok_or_else(|| format!("no account named {name}"))?;
    core.call_as(&admin, "auth.account.set_password", json!({"subject": subject, "password": password}), None).map(|_| ()).map_err(|e| e.message)
}

fn context(product: &impl Product, identity_file: PathBuf, listen: Option<String>, allow_hosts: Vec<String>, faces: Option<PathBuf>, until_saved: bool) -> SetupContext {
    let home = home_of(&identity_file);
    SetupContext { product: product.name().into(), catalog: product.catalog(), starting_point: product.starting_point(&home), identity_file, listen, allow_hosts, faces_dir: faces.unwrap_or_else(|| PathBuf::from("faces")), until_saved }
}

fn read_profile(path: &std::path::Path) -> Profile {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(&format!("cannot read {}: {e}", path.display())));
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("{} is not JSON: {e}", path.display())));
    nuah_kernel::setup::profile_from_json(&v).unwrap_or_else(|e| die(&format!("{}: {e}", path.display())))
}

/// The identity file: the one named with `--profile`, else `nuah.json` in `--home` (or the working directory).
pub fn identity_path(profile: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    profile.unwrap_or_else(|| home.unwrap_or_else(|| PathBuf::from(".")).join("nuah.json"))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime")
}

/// Resolves on Ctrl-C everywhere, and on SIGTERM on Unix (what `docker stop` and systemd send).
pub async fn wait_for_stop() {
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

pub fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}
