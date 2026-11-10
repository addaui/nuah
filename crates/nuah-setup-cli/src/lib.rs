// SPDX-License-Identifier: Apache-2.0
//! First-run setup in the terminal ([ADR 0039], [ADR 0041]): a [`Bootstrap`] a product picks when its
//! distribution has no browser (or the operator prefers a prompt). It asks which plugins run, where the web
//! server listens and who the first administrator is, then writes the identity file with the same checks as
//! the web wizard: nothing is written unless the Profile resolves.
//!
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
//! [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/

use nuah_shell::edit::{first_account_plugin, first_account_problem, has_accounts, put_first_account, ProfileStore, MIN_PASSWORD};
use nuah_shell::{Bootstrap, SetupContext};
use serde_json::json;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::Mutex;

/// Where the questions go and the answers come from. The default is the terminal; a test supplies both.
pub struct Io {
    input: Box<dyn BufRead + Send>,
    output: Box<dyn Write + Send>,
    /// Read passwords without echo (only when the input is a terminal).
    hidden: bool,
}

pub struct SetupCli {
    io: Mutex<Io>,
}

impl SetupCli {
    pub fn new() -> Self {
        let hidden = std::io::stdin().is_terminal();
        SetupCli { io: Mutex::new(Io { input: Box::new(std::io::BufReader::new(std::io::stdin())), output: Box::new(std::io::stderr()), hidden }) }
    }
    /// Answers from `input`, questions to `output`; passwords are read as ordinary lines.
    pub fn with_io(input: impl BufRead + Send + 'static, output: impl Write + Send + 'static) -> Self {
        SetupCli { io: Mutex::new(Io { input: Box::new(input), output: Box::new(output), hidden: false }) }
    }
}

impl Default for SetupCli {
    fn default() -> Self {
        SetupCli::new()
    }
}

impl Io {
    fn say(&mut self, text: &str) {
        let _ = writeln!(self.output, "{text}");
    }
    /// One line; `None` at the end of the input.
    fn line(&mut self, question: &str) -> Option<String> {
        let _ = write!(self.output, "{question} ");
        let _ = self.output.flush();
        let mut s = String::new();
        match self.input.read_line(&mut s) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(s.trim_end_matches(['\r', '\n']).to_string()),
        }
    }
    fn ask(&mut self, question: &str, default: &str) -> Result<String, String> {
        let q = if default.is_empty() { format!("{question}:") } else { format!("{question} [{default}]:") };
        let a = self.line(&q).ok_or("setup was cut short: no more input")?;
        Ok(if a.trim().is_empty() { default.to_string() } else { a.trim().to_string() })
    }
    fn yes(&mut self, question: &str, default: bool) -> Result<bool, String> {
        let hint = if default { "Y/n" } else { "y/N" };
        let a = self.ask(&format!("{question} ({hint})"), "")?.to_lowercase();
        Ok(match a.as_str() {
            "" => default,
            "y" | "yes" => true,
            _ => false,
        })
    }
    fn password(&mut self, question: &str) -> Result<String, String> {
        if self.hidden {
            let _ = write!(self.output, "{question}: ");
            let _ = self.output.flush();
            let p = rpassword::read_password().map_err(|e| e.to_string())?;
            let _ = writeln!(self.output);
            Ok(p)
        } else {
            self.line(&format!("{question}:")).ok_or_else(|| "setup was cut short: no more input".to_string())
        }
    }
}

impl Bootstrap for SetupCli {
    fn run(&self, ctx: SetupContext) -> Result<(), String> {
        let mut io = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let store = ProfileStore::new(&ctx.product, ctx.catalog.clone(), ctx.starting_point.clone(), ctx.identity_file.clone());
        let mut p = store.stored()?.unwrap_or_else(|| ctx.starting_point.clone());
        io.say(&format!("Setting up {} in {}", ctx.product, ctx.identity_file.display()));

        // which plugins run
        let manifests: Vec<_> = ctx.catalog.0.iter().map(|pl| pl.manifest()).collect();
        io.say(&format!("Running now: {}", if p.enabled.is_empty() { "nothing".to_string() } else { p.enabled.join(", ") }));
        if io.yes("Change which plugins run?", false)? {
            let mut enabled = vec![];
            for m in &manifests {
                let on = io.yes(&format!("  {} - {}", m.id, m.description.lines().next().unwrap_or("")), p.enabled.contains(&m.id))?;
                if on {
                    enabled.push(m.id.clone());
                    // a plugin that asks for a capability gets it; the operator chose the plugin
                    if !m.requires.capabilities.is_empty() {
                        let g = p.grants.entry(m.id.clone()).or_default();
                        for c in &m.requires.capabilities {
                            if !g.contains(c) {
                                g.push(c.clone());
                            }
                        }
                    }
                } else {
                    p.grants.remove(&m.id);
                    p.settings.remove(&m.id);
                }
            }
            p.enabled = enabled;
        }

        // where the web server listens
        if p.enabled.iter().any(|id| id == "@nuah/http") {
            let current = ctx.listen.clone().or_else(|| p.settings.get("@nuah/http").and_then(|s| s["listen"].as_str()).map(String::from)).unwrap_or_else(|| "127.0.0.1:3000".into());
            let listen = io.ask("Web server listens on", &current)?;
            let s = p.settings.entry("@nuah/http".into()).or_insert_with(|| json!({}));
            if let Some(o) = s.as_object_mut() {
                o.insert("listen".into(), json!(listen));
            }
        }

        // the first administrator
        let mut problems = vec![];
        if let Some(plugin) = first_account_plugin(&ctx.catalog, &p) {
            if !has_accounts(&p, &plugin) {
                io.say("Create the first administrator.");
                let name = io.ask("  Name", "")?;
                let password = loop {
                    let one = io.password(&format!("  Password (at least {MIN_PASSWORD} characters)"))?;
                    if one.chars().count() < MIN_PASSWORD {
                        io.say("  Too short.");
                        continue;
                    }
                    if io.password("  Again")? == one {
                        break one;
                    }
                    io.say("  They do not match.");
                };
                match first_account_problem(&plugin, &name, &password) {
                    Some(problem) => problems.push(problem),
                    None => put_first_account(&mut p, &plugin, &name, &password),
                }
            }
        }

        let problems = store.commit(p, problems)?;
        if !problems.is_empty() {
            for pr in &problems {
                io.say(&format!("  {}: {}", pr.code, pr.message));
            }
            return Err("the choices do not resolve; nothing was written".into());
        }
        io.say(&format!("Saved {}.", ctx.identity_file.display()));
        Ok(())
    }
}
