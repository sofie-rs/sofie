# Sofie

[![crates.io](https://img.shields.io/crates/v/sofie-framework.svg)](https://crates.io/crates/sofie-framework)
[![docs](https://img.shields.io/badge/docs-sofie.rs-1f7a7a)](https://sofie.rs)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**A full-stack Rust web framework that reads your whole app and writes the rest.**

Every Sofie app has the same fixed structure, so the framework knows where each
route, record, screen and listener lives. From that it derives the wiring,
endpoints, forms, permissions, telemetry, documentation and architecture checks
that you would otherwise write and keep in step by hand. You write the business
logic; the compiler and `sofie check` keep the rest honest.

> **Status: proof of concept.** Published on crates.io as 0.1.0. APIs change
> without notice, so do not build on it for production yet.

## Why it matters

- **About a fifth of the code.** The same shop is 365 lines in Sofie and 1,834
  in [Loco](https://loco.rs), measured by `bench/run.sh` with accounts
  included on both sides.
- **Fast to build, fast to run.** Against that Loco twin: a 4× faster clean
  build, a 3.5× smaller binary and about 4× the requests per second
  (24,000 vs 5,900 on the JSON list).
  [Methodology and caveats](https://sofie.rs/why/performance/).
- **Architecture that cannot drift.** A module reaching into another, a stale
  wiring file or an unsafe migration fails `cargo build`, not a code review.
- **Production concerns are on by default.** CSRF, sealed sessions, owner
  scoping, an allowlist for outgoing requests, masked logs, traces, metrics
  and health checks come without configuration.
- **One database.** Postgres (or SQLite) also carries the event outbox, jobs,
  schedules and cache. Redis and Meilisearch are optional.

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

- **A server-rendered page** whose form fields, labels and money input come
  from `AddProduct`, and whose table updates in place without a page reload.
- **The same URL as an API:** `Accept: application/json` returns the rows as
  JSON. No second handler is needed.
- **Validation:** a bad price answers `422` with the message under its field.
  Without JavaScript, a successful post redirects back to the page.
- **Security:** a cross-site post is refused with `403`, and what the action
  returns is saved in one transaction after it succeeds.
- **Wiring and checks:** `sofie gen` writes the wiring, and `sofie check` fails
  the build if a template binds a field the query does not return.
- **An app you can ask about:** `sofie routes` lists every endpoint, and
  `sofie explain catalog.web.add_product` shows its input, an example body,
  the call flow and the screen it renders.

## Features

| | |
|---|---|
| **Screens** | One template format rendered on the server and updated over htmx; [Svelte islands](https://sofie.rs/architecture/screens/) with typed props where you need rich UI |
| **Data** | Aggregates with checked migrations, typed queries, soft delete, slugs, audit, children rows, zero-downtime migration checks |
| **Messages** | Transactional outbox, listeners, retried jobs and schedules, all on the database |
| **Accounts** | `sofie-identity`: sign-up, email links, password reset, passkeys, OAuth, TOTP, revocable sessions, impersonation; organizations and seats with `sofie-tenancy` |
| **SaaS packages** | [Billing](https://sofie.rs/features/billing/) with Stripe or Paddle, [mail and notifications](https://sofie.rs/features/mail-and-notifications/), [feature flags](https://sofie.rs/features/feature-flags/), [search, storage and cache](https://sofie.rs/features/data-services/), [content pages](https://sofie.rs/features/content/) |
| **Security** | [On by default](https://sofie.rs/features/security/): CSRF, sealed `__Host-` cookies, rate limits, owner scoping, permissions, an allowlist for outgoing requests |
| **Observability** | [Traces, logs, metrics, product events and health checks](https://sofie.rs/features/observability/) without code, with personal data masked |
| **Testing** | A [test harness](https://sofie.rs/features/testing/) with a movable clock, scaffolded tests, fixtures, fuzzing, load tests and seed data |
| **Docs** | [Architecture, flow, security and telemetry pages](https://sofie.rs/features/generated-docs/), OpenAPI and TypeScript types, written from the app |
| **Coding agents** | A generated `AGENTS.md` and an [MCP server](https://sofie.rs/features/ai-agents/) (`sofie mcp`) over the same app model |
| **Deploy** | `sofie build` makes one binary or container image that runs as web, worker or both; `sofie split` moves a module into its own app later |

## Quick start

Requires Rust 1.99 and Postgres.

```sh
cargo install sofie-cli
sofie new app shop
cd shop
cp .env.example .env    # set DATABASE_URL and SOFIE_SIGNING_KEYS
sofie dev
```

`SOFIE_SIGNING_KEYS` takes the output of `openssl rand -base64 32`. For a local
database, `docker compose up -d` in this repository starts one at
`postgres://sofie:sofie@127.0.0.1:25432/sofie`.

Then follow [Your first page](https://sofie.rs/start/first-page/) to build the
catalog above, step by step, in about ten minutes.

## Learn more

- [Getting started](https://sofie.rs/start/getting-started/) and
  [How Sofie works](https://sofie.rs/start/how-it-works/)
- [Why Sofie](https://sofie.rs/why/philosophy/): the problem, the principles
  and what they cost
- [Architecture](https://sofie.rs/architecture/overview/): modules, contracts,
  aggregates and areas
- `examples/shop`: the benchmarked shop, with accounts, owned orders, events,
  an audit listener and an hourly job
- `docs/structure.md`: the app structure every app follows;
  `docs/core.md`: how the framework implements it

## Repository layout

- `crates/sofie`: the facade an app depends on (package `sofie-framework`,
  library `sofie`), re-exporting the other `sofie-*` crates behind features.
- `crates/sofie-cli`: the `sofie` command (`new`, `gen`, `check`, `dev`,
  `build`, `explain`, `mcp`, …; `sofie help` lists them).
- `examples/`: a shop, Svelte islands with typed props, and screens compiled
  into a Svelte client area. The shop is only a familiar domain; nothing in it
  is part of the framework.
- `bench/`: benchmarks and size limits checked against a Loco twin of the shop.

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
