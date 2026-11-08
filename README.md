# Nuah (نواه)

An open-source framework and runtime in Rust. A small kernel runs plugins; product distributions are made by choosing and configuring them.

Nuah is the foundation, not an end-user application. [Madar](https://github.com/addaui/madar) is an application built on it.

```bash
cargo test --workspace
```

## Layout

- `crates/nuah-kernel`: the kernel
- `crates/nuah-adapters`: adapters for the ports
- `crates/nuah-http`: HTTP projection (plugin)
- `crates/nuah-host`: the program that runs a product
- `faces/console`, `faces/setup`: generic faces

## License

[Apache License 2.0](LICENSE). Use it, change it, self-host it, and build your own products and services on it, commercially or not. The names and logos are covered separately, see [TRADEMARK.md](TRADEMARK.md). To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md).

## TODO

- [ ] Publish the documentation
- [ ] Usage guide: depend on a tagged release from a product
- [ ] Telemetry ports (log, trace, metric, heartbeat)
- [ ] Settings tiers, admin plugin, backup and migration
- [ ] Verify the macOS and Windows CI legs
