<p align="center">
  <img src="https://raw.githubusercontent.com/sofie-rs/sofie/main/.github/assets/logo.png" alt="Sofie logo" width="120">
</p>

<h1 align="center">Sofie</h1>

<p align="center">
  <b>A full-stack Rust web framework that reads your whole app and writes the rest.</b>
</p>

<p align="center">
  <a href="https://crates.io/crates/sofie-framework"><img src="https://img.shields.io/crates/v/sofie-framework.svg?color=1f7a7a" alt="crates.io"></a>
  <a href="https://sofie.rs"><img src="https://img.shields.io/badge/docs-sofie.rs-1f7a7a" alt="docs"></a>
  <a href="https://github.com/sofie-rs/sofie/actions/workflows/ci.yml"><img src="https://github.com/sofie-rs/sofie/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/rust-1.99%2B-1f7a7a" alt="Rust 1.99+">
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="license"></a>
</p>

<p align="center">
  <a href="https://sofie.rs/start/getting-started/"><b>Get started</b></a> ·
  <a href="https://sofie.rs/start/first-page/"><b>Your first page</b></a> ·
  <a href="https://sofie.rs/architecture/overview/"><b>Architecture</b></a> ·
  <a href="examples/shop"><b>Example shop</b></a> ·
  <a href="bench/results.md"><b>Benchmarks</b></a>
</p>

---

Every Sofie app has the same fixed structure, so the framework knows where each
route, record, screen and listener lives. From that it derives the wiring,
endpoints, forms, permissions, telemetry, documentation and architecture checks
you would otherwise write and keep in step by hand. **You write the business
logic; the compiler and `sofie check` keep the rest honest.**

