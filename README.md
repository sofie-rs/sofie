# sofie

A Rust web framework built around a fixed app structure. Because every file has
a known place and role, the framework reads the whole app at build time and
derives the wiring, endpoints, screens, permissions, telemetry, documentation
and checks from it, so an app writes only its own logic.

> **Status: proof of concept.** APIs change without notice and nothing is
> published on crates.io yet. Do not build on it for production.

## Contents

- `crates/sofie`: the facade an app depends on (package `sofie-framework`,
  library `sofie`), re-exporting the other `sofie-*` crates behind features.
- `crates/sofie-cli`: the `sofie` command: `new`, `gen`, `check`, `dev`,
  `build`, `bench`, `explain` and more (`sofie help` lists them).
- `examples/`: a small shop, Svelte islands with typed props, and the same
  screens compiled into Svelte and React areas. The shop is only a familiar
  domain; nothing in it is part of the framework.
- `docs/structure.md`: the app structure, the contract every app is built
  against. `docs/core.md`: how the framework implements it.
- `bench/`: benchmarks and size limits checked against a Loco twin of the
  shop.

## Starting an app

Requires Rust 1.99 and a local Postgres. Until the crates are published, the
CLI installs from this repository, and a new app depends on the framework at
the commit the CLI was built from.

```sh
cargo install --git https://github.com/sofie-rs/sofie sofie-cli
sofie new app shop
cd shop
cp .env.example .env
```

In `.env`, set `DATABASE_URL` (such as
`postgres://sofie:sofie@127.0.0.1:25432/sofie` from this repository's
`compose.yaml`) and `SOFIE_SIGNING_KEYS` (the output of
`openssl rand -base64 32`). Then `sofie dev` migrates the database, runs the
app and reloads it on changes. `AGENTS.md` in the new app lists every command.

A binary started outside `sofie dev` migrates only with `SOFIE_MIGRATE=on`.

## Working on the framework

```sh
docker compose up -d
cargo run -p sofie-cli --bin sofie -- check examples/shop
cargo test --workspace
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
