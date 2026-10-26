# The Smallest Core

> Status: **proof of concept**. How the framework builds what
> [structure.md](structure.md) describes with the least code.

## The rule

**Generate data, never behaviour.** Behaviour — loading, saving, querying,
routing, rendering, tracing — is written once, as generic library code. What
is generated only describes a type to that library: its table, its columns,
its fields, its name. When Rust's type system or module privacy can enforce a
rule, nothing is generated for it.

Three mechanisms, each used only where the others cannot reach:

| Mechanism | Used for | Why this one |
|---|---|---|
| **Generic library code** | All behaviour | Written and tested once; every app shares it |
| **Derives** (`#[derive(Aggregate)]`, `Value`, `Enum`) | Describing one type to the library: columns, encode and decode, field metadata | Local to the type, expanded where it is declared, errors point at the user's own line, no file to keep in sync. Small: they emit trait implementations, never logic |
| **`sofie gen`** | What no single type can know: the app's wiring across crates, and `.env.example`, and `docs/gen/` once an app keeps it; docs, OpenAPI and TypeScript are `sofie docs`, `sofie openapi` and `sofie typescript`, sofie-cli commands behind its features | Cross-crate collection and non-Rust output are impossible for a derive |

`sofie gen` therefore writes Rust in two places only — the app's wiring and what
each contract exports — and no Rust per type. Everything else it writes is not
Rust.

## The database is the only infrastructure