> [!WARNING]
> **Pre-1.0.** Every planned feature is built and has been through sixteen
> audit rounds, but APIs still change between minor versions and no
> production app runs on Sofie yet. Try it, but wait for 1.0 before building
> production on it; see [Road to 1.0](#road-to-10).

## Why Sofie

<table>
  <tr>
    <td width="50%" valign="top">
      <h3>✂️ A fifth of the code</h3>
      The same shop is <b>365 lines</b> in Sofie and <b>1,834</b> in <a href="https://loco.rs">Loco</a>, accounts included on both sides.
    </td>
    <td width="50%" valign="top">
      <h3>⚡ Fast to build and run</h3>
      About <b>4× the requests per second</b>, a 4× faster clean build and a 3.5× smaller binary than that Loco twin.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <h3>🧭 Architecture that cannot drift</h3>
      A module reaching into another, a stale wiring file or an unsafe migration fails <code>cargo build</code>, not a code review.
    </td>
    <td width="50%" valign="top">
      <h3>🛡️ Production defaults</h3>
      CSRF, sealed sessions, owner scoping, an outgoing-request allowlist, masked logs, traces, metrics and health checks, with no configuration.
    </td>
  </tr>
  <tr>
    <td colspan="2" valign="top">
      <h3>🗄️ One database</h3>
      Postgres (or SQLite) also carries the event outbox, jobs, schedules and cache. Redis and Meilisearch are optional.
    </td>
  </tr>
</table>

<p align="center">
  <img src="https://raw.githubusercontent.com/sofie-rs/sofie/main/.github/assets/benchmark.svg" alt="Sofie against a Loco twin of the same shop: 24,313 vs 5,894 requests per second, 20.4 s vs 80.8 s clean build, 9.4 MB vs 32.6 MB binary, 365 vs 1,834 lines of app code" width="100%">
</p>
<p align="center"><sub>Measured by <code>bench/run.sh</code> on an Apple M3 Pro on 2026-10-16. <a href="https://sofie.rs/why/performance/">Methodology and caveats</a> · <a href="bench/results.md">raw numbers</a></sub></p>

## How it works

```mermaid
flowchart LR
    subgraph write["You write"]
        direction TB
        A["Aggregates<br/><sub>records and rules</sub>"]
        B["Actions and queries<br/><sub>plain async functions</sub>"]
        C["Screens<br/><sub>one template format</sub>"]
        D["Routes<br/><sub>one line each</sub>"]
    end
    S{{"sofie gen<br/>sofie check"}}
    subgraph get["Sofie derives"]
        direction TB
        E["HTML pages and a JSON API<br/><sub>on the same URL</sub>"]
        F["Forms, validation, CSRF<br/><sub>owner scoping, permissions</sub>"]
        G["Migration checks, events, jobs<br/><sub>on the one database</sub>"]
        H["Docs, OpenAPI, TypeScript<br/><sub>traces, metrics, MCP</sub>"]
    end
    write --> S --> get
```

## What you write, what you get

A product catalog: a record, an action that adds one, a query that lists them
and a screen that shows both.

```rust
// modules/catalog/src/product/mod.rs
#[derive(Aggregate)]
pub struct Product {
    meta: Meta,
    name: String,
    price: Money,
}

impl Product {
    pub fn new(name: String, price: Money) -> Result<Self> {
        if !price.is_positive() {
            return Err(Error::invalid("price", "must be positive"));
        }
        Ok(Self { meta: Meta::new()?, name, price })
    }
}

// modules/catalog/src/actions.rs
#[derive(Input)]
pub struct AddProduct {
    name: String,
    price: Money,
}

pub async fn add_product(input: AddProduct) -> Result<Product> {
    Product::new(input.name, input.price)
}

// modules/catalog/src/queries.rs
#[derive(ReadModel)]
#[read(from = Product)]
pub struct ProductRow {
    name: String,
    price: Money,
}

pub async fn list_products(read: Read) -> Result<Vec<ProductRow>> {
    ProductRow::query().latest().limit(50).all(&read).await
}

// modules/catalog/src/areas/web/routes.rs
pub fn routes(routes: &mut Routes<Web>) {
    routes.screen("/products", "products", queries::list_products);
    routes
        .post("/products", actions::add_product)
        .then_screen("products", "products");
}
```

```html
<!-- modules/catalog/src/areas/web/screens/products.html -->
<Section title="Products">
  <Form id="add-product" post="/products" target="products">
    <Submit label="Add product" />
  </Form>
  <Table id="products" of={data} columns="name price" />
</Section>
```

No handler parses a request, no form lists its fields and no file registers a
route. From these few lines Sofie gives you:

| | You get |
|---|---|
| 🖥️ **A page** | Server-rendered, with form fields, labels and a money input taken from `AddProduct`; the table updates in place without a reload |
| 🔌 **An API** | The same URL answers `Accept: application/json` with the rows. No second handler |
| ✅ **Validation** | A bad price answers `422` with the message under its field; without JavaScript, a successful post redirects back |
| 🔒 **Security** | A cross-site post is refused with `403`; what the action returns is saved in one transaction once it succeeds |
| 🔧 **Wiring and checks** | `sofie gen` writes the wiring; `sofie check` fails the build if a template binds a field the query does not return |
| 🔎 **An app you can ask about** | `sofie routes` lists every endpoint; `sofie explain catalog.web.add_product` shows its input, an example body, the call flow and the screen it renders |

## Features

| | |
|---|---|
| 🖥️ **Screens** | One template format rendered on the server and updated over htmx; [Svelte islands](https://sofie.rs/architecture/screens/) with typed props where you need rich UI |
| 🗄️ **Data** | Aggregates with checked migrations, typed queries, soft delete, slugs, audit, children rows, zero-downtime migration checks |
| 📨 **Messages** | Transactional outbox, listeners, retried jobs and schedules, all on the database |
| 👤 **Accounts** | `sofie-identity`: sign-up, email links, password reset, passkeys, OAuth, TOTP, revocable sessions, impersonation; organizations and seats with `sofie-tenancy` |
| 💳 **SaaS packages** | [Billing](https://sofie.rs/features/billing/) with Stripe or Paddle, [mail and notifications](https://sofie.rs/features/mail-and-notifications/), [feature flags](https://sofie.rs/features/feature-flags/), [search, storage and cache](https://sofie.rs/features/data-services/), [content pages](https://sofie.rs/features/content/) |
| 🛡️ **Security** | [On by default](https://sofie.rs/features/security/): CSRF, sealed `__Host-` cookies, rate limits, owner scoping, permissions, an allowlist for outgoing requests |
| 📈 **Observability** | [Traces, logs, metrics, product events and health checks](https://sofie.rs/features/observability/) without code, with personal data masked |
| 🧪 **Testing** | A [test harness](https://sofie.rs/features/testing/) with a movable clock, scaffolded tests, fixtures, fuzzing, load tests and seed data |
| 📚 **Docs** | [Architecture, flow, security and telemetry pages](https://sofie.rs/features/generated-docs/), OpenAPI and TypeScript types, written from the app |
| 🤖 **Coding agents** | A generated `AGENTS.md` and an [MCP server](https://sofie.rs/features/ai-agents/) (`sofie mcp`) over the same app model |
| 🚀 **Deploy** | `sofie build` makes one binary or container image that runs as web, worker or both; `sofie split` moves a module into its own app later |

## Quick start

Requires Rust 1.99 and Postgres.

```sh
cargo install sofie-cli
sofie new app shop
cd shop
cp .env.example .env    # set DATABASE_URL and SOFIE_SIGNING_KEYS
sofie dev
```

> [!TIP]
> `SOFIE_SIGNING_KEYS` takes the output of `openssl rand -base64 32`. For a
> local database, `docker compose up -d` in this repository starts one at
> `postgres://sofie:sofie@127.0.0.1:25432/sofie`.

Then follow [Your first page](https://sofie.rs/start/first-page/) to build the
catalog above, step by step, in about ten minutes.

## Learn more

| | |
|---|---|
| 🚦 [Getting started](https://sofie.rs/start/getting-started/) · [How Sofie works](https://sofie.rs/start/how-it-works/) | The first ten minutes |
| 💡 [Why Sofie](https://sofie.rs/why/philosophy/) | The problem, the principles and what they cost |
| 🏛️ [Architecture](https://sofie.rs/architecture/overview/) | Modules, contracts, aggregates and areas |
| 🛒 [`examples/shop`](examples/shop) | The benchmarked shop: accounts, owned orders, events, an audit listener and an hourly job |
| 📐 [`docs/structure.md`](docs/structure.md) · [`docs/core.md`](docs/core.md) | The structure every app follows, and how the framework implements it |

## Road to 1.0

1.0 is a promise that the public API holds until 2.0. Before making it:

- [ ] A full audit round finds nothing of medium or high severity.
- [ ] The whole suite passes with every feature on, not only feature by feature.
- [ ] A real product, not only the example shop, is built on Sofie and the API
      changes it asks for are made.
- [ ] The public surface of `sofie-framework` and `sofie-cli` is reviewed and
      frozen.

## Repository layout

| Path | What it holds |
|---|---|
| [`crates/sofie`](crates/sofie) | The facade an app depends on (package `sofie-framework`, library `sofie`), re-exporting the other `sofie-*` crates behind features |
| [`crates/sofie-cli`](crates/sofie-cli) | The `sofie` command: `new`, `gen`, `check`, `dev`, `build`, `explain`, `mcp`, … (`sofie help` lists them) |
| [`examples/`](examples) | A shop, Svelte islands with typed props, and screens compiled into a Svelte client area. The shop is only a familiar domain; nothing in it is part of the framework |
| [`bench/`](bench) | Benchmarks and size limits checked against a Loco twin of the shop |

<details>
<summary><b>Working on the framework</b></summary>

```sh
docker compose up -d
cargo run -p sofie-cli --bin sofie -- check examples/shop
cargo test --workspace
```

`docker compose up -d` starts the development and test Postgres on port 25432.
Each test creates and drops its own database there; set
`SOFIE_TEST_DATABASE_URL` to use another server.

Before a change lands:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test -p sofie-check --test structure
cargo test --workspace --all-features
```

The structure test holds the comment-density, god-type and dead-code ceilings
that `sofie check` applies to apps. Tests that build and run the shop sit
behind the `slow-tests` feature of `sofie-check` and `sofie-cli`, which
`--all-features` turns on.

</details>

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
