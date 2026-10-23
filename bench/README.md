# Benchmark: the framework's shop vs a Loco twin

`bench/run.sh` measures the framework's example app (`examples/shop`) against
the same app built with [Loco](https://loco.rs) (`bench/loco-shop`) and writes
[results.md](results.md): app code size, framework and dependency size, build
times and binary size, and runtime throughput, latency, memory and start time.

```sh
bench/run.sh                                     # quick run, both apps (~1 min)
bench/run.sh --full                              # thorough run with clean builds (~12 min)
SKIP="sofie:runtime" bench/run.sh                   # leave one app's section out
APPS=loco SECTIONS="code runtime" bench/run.sh   # part of it
```

| | Quick (default) | `--full` |
|---|---|---|
| Release binaries | reused from `bench/target/`, built only if missing | built from clean, timed, then rebuilt after touching a handler |
| Data | 2 users × 200 orders | 2 users × 1 000 orders |
| Load | 2 s warm-up, 5 s per scenario | 5 s warm-up, 30 s per scenario |
| Audit rows counted | 3 s after the order run | 10 s after |

Both run the same three scenarios at 64 connections, and both report app
code, framework size, binary size, RSS and start time. `DURATION`, `WARMUP`,
`ORDERS_PER_USER`, `CONCURRENCY` and `AUDIT_WAIT` override either mode. Quick
mode reuses whatever binary is in `bench/target/`, so after changing an app run
`--full` once (or delete `bench/target/<app>`) before trusting its numbers.

Needs: Rust (`~/.cargo/env`), `oha` (`cargo install oha`), `psql`, `jq`,
`perl`, `openssl`, `lsof`, and Postgres at `127.0.0.1:25432` (user and password
`sofie`; override with `PG=postgres://user:pass@host:port`). Everything builds
`--offline --locked`: the run downloads nothing and changes no lock file.
Ports 8080 (framework; `SOFIE_PORT` picks another) and 5150 (Loco) must be free. The run creates and drops
the databases `bench_sofie` and `bench_loco`, and builds into `bench/target/`; raw
output (oha JSON, server logs, per-file counts) lands in `bench/out/`.

Run it on an otherwise idle machine: the load generator, both apps and Postgres
share the CPU, and the report records the load average so a noisy run shows.

## The two apps

Both implement the same features:

- Sign up, sign in, sign out, with a session cookie.
- Orders owned by the signed-in user: email, status (PENDING, PAID, SHIPPED,
  CANCELLED, REFUNDED, EXPIRED; PENDING → PAID/CANCELLED/EXPIRED,
  PAID → SHIPPED/REFUNDED), total in minor units plus a 3-letter currency,
  placed_at.
- Place an order (validated email; the amount parsed exactly from text such as
  `149.90` for the currency's decimals; positive), list one's own newest 50,
  cancel one's own (refused with a clear error when the status does not
  allow it); another user's order is a 404.
- A server-rendered HTML page with the list, a place-order form and a cancel
  button per row, and the same data as JSON.
- Placing and cancelling write an audit row through a background job; an
  hourly job expires orders still PENDING after 24 hours.
- Migrations for every table.

### Routes

| | Framework | Loco |
|---|---|---|
| Sign up / in | `POST /sign-up`, `POST /sign-in` (form or JSON) | `POST /sign-up`, `POST /sign-in` (form); `POST /api/auth/register`, `/api/auth/login` (JSON, from the starter) |
| Sign out | `POST /account/sign-out` | `POST /sign-out` |
| Session | cookie `__Host-shop-session` | cookie `auth_token` holding Loco's JWT (`Authorization: Bearer` also accepted) |
| Order list, JSON | `GET /account/orders`, `Accept: application/json` | `GET /api/orders` |
| Order page, HTML | `GET /account/orders` | `GET /orders` |
| Place an order | `POST /account/orders` | `POST /orders` (form), `POST /api/orders` (JSON) |
| Cancel | `POST /account/orders/{id}/cancel` | `POST /orders/{id}/cancel` (form), `POST /api/orders/{id}/cancel` (JSON) |

The routes, form bodies, cookie names and seed SQL for each app are variables
at the top of `run.sh`.

### How the Loco twin was made

`loco new --name loco_shop --db postgres --bg queue-postgres --assets serverside`
(Loco CLI 1.2.0, loco-rs 1.2.0): the SaaS starter with server-side Tera views,
users, JWT auth, mailers and a Postgres-backed job queue. Then, the way Loco's
docs and generators intend:

| Piece | How |
|---|---|
| `orders`, `audit_entries` tables and models | `cargo loco generate model …`, then edited: indexes in the orders migration; validation, money parsing, the status enum and transitions, finders on the model |
| `src/models/_entities/*` | Written by hand in exactly the shape `cargo loco db entities` writes, because that command needs `sea-orm-cli`, which was not installed (no downloads beyond the approved ones) |
| Order pages and JSON API | `cargo loco generate controller orders --auth`, then filled in; Tera templates by hand (Loco 1.2 has no HTML scaffold) |
| HTML sign-up / sign-in / sign-out | `cargo loco generate controller sessions`, then filled in: the starter's auth is a JSON API, so the pages that set the JWT cookie are app code. Loco's `auth.jwt.location` reads the cookie |
| Audit job | `cargo loco generate worker audit` (`BackgroundWorker`, enqueued with `perform_later`) |
| Expiry | `cargo loco generate task expire_orders` plus a `scheduler:` entry in the config, run hourly by `start --all` |
| Removed | Starter examples the shop does not use: the download worker, the hello page and its image and i18n strings, the playground example, the unused `dtos` module and its `ts-rs` dependency |
| Tests | The generated tests made to pass, plus request tests for orders and sessions and a worker test |

## How each number is measured

### App code

Non-blank, non-comment lines in every file the developer writes **and keeps**
in the app's tree, by kind:

- **Rust**: `.rs` outside migrations. Comments are `//` (doc comments
  included) and `/* */`.
- **Templates**: `.html`, Tera and mailer templates (`.t`), `.svelte`, `.ftl`.
  Comments are `<!-- -->` and `{# #}`.
- **Migrations**: `.sql` files and Loco's `migration/src/*.rs`. Comments are
  `--` or Rust's.
- **Config**: `Cargo.toml` files, `config/*.yaml`, `.cargo/config.toml`,
  `.rustfmt.toml`, `.env*`. Comments are `#`.
- **Tests**, reported separately and not in the total: `tests/`, Loco's
  `src/fixtures/` and insta snapshots.

A file a generator wrote once and the developer then owns counts as written
code: Loco's starter (auth controller, users model, mailers, user tasks, config
files, `app.rs`) and everything `cargo loco generate` produced. Left out are
files a tool regenerates and nobody edits: the framework's `app/src/wiring.rs`
(`sofie gen`) and generated contract output under `gen/`, and Loco's
`src/models/_entities/` (`cargo loco db entities`). Also left out on both
sides: `README.md`/`AGENTS.md`, `Cargo.lock`, `.gitignore`, `.github/`,
the local `.env` (not in git), coding-agent settings (`.claude/`, `.mcp.json`),
images, `target/`, deployment files (`Dockerfile`, `*.dockerignore`), which
Loco's twin does not have, and the app's `bench/` folder (the generated k6
script and the measured baseline `sofie bench` keeps).

