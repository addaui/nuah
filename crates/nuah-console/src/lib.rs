// SPDX-License-Identifier: Apache-2.0
//! The console plugin ([ADR 0033], [ADR 0039]): a data plugin that publishes the generic console Face. Its
//! files are in `<faces dir>/console` (default `./faces/console`). It needs the `http` plugin to be served.
//!
//! [ADR 0033]: https://docs.addaui.com/nuah/adr/0033-the-console-is-a-generic-face-and-an-admin-plugin-edits-the-profile-settings-have-tiers/
//! [ADR 0039]: https://docs.addaui.com/nuah/adr/0039-accounts-belong-to-the-auth-plugin-and-live-in-the-database-faces-setup-and-console-are-plugins/

use nuah_kernel::{Manifest, Plugin};
use serde_json::json;

pub struct Console;

impl Plugin for Console {
    fn manifest(&self) -> Manifest {
        serde_json::from_value(json!({
            "id": "console", "version": "0.1.0", "name": "Console", "runtime": "data",
            "description": "A generic Face that shows the Core's Operations, health and settings",
            "requires": {"plugins": ["http"]},
            "face": "console"
        }))
        .unwrap()
    }
}
