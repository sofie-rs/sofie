# Framework vs Loco: measurements

Produced by `bench/run.sh`; how each number is measured is in
[README.md](README.md). "not run" means that part was not measured in this run.

## Machine

- Date: 2026-10-16 01:16 UTC
- CPU: Apple M3 Pro, 11 cores (5 performance + 6 efficiency), 11 logical
- Memory: 18 GB
- OS: macOS 26.6.2 (25G83)
- Toolchain: rustc 1.99.0 (b940084d7 2026-09-28)
- Postgres: 18.6, on the same machine
- Load generator: oha 1.16.0, 4 threads
- Load average when the run started: 3.86 7.43 6.48
- Run: `APPS="sofie loco" SECTIONS="code size build runtime" SKIP="" bench/run.sh --full` (full mode)

## App code

Non-blank, non-comment lines the developer writes and keeps. Generated files
that are regenerated and never edited are left out (the framework's
`app/src/wiring.rs`, Loco's `src/models/_entities/`); files a generator wrote
once and the developer then owns are counted.

| Kind | framework (`examples/shop`) | Loco (`bench/loco-shop`) |
|---|---:|---:|
| Rust | 240 | 1387 |
| Templates | 11 | 108 |
| Migrations | 40 | 114 |
| Config | 56 | 225 |
| Other | 18 | 0 |
| **Total** | 365 | 1834 |
| of which accounts (sign-up, sign-in, users) | 0 | 889 |
| Files counted | 31 | 49 |
| Tests (not in the total) | 1289 | 1154 |

## Framework and dependencies

| Measure | framework (`examples/shop`) | Loco (`bench/loco-shop`) |
|---|---:|---:|
| Framework crates | 25 | 1 |
| Framework source, Rust lines (all its crates) | 96876 | 16725 |
| Framework Rust lines compiled into the app | 27521 | 18313 |
| Crates in the app's dependency tree (with the app's own) | 121 | 444 |
| Crates compiled for the app, excluding the app's own | 118 | 442 |
| Rust lines in those crates | 754311 | 2204449 |

## Build