Accounts are built into each framework differently: the framework ships them as
the `sofie-identity` crate, so the shop only depends on it and defines its
`Customer` area; Loco's starter generates the account system into the app,
where the developer owns it. So Loco's auth files count as app code. The report
also shows "of which accounts": the Loco files that exist only for accounts
(`controllers/auth.rs`, `models/users.rs`, `views/auth.rs`, `mailers/`, the
user tasks, the users migration, and the HTML sign-in/up controller and
templates), so the shop's own code can be compared without them.

`lib/count.awk` does the counting; a line with code and a trailing comment
counts as code.

### Framework and dependencies

- **Framework source**: Rust lines (same rule as above, with `tests/`,
  `benches/`, `examples/` and trailing `#[cfg(test)] mod … { }` blocks left
  out) of all of `crates/*` for the framework, and of the `loco-rs` crate for
  Loco.
- **Framework lines compiled into the app**: the same, for the framework
  crates in the app's dependency tree (`sofie*` crates; `loco-rs` and `loco-gen`).
  A framework crate counts only the files its build compiles: its root and
  each `mod name;` reached from it whose `#[cfg]` holds for the features the
  app enables (`lib/modules.awk`), so an opt-in value group the shop leaves
  off is not counted. Loco's crates come from the registry and count whole,
  as below; Loco's number is therefore an upper bound.
