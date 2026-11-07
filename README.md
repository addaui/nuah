# Nu'ah (نواه)

A plugin-first core: a small kernel runs plugins, and products are made by choosing and configuring them.
**Nu'ah is a product of its own and depends on no product.** Products (such as
[Madar](https://github.com/addaui/madar), a task manager) depend on it, pinned by tag, and add their own
plugins and faces. A product never edits Nu'ah; when something generic is missing it becomes a plugin or a
pull request here.

Documentation (PRD, ADRs, specs, plans, tasks, glossary) lives in docmd, not in this repo:
https://docs.addaui.com/nuah/ . Working rules for contributors and agents are in `the contributor notes`.

## Layout

| Path | What it is |
|---|---|
| `crates/nuah-kernel` | The kernel: manifests, resolver, data Port, call pipeline, Pulse, Listeners, services, contract generator. No dependencies on anything of ours; knows nothing about any product and has no transport. |
| `crates/nuah-adapters` | Adapters for the Ports: memory and SQLite data (one writer, a pool of readers), auth, policy, clock, ids; the data conformance suite. |
| `crates/nuah-http` | The `http` plugin: the HTTP and SSE Projection of the Link. A product adds routes with `Http::with_routes`. |
| `crates/nuah-host` | The one `main` a product needs: `nuah_host::run(product)` reads a Profile, starts the Core and its services, stops on SIGINT or SIGTERM. Also the setup wizard and a container health check. |
| `faces/console`, `faces/setup` | The console generated from `describe` and the setup wizard (vanilla JS, no build). |

## Use it from a product

```toml
# Cargo.toml of the product
[workspace.dependencies]
nuah-kernel = { git = "https://github.com/addaui/nuah", tag = "v0.3.0" }
nuah-adapters = { git = "https://github.com/addaui/nuah", tag = "v0.3.0" }
nuah-http = { git = "https://github.com/addaui/nuah", tag = "v0.3.0" }
nuah-host = { git = "https://github.com/addaui/nuah", tag = "v0.3.0" }
```

```rust
// main.rs of the product: its plugins, its Profile shortcuts, nothing else
fn main() { nuah_host::run(MyProduct) }
```

Taking an update is changing the tag (Madar's `nuah-update` workflow opens that pull request).

## Develop

Needs Rust (`rustup`, stable).

```bash
cargo test --workspace
```

Releases are git tags `vX.Y.Z`; the `release` workflow tests on Linux, macOS and Windows and publishes the release.