Build times are measured only by `run.sh --full`. Release profile for both: opt-level 3, no LTO, 16 codegen units, no debug
info, not incremental (Cargo's defaults, set explicitly).

| Measure | framework | Loco | Loco / framework |
|---|---:|---:|---:|
| Clean release build (s) | 20.4 | 80.8 | 3.96 |
| Release rebuild after touching one handler file (s) | 3.1 | 13.6 | 4.39 |
| Release binary (MB) | 9.4 | 32.6 | 3.47 |

## Runtime

Release binaries, each against its own database on the same Postgres, seeded
with 2 users who signed up through the app and 1000 orders each.
`oha` warms up for 5s, then runs 30s per scenario at 64 connections as the first user;
both apps use a pool of 22 database connections, and their jobs run
2 at a time (both defaults) on a pool of 20 of their own (Loco's queue default).
Requests/s counts only answers with the expected status.

| Scenario | framework | Loco | Loco / framework |
|---|---:|---:|---:|
| JSON order list: requests/s | 24313 | 5894 | 0.24 |
| JSON order list: p50 (ms) | 2.48 | 10.49 | 4.23 |
| JSON order list: p99 (ms) | 5.10 | 15.11 | 2.96 |
| JSON order list: errors (%) | 0.00 | 0.00 | - |
| JSON order list: bytes per answer | 6910 | 8119 | 1.17 |
| HTML order page: requests/s | 23804 | 5632 | 0.24 |
| HTML order page: p50 (ms) | 2.54 | 10.97 | 4.32 |
| HTML order page: p99 (ms) | 5.18 | 15.69 | 3.03 |
| HTML order page: errors (%) | 0.00 | 0.00 | - |
| HTML order page: bytes per answer | 11012 | 9212 | 0.84 |
| Place order (form post): requests/s | 9994 | 4173 | 0.42 |
| Place order (form post): p50 (ms) | 6.20 | 14.92 | 2.41 |
| Place order (form post): p99 (ms) | 10.36 | 22.71 | 2.19 |
| Place order (form post): errors (%) | 0.00 | 0.00 | - |
| Orders placed | 299833 | 125200 | 0.42 |
| Audit rows written per second during the order run | 64 | 23 | 0.36 |
| Orders without their audit row when the order run ended | 297908 | 124513 | 0.42 |
| Audit rows written from the order run's start to 10 s after it | 2152 | 868 | 0.40 |
| RSS after the runs (MB) | 19 | 34 | 1.79 |
| Start to first successful request (ms) | 49 | 92 | 1.88 |
| Machine load (1-min average) when the app's runs began | 9.98 | 10.19 | 1.02 |

## Limits against Loco

The framework's values against Loco's in `loco-baseline.json`, which holds
every ceiling; `run.sh --full` exits non-zero when one is broken.

| Measure | framework | Loco baseline | Ratio | Limit | Verdict |
|---|---:|---:|---:|---|---|
| App code lines | 365 | 1834 | 0.199 | ≤ 0.20 × Loco | ok |
| Crates in the app's dependency tree | 121 | 444 | 0.273 | ≤ 0.30 × Loco | ok |
| Rust lines of those crates | 754311 | 2204449 | 0.342 | ≤ 0.40 × Loco | ok |
| Release binary (MB) | 9.4 | 32.6 | 0.288 | ≤ 0.25 × Loco | **BROKEN** |
| Clean release build (s) | 20.4 | 95.1 | 0.215 | ≤ 0.25 × Loco | ok |
| Release rebuild after touching a handler (s) | 3.1 | 14.6 | 0.212 | ≤ 0.20 × Loco | **BROKEN** |
| JSON order list: requests/s | 24313 | 6026 | 4.035 | ≥ 3 × Loco | ok |
| HTML order page: requests/s | 23804 | 5747 | 4.142 | ≥ 3 × Loco | ok |
| JSON order list: p99 (ms) | 5.10 | 15.65 | 0.326 | ≤ 0.50 × Loco | ok |
| HTML order page: p99 (ms) | 5.18 | 16.63 | 0.311 | ≤ 0.50 × Loco | ok |
| RSS after the runs (MB) | 19 | 34 | 0.559 | ≤ 0.60 × Loco | ok |
| Start to first successful request (ms) | 49 | 93 | 0.527 | ≤ 0.80 × Loco | ok |
| Place order (form post): requests/s | 9994 | 4280 | 2.335 | ≥ 1 × Loco | ok |
| Audit rows written after the order run | 2152 | 613 | 3.511 | ≥ 1 × Loco | ok |

## Problems in this run

None.

## Files counted

### framework

| File | Kind | Group | Lines |
|---|---|---|---:|
| `.env.example` | config | shop | 2 |
| `app/Cargo.toml` | config | shop | 23 |
| `app/build.rs` | rust | shop | 3 |
| `app/src/lib.rs` | rust | shop | 7 |
| `app/src/main.rs` | rust | shop | 3 |
| `app/tests/accounts.rs` | tests | shop | 110 |
| `app/tests/binary.rs` | tests | shop | 26 |
| `app/tests/messages.rs` | tests | shop | 335 |
| `app/tests/orders.rs` | tests | shop | 452 |
| `app/tests/scaffolded/main.rs` | tests | shop | 2 |
| `app/tests/scaffolded/order_20261011064009.rs` | tests | shop | 150 |
| `app/tests/scaffolded/order_20261012072415.rs` | tests | shop | 9 |
| `app/tests/support/mod.rs` | tests | shop | 25 |
| `app/tests/telemetry.rs` | tests | shop | 180 |
| `modules/order/Cargo.toml` | config | shop | 13 |
| `modules/order/bench/place_and_cancel.ts` | other | shop | 14 |
| `modules/order/contract/Cargo.toml` | config | shop | 12 |
| `modules/order/contract/src/area.rs` | rust | shop | 12 |
| `modules/order/contract/src/events.rs` | rust | shop | 11 |
| `modules/order/contract/src/lib.rs` | rust | shop | 6 |
| `modules/order/migrations/0000_module_role.sql` | migrations | shop | 1 |
| `modules/order/migrations/0001_create_order.sql` | migrations | shop | 8 |
| `modules/order/migrations/0002_messages_demo.sql` | migrations | shop | 11 |
| `modules/order/migrations/0003_order_account.sql` | migrations | shop | 4 |
| `modules/order/migrations/0004_timestamps.sql` | migrations | shop | 12 |
| `modules/order/migrations/0005_stored_names.sql` | migrations | shop | 4 |
| `modules/order/migrations/migration-allow.toml` | config | shop | 6 |
| `modules/order/seeds/customer.place_order.json` | other | shop | 4 |
| `modules/order/src/actions.rs` | rust | shop | 26 |
| `modules/order/src/areas/customer/routes.rs` | rust | shop | 12 |
| `modules/order/src/areas/customer/screens/orders.html` | templates | shop | 11 |
| `modules/order/src/audit/factory.rs` | rust | shop | 11 |
| `modules/order/src/audit/mod.rs` | rust | shop | 15 |
| `modules/order/src/jobs.rs` | rust | shop | 17 |
| `modules/order/src/lib.rs` | rust | shop | 15 |
| `modules/order/src/listeners.rs` | rust | shop | 15 |
| `modules/order/src/module.rs` | rust | shop | 8 |
| `modules/order/src/order/factory.rs` | rust | shop | 21 |
| `modules/order/src/order/mod.rs` | rust | shop | 46 |
| `modules/order/src/queries.rs` | rust | shop | 12 |

### Loco

| File | Kind | Group | Lines |
|---|---|---|---:|
| `.cargo/config.toml` | config | shop | 5 |
| `.rustfmt.toml` | config | shop | 2 |
| `Cargo.toml` | config | shop | 44 |
| `assets/static/404.html` | templates | shop | 3 |
| `assets/views/layout.html` | templates | shop | 10 |
| `assets/views/orders/index.html` | templates | shop | 32 |
| `assets/views/sessions/sign_in.html` | templates | auth | 12 |
| `assets/views/sessions/sign_up.html` | templates | auth | 13 |
| `config/development.yaml` | config | shop | 54 |
| `config/production.yaml` | config | shop | 57 |
| `config/test.yaml` | config | shop | 48 |
| `migration/Cargo.toml` | config | shop | 15 |
| `migration/src/lib.rs` | migrations | shop | 17 |
| `migration/src/m20220101_000001_users.rs` | migrations | auth | 38 |
| `migration/src/m20261010_200403_orders.rs` | migrations | shop | 36 |
| `migration/src/m20261010_200519_audit_entries.rs` | migrations | shop | 23 |
| `src/app.rs` | rust | shop | 69 |
| `src/bin/main.rs` | rust | shop | 7 |
| `src/controllers/auth.rs` | rust | auth | 197 |
| `src/controllers/mod.rs` | rust | shop | 3 |
| `src/controllers/orders.rs` | rust | shop | 149 |
| `src/controllers/sessions.rs` | rust | auth | 87 |
| `src/fixtures/users.yaml` | tests | shop | 17 |
| `src/initializers/mod.rs` | rust | shop | 1 |
| `src/initializers/view_engine.rs` | rust | shop | 38 |
| `src/lib.rs` | rust | shop | 8 |
| `src/mailers/auth.rs` | rust | auth | 65 |
| `src/mailers/auth/forgot/html.t` | templates | auth | 9 |
| `src/mailers/auth/forgot/subject.t` | templates | auth | 1 |
| `src/mailers/auth/forgot/text.t` | templates | auth | 2 |
| `src/mailers/auth/magic_link/html.t` | templates | auth | 8 |
| `src/mailers/auth/magic_link/subject.t` | templates | auth | 1 |
| `src/mailers/auth/magic_link/text.t` | templates | auth | 2 |
| `src/mailers/auth/welcome/html.t` | templates | auth | 11 |
| `src/mailers/auth/welcome/subject.t` | templates | auth | 1 |
| `src/mailers/auth/welcome/text.t` | templates | auth | 3 |
| `src/mailers/mod.rs` | rust | auth | 1 |
| `src/models/audit_entries.rs` | rust | shop | 29 |
| `src/models/mod.rs` | rust | shop | 4 |
| `src/models/orders.rs` | rust | shop | 204 |
| `src/models/users.rs` | rust | auth | 251 |
| `src/tasks/expire_orders.rs` | rust | shop | 19 |
| `src/tasks/mod.rs` | rust | shop | 3 |
| `src/tasks/user_create.rs` | rust | auth | 84 |
| `src/tasks/user_delete.rs` | rust | auth | 67 |
| `src/views/auth.rs` | rust | auth | 36 |
| `src/views/mod.rs` | rust | shop | 2 |
| `src/views/orders.rs` | rust | shop | 38 |
| `src/workers/audit.rs` | rust | shop | 24 |
| `src/workers/mod.rs` | rust | shop | 1 |
| `tests/mod.rs` | tests | shop | 5 |
| `tests/models/audit_entries.rs` | tests | shop | 22 |
| `tests/models/mod.rs` | tests | shop | 3 |
| `tests/models/orders.rs` | tests | shop | 22 |
| `tests/models/snapshots/can_create_with_password@users.snap` | tests | shop | 8 |
| `tests/models/snapshots/can_find_by_email@users-2.snap` | tests | shop | 7 |
| `tests/models/snapshots/can_find_by_email@users.snap` | tests | shop | 10 |
| `tests/models/snapshots/can_find_by_pid@users-2.snap` | tests | shop | 7 |
| `tests/models/snapshots/can_find_by_pid@users.snap` | tests | shop | 10 |
| `tests/models/snapshots/can_validate_model@users.snap` | tests | shop | 9 |
| `tests/models/snapshots/handle_create_with_password_with_duplicate@users.snap` | tests | shop | 7 |
| `tests/models/users.rs` | tests | shop | 296 |
| `tests/requests/auth.rs` | tests | shop | 402 |
| `tests/requests/mod.rs` | tests | shop | 4 |
| `tests/requests/orders.rs` | tests | shop | 48 |
| `tests/requests/prepare_data.rs` | tests | shop | 45 |
| `tests/requests/sessions.rs` | tests | shop | 30 |
| `tests/requests/snapshots/can_auth_with_magic_link@auth_request.snap` | tests | shop | 5 |
| `tests/requests/snapshots/can_get_current_user@auth_request.snap` | tests | shop | 8 |
| `tests/requests/snapshots/can_login_without_verify@auth_request.snap` | tests | shop | 5 |
| `tests/requests/snapshots/can_register@auth_request.snap` | tests | shop | 8 |
| `tests/requests/snapshots/can_reset_password@auth_request.snap` | tests | shop | 5 |
| `tests/requests/snapshots/login_with_invalid_password@auth_request.snap` | tests | shop | 8 |
| `tests/requests/snapshots/login_with_valid_password@auth_request.snap` | tests | shop | 8 |
| `tests/requests/snapshots/resend_verification_user@auth_request.snap` | tests | shop | 8 |
| `tests/tasks/expire_orders.rs` | tests | shop | 16 |
| `tests/tasks/mod.rs` | tests | shop | 3 |
| `tests/tasks/user_create.rs` | tests | shop | 47 |
| `tests/tasks/user_delete.rs` | tests | shop | 29 |
| `tests/views/mod.rs` | tests | shop | 1 |
| `tests/views/render.rs` | tests | shop | 12 |
| `tests/workers/audit.rs` | tests | shop | 38 |
| `tests/workers/mod.rs` | tests | shop | 1 |

## Where the comparison is uneven

See [README.md](README.md#where-the-comparison-is-uneven).
