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
    /// Plugins of older versions of this product that a plugin of this version takes the place of: (old id, successor). A Profile that still
    /// enables an old one is moved to its successor when it starts (settings and grants too).
    fn replaced(&self) -> Vec<(String, String)> {
        vec![]
    }
    /// What a newer version of this product needs that an older saved Profile does not have: plugins it now requires, grants a
    /// plugin now asks for. It changes the Profile and says in a line what it did; with nothing to do it returns nothing and the
    /// file is left alone. Run each time the product starts, before the Profile is resolved; when it changed something the
    /// file is saved, the earlier one kept next to it as `<file>.before-upgrade`.
    fn upgrade_profile(&self, _profile: &mut Profile) -> Vec<String> {
        vec![]
    }
    /// What this build is made from ([ADR 0100]): `{name, version, commit, components: [{name, version, commit}]}`. A product
    /// records its commit at build time and reads its components from its own `Cargo.lock` ([`components_from_lock`]).
    ///
    /// [ADR 0100]: https://docs.addaui.com/nuah/adr/0100-a-distribution-says-what-it-is-built-from-its-version-and-commit-and-nuahs-shown-to-signed-in-people/
    fn about(&self) -> serde_json::Value {
        serde_json::json!({"name": self.name(), "version": null, "commit": null, "components": []})
    }
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
    /// Show an Upgrade report ([ADR 0091]): the Core refused to start because a plugin's stored data cannot be upgraded
    /// (`refused.code` is `upgrade_refused`; its `details` hold the problems). Returns after the operator chose to start
    /// without a plugin and the identity file was saved; the shell then starts the Core again. A bootstrap that cannot
    /// show a page leaves this as it is, and the shell prints the report and stops.
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    fn report(&self, _ctx: SetupContext, _refused: nuah_kernel::Problem) -> Result<(), String> {
        Err("this product cannot show the upgrade report; see the problems above".into())
    }
    /// While the Core upgrades data at start ([ADR 0091], point 6) it is not answering yet: serve a page on its address
    /// that says how far the upgrade has come (`progress` is updated as it runs). Returns what stops the page and
    /// frees the address for the Core; `None` when the product has no such page (the shell then only prints progress).
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    fn starting(&self, _ctx: SetupContext, _progress: std::sync::Arc<std::sync::Mutex<Option<nuah_kernel::upgrade::UpgradeProgress>>>) -> Option<Box<dyn FnOnce()>> {
        None
    }
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
    /// Compare a plugin's released schema file with its new one, as the Core would at start ([ADR 0091]); prints every
    /// problem and exits 1 when there is one. For a plugin's CI: `schema-check <released.json> <new.json>`.
    ///
    /// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
    SchemaCheck {
        /// The schema file as last released.
        released: PathBuf,
        /// The schema file now.
        new: PathBuf,
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
        Cmd::SchemaCheck { released, new } => {
            let read = |p: &PathBuf| -> Value {
                let text = std::fs::read_to_string(p).unwrap_or_else(|e| die(&format!("cannot read {}: {e}", p.display())));
                serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("{} is not JSON: {e}", p.display())))
            };
            let (text, pass) = schema_check(&read(&released), &read(&new));
            print!("{text}");
            std::process::exit(if pass { 0 } else { 1 });
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
                // the address the Core will answer on, for the pages shown before it does
                let listen = flags.listen.clone().or_else(|| read_profile(&path).settings["@nuah/http"]["listen"].as_str().map(String::from));
                let bootstrap = product.bootstrap();
                let progress = std::sync::Arc::new(std::sync::Mutex::new(None));
                let page: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
                let (shown, printed) = (std::cell::Cell::new(false), std::cell::Cell::new(None::<std::time::Instant>));
                let watch = |p: &nuah_kernel::upgrade::UpgradeProgress| {
                    *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
                    if !shown.replace(true) {
                        if let Some(b) = &bootstrap {
                            *page.borrow_mut() = b.starting(context(&product, path.clone(), listen.clone(), flags.allow_host.clone(), flags.faces.clone(), true), progress.clone());
                        }
                    }
                    if p.done == p.total || printed.get().is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(2)) {
                        printed.set(Some(std::time::Instant::now()));
                        match p.phase.as_str() {
                            "copying" => eprintln!("saving a copy of the database before upgrading"),
                            phase => eprintln!("{phase} {} ({} of {}): {} of {} done", p.collection, p.collection_number, p.collections, p.done, p.total),
                        }
                    }
                };
                let started = start_core(&product, &path, &flags, Some(&store), Some(&watch));
                // the starting page gives the address back before the Core, or the report, listens on it
                if let Some(stop) = page.borrow_mut().take() {
                    stop();
                }
                let (core, file_changed) = match started {
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
                        // a refused upgrade changed nothing: show the report, and start again if the operator leaves a plugin out
                        if let Some(refused) = problems.into_iter().find(|p| p.code == "upgrade_refused") {
                            let Some(bootstrap) = product.bootstrap() else { die("the upgrade was refused; nothing was changed") };
                            bootstrap.report(context(&product, path.clone(), listen.clone(), flags.allow_host.clone(), flags.faces.clone(), true), refused).unwrap_or_else(|e| die(&e));
                            continue;
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
            let core = start_core(&product, &path, &ServeFlags::default(), None, None).unwrap_or_else(|problems| {
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

/// The Schema check between two schema files of one plugin, without a database ([ADR 0091]): the name rules of the new
/// file, then what an upgrade from the released one would do or why it would be refused. Returns the report and whether
/// it passes.
///
/// [ADR 0091]: https://docs.addaui.com/nuah/adr/0091-plugins-declare-one-schema-nuah-upgrades-their-data-from-it-and-refuses-changes-that-would-lose-data/
pub fn schema_check(released: &Value, new: &Value) -> (String, bool) {
    use nuah_kernel::upgrade::{plan, static_problems, FieldOp, SchemaFile};
    let parse = |v: &Value, which: &str| SchemaFile::parse(v).map_err(|e| format!("the {which} schema file: {e}\n"));
    let (old, new) = match (parse(released, "released"), parse(new, "new")) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(e), _) | (_, Err(e)) => return (e, false),
    };
    let mut out = String::new();
    let line = |p: &nuah_kernel::upgrade::UpgradeProblem| {
        let at = if p.path.is_empty() { p.collection.clone() } else { format!("{} › {}", p.collection, p.path) };
        let params = if p.params.as_object().is_some_and(|o| !o.is_empty()) { format!(" {}", p.params) } else { String::new() };
        format!("- {at} — {}{params}: {}\n", p.code, p.message())
    };
    let mut problems = static_problems("", &new);
    match plan("", &old, &new) {
        Ok(pl) => {
            out.push_str(&format!("data version {} → {}\n", old.version, new.version));
            for c in &pl.collections {
                let head = if c.removed { format!("{}: removed", c.from) } else if c.from != c.to { format!("{} → {}", c.from, c.to) } else { c.to.clone() };
                out.push_str(&format!("{head}\n"));
                for op in &c.ops {
                    out.push_str(&match op {
                        FieldOp::Rename { from, to } => format!("  rename {} → {}\n", from.join("."), to.join(".")),
                        FieldOp::Remove(p) => format!("  remove {}\n", p.join(".")),
                        FieldOp::Default(p, v) => format!("  default {} = {v}\n", p.join(".")),
                    });
                }
            }
        }
        Err(ps) => problems.extend(ps),
    }
    if problems.is_empty() {
        out.push_str("ok\n");
        return (out, true);
    }
    out.push_str(&format!("refused: {} problem(s)\n", problems.len()));
    for p in &problems {
        out.push_str(&line(p));
    }
    (out, false)
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
fn start_core(product: &impl Product, path: &std::path::Path, f: &ServeFlags, store: Option<&edit::ProfileStore>, watch: Option<nuah_kernel::upgrade::OnUpgrade>) -> Result<(Core, Option<String>), Vec<nuah_kernel::Problem>> {
    // a plugin this version of the product replaced (renamed or rebuilt) gives way to its successor, settings and grants included, in
    // the saved file too, so the console and the product's settings edit the Profile as it now is
    match edit::move_to_successors(&product.catalog(), path, &product.replaced()) {
        Ok(moved) => {
            for (old, new) in moved {
                eprintln!("profile: {old} is now {new}");
            }
        }
        Err(e) => eprintln!("profile: could not move to successors: {e}"),
    }
    match edit::upgrade_saved(path, &mut |p| product.upgrade_profile(p)) {
        Ok(notes) => {
            for n in notes {
                eprintln!("profile: {n}");
            }
        }
        Err(e) => eprintln!("profile: could not upgrade the saved profile: {e}"),
    }
    let mut profile = read_profile(path);
    // The lock is checked against the file as it was saved, before this run's command-line changes.
    let changed = profile.contract.clone().filter(|c| *c != Core::contract_for(&Distribution { name: product.name().to_string(), catalog: product.catalog(), profile: profile.clone() }));
    apply_http_flags(&mut profile, HttpFlags { listen: f.listen.clone(), faces_dir: f.faces.clone(), hosts: f.allow_host.clone(), secure_cookie: f.secure_cookie });
    if let Some(level) = &f.log_level {
        profile.telemetry.level = Some(level.clone());
    }
    let listen_at = profile.settings["@nuah/http"]["listen"].as_str().unwrap_or("127.0.0.1:3000").to_string();
    // Who can reach the instance, when the file does not say: what the address suggests. The Resolver then checks every
    // plugin's declared limit against it (ADR 0087), so setup, the console and this start all see the same Problems.
    let from_address = profile.enabled.iter().any(|p| p == "@nuah/http").then(|| nuah_kernel::Reach::of_listen(&listen_at));
    if let Some(s) = store {
        s.set_default_reach(from_address);
    }
    profile.reach = profile.reach.or(from_address);
    Core::start_watched(&Distribution { name: product.name().to_string(), catalog: product.catalog(), profile }, watch).map(|c| {
        c.set_about(product.about());
        (c, changed)
    })
}

/// The components a Distribution is built from, read from its `Cargo.lock`: for each (shown name, crate), the tag and commit the
/// lock pins (`git+https://…?tag=v0.16.1#<sha>`); a local checkout has no tag or commit, so its crate version stands and the commit
/// is null; a crate the lock does not have is left out.
pub fn components_from_lock(lock: &str, wanted: &[(&str, &str)]) -> serde_json::Value {
    let mut out = vec![];
    for (shown, krate) in wanted {
        let mut found: Option<(String, Option<String>)> = None;
        for block in lock.split("[[package]]") {
            let field = |k: &str| block.lines().find_map(|l| l.trim().strip_prefix(&format!("{k} = \"")).and_then(|r| r.strip_suffix('"')).map(String::from));
            if field("name").as_deref() != Some(*krate) {
                continue;
            }
            let version = field("version").unwrap_or_default();
            found = Some(match field("source").and_then(|s| s.strip_prefix("git+").map(String::from)) {
                Some(src) => {
                    let (url, commit) = src.split_once('#').map(|(u, c)| (u.to_string(), Some(c.to_string()))).unwrap_or((src.clone(), None));
                    let tag = url.split_once("tag=").map(|(_, t)| t.split('&').next().unwrap_or(t).to_string());
                    (tag.unwrap_or(version), commit)
                }
                None => (version, None),
            });
            break;
        }
        if let Some((version, commit)) = found {
            out.push(serde_json::json!({"name": shown, "version": version, "commit": commit}));
        }
    }
    serde_json::Value::Array(out)
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
    let subject = list.output["accounts"].as_array().into_iter().flatten().find(|a| a["username"].as_str().is_some_and(|n| n.to_lowercase() == want)).and_then(|a| a["subject"].as_str()).map(String::from).ok_or_else(|| format!("no account with the username {name}"))?;
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