The core needs one database and nothing else. Postgres is the main one, and
SQLite (`sofie-sqlite`, sofie's `sqlite` feature) serves an app running as one
instance; every database-specific feature below sits behind the
[database seam](#the-database-seam), so another database is an added
implementation, not a rewrite.

| Need | How |
|---|---|
| Data | Each module's tables, in its own schema (a Postgres schema; on SQLite, tables named after the module) |
| Events, jobs, schedules | One `sofie.messages` table: once a worker is published, a commit writes, in one statement, one job per listener its process heard for each event it raises (unique on event and listener, each due when its event was), and nothing for an event no listener hears; an event committed before then is queued itself and fanned out to the process's listeners by the first worker hearing it, running inline as their lone listener only when the process has one and it is that worker's, its outcome and the event's deletion committed together, becoming the listener's job only if it fails; a job runs once per claim. Claimed with `FOR UPDATE SKIP LOCKED` under a lease whose attempt count fences late finishes, a fixed number in flight (`SOFIE_WORKER_BATCH`, 2), all polled on the worker's task, another claimed as soon as one finishes; a job's outcome and the deletion of its row commit together, so its effect happens once. Leases run on the database's clock and are renewed every third of the lease while the job runs; a job runs at most four minutes, below the five-minute lease, and a panicking job fails like any other. A fault, conflict or unavailability is retried with backoff; `NotFound`, `Moved`, `BadRequest`, `Invalid` and `Rule` are dead at once. A message that runs out of attempts stays, dead, out of claiming, and one whose worker died during its last attempt is claimed once more only to be buried. Schedules run at most once a second and at least once in 1000 years. Woken by `LISTEN/NOTIFY`, with a poll fallback whenever listening fails. One worker loop, in `sofie-db`, serves all three |
| Single-runner schedules | `sofie.schedules`: firing a due schedule and enqueuing its job is one atomic statement, so one instance runs it; no lock is held. The statement answers the schedule's next time, and a worker does not ask again before then |
| Cache (`cache`, opt-in) | sofie-cache: in process by default; `sofie.cache`, an unlogged table, with `cache-database`; Redis with `cache-redis`. Entries depending on an aggregate are dropped after each commit writing it; other instances' memory copies are dropped through `pg_notify('sofie_cache', …)`, one per 5 ms burst of commits with their drops merged, or Redis pub/sub |
| Live screens across instances (`live`) | `pg_notify('sofie_live', …)` after each commit naming the changed tables and owners; each instance listens and tells its open screens' streams |
| Sessions | Signed, encrypted cookies naming a session id. With sofie-identity (its `sessions` is on by default) each sign-in is an `account_session` row checked on every request, so signing out, "sign out everywhere else", a password reset, closing the account, ending an impersonation and removing a membership end the session for every copy of its cookie. The check is served from an in-process cache per app instance (100 000 entries, a minute each) that each commit to `account_session` invalidates by account, here at once and on other instances through `pg_notify('sofie_live', …)`; losing that listener empties it until listening holds again. An hourly job deletes rows unseen for longer than a cookie can live (`SessionClaims::ABSOLUTE_LIFETIME`, public so stores can drop records no cookie can outlive, plus an hour). Turned off, sessions live only in their cookies. They also travel between split apps unchanged |
| Feature flags, settings | Two small tables, cached in process, invalidated like the cache |

A worker publishes what it hears once built (`worker::publish_heard`,
called by `run`, `drain`, the web role and `task`), merged into its
process's heard set. From then on a commit on that database writes no
event row: for the events it raises it inserts one job per listener of the
heard set (a pruned record's event only to the listeners taking it) in one
statement built from JSON (`ON CONFLICT (event_id, name) DO NOTHING`), and
nothing when no listener hears them. Events pulled from another app's feed
fan out the same way in the commit receiving them, keyed by the producer's
entry id, so an entry received again adds no job; pruning raises only the
events a heard listener takes. Every committing process must therefore
publish the app's full listener set (a web process builds its worker only
for this): a listener missing from a process's set gets no job from that
process's commits. An event so reaches the listeners of the version that
raised it: during a rolling deploy each listener's job waits for a worker
that knows it, and two workers of one process each run their own. Before
any worker is published, commits queue the event itself, which the worker
claiming it fans out to the process's heard set, running it inline only
when that set has one listener for it and the listener is the worker's
own. A worker claims only the events and jobs it handles itself; any other
message waits an hour (`UNHEARD_KEPT_FOR`), whether another worker of the
process, a newer version or no worker handles it; then an event is dropped
with a warning naming it and its message id, and a job is buried at once.
Each message keeps the actor of the commit that queued it (`actor`, null for a
schedule, a feed entry or a commit acting for no principal); a listener's job
takes its event's, in the same single fan-out statement. The worker runs a
message with an actor as `Reach::AllAs(actor)`, so its writes, history and
the messages it queues name that principal, and one without as `Reach::All`;
a job committing through `Db::commit` or `commit_with` keeps the reach it
names. `<app> jobs dead` lists the 100 most recently dead messages (id,
time, name, attempts, the error's first line); `<app> jobs replay ID` makes
one claimable at once with its attempts reset, in one statement that refuses
a message that is not dead, and logs the replay at info with its id, name and
previous attempts as its audit.

No Redis, no broker, no search engine in the core. Each can be added later as
a plugin behind a gateway.

## Crates

Split by what a binary must pull in, not by how many concepts there are. A
crate exists when some app or some binary can do without it; otherwise it is
a module inside another crate.

```
sofie-model                             the app model and naming rules; no runtime dependencies
sofie-macros                            derives; emits `::sofie::…` paths, depends on no sofie crate
sofie-core                              value model, errors, ids, clock, encodings, registration rules
  ├── sofie-values                      shipped values
  ├── sofie-auth                        signing and sealing: session cookies, signed links, tokens (ring)
  ├── sofie-telemetry                   tracing setup; OTLP export behind a feature
  ├── sofie-engine                      templates, catalog, htmx, island mounting (uses sofie-values)
  ├── sofie-storage                     opt-in: files under generated keys, local or S3 (sofie-auth, sofie-client optional)
  └── sofie-db                          handler model, ORM, unit of work, messages, the seam's traits (uses sofie-auth, -values, -telemetry)
        ├── sofie-postgres              the seam's Postgres implementation
        ├── sofie-sqlite                the seam's SQLite implementation, built into the binary
        ├── sofie-client                the only way out: outgoing HTTP for gateways and contracts
        ├── sofie-flags                 flags and settings
        ├── sofie-cache                 opt-in: cache over memory, the database or Redis
        ├── sofie-mail                  opt-in: mail rendered by sofie-engine, sent by a job
        │                               (sofie-http optional, for unsubscribes and provider webhooks)
        └── sofie-http                  server on hyper, router, areas, guards, sessions, security
              │                         (sofie-engine, -flags, -storage, -values optional)
              ├── sofie-billing         opt-in: plans, entitlements, quotas, subscriptions (uses sofie-client)
              └── sofie-notify          opt-in: inbox, mail and web push per principal's choices (uses sofie-mail)
sofie                                   facade: re-exports, features choose the crates
  ├── sofie-identity                    opt-in module: accounts, memberships, passwords, sign-up, in and out
  ├── sofie-content                     opt-in module: markdown pages, slugs, publishing, link previews, sitemap
  │     └── sofie-tenancy               opt-in module: organizations owning records, members, roles, invitations
  └── sofie-testing                     the test harness; sofie-identity's `testing` feature builds on it
sofie-check ── sofie-core, sofie-engine, sofie-model   the rules `sofie check` and an app's build script run
sofie-cli                               sofie new, gen, check, fix, docs, openapi, typescript, explain, build
```

| Crate | Holds | Who can leave it out |
|---|---|---|
| `sofie-model` | The app model and the naming rules, parsed with `syn`. Shared by macros and CLI, so a name is derived by one function | Nobody at build time; nothing of it is in the binary |
| `sofie-macros` | `Aggregate`, `Value`, `Enum`, `ReadModel`, `Input` derives; the inert attributes | — |
| `sofie-core` | What crates below sofie-db share: the value model (`Cast`, `SqlValue`, `Show`/`Shown`, `Value`, `Variants` and their state moves, `Constraint`), `Error`, `Uuid`, `Instant`, `Clock`, the field-name rule forms and templates share, the `plural` a resource is mounted and generated under, `registrations::validate` (the route and job rules boot and `sofie check` share), `encoding`: the base64, hex and percent-encoding every crate shares, `sql_lexer`: the one Postgres SQL lexer (`lex` → `Lexed { tokens, lines, comments }`; `--` and nested `/* */` comments, `''`/`E''` strings, `"quoted"` names and `$tag$` bodies, in which a `;` ends no statement), with which sofie-postgres finds the `CONCURRENTLY` index builds that run outside a transaction and sofie-check reads migrations for `migration-lock`, the constraints and `cross-module-sql`, and `unpoisoned`: the locks every crate takes past a poisoning panic | — |
| `sofie-values` | `Money`, `Percentage`, `Email`, `PhoneNumber`, `Instant`, `Date`, `LocalDateTime` and `TimeZone`, `TimeOfDay`, `Weekday`, `Coordinate`, `Measurement` (`Weight`, `Energy`, `Length`), `Slug`, `Url`, `Locale`, `PersonName`, `Secret`; all but `Money`, `Currency`, `Email` and `Secret` behind the features `percentage`, `time`, `phone`, `person`, `geo`, `measure`, `web`. The every-surface traits: `Present` (form control, display component, locale format, sort key, CSV) with `FormField`/`FormFields` for forms; `Filter` and `LogFields` behind `filter` and `log-fields`, forwarded by the `sofie` facade as are sofie-engine's `layout` | An app that ships its own values only |
| `sofie-db` | The handler model (`Input` and `InputFields`, `Event`, `Reply`, `Create`/`Update`, `Resolve`, `Context`, `Call`), store, unit of work, query builder, read models, migrations, the message table and its worker, the seam traits, and `File` with `embed!`, how every crate and the wiring carry a file | — |
| `sofie-postgres` | `Driver`, `Dialect`, `Catalog` for Postgres, with the framework's own migrations and message SQL | An app on another database, once one exists |
| `sofie-auth` | Signing and sealing: `SigningKeys` sealing session cookies, signed links, bearer token digests, principal tokens between split apps. Used by sofie-db (jobs sign links), sofie-http, sofie-storage and remote contracts | — |
| `sofie-http` | The server on `hyper`, the router, areas, guards, signed-cookie sessions, CSRF, security headers, limits, the three answers of an endpoint, and `Module`, what one module registers | A worker-only binary |
| `sofie-client` | Outgoing HTTP: the client every gateway adapter and remote contract is given, with tracing, limits and egress rules | An app that calls nothing outside itself |
| `sofie-engine` | Template renderer and the loader of compiled templates, the parser behind `parse`, the component catalog, the embedded htmx and client, island mounting | A JSON-only API: `sofie-http`'s `engine` feature (on by default, and turned on by `sofie`'s) brings in screens, htmx notices and form bodies |
| `sofie-telemetry` | The `tracing` setup, log masking, metrics; OTLP export behind a feature | Nobody, but the exporter is optional |
| `sofie-flags` | Feature flags: `Feature` and `Variants` constants, rules in `sofie.flags`, held in process and refreshed on the messages channel's wake-ups (or every 30 s), `Features` per request, `gated` jobs; runtime settings in `sofie.settings`, held in the same snapshot and read through `sofie::RuntimeSettings` | An app without flags: it is behind `sofie`'s `feature-flags` |
| `sofie-cache` | `Cache` with typed `Key`s, explicit expiry, single-flight `get_or_compute` and drops by aggregate change, over a bounded memory tier, `sofie.cache` (feature `database`) or Redis through its own RESP client (feature `redis`) | Every app that caches nothing: it is behind `sofie`'s `cache`, `cache-database`, `cache-redis` |
| `sofie-storage` | `Storage` over a `Store`: files under generated `StorageKey`s, streamed in and out, signed download links; a local directory (feature `local`) or an S3-compatible bucket signed with SigV4 through sofie-client (feature `s3`) | Every app that keeps no files: it is behind `sofie`'s `storage`, `storage-s3`, `uploads` |
| `sofie-sqlite` | The database seam for SQLite: one file, statements on one connection, `BEGIN IMMEDIATE` transactions, polling instead of `LISTEN` | Every app on Postgres: it is behind `sofie`'s `sqlite` |
| `sofie-mail` | Mail rendered from templates and sent by a job over SMTP, to the log or a test's outbox | Every app that sends no mail: it is behind `sofie`'s `mail` |
| `sofie` | Re-exports everything an app uses, with features: `default = ["engine", "postgres", "postgres-tls", "trusted-proxies"]` | — |
| `sofie-identity` | A module on the facade: accounts, memberships per area, `Password` and `PasswordHash` (Argon2id in the PHC string format; older PBKDF2 hashes still verify and are rehashed at sign-in), sign-up, in and out; it checks membership itself before signing in, and creates it in the sign-up commit. `remove_membership` removes one and ends the account's sessions in that area in the same commit. Signing up answers alike for a new and a taken address and signs nobody in. Its `testing` feature adds `TestMember` (`TestPrincipal::<A>::member(&app)`) and `signed_up`, which sign up and then in | An app with its own identity |
| `sofie-tenancy` | A module on the facade and sofie-identity: organizations as owners (an organization area is an `OrganizationArea`, whose principal is `Organization` and which names its `HOME`), members with an owner, admin or member role, invitations mailed as signed links, and with `billing` members held to the plan's seats | Every app without organizations |
| `sofie-content` | A module on the facade: markdown pages with slugs (previous ones answered with a 301), publishing, search, link-preview tags and a sitemap | Every app without written pages |
| `sofie-cli` | The `sofie` command | Not in any binary |

An app depends on `sofie` alone and turns features off rather than wiring crates
by hand. The same app can build a web binary and a worker binary from one
codebase by features.

What not every app needs is a feature of its crate, forwarded by `sofie` and off
by default, so the framework compiled into an app stays small (its lines are
measured against Loco's by `bench/run.sh` and reported, though held to no limit): `idempotency`
(`.idempotent()` actions), `webhooks` (`Routes::webhook`), `signed-links`
(`.signed()` routes and `SignedUrls`), `steps` (`App::step`,
`App::step_all`), `cors` (`App::cors`, `SOFIE_CORS_ORIGINS`), `tls`
(`Serving::tls`, HTTPS from `SOFIE_TLS_CERTIFICATE` and `SOFIE_TLS_KEY`),
`feature-flags` (sofie-flags as `sofie::flags`, `.feature(…)` routes, `Features`),
`permissions` (`Permitted` areas whose principal is sofie-identity's `Permitted`, roles
and grants),
`trusted-proxies` (on by default: `SOFIE_TRUSTED_PROXIES`, `SOFIE_REQUEST_ID_HEADER`; without it
the client is the connection's peer, and a set variable refuses to start), `shield`
(`SOFIE_SHIELD`, `SOFIE_SHIELD_THRESHOLDS`, `SOFIE_SHIELD_SHARED`; sofie-identity's
`shield` adds sign-in locks, its `permitted-shield` a screen), `chosen-screens`
(`Screen::render`), `pagination` (`Query::paginate`, `Page`, `Paginated`),
`streams` (`Query::stream`), `raw-sql` (`Read::sql`), the engine's `layout`
(`Grid`, `Card`, `Image`), `sofie-values`' `filter` and `log-fields` (with
`sofie::telemetry::info!(order.total, "…")`), `contracts` (`Dep`, `Extensions`,
`#[sofie::contract]`, batch lookups, change events, mirrors), `jobs` (`#[job]`
actions queued through `Jobs`), `storage` and `storage-s3` (sofie-storage as
`sofie::storage`, `Serving::storage`, `SOFIE_STORAGE*`), `uploads` (`Upload`
input fields from `multipart/form-data`, `.file_limit(…)`), `image-variants`
(`Storage::make_variants` with the app's `Resizer`, `Dep<Storage>` in jobs),
`search` (`Searched`, `Aggregate::matching`), `search-meilisearch` (searched
records kept in Meilisearch with `SOFIE_SEARCH=meilisearch`, `<app> task
sofie.search.reindex`), `csv` (`.document(Media::Csv)`,
`CsvRows`), `retention` (`Prune` in jobs), `content` (the engine's `<Markdown>`
and `<Head>`), `tasks` (`#[task]`, `Tasks`,
`run_with_tasks`: `<app> task NAME [--name value]...`), `capture`
(`sofie::telemetry::capture`, for tests), and the telemetry features
`metrics` (`/metrics` with `SOFIE_METRICS_TOKEN`, OTLP with `otlp`),
`product-events` (`product_event!`), `product-reports` (events kept in
`sofie.product_events`, `<app> report retention`), `browser-telemetry`
(`SOFIE_BROWSER_SAMPLE`), `development` (span trees printed by debug builds),
`toolbar` (debug builds only: span tree, acting as a principal, flag
overrides and a shifted clock per browser) and `spans` (`#[span]`). Two more serve tools, not apps: the engine's
`parse` (template text: `Template::parse` and `compile_template`, for
build scripts, `sofie check` and `development`), `compile` (`parse` with every
syntax feature, what a build script compiles screens with), `analysis`
(template bindings and element ids, for `sofie check`) and
`sofie-core`'s `testing` (`TestClock`, turned on by `sofie-testing`).

**Kept small on purpose for the first version:**

- Values: the ones above, each one small file checked for shape only (no
  locale or numbering-plan tables).
- Behaviors: `Versioned`, `Timestamps`, `Owned` always; the others, child
  rows and read-model joins behind cargo features of `sofie` (`soft-delete`,
  `flags`, `codes`, `history`, `publishing`, `children`, `joins`).
- One database: Postgres.

**Written by us, because a dependency cannot do it:**

- The template parser. `sofie gen` must type-check every binding statically and
  the engine must render subtrees for fragments; existing template engines
  either compile through macros (no reload without recompiling) or do not
  expose a stable syntax tree to check. The syntax is deliberately small:
  components, `{expr}`, `{#if}`, `{#each}`.

Plugins (search, files, mobile, MCP, k6) are crates of their own, outside
the core; docs with diagrams, OpenAPI and TypeScript for API clients are
sofie-cli commands behind its features, reading the app model and never part
of an app's runtime. The Svelte island
toolchain is part of the CLI but runs only for apps that have islands.

## The web server

The binary is the server: no PHP-FPM, Octane or separate process in front of
the app's code. The choice is made for speed and security first; the cost of
building on a lower layer is the framework's, not the app's.

### `hyper`, directly

| | Why it was or was not chosen |
|---|---|
| **`hyper` 1.x** | Chosen. The most widely deployed HTTP implementation in Rust, under `reqwest`, `axum`, `tonic` and much of the ecosystem's production traffic; strict parsing that rejects ambiguous requests; an HTTP/2 stack (`h2`) hardened against the attacks that have hit HTTP/2 servers, such as Rapid Reset |
| `axum` | Not needed. Its value is its extractors, and the framework resolves arguments with `Resolve`; what remains is a router and `tower` glue, which the framework can own without boxing every service |
| `actix-web` | Fast, but its own HTTP implementation and runtime model carry less scrutiny than `hyper`'s, and the gain over `hyper` disappears behind a database call |
| `may-minihttp`, `xitca-web` and other benchmark leaders | Their speed comes partly from doing less of HTTP; a framework promising safety cannot inherit that trade |

On `hyper` the framework owns the whole request path:

- **Router:** `matchit`, the radix tree `axum` itself uses, built once at boot.
- **Pipeline:** the framework's own fixed sequence of steps — limits, request
  id, trace, area guard, CSRF, flags, handler, negotiation, compression —
  composed statically, with no boxed middleware chain. Apps add their own
  steps per area or app-wide through one trait, `PipelineStep`, registered
  with `app.step::<A>(…)` or `app.step_all(…)`: lists fixed at boot, a
  dynamic call per step for the request (`check`, which can only refuse) and
  one for its answer (`answered`, which reads the status and duration and
  sets headers other than the cookie, security and CORS ones).
- **Allocator:** `mimalloc` as the global allocator, behind `sofie`'s
  `mimalloc` feature, off by default: measured, it serves HTML pages 16%
  faster but doubles the process's RSS, past the limit against Loco.

### Secure by default

Nothing below needs configuring; an app loosens a default only explicitly,
per area, and `sofie explain` shows it.

| Concern | Default |
|---|---|
| Slow clients | Header read timeout (10 s); request timeout (30 s), stopped from the first file part of a `multipart/form-data` body sent to a route that takes files, and started again in full once the body is read, which takes at most 10 minutes (else 408); a connection that sends nothing for 10 s after connecting, or moves no byte either way for 60 s later, is closed; at most 4096 connections at once (`Serving::max_connections`), more wait in the backlog, and 64 per client address (IPv6 by /64; not for loopback or a trusted proxy), more answered 503 with `Connection: close` and cut after 10 s, outside the 4096 and at most 64 such answers at once (past that the connection is dropped unanswered), counted in a warning at most once a minute, the first such one with no trusted proxy set also logging a warning that names `SOFIE_TRUSTED_PROXIES`; on shutdown, open requests get header read plus request timeout plus 5 s |
| Oversized input | Headers within 32 KiB (431 past it, so a connection's read buffer stays small) and a count limit; body limit per route, 64 KiB unless the route raises it with `.body_limit(…)`; past it the 413 is sent after up to 4 MiB (2 s) of the rest is read and dropped, so closing the connection does not reset it before the client reads it |
| HTTP/2 abuse | Bounded concurrent streams (32); a connection is closed after 20 pending client resets (Rapid Reset) or 256 resets for protocol errors, through `hyper`'s settings |
| Request smuggling | Ambiguous `Content-Length` and `Transfer-Encoding` refused by `hyper`; paths normalized once, before routing |
| Response headers | `Content-Security-Policy` with per-response nonces (htmx and islands load without `unsafe-inline` or `unsafe-eval`), widened per area only by HTTPS hosts the app names (`Serving::content_security`); every other answer but a 304 (JSON, errors, health) gets `default-src 'none'; frame-ancestors 'none'`, `Strict-Transport-Security: max-age=63072000; includeSubDomains` (browsers ignore it over plain HTTP), `X-Content-Type-Options`, `frame-ancestors 'none'`, `Referrer-Policy`, `Permissions-Policy` refusing camera, microphone, geolocation, sensors, USB, MIDI, screen capture and topics (payment stays allowed for a provider's embedded form); no server banner |
| Cookies | `__Host-` prefix, `Secure`, `HttpOnly`, `SameSite=Lax`; session cookies signed and encrypted. Named after the app's package, `__Host-<app>-session` (and `__Host-<app>-toolbar`), because cookies do not separate by port: two apps on one host keep their own sessions. The wiring passes `env!("CARGO_PKG_NAME")` to `Serving::named`; a name that is not cookie-token characters refuses to serve, and an app built without the wiring (tests) is `sofie`. A handler takes that name as an `AppName` argument (`sofie::http::AppName`), for where the app names itself elsewhere, such as an authenticator's issuer |
| CSRF | Every unsafe method outside webhook routes and bearer-token requests must come from the app's own pages: `Sec-Fetch-Site: same-origin`, or, from a browser too old to send it, an `Origin` naming the host; a request carrying the session cookie with neither is refused. With the `cors` feature, a write from an origin the area's CORS policy lists also passes, carrying the session cookie only when that policy allows credentials. There is no form token: the browser's fetch metadata already proves the request is same-origin, which a token only approximates, as Go's `CrossOriginProtection` reasons |
| API tokens | With the `tokens` feature, an area that opts in (`const TOKENS: bool = true` on its `Area`) accepts `Authorization: Bearer`: 256 random bits, shown once, stored as a SHA-256 digest, expiring, revocable and narrowed by permission scopes; never a cookie or a session; 401 with `WWW-Authenticate: Bearer` when unknown, expired or revoked, 403 outside its scopes, 429 after 10 failures a minute per address |
| Rate limits | Per area (`app.throttle::<A>(…)`) and per route (`.throttle(…)`), by principal or client address, counted in-process, so per instance; none unless asked for, except identity's sign-in at 10 a minute |
| Attacks | With the `shield` feature: behaviour counted per caller on a thread of its own (scanning, id probing, refused, malformed and throttled requests, failed connections, failed sign-ins per caller and per account, sign-up bursts), answered by reporting, slowing down, blocking with 429 and `Retry-After`, or locking an account's sign-in and telling its owner; every block expires; never a trusted proxy or an unknown address; `SOFIE_SHIELD=dry-run` by default in debug builds; shared across instances with `SOFIE_SHIELD_SHARED=on`. No payload signatures, no volumetric defence (the edge's) |
| Client address | The connection's peer; with the `trusted-proxies` feature, taken from `X-Forwarded-For` only when the request came from a trusted proxy named in `SOFIE_TRUSTED_PROXIES` (addresses and CIDR ranges); read from the right, the first hop no trusted proxy holds is the client. Such a proxy's request id, from the header `SOFIE_REQUEST_ID_HEADER` names (`x-request-id` by default), is recorded as `edge_request_id` beside the server's own id when it is 1 to 128 plain characters (letters, digits, `-_.:;=/+@`); the server's id stays the request id, so no caller chooses it |
| Errors | No internals in any response; details go to telemetry only |
| Code | `#![forbid(unsafe_code)]` in every framework crate; the framework's CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, clippy, the tests against Postgres and `sofie check` on the examples; no `cargo audit` or `cargo deny` yet |

### TLS and protocols

- HTTP/1.1 and HTTP/2 from `hyper`.
- TLS with `rustls`, a memory-safe implementation, behind sofie's `tls`
  (`SOFIE_TLS_CERTIFICATE`, `SOFIE_TLS_KEY`); terminating at a load balancer
  or proxy instead remains a configuration choice. Automatic certificates
  (ACME) can follow.
- HTTP/3 is left out until `hyper`'s support is stable.

### Static files, roles, shutdown

- Island bundles, the engine's client and images are embedded at build, named
  by content hash and served with immutable caching, precompressed.
- `serve --role web | worker | all` chooses what a process does (no
  arguments: `all`); one binary covers a single process and a scaled
  deployment. Every role builds the app, so config fails at boot everywhere.
  In `all`, the web server or the worker ending, at boot or later, stops the
  other and the process with a logged error.
  A worker-only binary (`src/bin/worker.rs` calling `sofie::run_worker`,
  declared behind the app's `worker-binary`
  feature and built by `sofie build --worker`) never builds the app, so the
  HTTP server and screens are left out of it; an app with feature flags
  calls `sofie::run_worker_with_flags(MODULES, worker)` instead, and `sofie
  build --worker` names whichever the app needs.
- On a stop signal the process stops accepting, finishes open requests and
  claimed messages, then exits.
- After a split, each app is the same kind of binary, and the reverse proxy
  table `sofie split` writes (`proxy.toml`) routes each path prefix to its app.

## Mechanisms

### Aggregates: privacy instead of tokens

Rust makes a struct's private fields visible only to the module that declares
it and that module's children. That is the whole enforcement:

```
src/order/
  mod.rs        the aggregate: fields, behaviour, the schema attributes
  factory.rs    a child module, so it alone may construct an Order
```

```rust
// src/order/mod.rs
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete))]
pub struct Order {
    meta: Meta,
    #[owner]
    account: AccountId,
    status: OrderStatus,
    total: Money,
}

impl Order {
    pub fn cancel(&mut self, reason: CancelReason) -> Result<()> {
        self.status = self.status.to(OrderStatus::Cancelled)?;
        self.meta.raise(OrderCancelled { order: self.id().uuid(), reason })
    }
}
```

- Nothing outside `src/order/` can build an `Order` or change a field; the
  compiler says so. No token type, no check.
- `meta: Meta` is the one explicit line an aggregate carries: its id, version,
  loaded snapshot and pending events. The derive refuses an aggregate without
  it.
- The derive implements `Aggregate`: table name, column list, encode, decode,
  field metadata for screens and validation. Every behaviour lives in `sofie-db`.

### Unit of work: return what changed

An action returns the aggregates it changed; there is no hidden tracking.

```rust
// src/actions/cancel_order.rs
pub async fn cancel_order(mut order: Order, reason: CancelReason) -> Result<Order> {
    order.cancel(reason)?;
    Ok(order)
}
```

After the handler returns, the framework opens one short transaction, compares
each returned aggregate with the snapshot in its `meta`, writes a changed one
whole with one fixed `UPDATE` per type and a version check, and fans its
pending events out to `sofie.messages`. A loaded aggregate that only raised events
still takes the next version, so two copies cannot both commit an event. No transaction is open while the body runs. The driver sends
the whole transaction as one pipeline: a commit of inserts is one round trip,
and one with a counted step (a version check) is two, since `COMMIT` waits for
the count. A `Result<()>`
writes nothing; a tuple or a `Vec` writes several.

Snapshot comparison costs one encode per aggregate at commit and removes every
line of per-field change tracking.

### The ORM: two sides, one description

The ORM splits along the line DDD already draws: **aggregates** for writing,
**read models** for reading. Each side is small because it does only one job.

#### What a derive emits

Only a description, as data:

```rust
pub trait Aggregate: Sized + Send + Sync + 'static {
    const NAME: &'static str;               // the table, in its module's schema
    const MODULE: &'static str;
    const FIELDS: &'static [Field];         // column(s), SQL type, nullability, constraints
    const OWNER: Option<&'static str> = None;
    const BEHAVIORS: &'static [&'static dyn Behavior] = &[];   // timestamps, soft delete…
    const UNIQUE: &'static [&'static str] = &[];
    const SENSITIVE: &'static [&'static str] = &[];
    fn meta(&self) -> &Meta;
    fn meta_mut(&mut self) -> &mut Meta;
    fn encode(&self, out: &mut Vec<SqlValue>);
    fn decode(meta: Meta, row: &mut dyn ColumnReader) -> Result<Self>;
}
```

plus one typed column constant per field — `Order::STATUS: Column<Order,
OrderStatus>` — which is a name and a type marker, nothing more. Load, save,
query, scope, check and render are generic code in `sofie-db` and `sofie-engine`
that read these descriptions.

#### Writing: aggregates are loaded whole

```rust
let order = Order::find(&read, id).await?;                     // children included
let mut stale = Order::query().scope(stale_pending).stream(&read)?;   // for jobs
while let Some(order) = stale.next().await? { … }
```

- An aggregate always loads with its child rows; there is no relation to load
  later, so lazy loading and N+1 do not exist on this side. A child row is a
  `#[derive(Child)]` struct with an `id: Id<Self>` field, held in a
  `Children<T>` field (`push`, `retain`, and the slice's methods); its table
  is in the aggregate's module, named after the child, with a column named
  after the aggregate holding the parent's id. The load selects each kind of
  child as one JSON column of the same statement (`Dialect::nested_rows`),
  so a load stays one round trip.
- A reference to another aggregate is its typed id. Loading it is another
  `find`, explicitly.
- Saving is the unit of work: the snapshot decides whether to write, child
  rows deleted, updated or inserted by their ids, one version check. A change
  to a child alone still counts the aggregate's version up, so the version
  guards the children too.
- A record owned through its parent (`#[owner(through = parent)]`, D11) is
  read through a subquery on the parent's table, and an owned area's commit
  of one checks it among the principal's records after writing it, failing
  as `NotFound` otherwise; that check costs the commit's second round trip.
  `refuse_parent_beyond_reach` answers `NotFound` for a list under a parent
  the read cannot reach.
- Behaviors' columns are set at the commit, on the database's clock (an
  `Instant`, the framework's own time value, like `Clock::now`):
  `version` counts up and guards the update, `created_at` and `updated_at`
  are stamped, and `deleted_at` once `delete()` was called. Nothing is
  written, and nothing stamped, for an aggregate that did not change.
- A field whose enum lists `#[to]` transitions may only have moved along a
  listed edge between the loaded and the saved state; any other save fails as
  `Invalid` on that field (key `validation.move` with `from` and `to`),
  whichever code set the field, as does an unlisted `to()`, on the enum's
  name in snake case. Moves made through `to()` are
  recorded for the action (the request's or the job's task), so two moves in
  one action pass as the chain of listed edges they took; a direct assignment
  still needs a direct edge. A value carries no record, so each recorded move
  explains one record's save: moves made on two records cannot explain a third
  record's jump.
- There is no bulk `UPDATE … WHERE`: it would bypass invariants and events. A
  change to many aggregates is a per-item action (`#[every(…, each = …)]`).

#### Reading: read models are any shape

Screens, APIs and reports read through read models — plain structs that state
the shape they need:

```rust
#[derive(ReadModel)]
#[read(from = Order)]
pub struct OrderRow {
    id: OrderId,
    status: OrderStatus,
    total: Money,
    #[read(via = Order::ACCOUNT, field = CustomerMirror::NAME)]
    customer_name: Option<String>,
}

let page = OrderRow::query()
    .scope(due)
    .where_(Order::TOTAL.gt(minimum))
    .order_by(Order::CREATED_AT.desc())
    .paginate(&read, page)
    .await?;
```

- The derive lists the columns and the joins; the builder writes one SQL
  statement. A joined field is a subquery in the select list reading the
  record `via` names (null when there is none, or, in an owned area, when it
  is another principal's), so conditions and orderings on the base stay
  unambiguous. The field is an `Option` of the joined column's type (or that
  column's own `Option`), since the join can always read null. Joins stay
  inside the module: another module's data is joined through its mirror,
  which is a local table kept from the source's change events (`#[mirror(…)]`,
  behind `contracts`; structure.md, "Messages and contracts").
- A read model selects only its own columns, so a screen never loads what it
  does not show.
- A field reading a sensitive one (`Aggregate::SENSITIVE`, by mark or by type,
  joined fields included) fails to compile unless the read model marks it
  `#[sensitive]` too, since what it selects reaches JSON and tables.
- This is where Eloquent's `with()` went: a read model states its related
  fields up front, so there is no eager-loading API and nothing to forget.
- A read model without `from` is computed: its loader fills the fields from
  what it read, for counts, sums and groupings a screen shows. It has no
  `query()`, joins or route binding; it answers as JSON and to a screen like
  any read model, and `sofie gen` checks bindings through the read models its
  fields hold:

  ```rust
  #[derive(ReadModel)]
  pub struct OrderSummary { by_status: Vec<StatusCount>, total: Money }
  ```

#### The query builder

One generic `Query<T>` for both sides:

| | |
|---|---|
| Conditions | `where_` and `Condition::or`, with `eq`, `ne`, `gt`, `gte`, `lt`, `lte`, `between` (on a nullable column too, taking the bare value; a null row never matches an ordering comparison), `in_` (a multi-column value compares as a row; a single column binds the list as one array, `= ANY($1)`, where the dialect can, so one prepared statement serves every length; an empty list matches nothing), `sofie_db::in_select(column, selected, condition)` (the column among a one-column field of another aggregate's rows matching a condition, as `IN (SELECT …)` in the same query), `is_null`, `is_not_null`, and `like` on text columns only; a value of the wrong type, or `like` on a column that is not text, does not compile |
| Order and limits | `order_by`, `latest`, `limit` |
| Results | `find`, `first`, `all`, `count`, `exists` (reads at most one row), `paginate(read, page)` → `Paginated<T>` (offset, for screens; reads one record past the page to tell whether another follows, so no count runs; a `Page` holds 1 to 100 records and starts within the first 10 000, else a 400 asking to narrow the filter), `stream(read)` (keyset by id in batches of 500, one short query each; refuses a query that orders, limits or skips) |
| Scopes | Plain functions, `fn due(q: Query<Order>) -> Query<Order>`, applied with `.scope(due)` |
| Implicit filters | Soft delete from `BEHAVIORS`, lifted by `with_trashed()`, which only an aggregate using `SoftDelete` has; the owner filter when the connection is scoped to an owned area, left out when the query already states it, as `Account::find` by the principal's own id does. Nothing else is implicit |
| Raw SQL | `read.sql::<OrderRow>("…").bind(&minimum).all()`, decoded by the same read model. The text is `&'static str`, so input arrives only as parameters; it runs in a read-only transaction pipelined with the query (`Driver::query_read_only`), and an owned area cannot use it, since nothing would keep it to the principal's records. `sofie check` keeps it out of actions |

Values bind as parameters in the dialect's syntax; statements are prepared and
cached by the pool. No string is ever concatenated into SQL.

#### Checking across owners

An owned area's `Read` sees only the principal's records, so a rule over every
owner's records, such as "no two live bookings overlap", needs its own way in.
`#[derive(Aggregate)]` gives each aggregate a crate-private
`across(&read) -> Across<Self>`: only the aggregate's own module can call it.
It takes conditions with `where_`, keeps the aggregate's implicit filters (soft
delete) without the owner filter, and answers only `exists()` or `count()`: no
row, field or id of another owner leaves it. It works the same under any reach.

A check before the commit still races a concurrent one, so `.guard(refusal)`
turns the same conditions into a `Guard<A>` outcome that the commit checks
again. Returned beside the record, it replies nothing, and its steps run before
the commit's writes wherever it stands in the tuple: on Postgres a
`pg_advisory_xact_lock` on a hash of its key (the aggregate's module and name,
or narrower with `.keyed(key)`; any two commits that could clash must share
it), then a `SELECT 1 … LIMIT 1` expected to find no row. A row found rolls
the commit back as `Error::Invalid(vec![refusal])`, a 422. SQLite's
transactions already run one at a time (`BEGIN IMMEDIATE`), so it only checks.
`.except(id)` keeps an update from matching its own record. A guard runs in any
area, not only an owned one.

```rust
pub async fn book_slot(customer: Principal<Customer>, input: BookSlot, read: Read)
    -> Result<(Booking, Guard<Booking>)>
{
    let booking = factory::request(customer.id(), &input)?;
    let (begins, ends) = booking.span();
    let overlapping = Booking::across(&read)
        .where_(Booking::STATUS.in_(BookingStatus::LIVE))
        .where_(Booking::BEGINS.lt(ends))
        .where_(Booking::ENDS.gt(begins));
    let taken = || FieldError::invalid("starts_at", "is already booked");
    if overlapping.exists().await? {
        return Err(Error::Invalid(vec![taken()]));
    }
    Ok((booking, overlapping.guard(taken())))
}
```

Each `exists`, `count` and guard check is logged at info level as
`cross_owner_check` with `module`, `aggregate`, `actor` (the reach's
principal), `kind` (`exists`, `count`, `guard`) and `outcome` (`matched` or
`clear`), never the conditions' values. `sofie gen` lists each module's
`across` calls, by aggregate and caller, under "Cross-owner checks" on its page
in `docs/gen`.

#### Casts

A value encodes to the framework's own neutral `SqlValue` (null, bool,
integers, float, text, bytes, timestamp, date, zone-less date-time, uuid,
json), which the driver
binds as parameters. It decodes from a `ColumnReader` (sofie-core), which the
driver implements over its own wire format: the columns of a row are read in
order, each straight into its target, and text is lent as `&str`, so an enum
or a currency code is matched without allocating and a `String` field copies
once. A value that spans one column does this through its derive. A value that
spans several (`Money` with its currency, `PhoneNumber`) implements `Cast`,
which reads and writes a group of columns. Application code therefore never
names the driver, and a second database needs no change to any value.

#### Why not an existing ORM

The driver and pool are dependencies (`tokio-postgres`, `deadpool-postgres`);
the ORM layer is ours. SeaORM and Diesel each bring their own description of
tables — entities or `table!` macros, usually generated from the database —
which would be a second source beside the schema, and neither offers the
semantics this layer exists for: aggregates loaded whole, a unit of work that
diffs snapshots and writes events in the same transaction, owner scoping from
the area, read models joined through mirrors, and field metadata for screens
and `schema:check`. Wrapping one would mean writing all of that anyway, plus
generating its entity types. `sqlx` would serve as a driver, but its macros and
migrations would go unused. Only `sofie-db` touches the driver, so replacing it
stays local.

#### Migrations and the schema check

- Migrations are SQL files per module, embedded as `File`s (`embed!(path)`,
  or `embed!("migrations", [names])` for a crate's own folder) and named by
  their file name. They are applied in the order of the number
  their names start with (`10_x` after `9_y`) and recorded with a checksum, so
  an applied file that is edited later is refused. A byte-order mark and CRLF
  line ends are dropped before both, so a checkout's line ends change nothing. `sofie db fresh`
  rebuilds a database on this machine from scratch: it drops the schema of
  every module and the framework's and migrates again, refusing any other
  host. The command is built only with sofie's `development` (`Schema::fresh`,
  sofie-db's `fresh`), so a production binary cannot drop its tables.
- `sofie check` reads the migrations' SQL, without a database, for the rules
  the schema declares: a CHECK per field constraint and per enum column
  (listing every variant), a unique index per `#[unique]` field.
- `sofie check` refuses a statement that would lock a table the running
  version still uses (`migration-lock`), since a migration runs while the
  previous version serves: adding a column that fills every row (a volatile
  default, a serial, identity or stored generated column), adding one NOT
  NULL without a default, changing a column's type (except widening a
  `varchar`, `varbit` or `numeric`, which rewrites nothing), SET NOT NULL,
  building an index without `CONCURRENTLY` on a table an earlier migration
  created (an inline `UNIQUE` or `PRIMARY KEY` on an added column too),
  renaming a column or a table, dropping a column the schema still reads, and
  dropping or truncating a table it declares.
  Each finding explains the
  expand and contract steps instead; a justified case says why in
  `-- sofie:allow migration-lock: <reason>` above the statement; a migration
  already applied cannot change (its checksum is checked), so its
  exceptions go in `migrations/migration-allow.toml` as `[[statement]]`
  entries of `file`, the statement's beginning and `reason`; a table that
  stays small takes a `[[table]]` entry of `name` and `reason`, which keeps
  every lock on it, but not a change breaking the running version (a NOT
  NULL column without a default, a rename, a drop or truncate of what the schema still reads). An entry keeping nothing is a `stale-allowance`. A migration building an index concurrently holds only that statement and, on
  Postgres, runs outside a transaction under the same migrations lock
  (`Driver::migrate_alone`), as the framework's own index on waiting jobs does.
  Prefer `sofie gen migration` to a hand-written one; it follows these rules.
- `sofie check` refuses a migration naming another schema's table or
  function (`cross-module-sql`): a `REFERENCES`, a `FROM` (in a CTE's
  `DELETE` or `UPDATE` too), `JOIN`, `INTO`, `UPDATE`, `TABLE`, `TRUNCATE`,
  view, sequence, type or index target, any entry of a list of them, or a
  call, qualified by a schema that is not the module's own, and another
  schema named itself (`DROP SCHEMA`, `GRANT … ON SCHEMA`, `SET SCHEMA`, a
  `search_path`). A migration runs with its module's schema as the
  search path, so an unqualified name never crosses. `sofie` (the framework's
  internals, which the module's role is granted) and PostgreSQL's own
  schemas are allowed; `sofie_identity` is a module like any other, so an
  account is kept as a plain id column. Raw SQL in module code is held at
  runtime by the module's role instead (`module-roles`).
- `schema:check` reads the database's catalog through the seam and compares
  it with each aggregate's `FIELDS` and behaviors' columns, and each child
  table's `id`, parent column and fields: every column present, of the type
  its cast decodes, as nullable as the field, and no column the schema never
  writes that refuses null without a default (`Schema::mismatches::<A>` for
  one aggregate, `Schema::check` for every one the migrations list). Each
  mismatch names its `module.table`. It is generic code over the same
  descriptions; `sofie gen` only lists each module's aggregates beside its
  migrations (`Migrations::aggregates`, `AggregateTable::of::<A>()`). It
  runs as the scaffolded `schema <Aggregate>` fact
  (`sofie_testing::assert_schema`), so CI holds every aggregate's table on the
  migrated test database, and as `<app> schema check` (`sofie schema check`),
  in every build, against the database the app's settings name: a deploy can
  check the database before the new version serves, and the command exits
  non-zero with a line per mismatch.
- **A database role per module.** `sofie gen` writes each module's first
  migration, `0000_module_role.sql`, once (an applied migration never
  changes); it only calls `sofie.module_role('<module>')`, which the
  framework's migrations define, so the grants are library code and not
  copied into every module. It makes a role named as the module's schema
  that owns the schema and
  every table in it, and of `sofie` gets what a commit writes (messages,
  history, idempotency keys, flags read). The app's login migrates as the
  owner of `sofie` and becomes a member of every module role. It serves as
  itself: the framework writes SQL only for an aggregate's own table, so
  the builder, the unit of work and the worker cannot cross a module. With
  sofie-db's `module-roles`, the database holds it too: a commit enters
  `SET LOCAL ROLE <module>` before an aggregate's writes, switches as it
  moves to another module's records and returns to the login (`RESET ROLE`)
  for the framework's own; steps a caller puts first (a message's
  completion) run as the login. The SQL a module writes itself,
  `read.sql(…)`, runs as the role of its read model's module (`SET LOCAL
  ROLE` in its read-only transaction), so it cannot name another module's
  table. sofie-identity ships its own `0000_module_role.sql`.
  A split module logs in as its role. Roles are global to a Postgres
  server: apps sharing one need distinct module names.
- Migration files are SQL in the dialect of the database the module is
  configured for. Migrations the framework and plugins ship (the messages
  table, flags, settings) come in a folder per dialect; the framework's are
  module `sofie`, applied before any app module. Only the table recording
  migrations is created outside a migration.

### The database seam

Everything that differs between databases is behind traits in `sofie-db`:
`Driver`, `Dialect` and `Catalog`.
Everything above them — aggregates, unit of work, read models, the query
builder's API, values — is written once.

| Trait | Covers | Postgres today |
|---|---|---|
| `Driver` | Connections, pool, prepared statements, transactions (pipelined), and rows handed to a `RowSink` as they arrive | `tokio-postgres` with `deadpool-postgres` |
| `Dialect` | Placeholders, identifier quoting, `RETURNING` or its substitute, locking clauses, upsert, pagination, type names; the framework's migrations and the message table's SQL (claiming, fanning out, schedules), kept beside the migration that creates the table | `$1`, `"name"`, `RETURNING`, `FOR UPDATE SKIP LOCKED`, `ON CONFLICT DO NOTHING` |
| `Catalog` | Reading a table's columns, types, nullability and defaults for `schema:check` | `pg_catalog` |

SQLite is the second implementation (`sofie-sqlite`, sofie's `sqlite`
feature; `postgres` is the default): `?1` placeholders, a module's tables
named after it (`Dialect::qualified`), `pragma_table_info` as the catalog,
no `Wakeups`, so the worker polls, and `Dialect::engine` choosing a
module's `migrations/sqlite/` files where its SQL differs.

- **Rows are read where they arrive.** `Driver::query` takes a `RowSink`,
  which names the type each leading column must hold and receives every row
  as a `&mut dyn ColumnReader`. The driver compares those types with the
  prepared statement's columns once, before any row is read, and refuses a
  mismatch or a statement with too few columns naming the column and both
  types, never a value; the rows are then decoded as they stream in, never
  collected first. A read model's types come from its fields' casts
  (`Source::column_types`). An aggregate keeps its stored columns as
  `SqlValue`s, which a save compares to tell what changed; `query_values`
  answers rows as values for the few reads with no type. With `etag`, the
  reader hashes every value its target reads, so a changed row changes the
  tag.
- **Capabilities, not pretence.** A dialect declares what it supports; waking
  workers is `Driver::wakeups`, and `None` means the worker polls; with
  `live`, `Dialect::live_notify` sends a change notification and
  `Driver::change_notifications` hears other instances' payloads, and `None`
  keeps changes to the instance that made them. Where a
  database lacks something, the seam falls back explicitly — polling instead
  of `NOTIFY`, a lock row instead of an advisory lock — or the app fails at
  boot naming the missing capability. Nothing silently behaves differently.
- **Per module.** The dialect follows each module's database connection, so a
  split module can move to another database without touching its code, only
  its migrations.
- **Raw SQL is the exception.** `read.sql(…)` is written in the module's
  dialect by definition; it is the one place a database switch means editing
  queries.
- **One implementation until a second is needed.** The traits are kept to what
  Postgres uses today; a second database will widen them where it must, and
  not before.
- **A bounded wait for a connection.** The pool's size and wait come from
  `PoolSettings` (`SOFIE_DB_POOL_SIZE`, `SOFIE_DB_POOL_WAIT_MS`); a caller still
  waiting when the wait ends fails as `Error::Unavailable`, which HTTP
  answers 503 with `Retry-After` and the worker retries.
- **Idle connections close.** Every 30 s the pool closes connections unused
  for 5 minutes, keeping 2 open so the first request after a quiet spell
  finds a warm one; TCP keepalive (60 s unless the url sets less) keeps those
  alive through NAT gateways that drop idle sockets.
- **Select lists rendered once.** A source type's `SELECT <columns> FROM
  <table>` is rendered once per `Db` and reused; only the conditions, order
  and limits are rendered per query. A list with a joined column, which binds
  the reader's owner, is rendered each time.

### Handlers: one trait, resolved by type

Every argument of an action, query or listener implements `Resolve`, from
one `Context` trait that a request, a job and a task each implement:

```rust
pub trait Resolve<C>: Sized {
    fn resolve(context: &mut C) -> impl Future<Output = Result<Self>> + Send;
}

pub trait Context: Send {
    fn db(&self) -> &Db;
    fn reach(&self) -> Reach;
    fn read(&self) -> Read { … }                        // within `reach`
    fn clock(&self) -> Clock { … }                      // the `Db`'s
    fn bound(&self, name: &str) -> Result<Uuid> { … }   // a route segment or a job's payload
    fn input(&mut self) -> Result<Value> { … }          // a body, a payload or a task's arguments
    fn event(&self, name: &str) -> Result<Value> { … }  // a listener's message
}
```

A context leaves out what it does not carry, and the default refuses the
argument when it resolves: a task names no record, only a listener receives an
event. `Writes: Context` marks the contexts that commit.

| Argument | Implemented by | Resolved from |
|---|---|---|
| An aggregate (`Order`) | Its derive | The route segment named after it, loaded through the area's scope |
| A read model (`OrderRow`) | Its derive | The route segment named after its aggregate, read the same way |
| A value or input DTO (`CancelReason`) | Its derive | The request body, the job's payload, or a task's `--name value` arguments; a value alone reads the field named after its type (`cancel_reason`) |
| An event (`OrderPaid`) | Its derive | The message being delivered |
| `Principal<A>` | `sofie` | The session, in area `A` only; `id()` is an `Id` of the area's principal aggregate (`type Principal = Account`, or a marker such as `Permitted` naming the aggregate it signs in as) |
| `Session` | `sofie` | The session cookie; `sign_in::<A>` and `sign_out` are written after the commit. sofie-identity's sign-out route also ends the session's record (`sessions`), so a copy of the cookie stops too. Signing in takes a zero-sized `SignInRight`, which only `SignInRight::of::<K>()` mints for a `K: Admitter`; sofie-identity and sofie-tenancy each implement `Admitter` for a private type, so an app's actions sign nobody in unless the app declares an admitter of its own. Who may enter an area is the caller's to check (sofie-identity checks membership) |
| `Read`, `Clock`, `Features`, `Permissions`, `Listing<T>`, the module's config | `sofie` | The app state |
| `Flags`, `ShieldBlocks`, `Jobs` | `sofie` | The app state; `FlagRules` and `ActiveBlocks` are their read-only views |
| `Dep<dyn CustomerDirectory>` | `sofie`, for any contract trait | The app state, filled by the wiring |

Handlers are plain `async fn`s. A `macro_rules!` in `sofie-db` implements one
`Call<C, Args>` trait (resolve each argument from context `C`, then call) for
functions of up to twelve arguments, the way `axum` does. `erase(function)`
turns any of them into a `Run<C>`: the only code compiled per handler, it
resolves, calls and boxes the outcome as a `dyn AnyOutcome`. What follows is
the same for every handler and compiled once: a request commits the outcome
(`Db::commit_with` takes `&mut dyn Outcome`) and replies it, a GET only
replies, a job commits it with its message's deletion, a task after checking
its arguments. The worker keeps each function as an `Entry { name, trigger,
run }`, its trigger `On(event)`, `Every(interval)` or `Queued` (a message
naming it); a route keeps its `Run` in the router. One
action can therefore be an endpoint, a job and a listener with no adapter
written for each. An argument type without `Resolve` is a compile error at the route that
registers it.

A GET never writes, and the compiler holds it to that. `routes.get`,
`screen`, `island` and `shipped` resolve their handler in `ReadContext<A>`
(`RequestContext<A, Reading>`) and require `Loader<A, Args>`: every argument
resolves there and the outcome is `ReadOnly` (a read model, `Paginated`,
`Screen`, a reply). Write-side arguments resolve only in a context that
`Writes` (`RequestContext<A>` of the other methods, `JobContext` and
`TaskContext`): an
aggregate, an input or value DTO, `Session`, `SignIns`, `Flags`,
`ShieldBlocks`, `Jobs`, `Dep`, `Extensions`, `IdempotencyKey`. Naming one, or
returning an aggregate or `Queued`, fails at the route with "cannot answer a
GET … a GET never writes" (`#[diagnostic::on_unimplemented]` on `Loader` and
`ReadOnly`), and `sofie check` reports the handler as `writing-get` with its file
and line. The endpoint of a GET replies its outcome and never commits.
Nothing implicit covers "write while reading" (last seen, read receipts,
counters): a fire-and-forget job is still a write, so it belongs in a POST, or
in a job a domain event triggers.

Because no GET writes, every GET may be answered from the rows it read: with
`etag` each one is tagged and revalidated (see docs/structure.md), and a GET
could be routed to a read replica, since nothing it does needs the primary.

### Routes: built at boot, named by the wiring

`routes.rs` runs at boot and builds the router (`matchit`); there is no
generated router. Names and permissions need the handler's module path, which Rust does
not offer reliably at runtime (`std::any::type_name` is documented as
unstable in form). `sofie gen` already reads `routes.rs`, so each module's wiring carries a
table (`Module::route_names`, gathered by `App::modules`) from each route's method and
full path — unique, or the router refuses to start — to its name
`<module>.<area>.<handler>`, derived by `sofie_model::naming::route_name`. The
router looks names up once, at boot, and a request's span is named
`<METHOD> <name>`; a route without a name (the identity's own) keeps its path
pattern. Each route's permission comes in a table of the same shape,
`RoutePermission`, given to `Gates::route_permissions` (behind
`permissions`), and a route gated by `#[sofie::feature(…)]` on its handler in
one given to `Gates::route_features` (behind `feature-flags`); the wiring
hands both to `App::gates`.

What a registration must keep is written once, in
`sofie_core::registrations::validate`: no two routes the router cannot tell
apart (alike once parameter names are erased, or a parameter and a catch-all
at one place), one name per route, one name per job, and a schedule between
once a second and once in 1000 years. `sofie check` runs it over the routes,
identity's included, at their full paths, and over the jobs the wiring will
register, reporting file and line; boot runs it over the registered routes
before building the router and over the worker's jobs before any work, and
refuses to start.

### Outgoing requests: one way out

Every request that leaves the app goes through `sofie-client`, so every one is
traced as an outgoing request, limited, and checked against where it may go.
Behind the `sofie` facade's `client` feature, off by default.

```rust
// modules/payment/src/gateways.rs
#[sofie::gateway]
pub trait Payments: Send + Sync {
    async fn charge(&self, charge: &ChargeRequest, key: &IdempotencyKey) -> Result<ChargeResponse>;
}

// modules/payment/src/gateways/stripe.rs
pub struct Stripe {
    http: Outgoing,
    settings: StripeSettings,
}

impl Adapter for Stripe {
    const VENDOR: &'static str = "stripe";
    type Settings = StripeSettings; // its own keys: PAYMENT_STRIPE_API_KEY …

    fn new(http: Outgoing, settings: StripeSettings) -> Self {
        Self { http, settings }
    }
}

#[sofie::gateway]
impl Payments for Stripe {
    async fn charge(&self, charge: &ChargeRequest, key: &IdempotencyKey) -> Result<ChargeResponse> {
        let request = self.http.post("/v1/charges").bearer(self.settings.api_key.expose());
        request.json(charge).idempotency(key).send().await?.json()
    }
}
```

- **Only adapters get one.** `Outgoing` has no public constructor and no
  `Resolve`. `sofie gen` finds each `#[gateway]` trait and the types
  implementing it with `impl Adapter`, and the wiring's `gateways()` builds
  the adapter `<MODULE>_<GATEWAY>` names (`PAYMENT_PAYMENTS=stripe`, unset
  when there is one) from its vendor's keys, `<MODULE>_<VENDOR>_*`: the
  framework's (`BASE_URL`, `HOSTS`, `PRIVATE`, `CONNECT_TIMEOUT_MS`,
  `TIMEOUT_MS`, `MAX_RESPONSE_BYTES`, `RETRIES`, `REDIRECTS`) into its
  `Outgoing`, the rest into its `Settings`. Handlers take `Dep<dyn Payments>`.
  A remote contract's `Outgoing` is built the same way, from the split
  module's `<MODULE>_REMOTE_*` keys.
- **Egress allowlist.** An `Outgoing` reaches its base URL's host and the
  `HOSTS` listed, over HTTPS; anything else is refused before a connection
  opens.
- **SSRF guard.** The host is resolved, every address checked, and the
  connection opened to a checked address. Private, loopback and shared
  addresses are refused unless the vendor's `PRIVATE=true` (which also
  allows plain HTTP inside the network); link-local, cloud metadata,
  multicast and reserved addresses always are, IPv4-mapped and NAT64 forms
  included. Redirects are followed only for `GET` up to `REDIRECTS`, each
  hop held to the same rules, never from HTTPS to plain HTTP, and a hop to
  another scheme, host or port carries none of the caller's headers. URLs supplied by users, such as webhook
  targets, go through `Outgoing::untrusted`: any public host over HTTPS, no
  exception, no redirect, no credentials in the URL, and a connection of its
  own, closed after the request. A host that resolves to no public address
  is refused with one message whatever it resolved to, or whether it
  resolved at all, so a user cannot map the internal network.
- **Traced and measured.** Each call of an adapter's method is a `gateway`
  span named `<vendor>.<method>` (`stripe.charge`) with its outcome; each
  request under it an `http.client` span with the peer host, port, method,
  status and retries, carrying `traceparent`. With `metrics`,
  `sofie.gateway.duration` (adapter calls and `Outgoing::operation`) by
  vendor, operation and outcome, and `sofie.outgoing.duration` (each
  request) under the same labels: `gateway.vendor`, `gateway.operation`
  holding the method, and `outcome` holding the status class (`2xx` to `5xx`)
  or `error` when no answer came. Headers, bodies and secrets are never recorded.
- **Bounded.** Connect (5 s) and total (30 s) timeouts, a response size cap
  (4 MiB), and a circuit breaker per host and port: five failed requests in a
  row (each counted once however often it was retried; a refused one not at all)
  refuse calls for 30 s, then one probe is let through. Retries (two, with
  backoff) only on a failed connection or a 502/503/504, and only when the
  method is idempotent or the request carries an `IdempotencyKey`, sent as
  `Idempotency-Key`. A connection is reused while idle under 20 s.
- **Tests make no real calls.** An app sofie-testing starts builds no
  adapter and so no `Outgoing` (a process switch, not a feature a workspace
  build could turn on in the production binary);
  `#[sofie::gateway]` gives each gateway a `Fake<Gateway>` recording its
  calls, which the test provides with
  `sofie_testing::fake::<dyn Payments>(&app, Arc::new(FakePayments::default()))`.
  A gateway left unprovided fails the call. sofie-client's `testing` feature
  builds an `Outgoing` trusting only a test's own root certificate, for an
  adapter's own tests against a local server.
- **Other protocols the same way.** A gateway that speaks something other than
  HTTP — SMTP, an object store — wraps its own client in
  `Outgoing::operation`, which gives it a span, metrics, the total timeout
  and the vendor's breaker. The database and the telemetry exporter are the
  framework's own exceptions.

`sofie check` refuses, in any module, HTTP client and TLS crates (`reqwest`,
`ureq`, `hyper`, `hyper-util`, `rustls`, `tokio-rustls`, `native-tls`,
`openssl` and the like) and `sofie-client` itself, raw sockets (`TcpStream`,
`TcpSocket`, `TcpListener`, `UdpSocket` and the Unix sockets) and an
`Outgoing` named outside `src/gateways/`, so the one way out cannot be bypassed by adding a dependency. It also
reports a gateway without adapters and an adapter without `impl Adapter`.

`sofie-client` is `hyper`'s client with `rustls` and the Mozilla roots
(`webpki-roots`), owned by the framework rather than wrapped around
`reqwest`, so the address check sits between resolution and connection,
where a wrapper cannot reach.

### Files: storage and uploads

`sofie-storage` (sofie's `storage`, off by default) keeps files under keys it
generates itself, 128 random bits as hex: no caller ever names a path, so
none can leave the store's directory or bucket. `Storage` is the app's
handle (a handler argument, `Serving::storage`), over a `Store` adapter
chosen by `SOFIE_STORAGE`: `local` (the default; files under
`SOFIE_STORAGE_ROOT`, each written beside its name and renamed once complete)
or, with `storage-s3`, `s3` (`SOFIE_STORAGE_S3_REGION`, `_BUCKET`,
`_ACCESS_KEY_ID`, `_SECRET_ACCESS_KEY`, and for another S3-compatible
service `_ENDPOINT`, `_PATH_STYLE`, `_PRIVATE`). `put` streams (S3 in
8 MiB parts, so one part is in memory at a time), `get` reads in chunks (S3
by a 256 KiB range, then 8 MiB ones), `delete` and `exists`. The S3 adapter signs every request
with AWS Signature Version 4 written on `ring`, tested against AWS's
published examples, and sends it through sofie-client's `Outgoing`, so the
allowlist, limits, breaker and spans hold for storage too.

`download_url(key, content_type, valid_for)` is a link that opens the file
until it expires: S3 presigns it; `local` signs `/_sofie/files/<key>/<type>`
with the session keys, which sofie-http checks and streams before routing.
Only images and PDF are sent inline as their type; anything else is an
`application/octet-stream` attachment, so an uploaded page never runs as
the app's own.

sofie-http's `uploads` reads a `multipart/form-data` body as it arrives: text
fields into the input as a urlencoded form's, each file straight into the
storage, within the route's `.body_limit(…)` and `.file_limit(…)` (each
file), text held to 64 KiB whatever the route allows. Only a route with
`.file_limit(…)` takes files; elsewhere a file part is answered 415. On a route with `.file_limit(…)`, a body that is not
`multipart/form-data` is held to 64 KiB too. A file whose declared type its first bytes contradict (PNG, JPEG,
GIF, WebP, PDF) is refused as a field error and deleted. A file arrives as
an `Upload` field of an `Input` (key, sanitized file name, type, size),
sealed with a key of the process so a JSON body cannot name a stored file;
the files of a request answered with an error are deleted. A form whose
input has an `Upload` is rendered with `enctype="multipart/form-data"` and
a file control. CSRF and the session apply as to any form post.

### Data: search, CSV, retention, image variants

- **Full-text search** (`search`). `#[aggregate(behaviors(Searched))]` keeps a
  `search_text` column of the fields whose values are searched by their
  words or whole (`Present::SEARCH`: text, emails, slugs, enums; not
  amounts, times or secrets), rewritten with every write.
  `Product::matching("blue wool -red")` finds records holding every word; on
  Postgres `to_tsvector('simple', search_text) @@ websearch_to_tsquery(…)`
  (quoted phrases, `-word`), served by a GIN index on that expression;
  another database finds the words as one piece of text, ignoring case.
  `sofie new search MODULE Aggregate` adds the behavior, the column and an
  index built concurrently. A record written before the column existed is
  found once it is written again.
- **Search engines** (`search-meilisearch`). `matching` and listings keep
  their API whichever engine answers. The `SearchEngine` gateway (sofie-db)
  has two adapters, chosen by `SOFIE_SEARCH`: `database` (the default, and
  all an app without the feature has: the query above) and `meilisearch`
  (sofie-client's `Meilisearch`, through an `Outgoing` of vendor
  `meilisearch`: `SOFIE_SEARCH_MEILISEARCH_URL`, `_API_KEY`, `_PRIVATE`), so
  its requests are allow-listed, bounded and traced as `meilisearch.matching`,
  `.put` and `.remove`. An unknown `SOFIE_SEARCH`, or `meilisearch` in an
  app built without the feature, stops the start with an error naming the
  value and the feature. Each searched aggregate a module's migrations list is
  an index `<module>_<aggregate>`; a document is the record's id, its
  search text (recomputed from the `Searched` fields, and written back to a
  `search_text` column that differs) and, where the owner is a column
  of the row, its owner. A write that changes a record's text, owner or
  deletion queues `sofie.search.index` with its commit, and the worker reads
  the record as it is then and puts it, or removes it when it is gone or
  softly deleted, then reads it again and writes once more if its version
  moved meanwhile, so a retried or late job never leaves an old state; a
  pruned batch is removed by a `sofie.search.forget` job its prune's commit
  queues. An index gets its settings (`text` searched, `owner` filterable)
  once per process before its first documents, and a match on an index not
  made yet finds nothing. Meilisearch answers a document write once it is
  queued, as a task that may still fail: the worker keeps the task uids in
  memory (at most 10,000; a warning counts any dropped) and checks them every
  30 s in batches through `GET /tasks?uids=…`, logging each failed or canceled
  task once at error level with its uid, index, type, and error code and
  type, never Meilisearch's error message, which may quote a document. A
  restart loses at most the log line of a task still pending; a key without
  `tasks.get` stops the check with one warning. A match asks for every word (`matchingStrategy: all`, quoted
  phrases and `-word` as on Postgres, with typo tolerance on top), within the
  read's owner when it has one, and takes at most `MATCHES_READ` (1000) ids
  best first; the read then filters by those ids in the database with the
  rest of its conditions, the owner's included, and orders and pages there,
  so an owned area never sees another principal's records whatever the
  engine answers, and a read narrowing a match further sees at most those
  1000. A record owned through its parent carries no owner in its document
  (the parent's owner can change without the record being written), so its
  matches are not narrowed to the owner in the engine: an owned area sees its
  own among the best 1000 of every owner's. `<app> task sofie.search.reindex` queues the first page of every
  searched aggregate; each page job puts 500 records, removes the deleted
  among them, queues an index job for each one changed meanwhile and queues
  the next, as a mirror's backfill does: run it once
  after switching to Meilisearch, and whenever its index was lost.
- **CSV** (`csv`). `.document(Media::Csv)` on a GET answers its list of
  records as RFC 4180 CSV headed by the first record's fields; a text cell a
  spreadsheet would run as a formula (`=`, `+`, `-`, `@`, a tab) gets a
  leading `'`, a number does not. An action taking `CsvRows<I>` reads the
  body's CSV, each row as `I` reads a form post; a refused row refuses the
  import, its errors named `<row>.<field>`, so a returned `Vec` of records
  commits all rows or none. The body is held to the route's `.body_limit(…)`.
- **Retention** (`retention`). A scheduled job takes `Prune` and calls
  `prune.before(A::CREATED_AT, cutoff)`, on any ordered column, a nullable
  one keeping its null rows: deleted 1000 at a time, so no
  statement holds many rows' locks and a retried job resumes; an aggregate
  with children is refused. Each pruned record's change event carries only
  its id, version and `"sofie.pruned": true` (a key an app event may not
  set), and reaches only mirrors and feeds
  (other listeners are not handed it); the app's listeners hear `<Aggregate>Pruned`
  (`id`, `version`), which `sofie gen` writes into the contract's `pruned`
  module, queued in the prune's commit. Either event is queued only when a
  listener of the prune's worker takes it. Every mirror drops a pruned record, and one
  without `deleted_at` also a record its source deleted, remembering its
  version in `sofie.mirror_forgotten`, so an older change arriving late does
  not bring it back; a mirror's worker sweeps rows forgotten over 30 days
  ago (or longer, while a raised `max_attempts` still retries a change) hourly (`sofie.mirror.sweep_forgotten`). With `product-reports` the worker prunes
  `sofie.product_events` hourly, keeping `SOFIE_PRODUCT_EVENTS_KEEP_DAYS`
  (400 when unset); closing an account deletes its events in the close's
  commit (`sofie::db::forget_principal`) and records the account in
  `sofie.forgotten_principals` for good, with or without the feature: the
  writer skips its events still waiting then, and the hourly prune deletes
  any a batch racing the close wrote. `sofie::db::forgotten` reads the record
  and `unless_forgotten` refuses a commit planned for a principal forgotten
  since it was read (a shared lock against the erasure's on Postgres), so a
  job queued before the close writes nothing for it.
- **Image variants** (`image-variants`). `Storage::make_variants(&key,
  &[Variant::new("thumb", 320)], &resizer)` reads an uploaded JPEG, PNG, WebP
  or GIF of at most 32 MiB and keeps what the app's `'static` `Resizer` makes of it
  on the blocking pool
  under a key derived from the original's (`key.variant(&THUMB)`), so it is
  found again without being stored; a variant kept already is not made
  again. The framework decodes no images, so no image library is in its
  tree (user, 2026-10-14): the app's resizer brings one. The worker's
  database provides the `Storage`, so the job doing it takes `Dep<Storage>`.

### Mail: rendered with the commit, sent by a job

The `sofie-mail` crate, behind the `sofie` facade's `mail` feature, off by default.

- **`Mailer`, never constructed by an app.** `sofie::run` reads `SOFIE_MAIL_*` once
  (`TRANSPORT` `smtp` or `log`, `FROM`, `BASE_URL`, and `SMTP_HOST`, `PORT`,
  `SECURITY` `starttls`/`tls`/`none`, `USERNAME`, `PASSWORD`) for serving and for
  tasks; a handler, job or task takes `Mailer` as an argument.
  `mailer.body("order/receipt", &data)` renders `src/mails/receipt.html` of
  module `order` with the engine (the build script compiles it, the wiring
  embeds it as a `File` the way it embeds screens and registers it with
  `sofie::mail::templates`); the text part is derived from the
  HTML. `Mail::new(Address::new(&email)?, subject, body)?` is checked once: an
  address cannot end an SMTP command or a header, a subject is one line.
  `mailer.send(mail)` returns a `Queued` outcome, so a mail leaves only if the
  handler commits, and a request never waits on SMTP.
- **One job sends.** `sofie_mail.deliver` renders RFC 5322 headers (RFC 2047 for
  UTF-8) and a multipart/alternative base64 body, and speaks SMTP itself over
  tokio: EHLO, STARTTLS (`tokio-rustls`, Mozilla roots; bytes sent before TLS
  are refused) or TLS from the start, AUTH PLAIN or LOGIN only inside TLS,
  dot-stuffed DATA. The message id is the job's idempotency key, and the key
  is marked in `sofie.idempotency` right after the server accepts the mail, so a
  retried job sends nothing again. A `mail` span and `sofie.mail.duration` per
  send name the transport and outcome (`ok`, `refused` when every recipient
  was refused, or `error`), never an address, subject or body;
  a reply's text is kept in errors with each word holding an `@` left out.
  A 4xx reply, a lost connection or a failed TLS handshake is `Unavailable`
  and retried. A 5xx to `MAIL FROM`, `DATA` or the message is `Rule`, which
  the worker buries at once; one to the greeting, EHLO, STARTTLS or AUTH is
  the server's or the settings', so it stays `Unavailable`. A 5xx to
  `RCPT TO` leaves that recipient out and the others still get the mail; a
  mail every recipient refused is sent to no one and buried. A 550, 551 or
  553 there (not an enhanced 5.7.x, a refusal by policy) suppresses the
  address as a provider's hard bounce would, committed after the job is
  recorded as sent, so a failure there never sends the mail again.
- **`log`** writes mails, links included, to the log: debug builds only. It
  also writes each mail as a page under `target/mail-preview`
  (`SOFIE_MAIL_PREVIEW_DIR`, `off` to stop) to open in a browser.
  Tests use sofie-testing's `mail` feature: a fake transport, read with
  `sofie_testing::sent_mails(&app)` after `app.drain()`.
- **Bulk mail and suppressions.** `Mail::new(…)?.bulk("news")?` marks a mail of a list:
  one recipient, sent with `List-Unsubscribe` and `List-Unsubscribe-Post`
  (RFC 8058), its link sealed per recipient with the app's keys (sofie's
  `signed-links`). `sofie.mail_suppressions` holds what an address no longer
  gets: `All` after a permanent bounce, `Bulk` after a complaint, `List` after
  an unsubscribe; the delivery job leaves those recipients out and sends
  nothing when none is left. `sofie_mail::suppress` adds one by hand.
  sofie's `mail-web` (`sofie::mail::web::routes(routes, &Events::from_environment()?)`
  in a `Web` area) serves `/mail/unsubscribe` (a page asking first, and the
  one-click POST a mailbox provider sends, answered 2xx) and webhooks:
  `/mail/events/postmark` (basic authentication,
  `SOFIE_MAIL_POSTMARK_WEBHOOK_AUTH`), `/mail/events/mailgun` (HMAC over
  timestamp and token, `SOFIE_MAIL_MAILGUN_SIGNING_KEY`, fifteen minutes) and
  `/mail/events/signed` (any provider: HMAC-SHA256 of the body in
  `X-Sofie-Signature`, `SOFIE_MAIL_WEBHOOK_SECRET`). `sofie new mail MODULE
  name [--list LIST]` writes a template and the function sending it.

### Billing: plans held by entitlements

The `sofie-billing` crate, behind the `sofie` facade's `billing` feature, off
by default.

- **Declared, not configured.** A module declares `Entitlement`, `Limit` and
  `Quota` constants (`<module>.<name>`); one module's `src/plans.rs` declares
  `Plan`s granting them and `pub const PLANS: Plans = Plans::new(FREE, &[PRO])`,
  checked as the constant is built (names, repeats), so a malformed catalog
  fails the build. The first plan is the default every owner without a
  subscription holds. `sofie gen` installs it: `sofie::billing::install(&db,
  &PLANS)` in `app` and `worker`, `Gates::entitlements`, its migrations and its
  webhook.
- **Seen where flags and permissions are.** `.entitled(EXPORT)` on a route
  answers 403 to an owner whose plan lacks it, a menu entry follows its
  route, and a screen asks `{#if entitled("project.export")}`; a limit or
  quota above zero counts as held. sofie-http reads the principal's plan once
  for a request that needs it, through its `Entitled` seam. `sofie check`
  reports an `.entitled(…)` naming no `Entitlement`, `Limit` or `Quota`
  constant a module declares with a literal name, and a screen's
  `entitled("…")` naming no declared entitlement (rule `entitlement`).
- **In handlers.** `Entitlements` (the area's principal is the owner):
  `allow_another(PROJECTS, count)`, `consume(CALLS, 1)` returning an outcome
  that counts the use with the commit (an upsert refusing to pass the
  allowance, so racing requests cannot overspend), `used`, `seats`, `plan`
  and the subscription's `standing`. `Checkout`
  (any context, an explicit `Subscriber`): `subscribe` (refused while a plan
  renews at the provider), `buy` and `portal` returning a `Link`, followed by
  a route with `.then_follow()`. `Billing` (the same): `start_trial` (no card,
  once per owner), `seats`, and `change_seats` (the provider's
  subscription quantity, sent by a job after the commit and prorated: to
  Stripe with the job's idempotency key, to Paddle as a count that changes
  nothing more when sent again; seats follow the provider's notification;
  a job finding a later count asked once its own landed asks for it again,
  and a new provider subscription drops the asks of the one it replaced),
  `cancel_at_provider` (at once, for an owner that is gone; from a job, with
  its idempotency key) and `Standing::renews_at_provider`.
- **Kept in step with a provider.** `SOFIE_BILLING_PROVIDER` (`stripe` or
  `paddle`), `_API_KEY`, `_WEBHOOK_SECRET`, `_BASE_URL` (else `SOFIE_BASE_URL`), `_PRICES`
  (`pro=price_…`), `_API_URL`; without a provider, plans, trials and quotas
  still work. Its notifications arrive at `/webhooks/billing`, verified by the
  provider's HMAC within five minutes and answered once per delivery id. The
  subscription follows trialing, active, past_due and canceled as the
  provider reports them, unless the move is not one the states allow; a
  change older than the one held is dropped. `SubscriptionChanged` and
  `PurchaseCompleted` are raised for listeners. A price `_PRICES` does not
  name is logged as an error with its id and acknowledged: the subscription
  held keeps its plan, and a new subscription or purchase grants nothing.

### Notifications: one send, every channel

The `sofie-notify` crate, behind the `sofie` facade's `notify` feature (which
brings `mail` and `local-time`), off by default. Its tables are framework
migrations, so nothing is wired per app.

- **Declared and sent.** A module declares `Topic` constants
  (`<module>.<name>`) with the channels on by default (`Inbox`, `Mail`,
  `Push`), `urgent()` for what quiet hours and digests never hold, and
  `never_digested()`. A handler takes `Notify` and returns
  `notify.send(Recipient::from(id).email(&email), SHIPPED, notice)`, a job
  queued with its commit, so nothing is sent for a refused action.
- **Delivered by a job.** It applies the principal's choices
  (`Preferences::choose`), fills the inbox at once, holds mail and push until
  the principal's quiet hours end (in its zone, through `Jobs::queue_at`),
  keeps mail for a daily or weekly digest at a local time (`LocalJobs`; a
  digest mails the 50 oldest held, one mail per address they were held for,
  the rest wait for the next one), and
  queues one push job per browser subscription, so a failing push service
  retries only its own.
- **Inbox.** `Inbox` (the request's principal): `latest`, `unread`,
  `mark_read`, `mark_all_read`. A screen route listing it with `.live(…)`
  refreshes as the worker commits new rows.
- **Retention.** Hourly, `sofie_notify.prune` deletes inbox items read and
  digest items mailed more than 90 days ago; unread and unsent ones stay,
  and mail suppressions are kept.
- **Closed accounts.** `personal_data::exported` and `personal_data::forgetting`
  (settings, choices, inbox, held digest items and push subscriptions
  deleted, the digest schedule canceled, the principal recorded forgotten so a
  delivery queued before skips it) back sofie-identity's
  `account_data::Notifications` (its `notify` feature), which an app
  registers as a `PersonalData`.
- **Web push.** `subscribe_push::<Area>`, `unsubscribe_push::<Area>` (a
  browser signing out stops its principal's pushes) and `vapid_key` are
  handlers an app routes; `SERVICE_WORKER` is the script it serves. VAPID (RFC 8292) from
  `SOFIE_NOTIFY_VAPID_PUBLIC_KEY`, `_PRIVATE_KEY`, `_SUBJECT`; the message is
  encrypted `aes128gcm` (RFC 8291) with `ring`, and sent through
  `Outgoing::untrusted`, since the browser chose the push service. An
  endpoint the guard refuses, or an answer of 404 or 410, marks the
  subscription gone; a 400, 401 or 403 (most often the sender's own setup)
  logs an error and keeps it. A principal keeps its 20 newest browsers
  subscribed (older ones are marked gone), and a live subscription moves to
  another principal only when the browser shows the same `auth` secret.
- **Schedules at a local time** (sofie-db `local-time`). `LocalJobs::set(key,
  job, payload, LocalDateTime, Repeat::Daily)` keeps the local time and zone
  and places each run by the zone's rules when it is due, so a run stays at
  09:00 across a clock change and follows a change to the zone's rules; a time the clock change skips runs where the change ends, and a repeated one at its first occurrence, as wall-clock schedules do;
  runs a stopped worker missed are skipped, and a run is queued once
  whichever worker fires it.
- **Wall-clock schedules** (`local-time`). `#[every(daily_at = "03:00",
  zone = "Europe/Istanbul")]`, optionally with `weekly_on = "monday"`, runs
  a job when the zone's clocks show that time. A skipped time runs where
  the clock change ends; a repeated one runs once, at its first occurrence.
  The first run is the next such time, not at once. A missed run fires once
  late. The schedule shares the schedule table, so one worker fires each
  occurrence. The cadence is stored beside `next_at`: a schedule whose
  cadence changed (`every(24h)` → `daily_at 03:00`, `03:00` → `04:00`)
  starts over from the new cadence's first time when a worker starts. The time and day are checked as the app compiles, and an app whose sofie dependency lacks `local-time` is told to enable it; the zone by
  `sofie check` (`schedule-zone`) and again when the worker starts. In
  code: `worker.every(name, WallClock::daily(TimeOfDay::constant("03:00"),
  "Europe/Istanbul"), job)`.

### Content: written pages, found and previewed

The `sofie-content` crate, which an app depends on like sofie-identity;
`sofie gen` wires its migrations, screen, routes and settings.

- **Pages.** A `Page` (module `sofie_content`) has a title, a unique slug
  that follows it (`Slugged(from = title)`, so a title another page has is
  refused), a summary, a Markdown body, an optional image and a publishing
  time; only a published page is served, at `/pages/{slug}`; a slug it had
  before answers 301 to the current one. It is shown through the engine's
  `<Head>` (title, description, canonical URL, Open Graph and Twitter card
  tags, moved into `<head>`) and `<Markdown>` (a strict subset: raw HTML
  stays text, a link off the scheme allowlist goes nowhere). An app gives the
  screen `sofie_content/web/page` its own look by a template of that key.
- **Sitemap.** `/sitemap.xml` (a `.document(Media::Xml)` route) lists every
  published page with its last change, at most 50 000.
- **Where the app is reached.** `SOFIE_CONTENT_BASE_URL`, else `SOFIE_BASE_URL`, read at boot: an
  http or https URL, required in a release build; a debug build without it
  uses `http://127.0.0.1:8080`.
- **Writing.** With the crate's `admin` feature, `admin_routes::<A>` gives an
  area of the app resource screens to list, write and change pages, and
  actions to publish and withdraw one (`/pages/{page}/publish`, `/withdraw`);
  mounted by the app as `.area::<A>("sofie_content", sofie_content::admin_routes::<A>)`,
  best in a `Permitted` area, whose routes then each need a permission.

### Contracts and gateways: `dyn` at the boundary

A contract or gateway is a trait object chosen at boot: the module's
implementation in the monolith, the generated HTTP client after a split, the
configured vendor for a gateway, a fake in tests. Dynamic dispatch costs
nanoseconds beside the database or network call behind it, and it removes
generic parameters from every handler and every registry.

With `sofie`'s `contracts` feature, the wiring's `provide(&db)` hands each
module's implementation of its contract's traits (`src/contract.rs`, built
`From<Db>`, so it reads with full reach), each extension (`src/extensions/`) and each exposed aggregate's
`Lookup` to the `Db` the app and its worker share; `Dep<dyn Trait>` and
`Extensions<dyn Trait>` resolve from it in any context. `#[sofie::contract]`
turns a trait's and an implementation's `async fn`s into ones returning
`BoxFuture`, which `dyn` needs; reference arguments borrow for the call.
A module's handler, job or listener taking `Dep<dyn Trait>` of its own
contract would read past its area's owner filter, unaudited and racing, so
`sofie check` reports it (`own-contract`) and points to `across` and `guard`
(The ORM, Checking across owners). The same rule reports code calling
`Across::for_derived_across_only` by hand, which would reach another
module's records.

**Remote contracts** (sofie's `remote-contracts`, off by default). With it,
`#[contract]` also implements each trait whose methods can cross apps for
`sofie::remote::Remote<dyn Trait>` and implements `sofie::remote::Serve` for
`dyn Trait`: wiring and a match on method names, the arguments sent as a
JSON array (a `&str`, `&[T]` or `&T` read back owned and lent again) and
the answer as JSON. The behaviour is library code:

- *Client* (sofie-client `Remote`, `RemoteLookup`, `RemoteFeed` over a
  `RemoteApp`): `POST /_sofie/contracts/<contract crate>/<Trait>/<method>`
  in a `contract` span, `POST /_sofie/lookups/<DTO>` with every id of a
  `get_many` in one call, `GET /_sofie/feed?after=&limit=`. A refusal comes
  back as the same `Error` variant (field errors keep their key, arguments
  and message, so the calling app translates them as its own); a 5xx is an internal error naming the app.
- *Principal token* (sofie-auth): every call carries
  `sofie-principal`, signed with `SOFIE_SIGNING_KEYS` (HMAC under its own purpose
  and bound to the path), valid 60 s, holding the area and principal the
  work acts for, or none for the app itself. sofie-http runs each handler in
  `sofie::remote::acting_as` its session's principal, the client reads
  `sofie::remote::acting()`, and the server runs the implementation acting for
  the token's principal, so a chain of calls keeps it. A missing, forged,
  expired or replayed-elsewhere token is answered 401.
- *Server* (sofie-http `Served`, registered on the `Db` with `serve`): answers
  under `/_sofie/` before routing, outside every area, from what the wiring
  provided.
- *Event pull feed* (sofie-db): `Feeds::publish(event)`, attached to a worker, appends each such
  event to `sofie.feed` in the transaction that deletes its message, under a
  lock held to the commit so positions commit in order; entries live 7 days
  (hourly `sofie.feed.prune`). `Feeds::pull(source)` reads another app's feed
  after its cursor in `sofie.feed_cursors` and, in one transaction that
  fails for a consumer racing from the same cursor, queues a job for each
  listener the process heard, keyed by the producer's event id (before any
  worker is published it queues the events themselves), and moves the
  cursor, so each event is delivered once; pulls run every second, at once
  after a full page.
- *Checks* (`sofie check`): an app that lists a module as remote may not also
  depend on that module's implementation crate (`remote-dependency`), and
  no function of its modules may take `dyn Trait` of a remote contract
  trait that has a method unable to cross apps (`remote-call`), since only
  traits whose every method crosses are provided remotely.

### Screens: one template format, rendered at runtime

Views and pages become one thing: a template made of catalog components,
compiled by the app's build script, loaded at boot and rendered by the engine.

```html
<!-- src/areas/console/screens/orders.html -->
<Table id="orders" of={orders} columns="account status total created_at" />
```

- **Runtime rendering, one path.** Templates are parsed into a program of
  ops and rendered by walking it. The app's build script (`sofie check`'s)
  parses each screen and writes its program to `OUT_DIR` as data
  (`compile_template`); the wiring embeds each screen as one `File`, the
  template's path and the program's bytes (`embed!(template, compiled =
  "screens/<key>.sft")`), and `load_template` turns the bytes back into the
  program, so a built app compiles no parser. The
  format is hand-written: the header `sft` and a format version (2), then
  tagged ops and expressions with LEB128 lengths; components are kept by
  name and props and built by the runtime catalog, so a component or
  function the app's features leave out, or another format version, stops
  boot naming the screen and asking for a rebuild. Each `t(…)` call carries
  its source-language text (`lang/en.toml`), passed in by the build script
  (`compile_template_with_texts`): a build without `i18n` writes it with its
  arguments filled in (`one` where the count is 1), one with `i18n` where the
  translations lack the key. Development (`sofie-http`'s
  `development`) reads and parses the template at the `File`'s path instead, reloading it
  without compiling Rust. Without `i18n`, `t("key", …)` still parses, and a
  development build that rereads a screen or mail from its source
  (`parse_template_with_recorded`) takes the texts from the program the build script
  compiled (a key added since then is written as the key until the next build).
  `number(…)` and `money(…)` need `i18n`; without it a template using them is
  refused, naming the feature. Without `i18n`, `date(…)` writes an instant as a `Table`
  cell does (ISO day and minute on the display zone's wall clock, with the offset before
  the zone's name where it repeats), and writes a day or a wall clock as it reads. Built-in screens generated at runtime (resources)
  parse too, behind sofie-http's `parse`. There is no generated SSR code, no
  interpreter beside it and no suite proving the two agree.
- **Data read where it lies.** The engine reads bound data through
  `sofie_core::Shown`, a borrowed view: text, numbers, booleans, lists and
  records, with money a record of `minor` and `currency`. `#[derive(ReadModel)]`
  writes each read model's fields as a static table (`Record::FIELDS`, the
  same names and order its JSON has, `<field>_to` included), so a screen of
  read models builds no JSON tree and copies no text. A reply holding data
  that is only `Serialize`, or `Reply::Json`, is read from its JSON instead,
  with the same output.
- **Checked at generation.** `sofie gen` checks every binding against the loader's
  return type and the schema, so a typo is still caught before running.
- **Resources need no files.** `routes.resource::<Order>()` renders index,
  show and form screens from the aggregate's field metadata through the same
  catalog. `sofie scaffold screen order/console/order_index` writes the template out when a
  screen needs changing.
- **HTML over the wire.** Sorting, filtering, paging, forms and actions post
  to the server, which answers with the HTML fragment to swap in. htmx does
  the swapping, used only inside catalog components; the engine's own client
  is a few lines of local state and island mounting. Svelte islands carry
  everything richer; any other custom element can be mounted without support.
- **Labels from names.** A field's label is its name humanized; language
  files hold only what differs.
- **Translations, behind the engine's `i18n` feature.** An app speaking one
  language compiles none of it. With it, each module's `lang/<language>.toml`
  (tables, dotted keys and double-quoted strings, read without a TOML
  library) is loaded into one `Translations` declared with the app's
  languages, source language first; the engine's own texts (empty table,
  notices, `validation.*`) ship in en, de and tr, and an app's text replaces a
  framework crate's. engine.js's failure notices (`notice.unreachable`,
  `notice.failed`, `notice.passkey_*`) are written by the page shell as
  `data-*` attributes on `#sofie-notice` in the page's language, only where
  the text is not English, the script's fallback. sofie-identity ships
  `lang/en.toml` (source), `de.toml` and `tr.toml` under `identity.*`, and
  its screens use `t()` throughout. Templates write `{t("orders.count", count = data.n)}`
  and `{number(…)}`, `{money(…)}`, `{date(…)}` where text is written, never
  as a value, URL or id. A number argument of `t(…)` is written in the
  language's conventions. A key may instead hold plural forms `<key>.one` and
  `<key>.other`: a numeric `count` argument picks `one` by CLDR's rules for
  whole counts (1 in en, de, pl, he, sw, ar and the like; 0 or 1 in fr, pt,
  hi, fa; 1, 21, 31 but not 11 in ru, uk, hr, lt; never in ja, zh, ko and
  the like, where `Translations::gaps` reports a `one` form as never shown;
  a language whose rule the table cannot say, as sl or fil, takes `other`
  unreported), falling back to `other`, then to the
  plain key. `one` is read only from the viewer's language, never the
  source's, so a language picking `one` without it writes its `other` and
  `gaps` reports it missing; a language without a `one` form leaves it out. `date(…)` of a local date-time
  writes its wall clock and zone (`Oct 20, 2026, 09:30 Europe/Istanbul`), not
  UTC; of an instant, the display zone's wall clock and that zone's name
  (see [Config](#config)). Labels look up `label.<field>` and enum variants
  `enum.<enum>.<variant>`, falling back to the humanized name, so the source
  language needs no file for them. `Translations::negotiate` picks the
  language from the principal's choice, then `Accept-Language`; its
  conventions (en, de, tr; others neutral) format numbers, amounts and days.
  Until an app declares languages, compiling the feature in changes nothing:
  pages render in English with the neutral formats (`1234.50 EUR`).
  `check_translations` refuses a `t(…)` key the source language lacks or
  whose arguments are not its placeholders, and a call without `count` on a
  key that has only plural forms; `sofie check` runs it on every
  screen as the app is built, and a key missing at render is written as the
  key. `Translations::gaps` is the parity check
  `sofie check` runs; `Translations::keys` is what `sofie gen` writes as constants.
  `<Badge value={…} enum="order_status" />` and a select whose quoted
  `options` names a registered enum show translated variants with their tone.
  The `sofie` facade forwards the feature as `i18n` (sofie-http's `i18n`). `sofie gen`
  then declares the app's languages from the `lang/*.toml` files present
  (`en`, the language names are written in, first), embeds every module's
  files and sofie-identity's own, and hands them to `Screens::translate` (the
  screens were checked against them as the app was built). Each request
  negotiates its language from
  `Accept-Language` (pages vary by it); screens render through `Localized`,
  `Page.lang` is the negotiated tag, and the notice of a refused form, its
  field messages (`validation.*` keys) and their labels are translated, and
  so are a list's refused filters on the page and the messages of a
  handler's JSON 422; the currency box's label is `field.currency`. `sofie
  check` reports a file that does not load, a `t(…)` call that cannot render
  and every gap. An app without language files compiles none of it.

### Clients: screens compiled for Svelte at build time

An area is served as HTML over the wire unless it names a client:
`const CLIENT: Client = Client::Svelte;` in its `impl Area`. The same
templates then become Svelte components when the app is built; nothing about a
screen, a route or a loader changes. `examples/clients` publishes one screen in
a Svelte area and an HTML area side by side.

- **One description of the catalog.** Each component is declared once in the
  engine (`catalog::SPECS`): its name, what it is for, each prop with its kind
  and whether it is required, its children and the feature that brings it.
  Every prop the parser reads is held to it (a debug assertion, so the
  engine's own tests catch a drift), and `sofie components`, `explain
  <Component>` and the MCP `components` tool answer from it.
- **Compiled by the build script, beside the HTML program.** For a client
  area the build script (`sofie-cli`'s `clients`) parses each of the area's
  screens into the same ops the server renders, reads the elements back from
  them (the parser wrote every element in one form) and writes a `.svelte`
  component taking `data` and `view` (`compile_client`, the engine's
  `client-compile` feature). Expressions keep their meaning through the
  runtime `S`: a missing path is empty (`data?.a?.b`), truthiness is the
  engine's, the framework escapes text, a URL attribute is checked for its
  scheme. Catalog components become `<Catalog name=… props={…}>` with their
  props evaluated, a table's row content a snippet. esbuild bundles the
  screens, the runtime and the server's currency table (minor digits and
  symbols from sofie-values and the engine) into `<area>.client.js` and
  `.css`, precompressed and embedded through `client_bundle!`, which also
  names the screens the bundle holds. A template a client cannot compile
  fails the build naming the file: `Markdown` and `Head` render only in HTML
  areas.
- **Framework screens join the bundle.** The model reads each screen
  sofie-identity ships with a route (`.shipped(…, (include_str!(file), …), …)
  .name(handler)`) and which route functions the app mounts in which area, so
  the screens mounted in a client area (sessions, passkeys, two-step sign-in,
  connected accounts, account data, tokens, flags, settings, shield,
  impersonation) are compiled into its bundle under the key they render as.
  The role screens of `permitted_routes` render from `Role`'s and
  `RoleGrant`'s descriptions; sofie-identity keeps the same source as template
  files (`permitted/screens/role_*.html`, `{prefix}` where the area's prefix
  goes, held to the rendered source by a test), which the build script
  compiles for the area. An app's own replacement keeps its key; a framework
  screen a client cannot render stays an HTML page instead of failing the
  build.
- **One runtime, the catalog written once.** `crates/sofie-cli/web/client/core` builds
  each catalog component as an element tree written as the engine writes its
  HTML (fields of every control, tables, listings, forms, money, badges,
  layout, islands), formats with what the envelope describes, and runs the
  area in the browser: links and the back button load envelopes, forms and
  actions post with an `Idempotency-Key` until answered, a refused form
  keeps what was typed and shows its field errors, and a screen the bundle
  does not hold, or a request to sign in, loads as a whole page. It acts on
  what `engine.js` acts on in an HTML page's markup: a `data-sofie-passkey`
  button runs the same WebAuthn ceremony and posts the same answer, and a
  `data-sofie-autosubmit` form posts once its screen is shown.
  `crates/sofie-cli/web/client/svelte` only renders those trees and mounts the screens,
  so one stylesheet serves both kinds of area. Islands stay custom elements
  loaded from the area's islands bundle.
- **The screen envelope.** A page navigation to a screen the area's bundle
  holds answers the page shell (head, CSP nonce, stylesheets, bundle) with
  the first envelope as inert JSON (`<script type="application/json">`).
  Later loads and posts send `Accept: application/vnd.sofie.screen+json` and
  get the envelope alone: `screen`, `title`, `language`, `data` (the
  loader's JSON as a JSON caller gets it: `null` for each path bound only
  inside parts gated off for the viewer, nested, per item or under a value
  read whole there, and a boolean for a path only tested outside them),
  `navigation` (the area's menu as the viewer may follow it)
  and `view`, which the engine describes for the request (`client_view`): the
  gates the template asks, answered for the viewer; the messages it shows in
  the negotiated language; the language's formats and the display zone
  (`conventions.zone`), on whose wall clock the runtime formats `date(…)`
  and fills `datetime-local` controls; each form's fields and
  submit buttons as the input type describes them; the enums it shows. A post
  is answered by the route's view: a redirect is followed (303), `then_screen`
  answers the screen's envelope, a refusal 422 with its notice and field
  errors, each with `field`, `key`, `arguments` and the translated `message`;
  a listing's refused filters in `data` have their `message` translated too. The JSON API (`application/json`) is unchanged. Behind
  sofie's `clients` feature.
- **Browser telemetry as on a page.** The shell carries the same
  `sofie-telemetry` meta an HTML page gets, and the runtime sends what
  `engine.js` sends (vitals, long tasks, the page load, errors), adds each
  screen it pushes into history as a `client` navigation, and joins its
  requests to the page's trace with a child `traceparent`.
- **Rebuilt by `sofie dev`.** The development app gets each client bundle built
  beside its islands, and a template edit rebuilds it before the page is told
  to reload, so the shell loads the new screen from `/_sofie/dev/clients/`.
- **Enabled where it is used.** `sofie check` (`client-feature`) names an app
  with a client area whose `sofie` dependency lacks the `clients` feature,
  instead of leaving it to a compile error in the wiring.
- **Held to the server.** A slow test compiles screens covering every
  component and field control, renders them with Svelte on the server and
  compares the HTML with the engine's for the same template and data.

Not covered yet: React, as the web toolchain carries no React adapter
(decided 2026-10-15: Svelte is enough for now); sign-in pages are in `Web`, an HTML area; the telemetry
endpoint counts a `client` navigation under its catch-all name until it lists
that name.

### Telemetry: in the generic paths

Spans, metrics and log context are opened by the handler adapter, the screen
renderer, the store, the message worker and the gateway and contract
wrappers. Since every call
already passes through them, there is nothing to generate and nothing to
annotate, except `#[span]` on an inner step (feature `spans`): the function's
body runs in a `step` span named after its path, without its arguments.

Metrics follow the same rule as log lines: a request only sends a
measurement (instrument, up to four `Label`s, a value) into a channel; the
`sofie-metrics` thread drains it every 50 ms rather than waiting on it (a
waiting receiver costs each send a lock and a wake-up), adds them up into histograms and counters, renders the
Prometheus text a scrape asks for and, with `otlp`, encodes the same
aggregates as a cumulative OTLP snapshot every 60 seconds and at shutdown.
A `Label` is a `&'static str`
or a status code, so an id or a path cannot be one. Busy connections are an
atomic count read when the metrics are rendered; the outbox's depth is
polled: every 15 seconds the metrics thread has a running worker's runtime
count the waiting messages, never waiting on the query itself.

With `otlp`, `sofie-telemetry` exports traces, logs and metrics itself:
OTLP/HTTP with protobuf bodies to
`OTEL_EXPORTER_OTLP_ENDPOINT` + `/v1/traces`, `/v1/logs` and `/v1/metrics`,
over `hyper`, with `rustls` and the webpki roots for `https://`. It reads
`OTEL_EXPORTER_OTLP_HEADERS`, `OTEL_EXPORTER_OTLP_TIMEOUT` (milliseconds,
10 s by default), `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES`. Each
signal has a bounded queue that drops its oldest item when full and counts
the drop (spans 2048, logs 2048, metrics one snapshot); a task per signal
sends a batch of up to 512 when one fills or the interval ends (spans 5 s,
logs 1 s), retries 429, 502, 503, 504 and unanswered requests up to four
times with backoff or `Retry-After`, and empties the queue at shutdown.
Spans and records are masked like log lines before they are encoded.

A request only records an event's fields; a thread of its own formats the
line and writes it to stdout through a buffer, so no request waits on stdout
or on another request's line. The queue never blocks: past its capacity info
lines are dropped and counted (the count is logged as a warning), while
warnings and errors are queued, in order, up to a bound four times larger;
past it they are dropped and counted too, so memory stays bounded and a lost
error still shows as an error line. `SOFIE_LOG_SAMPLE` keeps the
info lines of that share of traces, decided from the trace id;
`SOFIE_TRACE_SAMPLE` does the same for exported spans, and a span's
attributes are built on the exporter task, not the request's thread. Trace and
span ids, request ids and nonces draw operating-system randomness a block
at a time (`sofie_core::random`), one system call for many ids.

### Config

The process's own settings, `DATABASE_URL` and `SOFIE_ADDRESS` (default
`127.0.0.1:8080`), `SOFIE_TRUSTED_PROXIES` (none by default) and `SOFIE_REQUEST_ID_HEADER`
(`x-request-id` by default), read only with the `trusted-proxies` feature (on by default), `SOFIE_MIGRATE`
(`on` migrates at start; off by default), with `SOFIE_SIGNING_KEYS` for
sessions and signed links (the old name `SOFIE_SESSION_KEYS` is still read,
with a warning at start; both set and different is refused), and the tuning keys `SOFIE_RUNTIME_THREADS`,
`SOFIE_DB_POOL_SIZE`, `SOFIE_DB_POOL_WAIT_MS`, `SOFIE_WORKER_BATCH` and
`SOFIE_WORKER_POOL_SIZE` (each a whole number above 0, a default when unset),
and `SOFIE_TIME_ZONE`, are read by `sofie::run`, so an app's `main` is the
one call `sofie::run(MODULES, app, worker)`; its arguments choose the role
(`serve --role web | worker | all`) or, built with `development`, `db fresh`. An app with feature flags
calls `sofie::run_with_flags` instead, which reads `SOFIE_ENVIRONMENT` too, starts
one `Flags` for the app and its worker, and answers `flags settled`.
Postgres is reached over TLS (rustls, public roots) as the url's `sslmode` asks:
`disable`; `prefer`, the default, encrypting when the server can; `require`,
which like libpq checks no certificate; `verify-ca`, a certificate signed by a
public root; `verify-full`, one also naming the host.
This is `sofie`'s `postgres-tls` feature (sofie-postgres's `tls`), on by
default. An app built without it leaves out `rustls`, `tokio-rustls` and
`webpki-roots`: `disable` and `prefer` (or no `sslmode`) then connect in plain
text, as libpq does without TLS, and `require`, `verify-ca`, `verify-full` or
any `sslrootcert` is refused at boot naming the feature, never sent in the
clear.
Instants are stored and sent as JSON in UTC; people see them in the app's
display zone, `SOFIE_TIME_ZONE` (an IANA id such as `Europe/Istanbul`; UTC
when unset; startup fails on a name that is not a tzdb zone a browser shows,
such as `posix/…`, `right/…`, `Factory`, `posixrules`, `localtime` or
`Etc/Unknown`), held as `sofie_core::DisplayZone`. `date(…)` and
`datetime-local` controls show an instant on its wall clock (to the
minute, or with its seconds and fraction and `step="any"` when they are not
zero; a fraction finer than milliseconds is shown as text with its
offset), and `Instant::parse` reads a `datetime-local` value back in it, a
fraction included. A wall clock a forward shift skips is read where the
shift ends (one rule with `LocalDateTime`, `LocalJobs` and wall-clock
schedules); one a backward shift repeats is refused as a field error naming both
offsets, so the user enters the time with the one they mean, and an
instant whose wall clock repeats is shown in a text box holding it with its
offset (`2026-10-25T02:30:00+01:00`), so a form re-saves unchanged. A
posted value shown again after another field fails keeps the control its
text needs: an offset or a repeated wall clock a text box, seconds
`step="any"`. Where the wall clock repeats, `date(…)` and table cells write
the offset before the zone's name (`2026-10-25 02:30 +02:00
Europe/Berlin`), in a client area too, which finds a repeated wall clock by
comparing the offsets three hours either side: it covers shifts of up to
three hours in whole minutes, and misses a longer fold or a zone whose
offset has seconds. A listing's instant filter given with an offset is a
text box too. A browser
whose time zone data lacks the zone shows UTC, labelled UTC, and an
instant's control and a listing's instant filter become text boxes asking
for an offset; the form's box holds the UTC instant. A `<Table>` cell
holding a typed instant (`Shown::Instant`) is written as `date(…)` writes
it; text written like an instant stays text, and a client area learns
which cells are instants from the envelope's `instants`. The zone is fixed
the first time it is read: production installs it at startup before
anything reads it. Tests set the zone with
`sofie_testing::set_display_zone`, called first by every test in the
binary: one zone per test binary, so a test in another zone goes in its own
file under `tests/`. There is no per-viewer zone yet. The `time` feature
also holds `TimeOfDay` (to the minute, stored as `HH:MM` text, `<input
type="time">`) and `Weekday` (stored by name like an enum, `MONDAY`…;
labelled through the built-in `weekday` enum, `enum.weekday.<day>`, in
en/de/tr; use `<Badge enum="weekday">` / `<Field options="weekday">`; an app enum named `Weekday` replaces the built-in one, as an app's texts replace a framework crate's).
A module's config struct derives `sofie::Config`, which describes its fields
(key, optional, kind, the `Database`/`Queue` fallback) and builds it from
parsed fields; `sofie::config` (feature `config`) reads and parses the module's
prefixed environment keys through each field type's own parsing, without
serde, once, when the generated wiring builds the app, and hands it to
handlers and jobs as `Config<OrderConfig>`. An adapter's `Settings` derives it
too. `sofie gen` reads the
same struct to write `.env.example`, and `sofie config check --env <name>` to
check `.env.<name>`. Both know a module's gateway keys too: `<MODULE>_<GATEWAY>`
choosing the vendor (needed when there are several), each vendor's
framework keys (`<MODULE>_<VENDOR>_BASE_URL`, timeouts, retries, …) and the
fields of its adapter's `Settings`, needed when it is the only vendor.
The framework's own keys in the app's `.env.example`, all optional unless
noted, with the feature that reads them:

- Always: `DATABASE_URL` (required), `SOFIE_SIGNING_KEYS` (required),
  `SOFIE_ENVIRONMENT` (required when the app has flags), `SOFIE_BASE_URL`
  (where the app is reached; `SOFIE_MAIL_BASE_URL`, `SOFIE_BILLING_BASE_URL`,
  `SOFIE_OAUTH_BASE_URL` and `SOFIE_CONTENT_BASE_URL` override it for their
  package, and a package that needs a base URL takes either), `SOFIE_ADDRESS`,
  `SOFIE_TRUSTED_PROXIES`, `SOFIE_REQUEST_ID_HEADER`, `SOFIE_CORS_ORIGINS`
  (cors), `SOFIE_TLS_CERTIFICATE` and `SOFIE_TLS_KEY` (tls),
  `SOFIE_SHIELD`, `SOFIE_SHIELD_THRESHOLDS`, `SOFIE_SHIELD_SHARED` (shield),
  `SOFIE_METRICS_TOKEN` (metrics), `SOFIE_MIGRATE`, `SOFIE_TIME_ZONE`,
  `SOFIE_LOG`, `SOFIE_LOG_SAMPLE`, `SOFIE_TRACE_SAMPLE` (otlp),
  `SOFIE_BROWSER_SAMPLE` (browser-telemetry), `SOFIE_PRODUCT_EVENTS_KEEP_DAYS`
  (product-reports, 400 days by default), `SOFIE_RUNTIME_THREADS`,
  `SOFIE_DB_POOL_SIZE`, `SOFIE_DB_POOL_WAIT_MS`, `SOFIE_WORKER_BATCH`,
  `SOFIE_WORKER_POOL_SIZE`, `SOFIE_CACHE_STORE`, `SOFIE_REDIS_URL` (cache),
  `SOFIE_SEARCH` (`database` or `meilisearch`; another value, or
  `meilisearch` without the search-meilisearch feature, is refused at start),
  `SOFIE_SEARCH_MEILISEARCH_URL`, `_API_KEY`, `_PRIVATE` (search-meilisearch), `SOFIE_STORAGE`, `SOFIE_STORAGE_ROOT`,
  `SOFIE_STORAGE_S3_ENDPOINT`, `_REGION`, `_BUCKET`, `_ACCESS_KEY_ID`,
  `_SECRET_ACCESS_KEY`, `_PATH_STYLE`, `_PRIVATE` (storage, storage-s3),
  `SOFIE_BILLING_PROVIDER`, `_API_KEY`, `_WEBHOOK_SECRET`, `_BASE_URL`,
  `_PRICES`, `_API_URL` (billing), `SOFIE_NOTIFY_VAPID_PUBLIC_KEY`,
  `_PRIVATE_KEY`, `_SUBJECT` (notify: all three or none).
- With a mail template: `SOFIE_MAIL_TRANSPORT` (required), `SOFIE_MAIL_FROM`
  (required), `SOFIE_MAIL_BASE_URL`, `SOFIE_MAIL_SMTP_HOST`, `_PORT`,
  `_SECURITY`, `_USERNAME`, `_PASSWORD`, `SOFIE_MAIL_PREVIEW_DIR`,
  `SOFIE_MAIL_POSTMARK_WEBHOOK_AUTH`, `SOFIE_MAIL_MAILGUN_SIGNING_KEY`,
  `SOFIE_MAIL_WEBHOOK_SECRET`.
- With sofie-identity's `oauth`: `SOFIE_OAUTH_PROVIDERS` and
  `SOFIE_OAUTH_BASE_URL`; each provider also reads
  `SOFIE_OAUTH_<NAME>_CLIENT_ID` and `_CLIENT_SECRET`, Microsoft `_TENANT`,
  another OIDC provider `_LABEL`, `_ISSUER`, `_AUTHORIZATION_URL` and
  `_TOKEN_URL`.
- With sofie-content: `SOFIE_CONTENT_BASE_URL`, else `SOFIE_BASE_URL` (one
  of them required).

A test holds every `SOFIE_` key named in a framework crate to these lists.
The standard OpenTelemetry keys (`OTEL_EXPORTER_OTLP_ENDPOINT`, `_HEADERS`,
`_TIMEOUT`, `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`) are read by
otlp but not listed.

## What `sofie gen` writes

| Output | Kind |
|---|---|
| `modules/<module>/src/wiring.rs`: `GENERATED`, the `Module` the manifest spreads: migrations with their aggregates, area routes and menus, the module's route names, listeners as `on::<Event, _>` with the event read from the listener's first argument through its file's imports, schedules, `#[job]` actions and mirrors' listeners, named from inside the crate so none of them needs to be `pub` | Rust, one file per module |
| `app/src/wiring.rs`: `MODULES`, each package's `Module` and each app module's `MODULE` with the screens the app's build script compiled and their forms; `app` and `worker` built from it with what is the app's own: config loading, contract implementations, extensions and lookups provided to the `Db` (`provide`, which names `#[job]` actions), the route flag and permission tables, jobs a flag gates, and `#[task]` functions (`tasks()`, names and paths only) | Rust, one file per app |
| `contract/src/gen/`: DTOs of the fields an aggregate exposes and their `<Type>Changed` events, the module's route builders and, in an app with `Permitted` areas, its permission constants | Rust, because a derive cannot write into another crate |
| `.env.example` per module and for the app | Text |
| Template binding checks | Errors only, no files |
| `AGENTS.md` at the app root: its modules and areas, the commands, where each kind of code goes, the rules `sofie check` holds it to and how to add a module, route, screen or aggregate; one written by hand (without the generated header) is left alone | Text |
| `docs/gen/` (`sofie docs`, held to the code by `sofie check`), OpenAPI and TypeScript (`sofie openapi`, `sofie typescript`) | Text, from sofie-cli commands behind its features |
| k6 | Plugin, text |

A `Module` is data: the manifest (`name`, `tier`, `context`, `settings`),
`migrations`, `screens` (a key under another module replaces that module's
screen), `forms`, `areas` (each a `Mount`, given the module's name),
`route_names` and `jobs`. A module crate's `MODULE` in `src/module.rs` states the manifest and
spreads `..crate::wiring::GENERATED`; the app's wiring adds the screens and
forms, since the screens are compiled by the app's build script into its
`OUT_DIR` and a module crate has no build script. `sofie::run` migrates
`sofie::http::migrations(MODULES)`; the wiring's `app` loads the screens with
`wired_screens(MODULES, COMPONENTS)` and mounts the areas with `App::modules`;
its `worker` starts from `sofie::http::worker(db, MODULES)`.
Packages write theirs in the same shape: `sofie_identity::module(&[member::<A>,
…])`, signing in at each member area, `sofie_content::MODULE`,
`sofie_tenancy::MODULE` and `sofie::billing::MODULE` (`Module::new` is a
package's base: the platform tier, no context or settings). What needs the
environment, the app's plans or its started `Flags` stays in `app` and
`worker`: content's and billing's `install`, billing's routes, and a job a flag
gates.

## Dependencies

Listed so the size of the core is visible.

| Crate | For |
|---|---|
| `tokio` | Async runtime |
| `hyper`, `hyper-util`, `http` | HTTP server and client; `Url` checks addresses with `http`'s `Uri` |
| `matchit` | Routing |
| `rustls`, `tokio-rustls` | TLS for the server, the client and Postgres (`postgres-tls`) |
| `flate2`, `brotli` | Response compression and precompressed engine assets (`compression`) |
| `mimalloc` | Global allocator (`mimalloc`, opt-in) |
| `tokio-postgres`, `deadpool-postgres` | Database driver and pool |
| `serde`, `serde_json` | Serialization |
| `syn`, `quote`, `proc-macro2` | Derives and `sofie gen` |
| `jiff` | Instants, dates and IANA time zones (read from the system zoneinfo database, so an image must ship it); log timestamps |
| htmx (browser) | Fragment swapping inside catalog components |
| Svelte 5.57.2, esbuild 0.28.2 (build toolchain, `crates/sofie-cli/web`, run by Node) | Building islands, only for apps that have them; only the bundles reach the binary |
| `tracing` | Telemetry; the subscriber is `sofie-telemetry`'s own, and so is the OTLP exporter behind `otlp`: spans built from its span store with the traceparent ids, log records from its log lines on the writer's thread, encoded as protobuf by hand and sent with `hyper` |
| `ring` | Session encryption and signing, content hashes, checking passwords hashed before Argon2id, and later TLS through `rustls` |
| `argon2` (RustCrypto, `alloc` and `password-hash` only) | Password hashing in sofie-identity: Argon2id, 19 MiB, two passes, one lane, PHC strings; brings `blake2`, `base64ct`, `password-hash` and `phc` |