- **Dependency tree**: `cargo tree -e normal --prefix none` for the app's binary
  package (`shop`, `loco_shop`), host platform, deduplicated by name and
  version. The Rust lines of each crate are counted from its source in the
  Cargo registry (or its path) for the resolved version, by the same rule.
  The app's own crates are left out of the line total. This counts registry
  crates whole (path crates as above): code behind disabled features or other platforms' `cfg`s is
  included, and C sources (for example SQLite's) are not.

### Build

Only with `--full`. Clean release build of the app's binary into an empty `bench/target/<app>`,
then the rebuild after `touch` on one handler file
(`modules/order/src/actions.rs`; `src/controllers/orders.rs`), and the release
binary's size. Both use the same release profile through Cargo's
`CARGO_PROFILE_RELEASE_*` variables: Cargo's defaults (opt-level 3, no LTO, 16
codegen units, no debug info, unwind, not incremental), so neither manifest's
own profile applies. The touched file's content is not changed.
The shop is built, and its dependency tree counted, with
`--no-default-features`: the framework's `tls` feature, which the shop turns on
by default, is left out, as Loco's twin serves plain HTTP too.

### Runtime

1. Recreate the app's database, start the release binary (it migrates), sign
   up and sign in two users through the app, stop it.
2. Seed the same data in both: `ORDERS_PER_USER` orders per user,
   statuses cycling PENDING/PAID/SHIPPED/CANCELLED, placed over the last
   seconds; then `VACUUM ANALYZE`.
3. Start the binary again and time process start to the first `200` from the
   public sign-in page (polled every ~5 ms with curl).
4. `oha` as the first user: a warm-up on the JSON list, then `DURATION`
   at `CONCURRENCY` (64) connections each for (a) the JSON order list,
   (b) the HTML order page, (c) placing an order through the no-JavaScript
   form post (`303` to the page). POSTs carry `Origin` and
   `Sec-Fetch-Site: same-origin`, as a browser's do.
5. Requests/s counts only answers with the expected status; p50 and p99 are
   oha's latency percentiles; errors are every other answer or connection
   error (requests cut off by the end of the run are not counted).
6. The audit job's pace under load: audit rows written per second during the
   order run, and orders still without their audit row when it ended (their
   jobs are queued already; the rows are only late).
7. RSS of the process (`ps -o rss`) after the three runs, then the number of
   audit rows written from the order run's start to `AUDIT_WAIT` seconds after
   it.

