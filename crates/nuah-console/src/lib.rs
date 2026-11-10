// SPDX-License-Identifier: Apache-2.0
//! The console plugin ([ADR 0033], [ADR 0039], [ADR 0041]): publishes the generic console Face (files in
//! `<faces dir>/nuah/console`, default `./faces/nuah/console`; it needs the `http` plugin to be served) and, when the
//! operator grants it the capability `shell`, lets an administrator read and change the identity file.
//!
//!   console.profile.get      the catalog and the stored Profile (secrets hidden), with its Problems
//!   console.profile.check    the Problems of a draft
//!   console.profile.save     write the draft if it has none; takes effect on restart
//!   console.restart          stop, read the file again, start
//!
//! [ADR 0033]: https://docs.addaui.com/nuah/adr/0033-the-console-is-a-generic-face-and-an-admin-plugin-edits-the-profile-settings-have-tiers/
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/
//! [ADR 0041]: https://docs.addaui.com/nuah/adr/0041-the-console-edits-the-identity-file-through-a-host-capability-and-an-operator-can-recover-a-locked-out-instance/

use nuah_kernel::{Bindings, Error, Manifest, Plugin};
use serde_json::{json, Value};

pub struct Console;

fn shell_error(message: String) -> Error {
    Error::internal(message)
}

/// The shell's services, or why the console cannot change anything here (the program running the Core attached none).
fn shell(cx: &nuah_kernel::Outbound) -> nuah_kernel::Result<std::sync::Arc<dyn nuah_kernel::ShellServices>> {
    cx.shell().map_err(|_| Error::reason(nuah_kernel::Category::Denied, "console.no_shell", "This instance does not let the console change its configuration (the program running it offers no shell services)"))
}

impl Plugin for Console {
    fn manifest(&self) -> Manifest {
        let draft = json!({"type":"object","required":["profile"],"properties":{"profile":{"type":"object"}},"additionalProperties":false});
        serde_json::from_value(json!({
            "id": "@nuah/console", "version": "0.3.0", "name": "Console", "namespaces": ["console"],
            "description": "A generic Face that shows the Core's Operations and health, and lets an administrator change the identity file",
            "requires": {"plugins": ["@nuah/http"], "capabilities": ["shell"]},
            "face": "console",
            "permissions": {"console.admin": {"description": "Read and change the identity file (plugins, grants, settings) and restart", "default": ["admin"]}},
            "operations": {
                "console.profile.get": {"kind": "outbound", "permission": "console.admin", "description": "The catalog and the stored Profile, secrets hidden.", "output": {"type":"object"}},
                "console.profile.check": {"kind": "outbound", "permission": "console.admin", "description": "The Problems of a draft Profile.", "input": draft, "output": {"type":"object","required":["problems"]}},
                "console.profile.save": {"kind": "outbound", "permission": "console.admin", "description": "Write the draft Profile if it has no Problems. It takes effect on restart.", "input": draft,
                    "reasons": {"console.problems": "The draft does not resolve; nothing was written", "console.no_shell": "The program running this instance offers the console no shell services"},
                    "output": {"type":"object","required":["saved"]}},
                "console.restart": {"kind": "outbound", "permission": "console.admin", "description": "Stop, read the identity file again and start.", "output": {"type":"object"}}
            }
        }))
        .expect("console manifest")
    }

    fn bind(&self, b: &mut Bindings) {
        b.outbound("console.profile.get", |cx, _| shell(cx)?.state().map_err(shell_error));
        b.outbound("console.profile.check", |cx, i| {
            let problems = shell(cx)?.check(&i["profile"]).map_err(|m| Error::invalid("profile", &m))?;
            Ok(json!({"problems": problems}))
        });
        b.outbound("console.profile.save", |cx, i| {
            let problems = shell(cx)?.save(&i["profile"]).map_err(shell_error)?;
            if !problems.is_empty() {
                let first = problems.iter().map(|p| p.message.as_str()).collect::<Vec<_>>().join(" ");
                return Err(Error::reason(nuah_kernel::Category::Invalid, "console.problems", &first));
            }
            cx.log(nuah_kernel::Level::Info, "console.saved", json!({"by": cx.who().subject}));
            Ok(json!({"saved": true, "restart_required": true}))
        });
        b.outbound("console.restart", |cx, _| {
            let shell_services = shell(cx)?;
            cx.log(nuah_kernel::Level::Warn, "console.restart", json!({"by": cx.who().subject}));
            shell_services.restart();
            Ok(Value::Object(Default::default()))
        });
    }
}
