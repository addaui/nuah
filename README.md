# Nuah (نواه)

A plugin-first core in Rust. A small kernel runs plugins; products are made by choosing and configuring them.

```bash
cargo test --workspace
```

## Layout

- `crates/nuah-kernel`: the kernel
- `crates/nuah-adapters`: adapters for the ports
- `crates/nuah-http`: HTTP projection (plugin)
- `crates/nuah-host`: the program that runs a product
- `faces/console`, `faces/setup`: generic faces

## TODO

- [ ] Publish the documentation
- [ ] Choose a license (currently `UNLICENSED`)
- [ ] Usage guide: depend on a tagged release from a product
- [ ] Contribution guide
- [ ] Telemetry ports (log, trace, metric, heartbeat)
- [ ] Settings tiers, admin plugin, backup and migration
- [ ] Verify the macOS and Windows CI legs