Each app runs in production mode with its defaults: the framework's binary
with `DATABASE_URL` and a fresh `SOFIE_SIGNING_KEYS`; Loco's with
`start --all` (server, queue workers and scheduler in one process, as the
framework's binary runs them), `LOCO_ENV=production` and the variables its
production config requires (a fresh `JWT_SECRET`; mailer variables are set
but no mail is sent). Server logs go to `bench/out/`.

## Limits against Loco

[loco-baseline.json](loco-baseline.json) holds Loco's measured numbers and,
in one place, every limit the framework is held to as a share of Loco's:
app code ≤ 20%, crates in the dependency tree ≤ 30% and their Rust lines ≤ 40%, binary ≤ 25%, clean build
≤ 25%, incremental build ≤ 20%, read requests/s ≥ 3×, read p99 ≤ 50%, RSS ≤
60%, startup ≤ 80%, write requests/s ≥ 1× and audit rows ≥ 1×. A limit is only
ever tightened; Loco's numbers change only by measuring the twin again.
Framework lines compiled into the app are still measured and reported, but
held to no limit: the user removed it on 2026-10-12, since the framework does
more than Loco and will outgrow it.

- `bench/limits.sh` checks the first four without building, counting as
  `run.sh` does; `cargo test -p sofie-check --features slow-tests --test limits` runs it.
- Every report has a "Limits against Loco" table; `run.sh --full` exits
  non-zero when a limit is broken. A limit whose Loco number is `null` (build
  times, until a full run of the twin records them) is shown, not enforced.

## Where the comparison is uneven

Things that favour one side or make numbers differ for reasons other than the
framework, as far as they are known:

- **Database pool.** Both get the framework's default pool size (logical CPUs
  × 2, deadpool's default) — Loco's production default is 10, raised through
  its documented `DB_MAX_CONNECTIONS`. Loco's job queue keeps its own separate
  pool (default 20 connections, 2 workers, 1 s polling). The framework's
  worker also runs 2 jobs at a time by default but shares the app's pool;
  the bench gives it a pool of its own of 20 (`SOFIE_WORKER_POOL_SIZE=20`), so
  the jobs compare on equal resources. It is still woken by `LISTEN/NOTIFY`,
  which matters only to an idle queue.
- **Request logging.** The framework writes a JSON line per request
  ("request finished", off the request thread); Loco in production writes none.
  This costs the framework time, not Loco.
- **Queries per request.** Loco's idiomatic `auth::JWTWithUser` loads the user
  on every request (two queries for the order list); the framework's sealed
  session cookie carries the account id (one query).
- **Durability of the audit job.** The framework writes the event to its
  outbox in the same transaction as the order; Loco enqueues the job after the
  order is committed, from a separate pool, so a crash in between loses the
  audit row. Cancelling in Loco is a read then an update without a version
  check; the framework checks the row's version.
- **Security work per request.** The framework refuses cross-site POSTs and
  sends a Content-Security-Policy with a per-response nonce and `nosniff`; the
  Loco app runs Loco's default middleware stack (request id, logging, panic
  catching and so on) and has no CSRF protection for its cookie-authenticated
  forms. This costs the framework time, not Loco.
- **Response bodies differ** in size (reported per scenario): Loco's JSON
  carries `placed_at`, a formatted total and `can_cancel`; the framework's
  carries the allowed transitions. The HTML pages differ in markup.
- **Validation answers.** Loco reports a failed transition as a `400`
  validation error; the framework as `422`. A form with several bad fields
  shows all of them in the framework, and the email error before the amount
  error in the Loco app.
- **Dependencies by default.** Loco's starter compiles SQLite support into
  sea-orm (with bundled C SQLite) and `sea-orm-cli`/`loco-gen` code into the
  binary, which also is the app's CLI (migrations, generators, tasks). This is
  how Loco ships, so it is kept; it shows in build time and binary size.
- **Postgres over TLS.** The shop is built without sofie's `postgres-tls`
  (default-on), so `rustls`, `ring` and `webpki-roots` (about 1.6 MB of its
  binary) are left out and a url asking for encryption is refused at boot.
  Loco's twin compiles sea-orm with `runtime-tokio-rustls`. Both reach the
  bench's Postgres in plain text.
- **Generated entities.** `src/models/_entities/` were written by hand in the
  codegen's format (see above); they are not counted.
- **Shared machine.** oha (4 threads), the app and Postgres (in Docker here)
  run on one machine; Postgres also serves whatever else runs on it. Results
  vary between runs, so compare runs made back to back on an idle machine.
