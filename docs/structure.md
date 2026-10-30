# Structure and Glossary

> Status: **proof of concept, partly built**; nothing here is promised to anyone
> yet.
>
> This is a general-purpose framework. Examples use a small online shop —
> catalog, orders, payments — only because it is widely understood; no name,
> currency, vendor, language or rule in them is part of the framework.
>
> This document is the contract every app is built against: what goes where,
> and what the framework does with it. [core.md](core.md) is how the framework
> does it with the least code. Change this document before changing code.

## Promise

An app written in this structure gets features without writing them. The
structure is fixed so the framework can read the whole app and derive wiring,
endpoints, screens, permissions, telemetry, documentation and checks from it.
Every rule below exists because something is generated or enforced from it; a
rule that feeds nothing does not belong here.

## Principles

1. **A role exists only if the framework acts on it.** It wires, generates or
   checks it. Everything else is plain private Rust inside a module, with no
   framework vocabulary.
2. **Location before annotation.** A file's place says what it is. An
   annotation carries only what the place cannot: a schedule, an event to
   react to.
3. **Generate data, never behaviour.** Behaviour is generic library code,
   written once. A derive describes one type to that library — its table,
   columns, fields — and nothing more. `sofie gen` writes only what no single type
   can know: the app's wiring, what contracts export, and everything that is
   not Rust. Where a type or module privacy can enforce a rule, nothing is
   generated for it.
4. **No hidden runtime machinery.** No container, no reflection, no service
   locator. Wiring is plain construction, generated once per app.
5. **The compiler enforces what it can; `sofie check` enforces the rest.** A rule
   that can be a type or a crate boundary is one.
6. **Every piece of knowledge lives in one place.** The schema for data, the
   manifest for a module's identity, the area folder for what an audience may
   reach.
7. **The framework knows no business.** It ships mechanisms — areas, values,
   behaviors, flags, gateways, the engine — and never an audience, a currency,
   a vendor, a language or a domain concept. Every one of those is declared by
   the app.
8. **One app model.** `sofie gen` reads the app once into a single model of its
   modules, schemas, routes, areas, screens, flows and rules. Every generator,
   check, document, `sofie explain` and the MCP server read that model, so they
   cannot disagree.

## Glossary

One term, one meaning. Code, docs, CLI output and error messages use these
words and no synonyms.

| Term | Meaning | Not to be confused with |
|---|---|---|
| **App** | The deployable binary: a list of modules and configuration. Holds no business rule. | — |
| **Module** | The unit of ownership and of `split`. Owns its data, its rules and its screens. A pair of crates: the module and its contract. | "Domain", which is not used: it also means a DDD layer and a web address |
| **Tier** | A module's rank: `Experience` → `Core` → `Platform`. Dependencies only flow downward. | — |
| **Context** | A named group of Core modules that belong to one part of the business. Metadata for docs and for splitting several modules together; grants no extra access. | A runtime context object |
| **Contract** | The module's public surface: the traits other modules call, the data they receive, the events it publishes. The only thing another module may import. | A gateway, which faces vendors rather than modules |
| **Schema** | An aggregate's struct and its field attributes. The single source for columns, validation, DTOs, permissions, screen fields and search fields. | The database schema; that is the migrations |
| **Migration** | A hand-written SQL file. Checked against the schema, never generated from it. | — |
| **Aggregate** | A record and its child rows, saved and loaded as one. A folder of its own: its fields are private to that folder, so only its own methods and its factory can touch them. | — |
| **Read model** | A struct stating the shape a screen, API or report reads: columns of one aggregate, joined within the module or through mirrors. | An aggregate, which is for writing |
| **Mirror** | A module's local, read-only copy of fields another module exposes, kept current from its change events, so reads can join without crossing modules. | A cache |
| **Value object** | An immutable value that cannot exist invalid (`Money`, `Email`). Framework-provided or module-private. | — |
| **Cast** | How a type maps to one or more columns. Implemented by value objects and enums; the field's type is its cast. | A Rust `as` conversion |
| **Behavior** | A reusable bundle of columns, rules, scopes and methods an aggregate opts into by name in its schema (`SoftDelete`, `Slugged`). | A Rust trait, which adds methods only; a PHP trait is the closest relative |
| **Enum** | A closed set of cases with labels, tones and, for a status, its allowed transitions. | — |
| **DTO** | Data crossing a boundary: HTTP, a contract, the queue. Derived from the schema and handler signatures where possible. | A value object, which has rules; a DTO only has shape |
| **Factory** | The only code that can construct an aggregate: a child module of the aggregate's folder. Holds the invariants of creation; a default is derived when there are none. | A test data factory; that is a **fixture** |
| **Fixture** | Test and seed data built from schema defaults, always through the factory. | — |
| **Store** | The library's persistence for any aggregate: load whole, save whole when changed, check the version. Used only inside the aggregate's module. | A cache, a key-value store |
| **Action** | One state-changing use case, named verb + object (`PlaceOrder`). Returns the aggregates it changed, which are committed in one short transaction after it returns. Knows nothing of HTTP; an area exposes it as an endpoint. Can also be a job, a scheduled task or an event listener, by annotation. | — |
| **Query** | One read, named for the question (`DueOrders`). Gets a read-only connection, so it cannot write. | — |
| **Event** | A fact a module publishes (`OrderPlaced`), declared in its contract and delivered through the outbox. | — |
| **Outbox** | The message table events are written to in the same transaction as the change that raised them. The same table carries jobs and scheduled runs; one worker delivers all three after commit. | — |
| **Engine** | The framework's UI runtime: renders screens to HTML on the server and swaps HTML fragments in the browser in answer to interactions. | A frontend framework |
| **Component catalog** | The set of components screens are built from (`Section`, `Table`, `Form`, `Field`, `Submit`, `Money`, `Action`, `Badge`; `Grid`, `Card`, `Image` for layout), each with typed props, implemented once in the engine. Modules may add entries. | — |
| **Screen** | A template of catalog components, rendered by the engine at runtime. Every screen is one, from a record table to a marketing page. | Server-driven UI described in code; a screen is a file |
| **Island** | A component or a whole screen the catalog cannot express, written in Svelte and mounted by the engine as a custom element. Any other custom element can be mounted too, without the framework's support. | A screen; the engine renders a screen, but only mounts an island |
| **Client area** | An area whose `CLIENT` names a client framework (`Client::Svelte`): its screens' templates are compiled into that framework's components when the app is built, and it answers navigations with screen envelopes. | An island; an island is one component inside a page, a client area renders every screen of the area |
| **Loader** | The query that supplies a screen or island its data, typed so the screen's bindings are checked by `sofie gen`. | — |
| **Resource** | A schema aggregate exposed as a standard set of screens and actions (index, show, create, edit, delete) by one line in an area's `routes.rs`. | — |
| **Area** | An audience of the app: its principal, its access, its URL prefix, its sign-in path, its layout. The framework ships one, `Web`, the app's public face. The app defines the rest, each once, in an app module's contract; their principal is the aggregate it signs in as, `sofie_identity::Account`. Every module contributes screens and endpoints to the areas it may reach. | — |
| **Endpoint** | An action, query, screen or island exposed at a URL in one area. Listed in the module's `routes.rs` for that area. | — |
| **Route name** | `module.area.handler`, derived, never written. Typed URL builders in Rust and TypeScript are generated from it. sofie-identity, whose routes `sofie gen` cannot see, names its own the same way when it registers them; a sign-in route served in `Web` is named after the member area it serves, so each member area's routes keep names of their own (`sofie_identity.customer.sign_in`). | — |
| **Principal** | Whoever makes the request: a signed-in account, a visitor, the system. | — |
| **Owner** | The schema field marked `#[owner]` naming the principal a record belongs to, or `#[owner]` on the struct for a record that is its own (an account). Owned areas read and write only their principal's records through it. | — |
| **Permission** | `module.model.field.action`, generated from the schema and the actions; wildcards per axis. | — |
| **Gateway** | A trait the module defines for a third-party capability, with one adapter per vendor. | A contract; a gateway faces outward to vendors, a contract inward to modules |
| **Adapter** | A vendor's implementation of a gateway, with its client and vendor types in its own folder. Its client is given an `Outgoing` by the wiring; it cannot create one. | — |
| **Outgoing** | The framework's client for every request leaving the app: traced, limited and held to the vendor's allowed hosts. Only gateway adapters and remote contracts are given one. | — |
| **Extension** | An implementation of an extension point another module or the framework declares (a metric, a search source, an approval handler). Registered by the wiring. | — |
| **Manifest** | `module.rs`: the module's declaration of itself — name, tier, context, settings, and features with flags. The first file `split` reads. | — |
| **Config** | Values fixed for a deployment, read from environment variables once at boot. Declared by the module in `config.rs`. | A setting |
| **Setting** | A value changed at runtime, stored in the database, declared in the manifest. | Config |
| **Entitlement** | A switch, a limit ("at most N") or a quota per period a plan grants, named `<module>.<name>`; a route, a menu entry or part of a screen can require one (`sofie-billing`). | A permission, which a role grants; a feature, which a rule switches |
| **Plan** | A set of entitlements an owner holds by subscribing, by a trial or by default, declared once per app in a module's `src/plans.rs`. | — |
| **Topic** | A kind of notification a module sends, named `<module>.<name>`, with the channels (inbox, mail, web push) on by default; each principal chooses its own (`sofie-notify`). | An event, which modules react to |
| **Feature** | A flag declared in the manifest that gates routes, actions, listeners, schedules, menu entries and parts of screens; on per environment, area, principal or percentage, or split into variants. | A setting, which is a value rather than a switch |
| **Benchmark** | A load test generated from routes, inputs and fixtures, run with k6 against a seeded environment and held to a baseline. | A unit benchmark of one function |
| **Scaffolded test** | A test generated once from the current schema, routes and areas, named by the module and the moment it was written, and owned by the module from then on. Never regenerated, so earlier tests are never lost. | Generated code, which is regenerated and never edited |
| **Telemetry** | Traces, logs and metrics, exported over OpenTelemetry. Recorded by the library at every boundary; written by hand only for what a boundary cannot see. | — |
| **Product event** | A business moment (`order.placed`) counted in full, never sampled, for product analytics beside the technical signals. | A domain event, which other modules react to |
| **Flow** | The chain a piece of work follows, derived from declarations: entry point → action → aggregate methods → events → listeners → further actions and gateway calls. | A process, which is written; a flow is only drawn |
| **Generated code** | What `sofie gen` writes: the app's wiring, contracts' exported DTOs and route builders, and non-Rust output. Lives in `app/src/wiring.rs`, each module's `src/wiring.rs` and under `gen/`, committed, never edited, verified by `sofie gen --check`. | A derive's expansion, which is never written to disk |
| **Database seam** | The traits in `sofie-db` (`Driver`, `Dialect`, `Catalog`) that hold everything one database does differently. Postgres and SQLite implement them. | — |

## Layout

```
AGENTS.md                     generated: the conventions, for agents and people
app/                          the binary
  build.rs                    runs `sofie check` (a finding fails `cargo build`) and
                              compiles the screens and islands into OUT_DIR
  src/
    main.rs
    wiring.rs                 generated: `MODULES`, `app`, `worker`, route table
  tests/
    scaffolded/               `sofie scaffold tests`: order_20261011064009.rs, main.rs
modules/
  order/
    contract/                 crate `order-contract`, the only importable part
      src/
        lib.rs                traits other modules call
        events.rs
        area.rs               an area, only in the module that owns its principal
        gen/                  generated: exposed DTOs, route builders
    src/                      crate `order`
      lib.rs
      module.rs               manifest: the module declares itself
      wiring.rs               generated: what the manifest spreads (routes, jobs, migrations)
      config.rs               its configuration and the environment keys behind it
      order/                  one folder per aggregate
        mod.rs                the struct (its schema) and its methods
        factory.rs            optional; the only code that may construct an Order
      actions/                one file per action
      queries/                one file per query
      areas/
        customer/
          routes.rs           this module's endpoints in the area
          menu.rs             its menu entries in the area
          screens/            templates: name.html, for what is not a resource
          islands/            Svelte components: name.svelte
        admin/
      gateways/
        payment.rs            the gateway trait
        stripe/               adapter, client, vendor types
      extensions/
      domain/                 private code: value objects, calculations, helpers
    migrations/               NNNN_name.sql
    .env.example              generated from config.rs
    mails/                    <name>.html: mail templates (sofie's `mail`); <crate>/<name>.html replaces a crate's own
    lang/                     <language>.toml, one per language the app declares
    seeds/                    <area>.<handler>.json: inputs `sofie seed` posts to that route
    bench/                    hand-written journeys; generated scripts land in bench/gen/
    web/                      the module's own island components, in every area's bundle, optional
web/
  gen/types.ts                `sofie gen`, an app with islands: the types its islands' props are
```

A module needs only `module.rs` and one aggregate folder to start. Every other folder
appears when its first file does.

## Roles and what each one buys

| Role | Location | The framework generates or enforces |
|---|---|---|
| Manifest | `module.rs` | Module registry, tier and dependency checks, feature and setting catalogs, architecture docs, the boundary `split` cuts along |
| Config | `config.rs` | Typed config struct, boot-time validation, the module's `.env.example`, injection into the module's handlers |
| Aggregate | `<aggregate>/mod.rs` | Through its derive: columns, typed column constants, field metadata. From that, in the library: load and save, validation, input DTOs, permissions, form fields and table columns, fixtures, `schema:check`. Through `sofie gen`: contract DTOs for exposed fields |
| Read model | beside the queries that use it | One SQL statement with its joins, decoding, screen binding |
| Migration | `migrations/*.sql` | Applied in order; compared with the schema by `schema:check`; held by `sofie check` to not lock a table the running version uses and to name no other module's schema. `sofie gen` writes `0000_module_role.sql` once: the module's database role |
| Factory | `<aggregate>/factory.rs` | The only constructor, enforced by module privacy; derived by default when absent |
| Action | `actions/*.rs` | Arguments resolved by type, the commit of what it returns, span and metrics, and by annotation: queued job, schedule, event listener |
| Query | `queries/*.rs` | Read-only connection (a type, so writing does not compile), screen data binding |
| Area definition | `contract/src/area.rs` | See [Areas](#areas) |
| Routes | `areas/<area>/routes.rs` | See [Routes](#routes) |
| Menu | `areas/<area>/menu.rs` | The area's navigation, linked by route name |
| Screen | `areas/<area>/screens/*.html` | Rendering, bindings checked against its loader, translation keys, cache keys from the queried aggregates |
| Island | `areas/<area>/islands/*.svelte` | Build and hot reload, props typed from its loader, mounting, one bundle per area |
| Resource | one line in `areas/<area>/routes.rs` | Index, show, create, edit and delete screens rendered from the aggregate's field metadata with no file, and their actions through the factory |
| Contract | `contract/` | In-process wiring today; the remote client and server when the module is split |
| Event | `contract/src/events.rs` | Outbox write, delivery, retries, listener registry |
| Gateway / Adapter | `gateways/` | The configured vendor chosen at boot, span and metrics per vendor call, test fake |
| Extension | `extensions/` | Registration with the extension point |
| Translations | `lang/` | Labels derived from field names, so files hold only what differs; parity check across languages; behind the `sofie` facade's `i18n` feature; `sofie gen` embeds the files and `sofie check` reports gaps |

## Screens and the engine

Every screen is a template of catalog components, rendered by one engine the
framework owns. No frontend framework's release can break an app.

```html
<!-- modules/order/src/areas/console/screens/orders.html -->
<Table id="orders" of={orders} columns="account status total created_at" />

<!-- modules/content/src/areas/web/screens/home.html -->
<Section title={t("home.title")}>
  {#each products as product}
    <Card href={product.link}>
      <p>{product.name}</p>
      <Money value={product.price} />
    </Card>
  {/each}
</Section>
```

- **One format for every screen.** A record table and a marketing page are
  the same kind of file; the second uses plain HTML and layout components
  (`Grid`, `Card`, `Section`, `Image`, behind the engine's `layout`
  feature) where the first uses `Table`. A `Card` with `href` is a link, so
  a link, button, form, `Action`, `Table`, `Island` or `Markdown` inside it
  is refused when the template is built.
  Components exist only for what HTML cannot do alone: binding data,
  posting with htmx, formatting money. A `Table`'s children are rendered in
  every row, with the row bound to its `as` name. A `Form` is refused inside
  `{#each}` or a `Table` row, as its id would repeat; picking one of N items
  is an `Action` posting fixed fields from its row: `<Action
  post="/slots/book" label="Book" with={{slot: slot.id}} />` renders a form
  whose hidden inputs post each field (without JavaScript too), and a client
  area posts the values as JSON, so the route's input reads them like any
  posted field. Each value is a path, a string or an integer, checked by
  `sofie check` like any binding and sent to a client area only where the
  action is shown; `sofie check` (`action-input`) also finds a `with` key the
  posted route's input has no field for, and a required field it does not
  post.
- **Authored markup stays as written.** Literal text and quoted attribute
  values are the author's HTML; only `{…}` values are escaped. Templates
  cannot carry `<script>`, `<style>`, `<meta>`, `<base>`, `<link>`,
  `<iframe>`, `<object>`, `<embed>`, SVG `<animate>`, `<set>` and
  `<animateMotion>` (they can set a link's URL), `style` or `on…`
  attributes: the page's head, scripts and styles come from the engine,
  under its strict CSP. A template's `<title>` names its page: written
  once, without attributes, at the template's top level and holding only
  text and `{…}`, it replaces the title made from the screen's name in the
  page's head (in a Svelte client area, through `<svelte:head>`). A
  `<title>` inside an `<svg>` names that image and stays in the body; a page
  is named by `<title>` or by `<Head title=…>`, never both.
- **Resources need no file.** `routes.resource::<Order>()` (the `sofie`
  feature `resources`, the trait `sofie::http::Resources`) renders its index,
  show, create and edit screens from the aggregate's description: the
  fields of its `CreateOrder` (their controls and constraints from
  `Present` and the schema), its JSON (the derive's `Serialize` under the
  feature), its sortable and filterable columns. The screens are template
  source the engine parses at boot (so `resources` brings in the parser),
  keyed `<name>_index`, `_show`,
  `_create` and `_edit`; a module overrides one with
  `screens/order_index.html`, and `sofie scaffold screen order/console/order_index`
  writes the source out. With `history`, the show screen lists the record's `sofie.history`
  rows (change, fields, actor, action, time). The show screen writes money through `Money`, a percentage as
  `12.50 %` and a value of parts (a measurement, coordinate or local
  date-time) as its parts. The edit screen shows a password or sensitive field empty and not
  required, and a blank one keeps what is stored (when the record's JSON
  holds it).
- **Lists page, sort and filter through fragments.** A loader taking
  `Listing<T>` (the query's `page`, `size`, `sort=-total` and filters such
  as `status=PAID` or `total.gt=100`, each checked against the fields
  the read model `T` selects, never a `#[sensitive]` one, and the
  operators their column type offers; a value is read as its type reads
  JSON text, so `rate.gte=19` is 19 %, a value the type refuses is that
  filter's error, and only a type without a text reader is read by its
  column type; a field and operator twice, or more than 16 filters, is a
  400) answers `Listed<T>`, `{items, page, size, has_next, sort,
  filter, query, controls, errors}`; a `Table` over it with `sort="…"` and
  `filter="…"` links each column, a filter form (a select of an enum's variants, a date-time
  picker for an instant, else a search box) and the neighbouring pages,
  every link fetching the table's own fragment (the `listing` feature; both
  types are `sofie::http::Listing` and `sofie::http::Listed`). A loader
  taking `Listing<T, Q>` also reads the query's own parameters into `Q`, a
  `#[derive(Input)]` struct (`Listing<T>` is `Listing<T, ()>`): a key naming
  a field of `Q` is read into it (a nested field from dotted keys,
  `budget.amount=5&budget.currency=EUR`, as `Query<Q>` reads it), held to
  its constraints, and
  `listing.parameters()` answers it (none when refused), which
  `listing.where_(condition)` narrows the list by. A field of `Q` named
  `page`, `size`, `sort` or `filter`, or `id` or a field of the listed
  aggregate (which would hide that field's filter), fails the build. A refused value is
  listed under `errors` by its field, like a refused filter (a 422 for a
  JSON caller); `Listed` answers the asked values under `query`
  (`data.query.near`), and the Table's sort, filter and page links keep
  them, as they keep a `size` other than the default 20. A filter
  value its field refuses, of any type (such as an instant without an
  offset in a repeated hour, or a malformed range), is listed under
  `errors` by its key with nothing in `items`. It reads in neutral words
  when the value is not of its column's kind ("must be a whole number",
  "must be an id" for the `id` column and a `Uuid` or `Id<A>` field alike,
  "must be true or false", "must be a date"…), and in the
  type's own words otherwise. Each error carries its `validation.*` key and
  arguments (`validation.kind.whole_number`, `validation.kind.id`,
  `validation.one_of` with `values`, `validation.range` with `value`,
  `validation.instant.*`), so it is shown in the viewer's language. `errors` holds each as
  `{key, arguments, message}`, in JSON as on a page; a client area's
  envelope writes each `message` in the viewer's language. The `Table` shows it beside that filter, an
  instant's as a text box that takes the offset; an error with no filter
  beside it (an operator filter such as `total.gt`, or a Table without
  `filter`) is shown above the table as `key: message`. A JSON caller, and
  every caller of a JSON route, gets the errors as a 422, as
  `Listing::query` answers, in the request's language and each with its
  `key` and `arguments` beside the message. An unknown field, operator or sort, or a
  `page` or `size` that is not a whole number or is out of range, is a
  400. A parent id in the path that its field refuses is not found.
- **Logic stays in Rust.** Expressions are field access and formatting only;
  anything computed is computed in the loader.
- **Checked before running.** `sofie gen` checks every binding against the
  loader's type, so a wrong field is an error, not an empty spot. Loaders
  answer with read models (`#[derive(ReadModel)]`, or a `Vec`, `Option`,
  `Paginated` or `Listed` of one), computed ones too (without `from`, filled by the
  loader); a field holding a read model is checked through, and a field's own
  `<field>_to` states are known too. Data
  of any other type is a finding asking for a read model, since `sofie gen`
  reads types without compiling.
- **Rendered at runtime.** The app's build script compiles each template to
  a program of ops (data, not code) that the binary embeds and loads at boot
  without a parser; it is rendered by walking it. Development reloads a
  changed template from disk without compiling Rust. Read models, the
  framework's values and derived enums and values implement `sofie_core::Show`,
  so the engine reads their fields in place; a reply with data that is only
  `Serialize` renders from its JSON (see core.md).

### One endpoint, three answers

A screen is served the way Laravel serves a view with its data, not as a
frontend calling an API. The query returns data; the route names the template
that shows it:

```rust
// modules/order/src/areas/console/routes.rs
routes.screen("/orders", "orders", due_orders);
```

| The request | The answer |
|---|---|
| A browser navigation | The whole page as HTML |
| An htmx request from a catalog component (sort, filter, page) | Only that component's fragment: the engine renders the subtree of the template the component belongs to |
| `Accept: application/json` | The loader's data as JSON |

So every screen is also an API, with no second handler. Forms follow Laravel
too: a validation error re-renders the form with each message under its field
and answers 422; success redirects (post-redirect-get) to the resource's show
screen or to `.then(redirect(…))`, or returns the updated fragment to htmx.

A form's fields come from the input its route reads. `sofie gen` finds the route
each `<Form post="…">` of a module's screen posts to, and the `Input` its
action takes, and lists `Form::of::<order::actions::PlaceOrder>
("order/customer/orders", "place-order")` in the module's `forms` (the input must be
reachable from its crate, so `actions` is a `pub mod`). The form then shows
every field of the input, in its order, with its label and the control its
value's type presents; a `Field` the template writes keeps its props
(`label`, `value`, `options`, `currency`), so a template lists only what it
changes:

```html
<Form id="place-order" post="/account/orders" target="orders">
  <Submit label="Place order" />
</Form>
```

A development build gives a screen its descriptions again whenever it rereads
the file. A form posting to a path bound to data, like sofie-identity's sign-in,
is described by its module (`sofie_identity::FORMS`, in its `Module`).

When the template depends on the outcome, a handler may return
`Screen::render("empty_orders", data)` itself, typed `Result<Screen<T>>`: the
name is a screen of the handler's own module and area, and JSON callers get
`data` alone. `sofie gen` checks that the template exists and its bindings
against `T` like any other. `sofie check` reports a screen whose loader or
`Screen<T>` answers no read model (a `Vec`, `Option` or `Paginated` of one
counts), even when the template binds nothing yet. Binding through the route stays the default,
because it keeps queries pure and reusable as JSON.

A frontend calling an API exists only where it is needed: an island receives
its first data as props in the page and calls the same endpoints as JSON for
more, and external clients reach the same queries through an area with a token
guard.

### The supported path and the open boundary

The framework owns one way of doing each common thing and supports it fully;
anything else crosses a web standard and is the app's own.

| | Owned by the framework | Open to the app |
|---|---|---|
| Interactions | **htmx**, pinned and embedded, configured strictly (no `eval`, same-origin requests only, a strict CSP), extended by the framework for CSRF, `traceparent` and error responses. Its attributes appear only inside catalog components, never in an app's templates | — |
| Rich components | **Svelte** islands: compiled by the app's build script (svelte and esbuild, pinned in the framework's `crates/sofie-cli/web`), hot-reloaded by `sofie dev`, mounted as custom elements the framework defines, props typed from the loader, one bundle per area | Any other custom element — React, Vue, plain JavaScript — mounted by the engine, with no typed props, build or reload support from the framework |

An app without islands needs no Node toolchain at all.

### HTML over the wire

Sorting, filtering, paging, forms and actions send a request and get back the
HTML fragment to swap in, through htmx inside the catalog components. Nothing
is hydrated and screens need no JavaScript of their own. Small local state —
an open menu, a toggled panel — is a few lines in the engine's client: a control with `aria-controls` and
`aria-expanded` shows and hides the element it names, and Escape closes every
open one; anything more is an island. The client also turns a request with
no answer, or a 5xx page the server did not make (a proxy's), into a notice
instead of a broken swap.

### Live screens

With the `live` feature (sofie-http, forwarded by `sofie`), a screen route opts in
with `.live("fragment")`, once per fragment to keep current:
`routes.screen("/notes", "notes", list).live("notes")`. Its loader runs under
`sofie_db::track_topics`, which names every aggregate table the reads touched (the
base, joined tables, `in_select` tables; raw SQL names none, so such a screen
hears every change). The page gets a hidden `#sofie-live` element naming its
stream, `/_sofie/live/<area>?topics=<hashed tables>&until=<unix
seconds>&seal=<signature>`, and the fragments (the seal keeps a caller to the
topics a page it was shown reads, never `*` at will, and binds a signed-in
principal's stream to that principal; it ends 60 to 70 minutes after the page
was shown, on a ten-minute step so refetches within one name the same link); a
fragment answer carries it out of band, so a refetch that read other tables
listens to those. The engine's client opens an `EventSource` on it; on an
event it refetches each fragment through its own route with htmx, so every
guard, gate and owner filter applies again and nothing rendered travels down
the stream. A lost stream reconnects with backoff (1 s doubling to 60 s) with
the newest link its fragments brought, and refetches once open, as it may have
missed a change; a stream refused before opening once its seal's `until` has
passed, or twice in a row (a sign-out elsewhere, a key change, a skewed
clock), refetches the fragments at once, so the seal is replaced by a fresh
one.

A stream is authenticated like its area (session cookie, confirmed with the
session store under `session-list`; an owned area without a principal is
`401`), refused to another site (`Sec-Fetch-Site`), and hears a
change only when its principal may read the record: a write to an owned
aggregate is heard by its owner and by unrestricted areas, one to an unowned
aggregate by everyone listening to that table. It sends a comment every 20 s,
coalesces a burst within 50 ms into one event, ends after 10 minutes with an
`end` event (the client reconnects at once and refetches once open, as
commits in the gap went unheard, so an ended session stops hearing) and on shutdown. At most
8 streams per principal (per client address without one, IPv6 by /64) and 10 000 per
process; past that `429` with `Retry-After`. Measured (release, macOS, 2 000
streams): about 41 KB of memory per open stream, of which about 35 KB is what
any kept-alive HTTP/1.1 connection costs in hyper and about 6 KB the stream
(its task, channel, broadcast receiver, topics and cap entry); over HTTP/2 a
tab's streams share one connection.

Changes come from the unit of work: every committed aggregate write (and
mirror row) is announced after the commit with its table and owner, to this
process at once (a broadcast channel; a stream that falls behind refetches)
and to other instances through `pg_notify('sofie_live', …)` from a background
task, batched per 5 ms, outside the commit. Each instance listens on its own
connection, skips its own notifications and listens again after a failure;
notifications sent while it was not listening are lost, so those screens
catch up on their next change or reconnect.

### Islands

Anything richer than the catalog — a rich editor, a chart, a map, an app-like
screen — is a Svelte island, mounted by the engine with props from its loader.
Islands are custom elements, so swapped fragments mount and unmount them
without help, and a Svelte major release touches only the islands, never the
screens.

```rust
// modules/content/src/areas/web/routes.rs
routes.screen("/", "home", home_products);
routes.island("/editor/{page}", "page-editor", page_draft);
```

```html
<!-- modules/order/src/areas/customer/screens/orders.html -->
<Island name="order-chart" props={data.summary} />
```

- **A page or a part of one.** `routes.island(path, name, loader)` (the `sofie`
  feature `islands`) answers a page that is the island
  `islands/<name>.svelte`, or the loader's JSON when asked; a module wraps
  it in more of a page with `screens/<name>.html` holding
  `<Island name="<name>" props={data} />`, which any screen may also hold.
- **Props as data.** The island's element carries its props as JSON in a
  `<script type="application/json">` child, `<`, `>`, `&`, U+2028 and
  U+2029 escaped, so no value closes the element; the browser never runs
  it, so it needs no nonce. The bundle's custom element reads it and mounts
  the component with Svelte's `mount`; the element is the framework's rather
  than Svelte's `customElement` option, so props never pass through
  attributes and a development build can swap a component without
  redefining its tag.
- **One bundle per area.** `src/areas/<area>/islands/*.svelte` of every
  module, and every module's `web/*.svelte`, compile into `<area>.js` and
  `<area>.css`: component styles are bundled into the stylesheet, never
  injected inline, so the strict CSP holds. The app's `build.rs` (through
  `sofie_cli::check_in_build_script`) builds them into `OUT_DIR` with Node and
  the framework's pinned `crates/sofie-cli/web` toolchain, minified for a release
  build; the wiring embeds them, and every page of the area loads its
  bundle, the script under the page's nonce, so an island swapped in by
  htmx mounts too.
  The toolchain is installed once with `npm ci` in `crates/sofie-cli/web`; the
  workspace's own test run needs it too, for the `examples/islands` demo
  and sofie-cli's island build tests. Without it they fail with that
  instruction instead of being skipped, since a skipped island build would
  let a broken one pass unnoticed.
- **Typed props.** `sofie gen` writes `web/gen/types.ts` (the `sofie typescript`
  types) for an app with islands; an island page's component declares
  `let { … }: ReadModel = $props();` with its loader's read model, and
  `sofie check` (`island-props`) finds another type or a field the read model
  lacks. An island a route or screen names that no module has, or a tag
  twice in one area's bundle, is an `island` finding.

### Permissions and flags in screens

A column needing `order.order.total.read`, a button needing
`order.order.*.cancel` or a section behind a flag is a condition in the
template, evaluated while rendering for the request's principal:
`{#if can("order.order.total.read")}…{/if}` and
`{#if enabled("order.split_payments")}…{/if}`, a quoted permission or flag
name. Nothing hidden is sent to the browser: a fragment request cannot
reach a hidden part by its id, and a render for no request shows nothing
gated. The engine asks `sofie_engine::rendering::Gates`, which sofie-http
answers from the request's gates (`Features::enabled_named`,
`Permissions::can_named`); the engine's `gates` feature comes with
`feature-flags` or `permissions`. A resource's screens in a `Permitted`
area show their New, Edit and Delete links only to who may follow them.

A part hidden by a gate (`can`, `enabled` or `entitled` in an `{#if}`, or
an `Action`'s `when`) sends no data either. When a template is built, the
engine records every data path it binds as a tree, from the data root
through loops and table rows (`orders[*].cost`): a `Table`'s columns as
fields of each row, an island's props, `<Markdown>` and `<Head>`. Each path
is recorded with how it is read (whole, tested by a condition or looped
over) and the gates around its part
(`rendering/withheld.rs`, the slot logic in `slots.rs` shared with `sofie
check`). A listing's rows are the same path whether reached as
`data.orders`, `data.orders.items` or a loop over either. For a screen's
JSON callers and a client area's envelope, sofie-http rewrites the data in
place for the viewer (`Rendering::withheld`, `Withheld::apply_to`),
failing closed:

- A part of the data holding no read the viewer fails, and under no whole
  read they fail, is sent untouched; so is everything under a path read
  whole in a part the viewer sees. A template whose gates all hold costs
  nothing.
- Anywhere else only the paths the viewer is shown are sent. Every other
  field is `null`, including fields no read names, so a record keeps its
  keys and stays truthy. Under a value read whole inside a hidden part
  (`{chart}`, `<Island props={data}/>`, `{data}` itself) only what is shown
  elsewhere is kept.
- A list keeps its length only where a shown loop or table counts it;
  otherwise it is `null`. A listing's page keys (`page`, `size`,
  `has_next`, `sort`, `filter`, `controls`, `errors`) are bound by its
  `Table` like any whole read, under the Table's own gates, so they are
  sent wherever a shown `Table` lists it; a `Table` of `data.orders.items`
  shows a plain list and binds none.
- A path the viewer's parts only test (`{#if data.discount}`) while a
  hidden part reads it is sent as `true` or `false`, with the truthiness
  the engine and the client share.
- A client area's `instants` marks only instants sent as text, so a
  withheld one's name is not sent.

A screen's ETag is computed from the rows its loader read before anything
is withheld, so a viewer may see the tag change when only data withheld
from them changed: it tells them something changed, never what.

## Persistence

Aggregates for writing, read models for reading, one generic query builder for
both. The mechanics are in [core.md](core.md#the-orm-two-sides-one-description);
the rules an app lives by are these.

```rust
// modules/order/src/queries/due_orders.rs
pub async fn due_orders(read: Read, minimum: Money, page: Page) -> Result<Paginated<OrderRow>> {
    OrderRow::query()
        .scope(due)
        .where_(Order::TOTAL.gt(minimum))
        .latest()
        .paginate(&read, page)
        .await
}

fn due(query: Query<OrderRow>) -> Query<OrderRow> {
    query.where_(Order::STATUS.eq(OrderStatus::Pending))
}

// modules/order/src/actions/cancel_order.rs
pub async fn cancel_order(mut order: Order, reason: CancelReason) -> Result<Order> {
    order.cancel(reason)?;
    Ok(order)
}
```

| Eloquent | Here |
|---|---|
| Query builder | One generic builder over typed columns; a wrong column or value type does not compile |
| Local scopes | Plain functions applied with `.scope(…)` |
| Global scopes | The owner filter from the area and soft delete from the behaviors; nothing else is implicit |
| `with()`, lazy loading | Neither. Aggregates load whole with their child rows; read models state their joins up front, so N+1 cannot be written |
| Relations to another module | A typed id. Reads join through a mirror; a page of records resolves through the contract's batch lookup in one call |
| Casts | The field's type: values, enums, instants |
| `paginate()`, `cursor()` | `paginate()` for screens, `stream()` for large sets |
| `DB::select` | `read.sql::<Row>("…").bind(&value).all()`: `'static` text, bound parameters, a read-only transaction, in queries only and never in an owned area |
| `save()`, `create()`, `update()` | None. An aggregate is born in its factory, changed by its own methods, and committed from what the action returns |
| Mass updates | None; a change to many aggregates is a per-item action |
| Observers | None; a change others react to is a domain event |

- **No transaction during the body.** A handler runs with no transaction open;
  the unit of work commits after it returns, writing changed records, child
  rows, events and audit entries in one short transaction with a version
  check. A stale write is a conflict, never an overwrite.
- **No compile-time SQL checking against a database.** The builder works from
  the schema and `schema:check` holds the schema to the database in CI, which
  gives the same guarantee without a database at build time.
- **Migrations are append-only.** Applied files are recorded with a checksum
  and an edited one is refused; `<app> db status` reports each migration
  applied, pending, edited or missing as a file, and `sofie db fresh` rebuilds
  a database on this machine from its migrations.
- **Postgres, or SQLite for one instance.** Every database-specific feature is
  behind the [database seam](core.md#the-database-seam); application code never
  names the driver.

## Messages and contracts

Modules publish and react asynchronously through one message table and reach
each other synchronously only through their contracts.

- **Events** are declared in the contract's `src/events.rs` with
  `#[derive(Event)]`, so any module may hear them, and stored as
  `<module>.<Type>` (`order.OrderPlaced`, also from crate `order-contract`).
  An aggregate raises one with `self.meta.raise(…)`; the commit writing it
  stores the event, and the worker delivers it to every `#[on]` listener of
  its type. `sofie check` refuses an event declared in the module's own crate
  (`event-in-contract`) and a listener of a type that does not derive `Event`
  (`listener-event`). A listener may also hear an event that sofie-identity,
  sofie-tenancy or sofie-billing raises (`#[derive(Event)]` or `impl Event
  for`), when the app or a module depends on that package, or for billing
  when a module declares plans.
- **Schedules**: `#[every("1h")]` runs an action every interval;
  `#[every("15m", each = stale_orders)]` runs the query `stale_orders` (any
  function of the module returning `Result<Vec<Order>>`) and queues the
  action once per record, each in its own job, unit of work and retries,
  the record bound as the argument named after its aggregate (D7). A record
  whose job still waits gets no second one;
  `#[every(daily_at = "03:00", zone = "Europe/Istanbul")]` runs at a
  wall-clock time (core.md). `sofie check` refuses an `each` naming no
  function of the module (`schedule-each`) and a wall-clock schedule naming
  a zone the zone database does not know (`schedule-zone`).
- **Jobs**: with `sofie`'s `jobs` feature, an action marked `#[job]` runs in the
  worker once a handler queues it: `jobs.queue(actions::send_receipt,
  &json!({ "order": order.id() }))` returns a `Queued` outcome stored with the
  handler's commit; the payload binds the job's arguments as a request body
  would. A job's `IdempotencyKey` (with `idempotency`) is its message's id, the
  same on every retry (D4).
- **Tasks**: with `sofie`'s `tasks` feature, an `async fn` marked `#[task]` in
  the module's `src/tasks.rs` or `src/tasks/` runs once by hand as
  `<app> task <module>.<function> [--name value]...` (beside `<app> jobs dead`
  and `<app> jobs replay ID`), in release builds
  too, for operational work such as a data backfill. It takes what a job
  takes (`Read`, `Clock`, `Config`, `Dep`, `Jobs`…) and, for arguments, one
  `#[derive(Input)]` struct read from the `--name value` pairs and
  validated as a request body is (unknown, missing and invalid ones
  refused before it runs); what it returns is committed as a job's outcome
  is. The binary builds the app first, for its configs and contracts, runs
  the task in a `task <name>` span and exits non-zero with the error.
  `sofie gen` writes `tasks()` into the wiring, which `main` hands to
  `sofie::run_with_tasks` (or `run_with_flags_and_tasks`); `sofie gen` also
  makes that switch in `main.rs`, re-exports `tasks` from `lib.rs` and turns
  sofie's `tasks` feature on in the module when they are missing; `sofie tasks` lists
  them with their arguments, and the model refuses a `#[task]` outside
  `tasks` or two of one name in a module.
- **The worker** keeps a fixed number of messages in flight
  (`SOFIE_WORKER_BATCH`, 2) and claims another as soon as one finishes.

With `sofie`'s `contracts` feature:

- **Contract traits** live in the contract crate; `#[sofie::contract]` on the
  trait and on each implementation lets their `async fn`s be called through
  `dyn`. The module implements its own contract's traits in `src/contract.rs`
  on a type built from the database (`impl From<Db>`, or a unit struct), and
  a handler of any module asks for `orders: Dep<dyn order_contract::Orders>`
  (D1, core.md). `sofie gen` provides each implementation to the app and the
  worker.
- **Extension points** are contract traits other modules implement under
  their `src/extensions/`; the declaring module asks for
  `Extensions<dyn Trait>`, every implementation in the wiring's order.
- **Batch lookups**: every aggregate with `#[expose]` fields is reachable as
  `Dep<dyn sofie::db::Lookup<order_contract::Order>>`; `get_many(&ids)` reads
  their exposed fields in one query (D13).
- **Change events**: each committed change of such an aggregate raises the
  generated `<Type>Changed` with its id, version and exposed fields (D9).
- **Mirrors**: `#[mirror(customer_contract::Customer)]` on an aggregate of
  the module keeps a local, read-only copy of some of `Customer`'s exposed
  fields, named alike, from its change events, never overwriting a newer
  version with an older one, so reads filter, sort and join by them without
  crossing modules (D8). Its migration creates the table like any
  aggregate's. `sofie check` refuses a mirrored field the source does not expose
  (`mirror-field`).

## Types

### Value objects

A value object is a type that cannot hold an invalid value: it is parsed once
at the boundary and trusted everywhere after. `#[derive(Value)]` describes it
to the library, which gives it serialization, its cast, its form field and its
place in every other surface.

```rust
// modules/catalog/src/domain/sku.rs
#[derive(Value)]
pub struct Sku {
    code: String,
}

impl Sku {
    pub fn try_new(code: &str) -> Result<Self, SkuError> { … }
}
```

- Construction goes through `try_new`; deserializing a request or a row calls
  it too, so an invalid value never enters the app.
- A value of several named fields takes them in `try_new` in declaration
  order, spans one column per field (`<field>_<member>`; each field must span
  one column) and is a JSON object of its fields. An `Option` of it is `None`
  only when every column is null, so an `Option` of one whose fields are all
  optional is refused when compiled.
- Equality, hashing and cloning are derived; values are immutable.
- A value marked `#[sensitive]` is masked in `Debug`, and so in logs and
  traces (`Iban(***)`); a shipped value may mask partially (`Email` prints
  `j***@example.com`). A field holding it, or a `Secret`, counts as a
  `#[sensitive]` field (`Cast::SENSITIVE` puts it in `Aggregate::SENSITIVE`;
  sofie-identity's `PasswordHash` sets it too):
  never filtered, sorted, listed, searched or written in the record's JSON.

The framework ships the values almost every app needs. `Money`, `Currency`,
`Email` and `Secret` are always compiled; the other groups are
cargo features of `sofie` (forwarded to `sofie-values`), off by default, so an app
compiles only the values it turns on (`Instant` is always there: the clock
and every record's timestamps are one): `time` (`Date`,
`LocalDateTime`, `TimeZone`, `TimeOfDay`, `Weekday`; see core.md), `phone` (`PhoneNumber`), `person`
(`PersonName`), `geo` (`Coordinate`), `measure` (`Measurement` and its
instances), `percentage` (`Percentage`) and `web` (`Slug`, `Url`, `Locale`).
They are imported from `sofie::values` (`sofie::values::Date`); the prelude
holds `Money`, `Currency` and `Email`, and `Date`, `PersonName` and `Url` behind
the `time`, `person` and `web` features; `Uuid` comes from `sofie::db`.

| Value | Holds | Notes |
|---|---|---|
| `Money`, `Currency` | Minor units as `i64` and an ISO 4217 currency | No default currency: every amount names its own. No floats anywhere: `Percentage::of` takes an amount, `scale` a ratio, each rounded once, half away from zero. Arithmetic across currencies is an error |
| `Email` | A normalized address | Sensitive |
| `PhoneNumber` | Country code and national number | Two columns (`_code`, `_number`); parsed from and written as E.164 on demand, split by the prefix-free calling codes. Sensitive |
| `Instant` | A point in time, UTC | For anything that happened; RFC 3339 with any offset in, UTC out, stored in UTC. People see it in the app's display zone (`SOFIE_TIME_ZONE`, see core.md): `date(…)` and a form's `datetime-local` control show it on that zone's wall clock (seconds and fraction kept, with `step="any"`; a fraction finer than milliseconds, which the control cannot hold, is shown as text with its offset), and `Instant::parse` reads a `datetime-local` value back in the same zone (a repeated wall clock in a DST fold is refused naming both offsets, and an instant in a fold is shown as text with its offset so it re-saves unchanged; a skipped one is read where the shift ends); `date(…)` and table cells write the offset before the zone's name where the wall clock repeats (`2026-10-25 02:30 +02:00 Europe/Berlin`). It shows as `Shown::Instant`, so a `<Table>` formats typed instant cells and leaves text written like one alone; a client area's envelope marks them in `instants`. There is no per-viewer zone yet; a future time a user schedules belongs in `LocalDateTime` |
| `Date` | A calendar day | No time zone; exactly `YYYY-MM-DD`; `Date::utc_of(instant)` and `Date::of(instant, &zone)` give the day an instant falls on |
| `LocalDateTime` + `TimeZone` | A wall-clock time and an IANA zone | For anything scheduled in the future, so it survives time zone rule changes. Two columns: the wall-clock time as `timestamp` (without time zone, `SqlValue::DateTime`) and the zone as text in `_zone`; the instant is worked out on demand from the system zoneinfo database; a time a forward shift skips is placed where the shift ends, a repeated one at its first occurrence (one rule for instants read from forms, `LocalDateTime`, `LocalJobs` and wall-clock schedules). An app that stored the time as ISO text before converts the column with `ALTER TABLE … ALTER COLUMN … TYPE timestamp USING …::timestamp` |
| `Coordinate` | Latitude and longitude | Whole ten-millionths of a degree; two float columns (`_latitude`, `_longitude`) |
| `Measurement<U>` | A quantity in a unit, exact to a thousandth, with conversion | `Weight`, `Energy`, `Length` are instances; two columns (the quantity as a float of its unit, `_unit`); conversion is integer arithmetic over exact unit steps |
| `Percentage` | Basis points | A JSON number is basis points; text is a percent with up to two decimals (`"19.5"` is 1950), which its form control shows and posts |
| `Slug`, `Url`, `Locale`, `PersonName` | Text, checked for shape | `Slug` is lowercase letters, digits and nonspacing or spacing marks of any script (categories L, N, Mn, Mc; not decomposed Latin accents, invisible fillers, format, private-use or unassigned characters) joined by single hyphens, the characters `Slugged` keeps, at most 100 bytes; `Url` is absolute `http`/`https` without credentials; `Locale` is language, script and region; `PersonName` is one field and sensitive |
| `Secret<T>` | Anything that must never be printed | Masked `Debug`, no `Display`, not serializable; read from input and stored as `T` |
| `{Aggregate}Id` | A typed id per aggregate, from its derive | An `OrderId` is not a `CustomerId` |

### One declaration, every surface

A value object carries its behavior on every surface it reaches, so a field's
type alone decides how it is stored, validated, shown, filtered and logged.
There is no list of field types for the UI or the logger to switch on; adding a
value means adding one file, and nothing else learns about it.

| Surface | What the value provides | `Money` as an example |
|---|---|---|
| Database | Its [cast](#casts): columns, encode, decode | `total` in minor units, currency from the row |
| Input | Parsing from JSON, a form field or a query string, with translated errors | `"149.90"` in EUR → 14990 minor units; `"abc"` → `validation.money.invalid` |
| Form | The input component and its constraints | Amount field with the currency beside it |
| Table and detail | Its localized text and CSV form; sorting is done in SQL through `sofie_db::Sort` | Formatted for the viewer's locale (`€149.90`, `149,90 €`) |
| Filters | The operators it supports | Equals, greater than, less than, between |
| TypeScript | The type, when the TypeScript plugin is used for islands or API clients | `{ minor: number; currency: Currency }` |
| Logs and traces | Structured fields, masked when sensitive | `total.minor=14990 total.currency=EUR` |
| Search | The index field type | Numeric, filterable |
| Fixtures | A generator for realistic test values | A plausible price in the currency |
| OpenAPI | The schema | |

`#[derive(Value)]` covers every surface it can from the fields: a value of one
integer filters like a number and logs as one field. A value overrides a
surface by implementing that one trait (`Present` for the display and input
components, `Filter` for operators, `LogFields` for logging), and nothing else.
A value names its components from the component catalog. The engine ships the
entries for the values the framework ships; a module's own value adds its
entries in the module's `web/` folder.

`Present` gives `INPUT` (a catalog control — `Input` typed by attributes such
as `type="email"`, `Password`, `Textarea`, `Checkbox`, `Select`, `Money`,
`Percentage`, or `Parts` for a value read as an object, one control per part
posted as `<field>.<part>` like `Measurement`'s quantity and unit — and its
attributes), `csv` and, with sofie-values' `web` feature (which the
engine's `i18n` turns on), `write_localized`, which writes through the
viewer's `Conventions` (`Language::format`). An input type describes its fields as
`FormFields::FIELDS` (name, the value's `INPUT`, required unless an `Option`,
the effective `sofie_core::Constraint`s); `Template::describe_form(form_id,
I::FIELDS)` (or `Screens::describe_form::<I>(screen, form_id)`) shows every
field of the input with its control, `required` and constraint attributes
(`minlength`, `maxlength`, `min`, `max`) and refuses a `Field` the input does
not have. A select without `options` offers its input's own
(`Input::select(&[…])`, a `#[derive(Enum)]`'s variant names, sorted in
declaration order). A money field without a `currency` or money `value` lets
the currency be typed beside the amount. A `#[derive(Value)]` type of one
field implements the marker `SurfacesFromInner` (`SENSITIVE` with
`#[sensitive]`) and so is presented, filtered and logged as the value it
wraps (only whole and masked when sensitive, and typed in a password
control); a `Uuid` or `Id<A>` is a hidden
input and sofie-identity's `Password` a password control. A `Field` no input
describes is a text input; there is no `kind="…"`.
`Language::field_error(key, arguments, fallback)` writes a `FieldError`
(`validation.max` with `max = 120`) in the viewer's language, a value's own
refusal (`validation.invalid`) in its own words; sofie-http writes every field
error, notice and re-rendered form this way. `Filter` (`OPERATORS`) and `LogFields` (members as
`LogValue`s, sensitive values as their mask) sit behind sofie-values' `filter`
and `log-fields` features.

Logging takes values, not strings: `info!(order.id, order.total, "order
placed")` writes structured fields through `LogFields`, so a sensitive value
cannot be interpolated into a message unmasked.

### Casts

The type of a schema field is its cast; there is no separate cast declaration.
A cast says which columns a type spans, so `schema:check` can compare them with
the migration:

```rust
pub trait Cast: Sized {
    const COLUMNS: &'static [ColumnSpec];
    fn decode(row: &mut dyn ColumnReader) -> Result<Self>;
    fn encode(&self, out: &mut Vec<SqlValue>);
}
```

Values are written as the framework's neutral `SqlValue` and read through a
`ColumnReader` the driver implements, never a driver's types, so no value
changes when a database is added.

`total: Money` spans `total` and `total_currency`; `phone: PhoneNumber`
spans `phone_code` and `phone_number`; `weight: Weight` spans `weight` and
`weight_unit`. A module writes its own
cast only for a value the framework does not ship.

### Enums

```rust
#[derive(Enum)]
pub enum OrderStatus {
    #[tone(neutral)]
    #[to(Paid, Cancelled)]
    Pending,
    #[tone(success)]
    #[to(Shipped, Refunded)]
    Paid,
    …
}
```

From the derive: the database mapping, `SCREAMING_SNAKE` serialization,
translation keys for labels, form options and badge tones. A status
enum with `#[to]` is a state machine: the unit of work refuses a save that
moves along an edge it does not list, whichever code made the change.

### DTOs

Most DTOs are never written:

- A handler's argument that is not bound from the route is its input: a
  `#[derive(Input)]` struct reads the body field by field, and a lone
  `#[derive(Value)]` (`cancel_order(order: Order, reason: CancelReason)`)
  reads the one field named after its type, `{ "cancel_reason": … }`, a
  JSON body or a form alike, validated by `CancelReason`'s own parsing.
  It is refused as an input's field is: absent is `validation.required`
  (`Option<CancelReason>` reads it or nothing), unparsable is
  `validation.invalid`, any other field `validation.unknown`, all as one 422;
  an empty body posts no fields. The field is named after the type, not the
  argument, because `Resolve` sees only the type. The body is read once, so
  a handler taking several values takes them as one `Input` struct.
- A GET takes no input; it reads its query string as `Query<Q>`, `Q` a
  `#[derive(Input)]` struct read as a form's fields are (a blank value is
  absent, `"5"` reads as a number): a refused value is a 422 with each
  field's `validation.*` key, a key `Q` has no field for a 400. Tracking keys
  (`utm_*`, `fbclid`, `gclid`, `msclkid`) that `Q` does not name are left
  unread here and in a listing, unless the listed type selects a field of
  that name, so a shared link with one appended opens. A bare
  `Query` reads keys by name (`query.get("state")`) for a caller sending
  keys of its own, such as an OAuth provider. A route builder's path takes
  parameters with `.with_query("near", &value)`.
- An aggregate whose module writes no `factory.rs` derives, with the `sofie`
  feature `factories`, `CreateProduct` (every stored field but the
  `#[owner]`, which the area gives), `UpdateProduct` (the same type) and the
  `Create` and `Update` traits applying them (D6). Writing `factory.rs`
  replaces both: a module with a rule of creation writes its own changes
  too. Every field type must then be readable from input.
- A read model (`OrderSummary`) is the output DTO of the queries that select it.
- Fields marked `#[expose]` become the contract DTO other modules receive:
  `sofie gen` writes `pub struct Order { pub id: sofie::db::Uuid, … }` into the
  contract's `src/gen/mod.rs` (`Id<X>` as its uuid, a module enum as its
  stored name, scalars and sofie values as they are; `sofie check` refuses any
  other type), which the contract exports with `pub use r#gen::Order;` and
  fills with `sofie::db::exposed(&order)`. Beside it `sofie gen` writes the change
  event `OrderChanged` (`id`, `version` and the same fields), which every
  committed change of an order raises with `sofie`'s `contracts` feature (D9),
  and the wiring provides `Dep<dyn sofie::db::Lookup<order_contract::Order>>`,
  whose `get_many(&ids)` reads the DTOs of many orders in one query (D13).
  For every aggregate, exposing or not, it also writes `pruned::OrderPruned`
  (`id`, `version`), the event a retention prune raises for each record it
  deletes, which the contract exports with `pub use r#gen::pruned;`.

A hand-written DTO is a plain struct with `#[derive(Input)]`, used where data crosses a
boundary in a shape no schema describes, such as a vendor's payload inside a
gateway adapter.

#### Field constraints

A field of an aggregate or an input declares its limits once, as
attributes: `#[min(n)]` and `#[max(n)]` (characters of text, the value of a
number, chosen by the type's `Measured`), `#[length(min = …, max = …)]`
for text and `#[range(min = …, max = …)]` for numbers (the build refuses
the other kind), `#[required]` (not blank, not absent) and `#[unique]`. They
become:

- **Validation**: reading an input refuses a field breaking one, with every
  other field error, as a `FieldError` keyed for translation
  (`validation.max_length` with `max = 120`) beside its English message.
  An aggregate's constraints reach input through its derived
  `CreateProduct`, or through `#[derive(Input)] #[input(from = Product)]`:
  each field without constraints of its own inherits those of the
  aggregate's field of the same name and type (its typed column carries
  them, `Product::NAME.constraints()`); a constraint attribute on the field
  replaces them, `#[unconstrained]` drops them. `#[input(validate)]` runs
  the input's `sofie::db::Validate` (`fn validate(&self) -> Result<(),
  Vec<FieldError>>`) for cross-field and custom rules once every field was
  read and kept its constraints. A JSON caller gets each as `{"field", "key",
  "arguments": {name: value}, "message"}` in a 422's `errors`.
- **Form attributes**: each constraint is a `sofie::core::Constraint`, the one
  description a form's fields carry (`FormFields` in sofie-values), the
  effective ones for an input derived from an aggregate. An input whose
  every field type is `Present` is a form: `#[derive(Input)]` writes
  `FormFields` under a higher-ranked bound, so an input with a list field
  still compiles and is simply not one.
- **Database rules**: `sofie check` holds the module's migrations to them: a
  CHECK on the column for every constraint but `#[unique]`, a unique index
  or constraint on exactly the column for `#[unique]`, and for a field of an
  enum the module declares, a CHECK listing every stored variant name. A
  field of input derived from an aggregate that replaces its inherited
  constraints must keep each kind the schema declares (the database still
  holds it); `#[unconstrained]` says the action keeps the value within them.

### Behaviors

A behavior gives an aggregate columns, rules, scopes and methods by being
named in its schema. It is visible on the aggregate's declaration, so nothing
is attached from a distance.

```rust
// modules/catalog/src/product/mod.rs
#[derive(Aggregate)]
#[aggregate(behaviors(SoftDelete, Active, Slugged(from = name), Audited))]
pub struct Product {
    meta: Meta,
    #[max(120)]
    name: String,
    price: Money,
    weight: Weight,
    status: ProductStatus,
}
```

| Behavior | Adds |
|---|---|
| `Versioned` | Optimistic lock column. Always on, first |
| `Timestamps` | `created_at`, `updated_at` as `Instant`, stamped by the commit; the `Timestamped` trait gives their columns and values. On in every derived aggregate, so it is never named |
| `SoftDelete` | `deleted_at`, the default scope that hides deleted rows, `with_trashed()`; the `SoftDeletes` trait gives `delete()`, `restore()`, `is_deleted()`, `deleted_at()` |
| `Active` | `is_active` (true for a new record), the scope `active` (`query.scope(active)`); `Activates` gives `activate()`, `deactivate()`, `is_active()` |
| `Default` | `is_default`; `Defaults::make_default()` clears it on the owner's other records in the same commit (per value of the `OWNER` column, so per parent for a record owned through one; the whole table without an owner). A unique partial index makes it hold under concurrent commits: `sofie gen migration` writes it as `<table>_is_default` on (owner column, `is_default`) `WHERE is_default`, or on `is_default` alone without an owner; an aggregate owned on its struct gets a note to add it by hand. The clear raises no change event, so an aggregate with `Broadcasts` too is refused when its type is first used |
| `Slugged(from)` | `slug`, derived from the text field `from` names when the record is created or that field changes (an unchanged field keeps its slug): lowercase words joined by hyphens, at most 100 bytes cut between words (a first longer word between characters); a small built-in table spells Turkish, German, accented Latin and Cyrillic letters in ASCII (`Öğle` → `ogle`, `ß` → `ss`, `Привет` → `privet`) any other letter, digit or nonspacing or spacing mark stays as it is, and everything else (spaces, punctuation, symbols, format and private-use characters) parts words (`关于`, `नमस्ते`). A `from` that gives no slug (no letter or digit, e.g. `🎉`) is refused under `from`. A duplicate in the table's unique `<table>_slug` index is refused under `from`. A slug the record leaves is kept in `sofie.previous_slug` (one per table and slug, dropped when another record takes it or the record is deleted); `query.by_slug(&read, slug)` finds the record by its current slug, else answers `Error::Moved { from, to }` for a previous one of a record the query finds, which a GET answers with 301 to its own path, the segment replaced and the query kept |
| `UniqueCode` | `code`, ten Crockford base-32 characters spelled from the random bits of the record's id on insert, so never reused while ids are not; the unique index is the migration's (`<table>_code`, which `sofie gen migration` writes) |
| `#[owner]` (a field attribute, not a behavior) | The owner field that `Owned` areas filter on; `#[owner(through = parent)]` on the struct for a record owned by its parent's owner (D11) |
| `Audited` | `created_by`, `updated_by`: the principal of the owned area that wrote, null for a public area or a job |
| `RecordsChanges` | A row of `sofie.history` per create, update, delete and restore, in the same commit: the changed fields, the principal (for a job or listener, the one whose action queued it), the action (the route's name, else the handler's path; a job's registered name; null outside both) and the time; `History::history(&read, id)` reads them; an owned area reads the whole history of a record within its reach now, so a record handed over shows its history to its new owner only. Not yet: the History tab (needs resources, C5) |
| `PublishLock` | `published_at`, set by `Publishes::publish()`; once committed, a change to the fields fails as `Rule` |
| `Broadcasts` | The event `<module>.<Type>Changed` (the type as written, `HTTPLogChanged`) with `id` and `version` in the message table, per committed change, so listeners get it after the commit; an aggregate exposing fields raises that event already, with them. Open screens hear every aggregate's changes without it, through `live` (see Live screens) |

`Versioned` and `Timestamps` are always compiled; the others are cargo
features of `sofie`, off by default, so an app compiles only what it names:
`soft-delete` (`SoftDelete`), `flags` (`Active`, `Default`), `codes` (`Slugged`, `UniqueCode`),
`history` (`Audited`, `RecordsChanges`), `publishing` (`PublishLock`,
`Broadcasts`). The derive finds the framework's behaviors by name, so they
need no import and `Default` does not shadow the standard trait.

A behavior can only derive values before a save, refuse a save, or signal
after commit. It cannot call a service or a gateway, so it never hides I/O.
A module defines its own behavior the same way the framework does: a type
implementing `Behavior` for its columns (stored after the id, before the
fields), the values it sets on a write (`written`, reading the record as a
`Write`: its columns as loaded and as written, the commit's `Stamp` of time,
principal and action), whether an update must find them unchanged, the column whose
value hides a record, and what it adds to the commit (`planned`: refuse it,
or add statements before the record's own); and a trait for its methods,
implemented for every aggregate that `Uses<ItsBehavior>`, which the derive
implements for each behavior it names. An argument names a field
(`Slugged(from = name)`): the derive passes the field's column name, after
checking the field's column constant exists.

## Observability

Every request, job, event delivery and contract call can be followed from its
first span to its last, across processes and across a split. Almost none of it
is written by hand: every entry point already passes through the framework's
handler adapter, store, worker and gateway and contract wrappers, so those open
the spans, record the metrics and carry the context. Telemetry is
exported over OpenTelemetry (OTLP) and configured with the standard `OTEL_*`
variables. The collector is expected beside the app, on its host or network,
reached over plain HTTP; an `https://` endpoint is sent over TLS (rustls).

### One trace per piece of work

- **Incoming.** A request from a trusted proxy (`SOFIE_TRUSTED_PROXIES`)
  continues its W3C `traceparent`; any other request starts a trace, so a
  client cannot choose trace ids or sampling. Its request id is always the server's own, never the client's. A
  trusted proxy's request id, from the header `SOFIE_REQUEST_ID_HEADER` names
  (`x-request-id` by default), is recorded beside it as `edge_request_id`
  when it is at most 128 plain characters. The server's id and the
  `traceparent` are returned in the response headers.
- **Across async work.** The outbox row and the queued job store the
  `traceparent` of the work that created them, so a listener's span joins the
  trace of the request that raised the event, minutes later and in another
  process.
- **Across modules.** A contract call is a child span in the monolith and a
  propagated HTTP call after a split, so `split` changes the topology of a
  trace, not its continuity. Each split app reports under its own service name.
- **Outgoing.** Gateway calls inject `traceparent` into the vendor request.
- **Browser.** The engine's client propagates `traceparent` on every call it
  makes; behind a trusted proxy a click, the request it caused and the
  queries under it are one trace, otherwise the request starts its own.

### Spans

| Boundary | Span | Attributes |
|---|---|---|
| Request | `GET order.customer.my_orders` (route name, not path) | area, module, route, status, flags evaluated, permission granted or refused |
| Action, query, loader (`handler`) | `order::actions::cancel_order` (the handler's path) | module |
| Render (`render`), a page, fragment or out-of-band reset | `order/customer/invoice` (the screen key) | `screen`, the screen key |
| A marked step (`step`) | `order::actions::reserve_stock` | module |
| Database | Statement without bindings | rows, duration; slow ones logged at warning |
| Unit of work commit | `commit` | aggregates written, events queued |
| Outbox and jobs | `deliver order.OrderPlaced`, `job order.expire_pending` | attempt, lifecycle |
| Contract call (`contract`) | `Customers::get_many` (contract and function) | the implementing module; a remote call's span carries `contract.app`, the app it calls |
| Gateway (`gateway`, `http.client`) | `stripe.charge`, then `POST` | vendor, operation, outcome; peer host and port, status, retries |

A step inside a handler that deserves its own span is marked with
`#[span]` (feature `spans`); its arguments are never recorded. That is the
only span written by hand. `#[sofie::contract]` puts each call of an
implementation in a `contract` span; `#[sofie::gateway]` each call of an
adapter in a `gateway` span, with an `http.client` span per request under it
(see core.md, "Outgoing requests").

### Logs

- Structured only: fields are values, not text. Logging takes values through
  `LogFields`, so a sensitive value is masked by its type (see
  [One declaration, every surface](#one-declaration-every-surface)).
- Every record carries `trace_id`, `span_id`, `request_id`, module and area
  without being asked: the request span holds the route's module and area,
  the handler and `step` spans their own module.
- `sofie::telemetry::info!(order.id, order.total, "order placed")` (and
  `warn!`, `error!`, `debug!`; feature `log-fields`) logs each value through
  its `LogFields`, named by the last part of its path, so `Money` writes
  `total.minor` and `total.currency` and a sensitive value only its mask.
  The message is a literal, so nothing is formatted into it.
- What has no type to mask by is still scrubbed. An aggregate field marked
  `#[sensitive]` shows as `***` in the aggregate's `Debug` (the derive writes
  it, so a second `#[derive(Debug)]` is refused) and in
  `info!(payer.tax_number, …)`, which asks the owner's `SensitiveFields`
  (feature `log-fields`). On the writer's thread, before a line is written or
  exported, a field whose name ends in a key of the payload-key list
  (`password`, `secret`, `token`, `authorization`, `cookie`, `api_key`,
  `signature`, `session`, `card_number`, …) logs `***`, and the message
  scrubber masks in every text value: such a key's value in `key=value`,
  `key: value` and JSON (`"client_secret":"***"`), an unquoted `authorization` value to its
  line's end, a URL's password and query
  values, a `Bearer` or `Basic` credential, and Postgres's `Key (…)=(…)` and
  `Failing row contains (…)` details. It also turns email addresses (in any
  script, `xn--` top levels too, and a URL's user) and IPv4 and IPv6 addresses into `***`, found by a small scanner
  and checked with the standard IP parsers. Times, versions like 1.2.3,
  durations, UUIDs, hex ids and bare ports like `:8080` are left alone; a
  four-part version like 1.2.3.4 reads as an IPv4 address and is masked.
  Loopback and unspecified addresses (127.0.0.0/8, `::1`, `0.0.0.0`, `::`,
  also IPv4-mapped) are not personal data and stay visible, so the server's
  own listening address still reads at startup. The scrubber runs on the
  writer's thread for lines and OTLP logs, on the exporter's task for OTLP
  spans, and over the development span tree (`development::span_tree`, which
  the toolbar shows).
- Sampling is decided from the trace id, so lower rates are subsets of higher
  ones and a kept trace keeps all its logs. Warnings and above are always kept.
  `SOFIE_LOG_SAMPLE` (0 to 1, all when unset) sets the share, compared with the
  trace id's low 63 bits as OpenTelemetry's ratio sampler does.
  `SOFIE_TRACE_SAMPLE` does the same for exported spans, decided once at a
  trace's root (a continued trace keeps its caller's flag); a closing span
  only queues its fields, and its record is built on the exporter's task.
- Lines are formatted and written on a thread of their own, through a
  buffer: a request only records the event's fields and queues them. The
  queue never blocks; past its capacity, info and quieter lines are dropped
  and counted, and the count is logged as a warning. Warnings and errors are
  queued, in order, up to a bound four times larger; past it they are dropped
  too, and dropped errors are counted and logged as an error. Stopping the
  process writes what is queued.
- With `otlp`, the writer's thread also turns each line it wrote into an
  OTLP log record: severity, the message as body, the event's fields (already
  masked) as attributes, the target as scope, and the trace and span id of the
  innermost traced span around it, so a collector links logs to traces. The
  resource (service name) is the one spans and metrics carry. A task on the
  runtime exports them in batches of up to 512, gathered for a second, one
  export at a time; meanwhile records wait in a queue of 2048, and past it
  they are dropped and counted in a warning line. Stdout keeps every line;
  the development span tree is not exported. A failing export makes readiness
  report `exported logs` as degraded.

### Metrics

Behind the `metrics` feature.

- **Recorded by the framework:** `http.server.request.duration` by route
  name, method, status and area; `sofie.handler.duration` by handler and
  module (actions and loaders, commit included);
  `db.client.operation.duration` by operation; `db.client.connection.count`
  (busy connections); `messaging.process.lag` (how long a message waited
  past its due time) and `messaging.process.outcomes` (completed, retried,
  dead) by work and registered name; `messaging.outbox.depth` (messages
  not yet handled, dead ones left out, counted every 15 seconds while a
  worker runs: the metrics thread asks, the worker's runtime queries);
  `product.events`; `sofie.browser.events`; with sofie-client,
  `sofie.gateway.duration` by vendor, operation and outcome, and
  `sofie.outgoing.duration` by `gateway.vendor`, `gateway.operation` (the
  method) and `outcome` (status class or `error`). Cache hits come with
  screen caching.
- **Kept in process, off the request path.** A request only sends a
  measurement into a channel; a thread of its own drains it every 50 ms
  and adds measurements up, so no send wakes it. The
  Prometheus text is scraped from `/metrics` with `Authorization: Bearer`
  and the token `SOFIE_METRICS_TOKEN` (32 characters at least, never printed;
  without it the endpoint does not exist), and with `otlp` the same
  measurements are exported over OTLP. No external store is needed.
- **Cardinality is bounded by type.** A label is a `Label`: a `&'static str`
  (a route name, an enum variant's name, a module name) or a status code;
  an id or text made while serving is not `'static` and does not compile as
  a label.

### Product events

`product_event!(order.placed, order.total)` (feature `product-events`)
records a business moment counted in full, never sampled, on its own signal:
the target `product`, which neither `SOFIE_LOG` nor `SOFIE_LOG_SAMPLE` drops, with
`product.event="order.placed"` and its values as `LogFields`. Funnels and
conversion are read from it without mixing analytics into technical logs;
tests read them with `Capture::product_events`. An event inside a request,
or a job a flag gates, carries the flags that flow evaluated before it as
`feature_flags` (`order.checkout_layout=COMPACT`), so an experiment's
funnels split by variant without the caller naming it; the span holds the
flow's `Features` (`sofie_telemetry::attach_feature_flags`), read only when a
product event is emitted.

With sofie's `product-reports`, product events are also kept in
`sofie.product_events` with their time and principal (the one a request
was signed in as, or signed in during it; never logged): queued without
blocking the raising thread, at most 8192 waiting (beyond that dropped and
counted), written in batches by a task, flushed as the process stops.
`sofie_db::cohorts` reads them as a retention table: principals grouped by
the day, week or month (UTC) of their first `start` event since a time, and
how many of each cohort raised the `returning` event in each period after;
`<app> report retention --start EVENT --returning EVENT [--period day|week|month]
[--since DATE]` prints it as shares of each cohort. An event a job raises
has no principal and joins no cohort.

### Health

- `/live` answers as soon as the process serves.
- `/ready` checks the dependencies requests need — database, queue (the
  message table), each dependency declared with
  `app.serving(|serving| serving.dependency(name, check))` — and answers
  503 `unavailable` when one is down, naming which and why only in the log,
  as anyone may ask; its answer is kept a second, so a flood of probes costs
  one round of checks a second. `Serving` holds how the
  app is served beside its routes: session keys, these dependencies and,
  with their features, trusted proxies, the metrics token and the browser
  sample; `sofie::run` adds what the environment sets. A telemetry outage (log output, span or metric export failing) is answered
  200 `degraded` (the output named in the log) and never takes an instance out of
  rotation.
- Health probes open no request span and log no line; metrics still count
  them.

### Browser

The engine's client records Web Vitals (LCP, CLS, INP, FCP, TTFB), errors,
navigations and long tasks, and sends them through the app's own endpoint
(`/_sofie/telemetry`, feature `browser-telemetry`) to the collector, so the
collector is never exposed publicly. The endpoint takes 60 posts a minute
from one address; past that it answers 429. A page names the endpoint, its route,
its traceparent and the sampled share (`SOFIE_BROWSER_SAMPLE`, all when unset)
in a `<meta name="sofie-telemetry">`; the client sends in the page's trace and
gives each htmx request a child `traceparent`. Only a kind, a name from a
fixed list and a number are accepted, so nothing personal is recorded.
Session replay and visitor attribution are opt-in modules, not part of the
core.

### Development and tests

- `sofie dev` prints each request's span tree in the terminal; no collector is
  needed locally (feature `development`: a debug build writes each
  finished root span's tree with its log lines).
- Tests can assert on recorded spans, logs and product events through an
  in-memory exporter, so telemetry is tested as behaviour.

## Generated documentation

The structure is fixed, so the documentation is a view of it. `sofie docs`
(sofie-cli's `docs` feature) writes Markdown with Mermaid diagrams into
`docs/gen/`, committed; once an app keeps `docs/gen/`, `sofie gen` rewrites it
with the wiring (and `sofie gen --check` reports it stale), and `sofie check`
reports a page that differs from the model as `stale-docs`, like stale
wiring, which `sofie gen`, `sofie docs` or `sofie fix` answers. Prose comes from the code's own doc
comments: the first line of an action's `///` is its description everywhere
it appears.

| Page | Derived from | Diagram |
|---|---|---|
| `architecture.md` | Manifests, dependencies, areas, contracts | Modules by tier and the edges between them; a table of areas (prefix, principal, defining contract, modules routing in) and of contracts (areas, events and their listeners, generated exports, implemented traits) |
| `modules/<module>.md` | Everything in the module | Its areas and routes with each route's description, permission, owner scope, flag and modifiers (the security surface); screens; data model; state machines; read models; inputs with their constraints; events with who raises and who hears them; cross-owner checks (each `across` call, by aggregate and caller); jobs and schedules; flags; permissions, in an app with `Permitted` areas; config keys and settings |
| Data model (in the module page) | Schemas, owners, behaviors, ids | An entity diagram per module; a reference to another module's record (`AccountId`, `Id<Customer>` of a contract) is a dashed edge, since no foreign key crosses modules |
| State machines (in the module page) | Enums with `#[to]` | A state diagram per status enum; a state it never leaves is final |
| `flows.md` | Routes, handler signatures and bodies, aggregate methods, events, listeners, jobs | A sequence diagram per endpoint and per schedule, followed until no new step appears; a step reached twice is marked, not followed again |

| `security.md` | Routes, areas, permissions, modifiers, inputs, gateways, split modules, config | Who reaches each area, and which vendors the app calls; a row per route with its area, access, permission, owner scope, rate limit and checks (CSRF, webhook signature, signed link, body limit, upload, flag); public endpoints (public areas, sofie-identity's screens, `/live`, `/ready`, `/metrics`, `/_sofie/telemetry`); each vendor's allow-list keys; secret config keys (`Secret<…>`, database URLs, session keys); what the model cannot check (raw SQL, webhook verifiers, gateways without `Adapter`, area-wide limits and CORS) |
| `telemetry.md` | Routes, handlers, listeners, jobs, schedules, tasks, contracts, gateways, `#[span]`, `product_event!`, field types | How spans nest; each span name this app produces; each metric with its labels and the bounded names they take here; product events with their values; fields logged only as their mask (`Email`, `PersonName`, `PhoneNumber`, `Secret`, a `#[sensitive]` value). sofie-cli keeps the framework's metric names and masked types, and its tests hold both lists to the crates that declare them |

### How flows are derived

A flow joins edges the model reads exactly, without types:

- **Endpoint → handler**, from `routes.rs`; a schedule starts its own flow.
- **Function → function**, from calls whose path resolves to a function of
  the module through `crate`, `self`, `super` and the file's imports.
- **Function → aggregate method**, from calls on an argument whose type the
  signature names (`order: Order` then `order.cancel()`), on `self` inside
  an impl, or written `Order::place(…)`.
- **→ event**, from `.raise(OrderPlaced { … })` wherever it is written.
- **Event → listener**, from `#[on]` and the type of the listener's first argument,
  in every module.
- **→ job**, from `jobs.queue(actions::send_receipt, …)` naming a `#[job]`.
- **→ contract**, from a `Dep<dyn Trait>` or `Extensions<dyn Trait>` argument.

Branches inside a function are not drawn: a flow shows what can happen, not
under which condition. An edge the model cannot resolve — a method of the
module's types called on a value whose type only inference knows, an event
raised from an expression, a queued function that is not a `#[job]` — is
listed under the diagram with its file and line rather than guessed or
dropped, so a diagram never claims more than the code says.

### Why it is committed

A pull request that adds a cross-module dependency, a new event listener or an
endpoint in the customer area shows it as a diff in `docs/gen/`, beside the
code that caused it. Reviewing architecture becomes reading a diff.

### API descriptions

Every screen answers its loader's data to a JSON caller and every action
reads its `Input`, so the routes are an API. A part of the template gated by
a permission, flag or entitlement withholds the data bound only inside it
as `null`, or as a boolean where only a condition outside the gate tests
it (see [Permissions and flags in screens](#permissions-and-flags-in-screens)).
`sofie openapi` and `sofie typescript` type such paths nullable
(`T | null`, or `T | boolean | null` where a test can hold while the read
fails; `anyOf` in OpenAPI, as for every optional field), and every field
of a record holding a gated read that no shown read names (a value object
with some parts named answers a `Withheld<Value>` copy, such as
`WithheldMoney`, whose other parts are nullable), loosening the read model's shared schema
once every endpoint is built. A screen a handler renders by name counts as
the route's own. Other data some viewers may not see is left out by the
loader or gated on the route. `sofie openapi` (feature `openapi`) prints an OpenAPI 3.1 document of them: each area a tag, each
read model, input and enum a schema, values as their JSON shape (`Money`
as `minor`, `amount` and `currency`), a committed aggregate as its `id` and
`version`. `sofie typescript` (feature `typescript`) prints the same as
TypeScript types with an `Endpoints` map from route name to method, path,
parameters, input and output. A GET's `Query<Q>` or `Listing<T, Q>` is
described as `Q`'s fields: each an OpenAPI `in: query` parameter, and
`query: Q` in its TypeScript endpoint; the listing's own `page`, `size`,
`sort` and filters are not. Resources and webhooks are left out: the
first are described only inside the compiled app, the second are called by
a provider. Both are text on standard output, written where the client
needs it.

## Feature flags

Flags are built in, because everything a flag can gate is already known to the
framework.

```rust
// modules/order/src/module.rs
pub const SPLIT_PAYMENTS: Feature = Feature::new("order.split_payments").added("2026-09-01").retire_after_days(30);
pub const CHECKOUT_LAYOUT: Variants<CheckoutLayout> = Variants::new("order.checkout_layout").added("2026-10-01");
```

They live in `sofie-flags`, behind the `sofie` facade's `feature-flags` feature
(`flags` already names the `Active` and `Default` behaviors), as
`sofie::flags`. A flag is declared in its module's manifest, `src/module.rs`
(the module's features), and named `<module>.<flag>` by a literal, once in
the app (`sofie check`, `feature-flag`).

| What a flag gates | How |
|---|---|
| An endpoint | `routes.post(…).feature(SPLIT_PAYMENTS)`; off answers 404, so a hidden feature is not revealed; another method on its path is never told of it in a 405's `Allow` |
| An action, listener or schedule | `#[sofie::feature(SPLIT_PAYMENTS)]` (a bare `feature` is Rust's own crate attribute); off, it is not run and the skip is traced. The wiring gates a routed action through `Gates::route_features` (a 404, as `.feature(…)`), a listener or schedule through `sofie::flags::gated(&flags, FLAG, handler)` |
| A menu entry | `.feature(…)` on the entry |
| Part of a screen | A condition in the template, evaluated while rendering like a permission. No gated markup is sent. A JSON caller or a client area's envelope gets `null` for each data path bound only inside parts gated off for the viewer (a field, a nested field, a field of each item or table row, and everything under a value read whole there except what is shown outside), and `true`/`false` for a path only tested outside them |
| A branch in code | `features.enabled(SPLIT_PAYMENTS)` or `features.variant(CHECKOUT_LAYOUT)` from the handler's signature |

- **Targeting:** a `Rule` per flag: switched on, narrowed to environments
  and areas (none listed means all), then open to listed principals and to
  a percentage of the others, hashed (FNV-1a of the flag and the account id)
  so each person stays in or out, or split by weight between variants. A
  request without a principal, and a job, are in only at 100%. A flag
  nobody stored a rule for is off.
- **State:** declared in code, stored in `sofie.flags` (name, rule as JSON,
  `changed_at`), switched with `Flags::set` (its targeting is
  validated there: names of letters, digits, `_` and `-`, none twice, a
  percentage up to 100) or on sofie-identity's flags screen. `Flags::start(&db, environment)` listens, then reads
  them once (so a change committed after it returns is never missed) and
  keeps them in process: a change commits with a wake-up on the
  messages channel, and every process reads the table again (a burst once,
  after 200 ms); without a wake-up it reads again every 30 s. Listening
  that fails is resumed a second later and the table read right after (the
  shared `sofie_db::Listening` loop, which the shield's sharing also uses). A request
  reads what is held and never waits; a failed read keeps the last state.
  `App::gates(Gates::default().features(flags))` gives them to routes and
  handlers.
- **Wired:** once a module declares a flag, `sofie gen` writes
  `app(db, flags: Flags)` with `.gates(Gates::default().features(flags)
  .route_features(ROUTE_FEATURES))` and `worker(db, flags: &Flags)` adding
  each marked listener or schedule, wrapped in `sofie::flags::gated`, to the
  modules' jobs, and the app's `main` calls
  `sofie::run_with_flags(MODULES, app, worker)`
  instead of `sofie::run`: it starts the flags once per process, for both
  roles, in the environment `SOFIE_ENVIRONMENT` names (such as `production`;
  required, so a flag meant for staging is never guessed on in production).
- **Traced:** the flags a request evaluated, with their answers, are the
  request span's `feature_flags` attribute (`order.split_payments=on`); a
  skipped job logs its flag. A product event carries those evaluated before
  it in its request or gated job, variants included (see Product events).
- **Removed safely:** a flag is a constant, so deleting it breaks the build at
  every place that still uses it. The generated docs list everything each flag
  gates. A flag is temporary: it declares the day it was added
  (`.added("2026-09-01")`, a literal, required) and how long it may live
  (`retire_after_days`, 90 by default), and `sofie check` reports it, with its
  file and line, once it is older (rule `flag-age`). The check stays
  offline, so it cannot see whether the flag is fully on or off; a flag
  past its age is either finished and due for removal or rolling out longer
  than declared, so the age counts from the day it was added (a flag settles
  only after that, so the database's settled state could never report one
  sooner). It runs in `sofie check`, not the build script, so a build never
  starts failing as days pass; today's date is the one system time
  sofie-check reads (`Date::today`). `<app> flags settled` still prints the
  stored states (`order.split_payments on 120`) for an operator.

## Cache

Opt-in, in `sofie-cache`, behind the `sofie` facade's `cache` (memory),
`cache-database` and `cache-redis` features; `sofie::run` starts it from
`SOFIE_CACHE_STORE` (`memory` when unset, `database`, `redis` with
`SOFIE_REDIS_URL`) and handlers take `Cache` as an argument.

```rust
const TOTALS: Key<Totals> = sofie::cache::key!("totals"); // under the declaring module

async fn totals(cache: Cache, read: Read) -> Result<Totals> {
    cache.entry(&TOTALS).part(customer)
        .expires_in(Duration::from_secs(300))   // no entry without expiry, at most 7 days
        .depends_on::<Order>()                  // or .depends_on_record(id)
        .get_or_compute(|| compute(&read))
        .await
}
```

- **Keys** are typed and namespaced by the crate that declares them; parts
  are escaped, so they cannot forge a separator. Values are JSON; one an
  earlier build wrote in another shape reads as a miss.
- **Invalidation** by key (`entry.delete()`, and `set`, which drops other
  instances' copies) and by aggregate change: sofie-db's `cache` feature hands
  every commit's written records to a synchronous `CommitHook`, which drops
  this instance's memory entries depending on the aggregate or the record
  before the commit returns, and hands the shared store's drop and the
  announce to a task, so a commit does not wait for them and a cancelled
  request still invalidates. Until that drop lands, this instance reads a
  value the shared store holds under one of those tags as a miss. A
  value computed while one of its dependencies was dropped is answered but
  not stored, and one dropped while it was being written is deleted again.
  A drop another instance made in the shared store is known here only once
  its message arrives, so a value computed meanwhile can outlive it until it
  expires. One computation per key at a time runs in a process; when its
  value was dropped meanwhile one waiter computes again for the rest, and
  when that value is dropped too the waiters compute on their own.
- **Stores.** Memory holds at most 10 000 entries and 64 MiB of keys and
  values (clock eviction; a larger value is not held) and sits in front of the shared stores; instances drop each other's memory copies
  through `pg_notify('sofie_cache', …)` or Redis pub/sub, subscribed before
  `Cache::start` returns, and drop everything when a subscription is
  renewed. `database` keeps `sofie.cache`, an unlogged
  table created when the cache starts (migrations run in every app, the
  cache does not), pruned each minute. `redis` speaks RESP2/RESP3 itself:
  a pool of 16, `AUTH`, `SELECT`, TLS for `rediss://`, timeouts, backoff
  after a failed connect, tag sets (`sofie:tags:<tag>`, sorted by expiry)
  written with their entry in one `MULTI` that prunes expired members,
  dropped in chunks of 512, each read and then deleted with its members
  in one `MULTI`, no `SCAN`. Store calls are `cache` spans and `sofie.cache.duration`; lookups
  are `sofie.cache.lookups` by tier.
- **Failure.** A store that fails during `get_or_compute` is a miss with a
  warning; `get`, `set` and `delete` fail with the error. A drop after a
  commit that the shared store fails is still announced to other instances
  and retried for about 20 s, then logged as an error.

## Benchmarks

`sofie bench [--target URL] [--environment NAME] [ROOT]` generates k6 load
tests from what the app declares and runs them.

Built (wave 12): `bench/gen/endpoints.js`, one k6 scenario per GET route and
answer (a route with a screen is loaded as HTML and as JSON), run one after
another (16 virtual users, 10 s each). Areas sign in once in `setup()` through
sofie-identity: sign-up when the area opens it, then sign-in, with fixed
`bench-<area>@example.com` credentials on this machine. A target on another
host is never signed up on: each member area needs an existing account in
`SOFIE_BENCH_<AREA>_EMAIL` and `SOFIE_BENCH_<AREA>_PASSWORD`, or the run is
refused. Accounts reach k6 in its environment, never in a script; a path parameter takes the first `id`
the GET at the path before it answers as JSON. A `Permitted` area, an area outside
sofie-identity and a `.signed()` route are reported as not loaded. Without
`--target` it runs `sofie build`, starts the binary on a free local port with
`.env`, `SOFIE_MIGRATE=on` and `SOFIE_SHIELD=off`, and stops it afterwards,
refusing a `DATABASE_URL` not on this machine as `db fresh` does; a
target must run with `SOFIE_SHIELD=off` itself. The environment is `--environment`,
else `SOFIE_ENVIRONMENT`, else `local` when there is no target; anything but
`local`, `testing` or `staging` is refused, and a target with no named
environment is refused too. A POST route that creates a record from an input
(no path parameter, outside sofie-identity) is loaded too: 32 JSON bodies made
from the input's fields with sofie-values' fixtures and a fixed seed, replayed
in turn, with an `Idempotency-Key` for an `.idempotent()` route; a write fails
on a status of 400 or above. Every run also writes `bench/gen/client.ts`, a
typed TypeScript client k6 runs as it is: the types `sofie typescript` writes,
one exported function per reachable route named after it
(`order_customer_place_order(session, input)`), `signIn(area)` and
`signInAll()`. A module's `bench/*.ts` files are journeys: each default-exports
a function taking the signed-in `Sessions`; `bench/gen/journeys.ts` runs them
one after another like the endpoints, a journey failing when one of its
requests is refused, and their p95 and error rate join `bench/baseline.json`
as `journey <module>.<file>`. A renamed or removed route drops its function
from the client, so a journey calling it fails as k6 loads it.

- **Generated per endpoint:** the path from route builders, a valid payload
  from the input DTO and the values' fixture generators, sign-in for the
  endpoint's area, and records from the seeded data that the principal may
  reach.
- **Journeys:** a sequence of endpoints a person takes — browse, add to cart,
  check out — written by hand in the module's `bench/` folder against a
  generated, typed k6 client, so a journey is a few lines and breaks at
  generation when a route changes.
- **Data:** `sofie seed [--volume N] [ROOT]` fills the database the way the app
  does: it posts to every POST route without a path parameter that takes an
  input, so the route's action calls the factory, as the member `sofie bench`
  signs in with (`bench-<area>@example.com`), so the load reads those records.
  It posts each module's `seeds/<area>.<handler>.json` first (a JSON array of
  inputs to that route, for fixed records to test by hand; a file naming no
  such route is an error), then N inputs per route made from the input's
  fields with sofie-values' fixtures. It runs the development app on a free
  local port with `SOFIE_SHIELD=off`, refuses a `DATABASE_URL` not on this
  machine as `db fresh` does, and reports per route the records made and
  refused, with the first refusal. An aggregate no route creates is not
  seeded; a request per record is slower than a bulk insert.
- **Baseline:** p95 latency and error rate per route are kept in
  `bench/baseline.json` and held like the god-type ceiling: a regression beyond
  the allowed margin (p95 20% above, error rate one point above) fails and
  leaves the file alone, an improvement lowers the baseline, a route no longer
  loaded drops out.
- **Linked to traces:** every k6 request carries a `traceparent`, so the
  slowest requests open as traces; a request over one second is printed with
  its trace id.
- **Never against production:** `sofie bench` refuses unless the target's
  environment is local, testing or staging.

k6 is an external tool run by the CLI, not a dependency of the app.

## Fuzzing

`sofie fuzz [--seed N] [--requests N] [ROOT]` sends generated requests to every
route and reports each 500, dropped answer (a panicking handler), crash of
the app, or success answering a signed-in member's request for another
member's record, with no test written. It runs the development app as `sofie dev`
builds it, on a free local port with `SOFIE_SHIELD=off`, and refuses a
`DATABASE_URL` not on this machine as `db fresh` does.

- **Routes:** every module route and, for an sofie-identity member area, its
  sign-in and sign-up routes. Two members per area are signed up and in
  (`fuzz-a-<area>@example.com`, `fuzz-b-…`); the second gets three records
  from each creating route first. One request in eight to a member route is
  sent signed out. A route of a `Permitted` area or one outside sofie-identity is
  fuzzed signed out only, and the report says so.
- **Inputs:** from the handler's `Input` type, field by field: a valid value
  (sofie-values' fixtures and accepted samples), boundary values, malformed ones
  (sofie-values' refused samples, control characters, markup, SQL), oversized
  ones (100,000 characters, numbers past 64 bits), a wrong JSON type, the
  field missing, an unknown field; and broken bodies (empty, truncated, not
  JSON, an array, nested 10,000 deep, a form, the wrong content type). A GET
  takes a query instead (paging, sorting and filtering gone wrong).
- **Path parameters:** an id the member's own list at the path before it
  answers, a foreign one only the other member's list answers, a random
  uuid, or a malformed id.
- **Answers:** page, JSON or fragment (`hx-request`), each request one.
- **Reproducible and bounded:** the routes take turns and every choice is
  drawn from `--seed` (a new one each run without it, named in the report),
  so a seed sends the same requests again apart from the ids the database
  lists; `--requests` is 300 by default.
- **Findings:** each names the route, method and path, the answer, signed in
  or out, the input's kind and body, the status, and the request's span tree
  and the app's log lines while it ran, by trace id. One finding stands for a
  route, outcome and kind of input; the rest are counted. The report also
  counts every status answered. A finding exits with 1.

## Scaffolded tests

`sofie scaffold tests` writes tests for what the app declares today, in
files named by the moment: `app/tests/scaffolded/<module>_<YYYYMMDDHHMMSS>.rs` (a dry run shows that form; the real run prints the name), one test crate whose `main.rs` the command keeps declaring
each file, appending the `mod` lines it lacks and leaving the rest as the app
wrote it. Scaffolded tests are extended in their file; hand-written tests go in
`app/tests/<topic>.rs`, which the tool descriptions and `AGENTS.md` say too. They say the style as well: each test is `#[test_app] async fn name(app: TestApp)`, as scaffolded ones are, and `#[tokio::test]` only where `#[test_app]` cannot run it (a multi-thread runtime, no app), with a line saying why. A run that fails part-way undoes what it wrote. A file is written once and belongs to the module afterwards: it
is never regenerated or overwritten, so a test written for last month's
schema still runs today. A test is a call of one of sofie-testing's checks
with the fact as declared; the checks are the behaviour. Transitions,
constraint bounds and the owner flag are read from the app model
(an `EnumDeclaration`'s variants and `path`, `FieldDeclaration.bounds`,
`Aggregate::is_owned`), not from tokens of their own.

What is scaffolded:

| From | Fact | Test |
|---|---|---|
| Schema and factory | `factory Product makes a valid record` | A fixture made by the default factory (an aggregate with a hand-written `factory.rs` is made through the module's route creating it) |
| | `constraint Product.name max 120` | `assert_boundary`: the value at the bound passes, the first past it is refused with the constraint's key (`required`, `min`, `max`, from `#[length]` and `#[range]` too); `unique` with `assert_unique`, a second fixture with the first one's value refused |
| Values and casts | `value Money` | `assert_value`: the value's `Samples` parse as they say; fixtures come back equal through JSON and through the database. sofie's values only: an app's own values have no samples yet |
| Status enums | `transitions OrderStatus PENDING>PAID,CANCELLED,EXPIRED …` | `assert_moves`: the declared moves are exactly the enum's, each succeeds and every other pair is refused |
| Schema | `schema Order` | `assert_schema`: the migrated table has every column the aggregate reads and writes, of its cast's type and nullability, and none it never writes that refuses null (`schema:check`) |
| Store | `store Order` | `assert_store` on two records: one saved with the other's fields loads with them one version on, and a save from the stale version is a `Conflict` |
| Routes and areas | `route order.customer.cancel_order …` | No principal is 401; another owner's record is 404; a `Permitted` area's principal without the permission is 403, and with it (`sofie_identity::testing::granted`, holding exactly what the route and the records its path names need) the call succeeds; the valid call (a member signed up, the records its path names made, the input a fixture) succeeds; a route behind a flag set off is 404. A resource's routes too, posting a fixture of its `Create` or `Update` input |
| Screens | `screen order/customer/orders renders` | The page renders as HTML with a record of each aggregate the area's routes create, and shows each field of the answered read model it writes bare as `{data.<field>}` whose type is text (`String`, `Email`, `Id`, an enum of the module), escaped as the engine escapes it; formatted fields (money, dates) are not checked. A list screen whose `Table` links each row to a GET route of its area gets a test rendering it for a member with one record and checking the row links to that record's page (`sofie_engine::table_links_with`) |
| Contracts | `lookup Order id,status` | `assert_lookup`: the batch lookup answers exactly those fields |

What no test can be scaffolded for is printed with why: an aggregate with a
hand-written factory that no route creates, a unique field of one, a call
holding a permission in an app whose sofie-identity does not sign members in
to the area, a route of a module whose contract exports no route builders.
The CLI lists every gap. MCP `scaffold_tests` lists in full the gaps of the
facts the run adds and of subjects nothing tests, and counts on one line the
gaps of subjects earlier files already test (`all: true` lists them too).

How the history is kept:

- Each scaffolded file lists the facts it covers, one line each with its
  test: ``//! fact `constraint Product.name max 120`: `product_name_keeps_max_120` ``.
  The next run compares the facts in the app with every file's list and
  writes a new file for what is new or changed only.
- When a fact changes, the old test asserting it starts failing. That is the
  signal: the behaviour changed, and the pull request shows the developer
  updating or removing that test deliberately. The generator lists such
  superseded tests and never deletes them itself.
- Scaffolded tests run on the real database, each test on its own database.

## Developer experience

### One app model, queryable

`sofie gen` builds a single model of the app (principle 8). Three tools read
it, and `sofie feature` changes the app's features:

- **`sofie explain <name>`** answers everything known about a route (by name or
  handler path), area, module, event, job, task, read model, aggregate (with the
  columns its behaviors add), screen (by key, `order/customer/orders`, with
  the route each `<Form>` and `<Action>` posts, that route's input fields and
  a body it takes, as a form draws the fields of its posted route's `Input`
  that its markup does not show), a
  sofie-identity screen by the key the app renders it under
  (`sofie_identity/web/sign_in`: its package, area, template and the routes
  showing it), a framework package's route (by name) or
  flag (by name, with the routes it hides and the jobs it skips): its files,
  permission, flag, owner scope, what it answers and the screens that show
  it; a route also names its handler's arguments and the input it takes:
  each field of its `Input` type, required or optional, with its JSON shape
  where the Rust type does not say it (`Money` as `{"minor":14990,"currency":"EUR"}`
  or `{"amount":"12.50","currency":"USD"}`, an enum's stored names), and a
  body it takes, made from the fixtures `sofie fuzz` sends, or, without
  one, the field a `#[derive(Value)]` argument reads, and the query
  parameters its `Query<Q>` or `Listing<T, Q>` reads. Flows join it when they
  are built.
- **`sofie feature add|remove NAME [ROOT]`** (MCP `feature`, with `dry_run`)
  turns a feature of the `sofie` facade on or off for the app: it checks NAME
  against the features `cargo metadata` lists for `sofie-framework`, edits
  only the features array of the app's `sofie` dependency in
  `app/Cargo.toml` as text (keeping comments and layout; inline, dotted or
  `[dependencies.sofie]` form), then runs `sofie gen` and `sofie check`.
  `sofie components` names it for a component whose feature is off.
- **`sofie mcp [ROOT]`** (sofie-cli feature `mcp`) serves the same model to coding
  agents over the Model Context Protocol on stdio (JSON-RPC 2.0, the
  `initialize`, `ping`, `tools/list` and `tools/call` subset), each tool
  built on the function its CLI command uses and reading the app afresh:
  read tools `explain`, `modules`, `routes`, `tasks`, `components`, `schema`,
  `screens`, `flows`, `tests` (with `name`, one test's file, lines and source),
  `check` and `docs_page`; `changes`, which lists the files added, modified,
  renamed or deleted under the app root since git's last commit (read-only
  `git status --porcelain -z` scoped to the app root, refusing outside a
  repository) and with `path` answers that file's `git diff -M HEAD`, or a new
  file's text; generate tools `new_module`,
  `new_migration`, `new_aggregate`, `new_event`, `new_listener`, `new_task`,
  `new_route`, `new_menu_entry`, `new_search`, `new_plan`, `new_mail`,
  `new_area`, `new_account_screen`, `new_impersonation`, `new_tenancy`,
  `scaffold_screen`, `scaffold_tests` (also taking `all`), `gen`, `fix`,
  `feature` and `split` (the CLI commands below), and `edit_screen`, which
  replaces one exact text of a screen template, each followed by `sofie gen` (and `sofie docs` for an app
  keeping docs) and `sofie check`, and each with `dry_run`, which runs them on
  a scratch copy of the app's Cargo workspace and reports the files they
  would create, change or remove, comparing the copy before and after the
  run, so a change made to the app meanwhile (as `sofie dev` regenerating
  it) cannot hide one; run tools `build`, `test` and `lint` (`cargo fmt` and
  clippy, formatting with `write`), which run
  `cargo --offline --message-format short` over the app's packages and
  answer only diagnostics, failures and result lines; `test` answers the
  passed, failed and ignored counts first, every failure in full, and the
  passed and ignored tests by file: every name in files git sees changed
  (the status `changes` reads), counts elsewhere, and every name under a
  `filter`, and fails when its
  `filter` matched no test; cut at 16 KiB; `fuzz` and `seed`, the CLI
  commands of the same names, run against a local database;
  `db_status`, which builds the development binary as `sofie dev` does and
  answers `<app> db status` run with the app's `.env`, cut at 16 KiB, and
  fails when a migration is edited or missing; `task`, which builds it the
  same way and runs `<app> task NAME` with `arguments` (a JSON object of
  scalars) passed as `--name value` process arguments, never through a
  shell. A table compared with its
  aggregate needs the compiled type, so that check is the scaffolded
  `schema` fact, run through `test`. App tools run the app as `sofie dev`
  does (one `DevApp` serves both), on a free local port passed as
  `SOFIE_ADDRESS`, refusing a `DATABASE_URL` not on this machine as `sofie
  seed` does (and so do `task` and `db_status`), with its stdout and stderr kept in a ring buffer of the
  last 2,000 numbered lines: `dev` starts (answering once `/live` does, or
  the exit and the log's last lines), stops or reports it (pid, address,
  last exit), or with `run` queues a schedule's job now through
  `<app> schedule run NAME` (sofie's `development`; `sofie schedule run NAME`
  from a terminal), which the running worker takes next whatever its
  interval, an unknown name answering the app's schedules; `logs` answers the lines after a cursor, filtered by text,
  and the next cursor; `traces` lists the recent span trees with their
  trace ids, or every tree of one trace, since a debug build logs each
  tree's line with a leading `trace_id` field; `request` sends one
  request over plain HTTP/1.1, starting the app or first acting on what
  changed since its last build (a rebuild for code, a reload signal for
  templates), with an `Origin` naming its host so the CSRF check passes,
  `Accept` or `HX-Request` for a `page`, `json` or `fragment` answer, a
  named cookie jar as `session` (cookie values are never shown) and its
  own `traceparent`, and answers the status, `location`, `content-type`,
  `x-request-id`, the cookies set by name, the request's span tree and
  the body cut at 16 KiB; its description, and a 400 or 422 answer, point
  to `explain` for the input a route takes. A failed build answers the compiler's errors,
  cut at 16 KiB. A fixed structure is the best ground an agent can work
  on: it is told where each kind of code goes, and `sofie check` catches what
  it puts in the wrong place. The `initialize` answer's `instructions` map
  each step to its tools (read, change, run, test, finish, rarely), and every
  tool's description starts with its step and when to use it; under each
  step the instructions list its tools by client name
  (`select:mcp__sofie__…`), so a client that defers tools loads a step in one
  search.
- **`AGENTS.md`** is written by `sofie gen` at the app root from the
  structure's rules and the app's own modules and areas, so agents and
  people read the same conventions; `sofie check` reports it stale like the
  wiring, and leaves one written by hand alone.

### Development toolbar

Development builds only; release builds do not contain it (feature
`toolbar`, compiled only with `debug_assertions`; sofie-http's
`tests/toolbar_release.rs`, run with `--release`, shows a release build
answers `/_sofie/toolbar` as not found and ignores its cookie). Under each
page it shows the request's span tree so far: the handler, the screen,
every statement with its SQL, the flags evaluated, the route's permission
check, every permission check a handler or screen made (a `permission` span
naming it, granted or refused) and the area. Its summary line names every override this browser
holds, so one is not forgotten while the toolbar is closed. Its forms post
to `/_sofie/toolbar` (same-origin, and from this machine only), which keeps
the overrides in a `__Host-<app>-toolbar` cookie of this browser alone, ending
with its session and sealed with a key of the process, so no client writes
one itself and a restart forgets them:

- act as a principal of any signed-in area, given its id (member or not;
  it is development only): the session then carries that principal, so
  guards, `Principal`, grants and flag rules see it; "Stop acting" returns
  to the signed-in one;
- answer a flag `on`, `off` or with a variant's name, over its stored
  rule (`Features::overridden` in sofie-flags' `toolbar`); the stored flag is
  untouched;
- set the clock to an instant or shift it by hours: the request's `Db`
  carries the app's clock shifted by that much (`Clock::shifted`), so
  handlers, loaders and commits run at that moment while it keeps running.

The span tree is the telemetry the app already records, so it adds no
instrumentation of its own.

### `sofie dev`

`sofie dev [--at INSTANT] [--remote-database] [ROOT]` runs `sofie gen`, builds
the app with `sofie/development`, and runs it with `SOFIE_MIGRATE=on` and the
keys of the app's `.env` the environment does not set. A `DATABASE_URL` on
another host is refused unless `--remote-database` is passed. It polls the modules, the app
crate and `web/` (no watcher dependency): an edited template reloads open
pages, since a debug build reads templates from disk (a mail too: with
sofie's `development` feature it rereads its template's source on every
send, so a `src/mails/*.html` edit needs no rebuild); an edited island is
rebuilt into the development directory and swapped in place, its islands
remounted with their props; anything else, or a file added or removed,
regenerates, rebuilds and restarts the app, keeping the running one when
the build fails. Pages learn of a change from the development client
(sofie-http's `development`, debug builds only), which holds a request to
`/_sofie/dev/reload` and reloads when the generation `sofie dev` writes moves or
the server's boot id changes. `sofie dev`, `sofie mcp`, `sofie fuzz` and
`sofie seed` stop and reap every long-running child process sofie started
(the app, cargo builds, `<app> task`, `schedule run`, `screen source` and
`node`) on Ctrl-C, SIGTERM or the SIGHUP of a closed terminal, then exit
with 130, 143 or 129. A SIGHUP ignored when sofie starts, as under `nohup
sofie dev &`, stays ignored for sofie and its children; sofie learns this
before installing any handler from a shell that sends itself SIGHUP, since
reading the disposition directly would need `unsafe`. A child whose output sofie captures for `sofie mcp`
runs in its own process group, and stopping it kills the whole group, so
cargo's test binaries and rustc stop with cargo and esbuild with `node`
(the island and client builds). A child run in the terminal stays in
sofie's group, so Ctrl-C reaches it directly. `sofie mcp`'s `cargo
metadata` is tracked in its own group. Other short-lived helpers (`git`,
`rustfmt`) and terminal tools (docker, k6, bench) are not tracked: an
interrupt reaches them through sofie's terminal group, or they finish on
their own.

### The clock

Every handler that needs the time asks for `Clock`, whose `now()` is an
`Instant`; nothing reads the system time directly. In return:

- `sofie dev --at 2027-01-01T09:00` runs the app at another moment, schedules
  included: the binary's `serve --at` (feature `development`) gives the
  app's and the worker's database `Clock::starting_at`, the system's time
  moved to start there and run on; an instant without an offset is read in
  the display zone (`SOFIE_TIME_ZONE`), as the toolbar's clock is: the
  binary reads its settings before its command.
- In a debug build the toolbar moves one browser's requests to another
  moment; the worker and other browsers keep the app's clock.
- Tests move time with `app.advance(Duration::from_hours(25)).await`: the
  clock stops at each moment a schedule, a retry or a delayed job comes due
  in that window, in order, and the worker drains there.

### Test harness

The harness knows the app model, so tests are short and typed:

```rust
#[test_app]
async fn customer_cancels_own_order(app: TestApp) {
    let customer = TestPrincipal::<Customer>::member(&app).await;
    let order = app
        .as_(&customer)
        .request(&order_contract::routes::customer::place_order())
        .json(&fixture_input::<PlaceOrder>(&app))
        .await
        .record_id();

    app.as_(&customer)
        .request(&order_contract::routes::customer::cancel_order(Id::<Order>::from_uuid(order)))
        .accept_json()
        .await
        .assert_ok();

    assert_raised::<OrderCancelled>(&app);
}
```

- `#[test_app]` runs the test on the app of the crate under test (its lib
  as the build script names it in `SOFIE_APP_CRATE`, else the package), as its
  wiring builds it (`MODULES`, `app`, `worker`; with flags when the
  wiring takes them), on a runtime of its own; `sofie_testing::start` and
  `start_with_flags` build one by hand. A wiring with flags gives the app
  and its worker one `Flags`, which `app.flags()` switches.
- `app.as_(&principal)` calls as a principal with a session sealed by the
  app's keys at the clock's moment, without signing in, so it keeps working
  across `app.advance`; a session the app issued (a `TestSession` from
  signing in) ends after 12 idle hours of test time like any other, so a test
  advancing past that signs in again or calls through `app.as_`; `TestPrincipal::fresh` is a new id and
  `TestPrincipal::member` (sofie-identity's `testing` feature, with
  `sofie_identity::testing::TestMember` in scope) signs an account up to an
  sofie-identity area and then in, as `sofie_identity::testing::signed_up`
  does for a chosen address; `sofie_identity::testing::granted::<A>(&app,
  &["shop.order.*.cancel"])` (with `permissions`) is a new member of the
  `Permitted` area `A` whose role holds exactly those grants, added without a
  sign-up; `app.principal_of::<A>(&session)` opens a session the app issued.
- Requests are built with the generated route builders and typed inputs.
- Contracts are replaced by fakes `#[contract]` generates beside each trait
  (`FakeOrders`, with `fixtures`): each call is recorded in `calls` and
  answered with a fixture, or fails once `calls.fail("method")` says so.
- Events are delivered with `app.drain()`, so a whole flow runs inside one
  test; a job that fails or panics fails the drain, and
  `sofie_testing::drain_failing(&app)` returns the failures instead; `assert_raised::<E>(&app)` finds every `E` committed, delivered or
  not, from an in-process log sofie-testing turns on (a rolled-back commit
  records none); `sofie_testing::raised::<E>(&app)` returns them without
  asserting, for a test that polls.
- Fixtures are deterministic from a seed (`SOFIE_TEST_SEED` picks another):
  each value of sofie-values has a realistic `Fixture` and `Samples` (feature
  `fixtures`), `#[derive(Input)]` and the default factory's input make
  theirs field by field held to the fields' constraints, and an enum's is
  its first state. `app.fixture::<Product>().owned_by(id).with("name",
  "Mug").await` makes a record through the default factory.
- Every test runs on the real database, each on its own; it is dropped on a
  thread of its own, never blocking the test's runtime (a connection left
  half open would hold the drop for a minute), and one a killed test
  process left is dropped by the next test process.
- `sofie-identity-unverified` is a test-only crate that builds sofie-identity
  with `password-reset` and `sign-in-links` but without
  `email-verification`; run it with `cargo test -p
  sofie-identity-unverified`, since a workspace run unifies features.

### Diagnostics

`sofie gen` and `sofie check` report like the Rust compiler: the file and line, what
is wrong, why the rule exists and the fix. `sofie fix` applies the fixes that have
exactly one form — a missing `mod` line for a module's manifest, config,
routes or menu; the app's missing dependency on a module or on the contract
defining an area it routes in; a contract's missing `mod r#gen;` and
`pub use r#gen::…;` and a `main` not handing tasks to the runner (both of
which `sofie gen` also writes); a stale generated file or documentation page — then
reports what `sofie check` still finds. Nothing that needs a decision is
changed. Still to come: a missing translation key drafted in every language.

### New apps and modules

`sofie new app <name>` creates the app's workspace (depending on the framework
by path until it is published, with the release profile `sofie build` uses),
its `app/` crate and an empty `modules/`;
`sofie new module <name>`, run in an app, adds the module with its manifest,
contract crate, config, a `web` area with an empty `routes.rs` and a
`screens/` folder, and folders for migrations and translations, then makes
the app depend on it. Both end by running `sofie gen`, so the result passes
`sofie check` and compiles.

Inside a module, `sofie new` writes stubs that compile and are wired, then runs
`sofie gen` and `sofie check`; it refuses to overwrite and validates names (snake
case for modules, functions and screens, Pascal case for types):

- `sofie new migration MODULE NAME`: an empty `migrations/NNNN_<name>.sql`,
  numbered after the module's last migration.
- `sofie gen migration MODULE NAME [ROOT]`: the module's next migrations,
  from the difference between its aggregates and what its migrations create,
  read from the migration files (no database): new tables with enum CHECKs
  and owner, reference, unique and behavior (`Slugged`, `UniqueCode`,
  `Default`) indexes; new columns nullable, or `NOT NULL DEFAULT` where a
  behavior fills old rows, the default's drop commented as a contract step for
  the version still inserting without it (the SQLite twin fills `now()` with an `UPDATE`);
  a `#[unique]` added over a plain `<table>_<column>` index as
  `<table>_<column>_key`, with the plain index's drop commented; a required column without a
  default added nullable with its contract step commented; CHECKs on
  existing tables `NOT VALID` with `VALIDATE` in a following file; each
  index on an existing table `CONCURRENTLY` in a file of its own; dropped
  columns only as a commented `DROP COLUMN`. Types it cannot read, bound
  CHECKs and changed enum CHECKs are written as comments, a changed column
  type or nullability as a note. The schema follows `DROP INDEX`, `DROP
  TABLE`, `RENAME TO` and primary keys. It refuses when
  there is nothing to write. A module with `migrations/sqlite/` gets a twin
  of each file, as of each migration `sofie new` writes (a step SQLite has
  no use for says so), since SQLite would otherwise run the Postgres file.
- `sofie new aggregate MODULE Name`: `src/<name>/mod.rs` with
  `#[derive(Aggregate)]` and `meta: Meta`, its `mod` line and root re-export
  in `lib.rs`, and a migration creating its quoted table with `id` and the
  default behaviors' columns. No `factory.rs`: one nothing calls is dead
  code, and with `sofie/factories` the derive writes the default one. Until code
  names the aggregate, `sofie check` reports it unused.
- `sofie new event MODULE Name`: a `#[derive(Event)]` struct in
  `contract/src/events.rs`, declared and re-exported by the contract, which
  then depends on `sofie`.
- `sofie new listener MODULE Event handler`: an `#[on]` function in
  `src/listeners.rs`, importing the event from the one contract declaring
  it, which the module then depends on.
- `sofie new task MODULE name`: a `#[task]` function returning `Ok(())` in
  `src/tasks.rs` (or `src/tasks/mod.rs`), the `tasks` feature on the
  module's `sofie` dependency and, for the app's first task, `main` calling
  `sofie::run_with_tasks` with the wiring's `tasks`, re-exported by `lib.rs`.
- `sofie new route MODULE AREA METHOD PATH handler [--screen NAME] [--fields FIELDS] [--read-model NAME] [--title TITLE] [--back HANDLER]`:
  PATH is relative to the area's prefix (`/orders/{order}` in an area under
  `/account`; a path repeating the prefix is refused); a handler
  returning `Ok(())` in `actions.rs` (writes) or `queries.rs` (`get`), or a
  file of its own when the module keeps `src/actions/` or `src/queries/`; a
  write whose path names an aggregate of the module (`{invoice}`) takes and
  returns it. The route joins `pub fn routes` in `src/areas/<area>/routes.rs`,
  created for an area the app knows; `--screen`, for a `get`, routes it with
  `routes.screen`, writes `screens/<name>.html` with an empty `Section`, and
  makes the handler a loader taking `_read: Read` and answering a computed
  read model named after it (`order_summary` answers an empty `OrderSummary`
  to fill in). A `get` whose path names an aggregate (`{order}`) loads it
  instead: its read model reads the aggregate's stored fields (or only the
  comma-separated `--fields`; without `--fields` it leaves out the owner
  field, a field naming another record (`Id<T>`, `AccountId`) and a secret (a
  field marked `#[sensitive]`, typed `Secret<…>` or of a value the app marks
  `#[sensitive]`); personal data such as an email stays; it names each field
  it leaves out in its output), and its screen shows them as label and value
  pairs, an instant as `{date(data.x)}`, the record's id only when
  `--fields` names `id`. The screen's `Section` is titled `--title`, which
  also opens the screen as its `<title>` and so names the browser tab
  (default: the screen's name humanized, for both) and starts with a link back to the area's page
  listing the aggregate: a parameterless `get` screen whose read model reads
  it (`#[read(from = Order)]`), else the page at the record path's plural
  (`/orders` for `/orders/{order}`); `--back HANDLER` names another
  parameterless page of the area and `--back none` leaves the link out. The
  output says which link it chose and why, or that it found none.
- `sofie new menu MODULE AREA handler LABEL`: `menu.entry("LABEL",
  <module>_contract::routes::<area>::<handler>())` appended to `pub fn menu`
  in `src/areas/<area>/menu.rs`, created with the area's import and declared
  in `lib.rs` when missing, and the `menu` feature on the module's `sofie`
  dependency (the app gets it through the module). It refuses a handler that
  is not a `get` route of the module in that area, a page with path
  parameters, and one the menu links to already.
- `sofie new area MODULE NAME ACCESS`: an area in the module's contract
  (`contract/src/area.rs`, declared in its `lib.rs`) under the prefix
  `/<name>`. ACCESS, one of `Access`'s variants, sets its principal:
  `public` (`sofie::http::Visitor`, `SIGN_IN` `/`), `owned`
  (`sofie_identity::Account`) or `permitted` (`sofie_identity::Permitted`,
  turning on sofie-identity's `permissions` in the contract). A member area
  signs in at `/<name>/sign-in`, and its first page is written as `sofie new
  route MODULE NAME get /home <name>_home --screen home` would write it, in
  `src/areas/<name>/`; members land there after signing in (the generated
  `MemberArea::HOME`). It makes the contract and the app depend on
  sofie-identity and the app on the contract, so `sofie gen` then lists
  sofie-identity's module and migrations and mounts its sign-in routes for
  each member area. Sign-up stays opt-in through `impl sofie_identity::SignUp`.

`sofie scaffold screen <module>/<area>/<screen>` writes a resource's built-in
screen to `screens/<screen>.html` in the module's area, which then overrides
it. The source comes from the app's own binary (`<app> screen source <key>`,
with `sofie`'s `resources`, which builds the app without serving it and prints
`App::built_in_screen_source`), since only the compiled app holds the
resource's description. A dry run (`sofie mcp`) asks the original app for the
source and writes it into its scratch copy, so nothing builds outside the
app's own target directory.

### Upgrades

The structure is fixed, so a framework upgrade can rewrite apps. Each release
that changes an API ships codemods; `sofie upgrade [--write] [ROOT]` applies them with `syn`, shows
the diff and runs `sofie check`. Catalog components are versioned the same way. Upgrading is running a command and reviewing a diff.

Built: `sofie upgrade` runs every codemod over the app's and its modules'
Rust files and prints each change as the line before and after, naming the
codemod and the breaking change it carries; `--write` writes them, runs
`sofie gen` and reports what `sofie check` still finds. A codemod that brings
in another crate's path adds that crate to the rewritten crate, the app and
the workspace's dependencies in the same write. A codemod rewrites only
the old form of an API, by `syn` spans so the rest of a file keeps its
formatting, so running it on upgraded code changes nothing and an app keeps
no record of which ran. Two ship: `content-security-on-serving` (e586a89)
and `password-from-identity` (1eb6a1d, including the import a file using
`Password` through `sofie::prelude::*` now needs). A `use` naming a moved
item inside a group of others is left for the compiler to point at. Manifest
codemods run first, over every `Cargo.toml` under the root, since `cargo
metadata` refuses the app until they are written; one ships,
`concurrent-indexes-dropped` (11fce7b), and while it has changes to show, a
run without `--write` shows only them and says the source changes are planned
once they are written. No
catalog component has changed yet, so no template codemod exists; the first
one walks templates the same way.

### Build and deploy

- `sofie build [--image] [ROOT]` refuses a stale `sofie gen` and any `sofie check`
  finding, then runs `cargo build --profile dist` for the app and prints the
  binary's path and size. The binary carries the screens (their compiled
  programs, written by `app/build.rs` into `OUT_DIR`, `include_bytes!`;
  package screens ship through `build.rs` and the module's `screens`, with
  no public `SCREENS` constant),
  island bundles (built by `app/build.rs` into `OUT_DIR`), migrations and
  translations (`sofie gen`'s wiring); only a `development` build reads
  templates or island bundles from disk. It builds with the workspace's
  `dist` profile: fat LTO, one codegen unit and stripped symbols: the
  shop's binary goes from 6.6 MB to 3.4 MB and its CPU per request falls
  7–14%. `panic = "abort"` would save another 0.6 MB but turns a panicking
  handler, which today ends only its own connection, into the end of the
  process; it stays off. `release` keeps Cargo's defaults, so building while
  working skips the single-threaded fat-LTO link (about half of the shop's
  28 s clean build, measured 2026-10-12). `sofie new app` writes the `dist`
  profile; for an older app's root manifest, `sofie build` names the lines
  to add.
- `--image` builds nothing on the host: it writes `Dockerfile` and
  `Dockerfile.dockerignore` beside the app once and never rewrites them, then runs `docker build` from the workspace
  root (the app reaches the framework and its modules by path) when docker
  is installed, refusing first when a path dependency lies outside that
  root (as the framework does for an app `sofie new app` made elsewhere),
  since docker could not copy it in. The ignore file keeps `.git`, `target`, `node_modules` and
  every `.env*` but `.env.example` out of the build context, so an
  environment's secrets never reach the builder's cache. The builder is
  `rust:1-bookworm` pinned by digest:
  the file is written with a `<digest>` placeholder and `sofie build --image`
  refuses until it is replaced. The runtime is
  `gcr.io/distroless/cc-debian12:nonroot`, pinned by digest the same way: glibc and libgcc (the binary links
  glibc dynamically, so not `static`), CA certificates and tzdata in
  `/usr/share/zoneinfo`, which time zone values read; no shell, no package
  manager, user `nonroot`, `SOFIE_ADDRESS=0.0.0.0:8080`. The binary writes
  nothing to disk, so it runs with a read-only root filesystem. There is no
  `HEALTHCHECK` (nothing in the image could run one): the orchestrator
  probes `/live` and `/ready`. An app with islands also gets Node in the
  builder for the island toolchain.
- The process tunes itself from `SOFIE_RUNTIME_THREADS` (one per logical CPU
  when unset), `SOFIE_DB_POOL_SIZE` (twice the runtime threads),
  `SOFIE_DB_POOL_WAIT_MS` (5000: past it a request is answered 503 with
  `Retry-After`, never left waiting), `SOFIE_WORKER_BATCH` (2) and
  `SOFIE_WORKER_POOL_SIZE` (a pool of the worker's own; the app's when unset).
- `sofie config check --env production` validates every module's config against
  an environment before a deploy, naming each missing or malformed key.
- `--worker` builds `<app>-worker`, a binary that only runs jobs:
  `src/bin/worker.rs` calls `sofie::run_worker(MODULES, worker)`; an app
  with feature flags calls `sofie::run_worker_with_flags(MODULES, worker)`
  instead, and `sofie build --worker` names whichever the app needs. The app declares it
  behind a `worker-binary` feature, so other builds skip it. It answers
  `serve` and `schema check` and never builds the app, so the HTTP server and
  screens are left out of it.
- `--pgo` builds the binary instrumented, runs it under `sofie bench`'s load
  (endpoints and journeys) with `.env`, `SOFIE_MIGRATE=on` and
  `SOFIE_SHIELD=off` on a database on this machine, stops it with SIGTERM so the profile is written, merges
  the profile with `llvm-profdata` (`rustup component add llvm-tools`) and
  rebuilds with it. Both builds pass `--target <host>`, so the profile flags
  reach the app's code but not build scripts or macros, and land in
  `target/<host>/dist` without invalidating the ordinary `dist` build. The
  profile flags join the rustflags cargo would read (`RUSTFLAGS`, else
  `build.rustflags` or `target.<host>.rustflags` of `.cargo/config.toml`),
  rather than replacing them.
  C and C++ a build script compiles stay unprofiled (`-fno-profile-generate
  -fno-profile-use` after any `CFLAGS`): the `cc` crate hands rustc's profile
  flags to the C compiler, whose raw profile format need not match rustc's,
  and the mix crashed the profile writer at exit (seen with Apple clang).
  It needs k6 and a local database, and does not combine with `--image`,
  which builds inside the image.
- With sofie's `tls`, which the shop turns on through its default features
  (`default = ["sofie/tls"]`; `bench/` builds it with `--no-default-features`,
  so the comparison with Loco stays on plain HTTP), the binary serves HTTPS on its own address from
  `SOFIE_TLS_CERTIFICATE` (the chain, leaf first) and `SOFIE_TLS_KEY` (PKCS#8,
  PKCS#1 or SEC1), both PEM files read at start; one without the other
  refuses to start. rustls with ring, TLS 1.2 and 1.3, HTTP/2 by ALPN, 10 s
  for a handshake; a failed handshake counts for the shield, one that ran
  out of time does not (a slow network, not an attack). A renewed
  certificate is picked up without a restart: at most once a minute a
  handshake compares the files' modification times and, if they changed,
  reads and pairs them again; a pair that fails keeps the certificate held
  and logs an error naming both paths. Behind a proxy that ends TLS, leave
  it off.
- The binary is `sofie::run(MODULES, app, worker)` (`sofie::run_with_flags`
  for an app with feature flags, which also answers `flags settled`): `serve --role web |
  worker | all` (no arguments: `all`) chooses what a process runs, and every
  role builds the app, so a worker fails at boot on what a web process would.
- It migrates on start only when told to, with `SOFIE_MIGRATE=on`; unset, it
  does not, so which process migrates, and when, is the deployment's choice.
  It refuses to start on a database missing a migration, naming the first.
  A test database is migrated by the test's own `app` function.
- `sofie db fresh` runs the app's binary, built with sofie's `development` as
  `sofie dev` builds it, as `db fresh`: it drops the schema of
  every module and the framework's, then migrates again, and refuses a
  `DATABASE_URL` whose hosts are not this machine (localhost, 127.0.0.1, ::1
  or a socket), read as the driver reads it: one with a `hostaddr` off this
  machine or a `service` is refused too.
- `<app> db status` (sofie's `development`) prints per module, the framework's
  `sofie` first, each migration applied, pending, edited after it was applied
  (its checksum differs) or applied but missing as a file, from
  `Schema::migration_status` (sofie-db feature `migration-status`), and exits
  non-zero when one is edited or missing.
- `<app> schema check`, in every build, compares the tables of every
  aggregate the migrations list, their child tables too, with the database
  `DATABASE_URL` names, prints a line per mismatch, naming its `module.table` and column, and exits
  non-zero on any. `sofie schema check` builds the app as `sofie dev` does and
  runs it with the app's `.env`.
- The binary serves `/live` and `/ready`, and
  shuts down gracefully: it stops accepting, drains requests and jobs, then
  exits.
- Three performance switches of `sofie`, each off by default. Measured on
  the shop (docs/gaps.md, perf14), `mimalloc` serves pages 16% faster and
  `compression` makes them 9× smaller, but each pushes RSS past the limit
  against Loco and `compression` also lowers throughput:
  `mimalloc` (the global allocator); `compression` (Brotli, else gzip, by
  `Accept-Encoding`, for HTML, JSON, CSS, JavaScript, SVG and plain text of
  1 KiB or more, `Vary: Accept-Encoding, Sec-Fetch-Site`; the engine's CSS and
  JavaScript are compressed by sofie-engine's build script and embedded
  beside the originals); and `etag`. Against BREACH, an answer to a
  request another site started (`Sec-Fetch-Site` neither `same-origin` nor
  `none`) is never compressed, and CSRF protection reads headers, so no token
  sits in a body.
- With `etag`, every `GET` of a JSON route or a screen (none can write) is
  tagged `W/"…"` from every row its loader read (each aggregate's id and version
  among them, lookups through other modules included: sofie-db fingerprints the
  rows a request's task reads), the binary, the principal and the request
  headers in its `Vary`; it is `private, no-cache`, so the browser alone keeps
  it and asks again each time. A held tag is answered `304` before the screen
  renders. In an app holding flags (`Gates::features`) the tag is also keyed
  by the flags' version (`Features::version`, a hash of every rule and stored setting, equal on
  every instance), since a switched flag changes a page without a row read.
  A live page's tag is keyed by its seal's end, and every tag by the
  principal's entitlements, read before any row. Off with `development`,
  whose templates change on disk, and for a `.cached(…)` screen, whose hits
  read no rows. A sofie-cache hit keys the tag by the value it gave; an
  answer that used a sofie-client reply (another app, a vendor) is not
  tagged, since nothing here versions that reply.

## Mobile

Planned after v0.1, as the opt-in `sofie-mobile` crate. Tauri wraps an area in a
native app; Tauri is a dependency of the mobile shell only, never of the server.

1. **Remote shell.** The app loads an area from the server in a webview. No
   new area is needed: mobile is another shell for the same audience, signing
   in through a token guard the area accepts beside its session guard. Native
   capabilities — push registration, biometrics, camera, share — are catalog
   components backed by Tauri plugins, with a browser fallback. Deep links are
   generated from route names. UI changes ship without a store release.
2. **Embedded rendering.** The engine is a Rust library, so it also compiles
   for the device: screens render locally from data fetched through the API,
   start faster and can be cached for offline use.

App stores reject apps that only wrap a website, so the first stage must lean
on native capabilities and the second is the long-term shape. Because screens
are templates rendered at runtime, templates downloaded after release could
update the UI without a store release; that needs checking against store rules
on dynamic code before it is offered.

## Module declaration and configuration

A module declares itself in one file and carries its configuration with it, so
that splitting it out moves files rather than untangling them.

```rust
// modules/order/src/module.rs, declared by `pub mod module;` in lib.rs
pub const MODULE: Module = Module {
    name: "order",
    tier: Tier::Core,
    context: Some("commerce"),
    settings: &[Setting { name: "free_shipping_above", default: SettingValue::Number(5000) }],
    ..crate::wiring::GENERATED // `sofie gen`'s src/wiring.rs, declared by `mod wiring;`
};

// modules/order/src/config.rs, declared by `pub mod config;` in lib.rs
#[derive(sofie::Config)]
pub struct OrderConfig {
    /// Printed before every invoice number.
    invoice_prefix: String,
    /// Hours a placed order may stay unpaid; 24 when unset.
    payment_window_hours: Option<u64>,
    payment_api_key: Secret<String>,
    database: Database,
    queue: Queue,
}

// a handler or job asks for it by type
pub async fn expire_unpaid(read: Read, config: Config<OrderConfig>) -> Result<Vec<Order>>
```

- **Keys are prefixed by the module.** `OrderConfig` reads `ORDER_*` and only
  those: `ORDER_INVOICE_PREFIX`, `ORDER_PAYMENT_API_KEY`. Two modules cannot
  collide, and a split takes exactly its own keys. A field of a struct
  declared in the same file, deriving `sofie::Config` too, is a group:
  `payment: Payment` reads `ORDER_PAYMENT_*`.
- **Infrastructure is per module and inherits by default.** `Database` reads
  `ORDER_DATABASE_URL` and falls back to the app's `DATABASE_URL`; `Queue`
  reads `ORDER_QUEUE_URL` and falls back to `DATABASE_URL` too, since the
  messages live in a database. In a monolith nothing is set and everything is
  shared; after a split the module points at its own database by setting one
  key, with no code change. Before a split, an own `ORDER_DATABASE_URL` or
  `ORDER_QUEUE_URL` that differs from `DATABASE_URL` stops the start with an
  error saying it applies after `sofie split`; setting it equal is allowed.
- **Declared once.** The struct, deriving `sofie::Config`, is the only list of
  keys; `sofie::config` loads it behind the `sofie` feature `config`. The derive
  only describes the struct (each field's key, whether it is optional, its
  kind, the `Database`/`Queue` fallback) and builds it from parsed fields;
  the library reads and parses each key and reports a missing or malformed
  one, with no serde. A field is text, a number, `bool`, a `SocketAddr`, a
  `Secret` of one, an `Email` or `Currency`, a group, or an `Option` of
  any. A key may stay
  unset only when its field is an `Option`; the module applies its default
  where it reads the value. `sofie gen` reads the same struct to write the
  module's `.env.example` (with each field's doc comment, an optional key
  commented out) and the app's at the app root, as the framework's keys and
  the union of its modules'.
- **Fail at boot, never later.** The generated wiring loads every module's
  config before the app is built, in every role; a missing or malformed key
  stops the app before it serves anything, naming the module and the key. A
  `Secret` is never printed, logged or serialized, and no error repeats a
  value. `sofie config check --env production` holds `.env.production` at the
  app root to the same keys before a deploy, naming each missing or malformed
  one; a value type's own rules are checked at boot only.
- **A module reads only its own config.** Handlers and jobs ask for
  `Config<OrderConfig>`; no code outside the framework's loader reads
  environment variables.
- **One real `.env` per deployment.** Modules own the declaration and the
  example, not the values: a deployment's secrets stay in one place, and a
  split app gets its own `.env` built from the keys its modules declare.
- **Settings are declared in the manifest.** Each has a name unique in its
  module (`sofie check` refuses a repeat) and the value it holds until a
  permitted principal changes it. Behind `feature-flags`, values so
  changed are stored in `sofie.settings` (name, value as JSON, `changed_at`) and
  held by `Flags` in the same snapshot as the flag rules, refreshed by the
  same wake-ups. `sofie::declare_settings(&flags, MODULES)`, which `sofie gen` writes into
  `app` when a module declares settings in an app with flags, names the
  settings a permitted principal may change (`<module>.<setting>`) and `Flags::change_setting`
  stores one, read as its declared default's kind (`true`/`false`, a whole
  number, text) and refused otherwise. A handler takes `sofie::RuntimeSettings`
  (in a GET too) and reads `settings.number(&MODULE, "free_shipping_above")`
  from the snapshot its request started with: the stored value, or the
  manifest's default when none is stored.

- **Read from source.** `sofie::Module` and `sofie::Tier` (sofie-http's, the
  same `Module` the app registers) make the manifest compile; it spreads the
  generated `src/wiring.rs`, which names the module's routes, listeners and
  jobs from inside the crate, so `areas`, `listeners` and `jobs` may be
  private modules (`sofie check`'s `visibility` rule names what a private
  module below the crate root hides). `sofie check` reads it without compiling and reports a module without
  one, one no `mod` reaches, and one whose `name` is not the crate name (the
  name is its schema and key prefix). `context` names the part of the
  business a Core module belongs to, or is `None`. Features join the
  manifest with flags.

`sofie split order` therefore starts from `module.rs`, takes the crate pair, the
area folders, migrations, translations, `config.rs` and `.env.example`, and
generates a new app whose only module is `order`, with the contract served
remotely to the app it left.

### Splitting a module

`sofie split <module>` prints a plan and changes nothing; `sofie split <module>
--write` carries it out (the MCP tool `split` does the same with `write`);
a failure part-way, `sofie gen`'s findings on either app included, undoes
every move and edit, as a failing `sofie new` or `sofie upgrade --write` does.
The plan names what moves, what is served over HTTP, the contract methods
that cannot cross apps (not `async`, generic, taking `&mut` or a borrowed
type, or answering other than `Result` of an owned type; their callers stop
compiling), the reverse-proxy prefixes and the keys each app needs. It also
lists the app's test files naming `<module>::`, which no longer compile once
the app stops depending on the module; it does not move them.

- **Where it goes.** The module folder (crate pair, areas, migrations,
  `lang/`, `config.rs`, `.env.example`) moves to `split/<module>/modules/`,
  beside a new app crate `split/<module>/app` named `<module>-app`, whose
  manifest copies the app's package keys, `sofie` dependency, build
  dependencies and lints; relative paths in the moved manifests are
  rebased. Both apps stay members of one Cargo workspace, whose `members`
  gain the new crates, so the contract crate is shared by path; when the
  module was the app's last, its `modules/*` members leave the workspace, as
  Cargo refuses a glob matching nothing. Then `sofie gen`, and `sofie docs` where
  an app keeps its docs, run in both apps.
- **Data, not code.** Each app's `[package.metadata.sofie]` says what it
  `serves` and which modules are `remote`; `sofie gen` reads it. The app left
  drops the module's crate, keeps its contract, turns on sofie's
  `remote-contracts` and lists the module as `remote`; a module of it that
  the split module calls is listed as `serves` there and as `remote` in the
  new app. Sign-in stays where `sofie-identity` is; the session cookie is read
  by both apps, which share `SOFIE_SIGNING_KEYS`.
- **Wiring.** For a `remote` module the wiring calls
  `sofie::remote::connect("<module>")?` (keys `<MODULE>_REMOTE_*`: `BASE_URL`,
  `PRIVATE=true` for plain HTTP inside the network, and an `Outgoing`'s
  other keys, which the app's `.env.example` and `sofie config check` list),
  provides `sofie::remote::Remote<dyn Trait>` for each contract
  trait that crosses apps and `RemoteLookup<Dto>` for each exposed DTO, and
  the worker's `Feeds` `.pull(sofie::remote::feed("<module>"))`. For a served module it
  registers `Served::default().contract::<dyn Trait>()…lookup::<Dto>()
  .feed().serve(&db)` and `.publish("<module>.<Event>")` in the worker's
  `Feeds`, attached to it with `Feeds::attach`, for each
  of its contract's events. An app sofie-testing starts connects nowhere; a
  test provides the contract's fake.
- **Reverse-proxy table.** `proxy.toml` beside the app left maps each path
  prefix of the module's routes (up to the first parameter) to the new app,
  `default` naming the app left; a later split adds to it. The plan warns
  when a prefix also starts a route the app keeps. `/_sofie/` is never routed
  from outside the network.

## Areas

An area is an audience of the app. Everything an endpoint needs to know about
who may call it, and on whose records, comes from its area rather than from the
endpoint.

**The framework ships one area: `Web`,** the app's face to the outside world.
Every app has it and every module can contribute to it, whatever its tier,
because it belongs to the framework rather than to a module:

```rust
// sofie-http
pub struct Web;

impl Area for Web {
    const NAME: &'static str = "web";
    const PREFIX: &'static str = "";
    const SIGN_IN: &'static str = "/";
    type Principal = Visitor;
}
```

- It holds what anyone may reach: public screens, sign-in and sign-up,
  webhooks, the JSON endpoints an outside client calls without an account.
- Its principal is a `Visitor`: anonymous, or carrying the account of a
  recognized session so a page can greet someone. A visitor never gains access
  to records through it; anything that acts as someone belongs in another area.
- The framework's own endpoints (`/live`, `/ready`, the metrics endpoint,
  static files) are not part of it and cannot be shadowed by it.

Every other area is the app's. The example app adds `Customer`, and the
examples here add a `Permitted` area named `Console`;
another app may add `Tenant`, `Partner` and `Operator`, or nothing at all —
a public site or a public API is just `Web`.

**An app's own areas are defined once, in an app module's contract.** Their
principal is the account of the framework's identity, `sofie-identity`
([D2](scenarios.md#d2-identity-owns-areas)), which `sofie gen` wires into an app
that depends on it: its migrations, its sign-in and sign-up screens (an app
replaces one with `src/areas/web/screens/sofie_identity/<name>.html` in a module,
which takes the built-in screen's key; app screens sharing a key are still
refused) and, per member area (a contract area that implements or gets a
`MemberArea`, `AreaDefinition::is_member_area`; a public area is none), its
`MemberArea` (its `HOME` and optional
`SIGN_UP`; `sofie gen` writes it into the contract's `src/gen/mod.rs`, declared by
`mod r#gen;`, when the contract leaves it out: `HOME` is the area's first
screen without parameters, and sign-up stays closed unless the area
implements `sofie_identity::SignUp` with its `PATH`), sign-in at `SIGN_IN`, sign-up at `SIGN_UP` and
sign-out at `<PREFIX>/sign-out`, all in `Web`. An account signs in to an area
only as its member, and signing up at an area's `SIGN_UP` path makes it one.
Signing up signs nobody in and answers alike, after hashing the password
either way, whether or not the address has an account: a new address gets its
account, a taken one changes nothing. The browser goes to `{PREFIX}/link-sent`
with `email-verification` and to `SIGN_IN` without. Only a sign-up racing
another for one address to the unique index is answered "is already
registered". Passwords are hashed with Argon2id (19 MiB, two passes, one lane)
in the PHC string format, at most one hash per core at a time; a PBKDF2 hash
stored before still verifies, and a sign-in that proves it stores an Argon2id
one in the same commit.
`sofie gen` maps the folder `areas/<NAME>/` to the type; two definitions of one
name are a generation error, and the tier rules apply unchanged: a module
contributes only to areas whose contract it may import. A `PREFIX` is `""`
or starts with `/` and doesn't end with one (`sofie check`'s
`area-prefix`); prefixes are compared as written, and an area at the root
collides with the built-in web area.

```rust
// modules/order/contract/src/area.rs
pub struct Customer;

impl Area for Customer {
    const NAME: &'static str = "customer";
    const PREFIX: &'static str = "/account";
    const SIGN_IN: &'static str = "/sign-in";
    type Principal = sofie_identity::Account;
}

impl sofie_identity::SignUp for Customer {
    const PATH: &'static str = "/sign-up";
}
```

A handler asks for `Principal<Customer>`, resolved from the session; without
one, a browser is sent to `SIGN_IN` (with `next` for a page asked for with
`GET`), htmx is told to go there and an API client gets 401. Once signed in,
the browser returns to `next` only when it is a page of this app that the new
session opens: a public page, or one in the area just signed in to (the
cookie opens one area, so any other would bounce back to sign-in). Anything
else, escaped or not, sends it to `HOME`. No database is read to decide.

The session lives in its sealed cookie, which carries an id chosen at sign-in.
It ends after 12 hours without a request (the cookie is refreshed at most every
five minutes) and 30 days after sign-in, whichever comes first
(`sofie-auth`'s `session.rs`).
With sofie-identity, whose `sessions` is on by default (and turns on sofie's
`session-list`), each sign-in is an `account_session` row with its device,
every request asks it (`AreaMembers::session_record`), and an ended one is
cleared like an expired cookie: signing out, `sessions::routes` ("sign out
everywhere else", feature `session-screens`), a password reset, closing the account, ending an
impersonation and removing a membership (`remove_membership`, or a
sofie-tenancy member removed from an organization) end sessions for every copy
of their cookie, each in the commit of its change. A cookie a request re-sealed
after its session ended is refused all the same, since the id it carries is
ended. Session records are kept after they end, so a cookie still carrying
one is refused; the hourly job `sofie_identity.prune_sessions` (registered
by `sofie_identity::worker`) deletes those unseen for longer than the
session cookie's absolute lifetime (`SessionClaims::ABSOLUTE_LIFETIME`, 30
days) plus an hour, after which no cookie can carry them.

The answers are cached per app instance (`session_cache.rs`), so a request
whose session is held reads nothing. An entry is trusted for a minute by the
database's clock, the bound on a notification another instance failed to send;
at 100 000 entries expired ones go first and a new one is not kept while none
has. Every commit to `account_session` announces its table and owner after the
commit (sofie-db's `live`, `pg_notify('sofie_live', …)` to other instances):
the cache drops that account's entries, draining the announcements under its
lock on each lookup, so the request after a sign-out on this instance never
finds the ended session, and from a task woken by each announcement, so a
quiet spell does not lag the feed. A missed announcement, a lagging reader, or
listening that is down or not yet up (`sofie_db::hears_other_instances`) drops
everything and caches nothing until it holds. Last-seen is still written at
most every five minutes, when the cookie is refreshed, which bypasses the
cache.

An app can turn it off for stateless, API-only use
(`sofie-identity = { default-features = false, … }`): sessions then live only
in their cookies, which brings back four gaps. A copied cookie outlives signing
out; "sign out everywhere else" and a password reset end nothing open
elsewhere; removing a membership ends nothing; and a request sent before
signing out and answered after it re-seals the cookie and signs the browser
back in.

Every way of signing in ends in one place, which checks membership: a
password, a mailed link (`sign-in-links`: `{PREFIX}/sign-in-link`, fifteen
minutes, used once, confirming the address), a passkey and an OAuth provider.
A first step that is not a second factor itself sends an account with a
confirmed second factor (`totp`) to a signed five-minute page at
`{PREFIX}/two-factor/{account_link}` asking a code from an authenticator app
(RFC 6238, one step either side, each code once) or a recovery code; five
wrong codes in fifteen minutes stop codes for the account. A sign-in route
redirects as before (`next` included) unless a second step is due, which it
replies as a local `location` (`then_redirect_or_follow`) with the `next`
signed in, so the proved code goes there; a JSON caller gets 204, or
`{ "location", "second_factor": true }`.

A member's own screens, each mounted for a member area with `sofie new
account-screen AREA SCREEN`: `sessions::routes`, `two_factor::routes` (set up
and confirmed only by a session signed in within the last ten minutes, so `totp`
records sessions; the key sealed with the app's keys and sealed again with the current one on each use,
key and recovery codes shown until confirmed, and only to such a session (the setup screen shows the
`otpauth://` address as a QR code, drawn by an own encoder in byte mode at level M as inline SVG,
beside the key as text), then kept as digests of 80 random bits
each and checked before the key is opened), `passkeys::routes` (WebAuthn: ES256, EdDSA, RS256, attestation
"none"; added, like a provider on `oauth::routes`, only by a session signed in
within the last ten minutes, so both features record sessions; sealed challenges, so nothing is stored between page and answer; a
credential refuses a challenge older than its last and a counter that goes
back; a user-verifying passkey signs in without the second factor; the
browser side is `engine.js`), `oauth::routes` (connected providers) and
`account_data::routes` (an export of the identity's records, tokens and the
second factor without their secrets, and impersonations, with each
registered `PersonalData`, and closing the account: anonymized rather than
deleted, so other modules' records stay whole, all in one commit raising
`AccountClosed`; with the `notify` feature, a registered
`account_data::Notifications` exports and erases sofie-notify's rows and stops the digest).

**Impersonation** (`impersonation`, with `permissions` and `sessions`): a
principal of a `Permitted` area holding `sofie_identity.impersonation.*.create`
signs in as a member of a member area with a reason (`sofie new impersonation
PERMITTED MEMBER` mounts `impersonation::routes` and `stop_routes`). Every use is
an `impersonation` row the member owns and raises `ImpersonationStarted` and
`ImpersonationEnded`; the session is listed to the member as impersonated,
ends by itself after thirty minutes, and `/impersonation/stop` resumes the
impersonator's own session if still live (`Session::resume`: same id, first sign-in kept, so
no recent sign-in is faked), else signs out. sofie-http's `SessionRecord::Live` carries `impersonator`, the account
signed in as the session's account, if any, and every request made in such a session is logged at
info with `impersonation.account` and `impersonation.impersonator` (account ids only). The
impersonated session adds or removes no passkey,
OAuth connection or second factor, sees no factor being set up, ends no
session, revokes no token, exports nothing and closes no account (a `Rule`
refusal), so nothing it does outlasts it. Removing the impersonator's
membership of any area, closing its account or ending all its sessions ends
the impersonations it started. A member of the impersonating area is never
impersonated, nor a member whose role's grants in its area are not all covered
by the impersonator's own permissions, the rule granting follows. The member's
permissions at the start are kept (`impersonation.granted`); once the member
holds one more, the impersonation ends when its session record is next read
(at most a minute later, through the session cache). The dev toolbar's "act as" stays a debug-build tool.

**Organizations** (`sofie-tenancy`): an organization area's principal is the
organization (`type Principal = sofie_tenancy::Organization;` and
`impl OrganizationArea`), so `#[owner] organization: OrganizationId` records
belong to it and every member reaches them; the account acting is read from
the session record (`Acting`), never from the cookie alone. A member area
lists, founds and enters organizations and accepts invitations
(`member_routes`); the organization area manages members
(`organization_routes`). `sofie new tenancy ORGANIZATION MEMBER` mounts both.
A member leaves (`/members/leave`); an owner hands the organization over to
another member and stays an admin (`/members/{organization_member}/handover`),
renames it (`/organization/rename`) and deletes it (`/organization/delete`):
the organization is deleted softly under its row's lock, every member leaves
and their sessions acting for it end, its open invitations are withdrawn and
`OrganizationDeleted` is raised (with `billing`, refused while its plan renews
at the provider, and a plan whose checkout completes after the deletion is canceled at
the provider by the `sofie_tenancy.cancel_deleted_plan` listener; inviting to or accepting for a deleted organization is 404); records it owns in other
modules stay until those modules erase them on that event. Accepting an
invitation needs its signed link and an account of the invited address, and
confirms that address.
Role changes, removals, invitations and acceptances take the organization's
row and check again in their commit that an owner remains, the seats hold
(an invitation accepted after the plan's seats shrank is refused), an
address has one open invitation and its inviter may still send it. Accepting while already a member keeps the
higher of the two roles. A member removed has its open invitations withdrawn,
and one given a lower role those its new role could not have sent. With
`account-data`, an app registering `OrganizationMemberships` exports an
account's memberships and ends them as it closes, withdrawing the open
invitations it sent; the sole owner of organizations cannot close its
account, and the refusal names them (`PersonalData::closing`).

**OAuth and OpenID Connect** (`oauth`): `sofie_identity::oauth::install(&db,
OAuth::from_environment()?.…)` at boot (`SOFIE_OAUTH_PROVIDERS`, per provider
`CLIENT_ID`/`CLIENT_SECRET`; Google, Microsoft, GitHub or any OIDC provider by
its endpoints), and the sign-in page shows a button per provider. The
authorization code flow uses PKCE, a sealed state and a ten-minute flow cookie
(sofie-http's `flows`), so a callback begun in another browser signs nobody
in; the callback page posts the code on. A new confirmed address signs up; an
address an account here holds is refused until that account connects the
provider from `oauth::routes`, which connects it only while the account is
still open and a member. OIDC ID tokens are verified: the provider's keys
come from its discovery document (`{issuer}/.well-known/openid-configuration`
→ `jwks_uri`; Google and Microsoft use their known key URLs; keys on another
host than the issuer are fetched as public HTTPS only), are cached for
an hour and fetched again once on an unknown `kid` (at most once a minute).
RS256, RS384, RS512 and ES256 are accepted; `none` and HMAC are refused, as
are keys whose type or `alg` does not match. Then `iss`, `aud` (and `azp`
with several audiences), `exp` and `iat` (60 s leeway) and `nonce` are
checked; a refusal is logged with its reason, never the token.

**Mailed links** (sofie-identity's `password-reset` and `email-verification`,
off by default; they need sofie's `mail`): per member area, `{PREFIX}/forgot-password`
and `{PREFIX}/verify-email` ask for a link and always answer the same, so they
tell nobody which addresses have accounts: the request only commits a
`sofie_identity.LinkAsked` event, and the worker (`sofie_identity::worker`, which
the wiring wraps every worker in) looks the address up and mails a link to a
member, at most three an hour per account, the routes five a minute per
address. A link is an `account_link` row (purpose, expiry an hour away, used
once) at `{PREFIX}/reset-password/{account_link}` or
`{PREFIX}/verify-email/{account_link}`, opened only through `SignedUrls`; its
page posts back to the same signed address. Setting a new password uses up
every reset link of the account, revokes its access tokens (`tokens`) and
confirms the address; sessions open elsewhere stay open unless `sessions`
records them, which ends them all. With `email-verification`, sign-up mails a
new address a confirmation link, in every area. `account.verified_at` records
every proof of the address whatever the features: that link, a reset, a sign-in
link, a join link and a provider's new account. Signing up with a taken
address answers alike: a member of the area is mailed a reset link saying
someone tried (`sofie_identity/sign_up_repeated`, with `password-reset`), an
account of another area a link to sign in and join it (`sofie_identity/join`,
single-use, one hour, only where `SIGN_UP` is set); signing in through it adds
the membership and confirms the address, within the same hourly limit. An area with `MemberArea::VERIFIED_EMAIL = true` signs in
only confirmed accounts, by password, passkey or provider alike (a mailed link
and a provider's new account confirm the address as they sign in; a sign-in
link or reset that confirms it first also makes its password unmatchable (a
reset then sets the new one) and ends its sessions, tokens, passkeys, provider
connections and second factor, as whoever set them never proved the address;
such a link then signs in without a second step; the confirmation link keeps the
password and sessions but removes the passkeys, provider connections and second factor added
before it), and answers an unconfirmed one's right password as a
wrong one, so signing in with the password one just signed up with tells
nothing; without it, an unconfirmed account signs in, and that tells a new
address from a taken one. The screens
(`forgot_password`, `reset_password`, `ask_verification`, `verify_email`,
`link_sent`) and mails (`sofie_identity/password_reset`,
`sofie_identity/email_verification`, `sofie_identity/sign_up_repeated`) are
replaced like the sign-in screen.

**Bearer tokens** (the `tokens` feature of `sofie` and sofie-identity, off by
default): an area opts in with `const TOKENS: bool = true;` in its `Area`
definition and then accepts
`Authorization: Bearer <token>` beside its cookie. A token is 256 random bits
(ring's `SystemRandom`), shown once in the reply of
`sofie_identity::tokens::issue::<A>(read, account, scopes, expires_at)` and
stored only as its SHA-256 digest, looked up by that digest (so no
constant-time compare is needed: timing could only tell digests apart),
with its owner account, area, scopes, expiry, last use (kept to the minute)
and soft-deleted revocation (`tokens::revoke`, `tokens::list` for an
account). The account must be the area's member when the token is issued
and on every request: the lookup by digest checks the membership in the same
query, so a removed member's token is refused at once.
A token's request resolves the same `Principal<A>` as a session, never opens
a session or writes a cookie and skips the cookie's CSRF check (a browser
never sends the header on its own); an unknown, malformed, expired or
revoked token, or one whose account left the area, is answered 401 with `WWW-Authenticate: Bearer`, and more than
10 such failures a minute from one address 429, which the shield counts as
throttled. Scopes are permission patterns matched like grants: in a
`Permitted` area a route needs both the account's grant and a covering
scope, else 403; in an `Owned` area routes name no permission, so a token
acts as its account there. Logs name a token only by its id. With
sofie-identity's `token-screens`, `tokens::screen::routes::<A>` mounts `/tokens`
in a token area: each principal lists the tokens its own account issued and
revokes one (`sofie_identity.access_token.*.read` and `.delete`); another
account's token is not found. Issuing has no screen yet. Like every account
screen (`sessions`, `two_factor`, `passkeys`), it mounts only in an
`AccountArea`, one whose principal is the account itself.

An area's access follows from its principal, so the two cannot disagree:
`Visitor` makes it `Public`, an aggregate such as `Account` `Owned` and
sofie-identity's `Permitted` `Permitted` (behind the `permissions` feature of `sofie` and
sofie-identity). `Access` is one of three:

| Access | Who passes | On which records |
|---|---|---|
| `Public` | Anyone; the endpoint states its own limits. `Web` uses it | Any |
| `Permitted` | A signed-in principal holding the endpoint's permission; otherwise 403 | Any |
| `Owned` | The signed-in principal | Reads only records whose owner is that principal, so any other is a 404, and records without an owner; writes only the principal's records, refused at commit otherwise |

**Permissions** are `module.model.field.action`, derived by sofie-model
(`sofie_model::permissions`), never written: each aggregate grants its
record's `read`, `create`, `update` and `delete` (field `*`) and each
field's `read` and `update`; a route needs `read` for a page, else its
handler's first word, on the module's aggregate its name mentions, or its
last word when it mentions none (`cancel_order` → `order.order.*.cancel`);
a write never needs `read`, so one named `read_…` (`read_notifications`,
which marks them) needs `update`.
A grant may put `*` on any axis (`order.*.*.read`); a needed `*` (the
whole record) is covered only by a `*`. The wiring gives the router each
route's `Permission` (`Gates::route_permissions`, a table like the route
names, given to `App::gates`; `sofie gen` writes it once an area admits by
permission, with each module's permissions as constants in its contract,
`order_contract::permissions::ORDER_CANCEL`, exported by
`pub use r#gen::permissions;`), and the app's members hold the grants
(`AreaMembers::grants`, which `sofie_identity::Memberships` answers); a route of a
`Permitted` area without one stops the app at boot, as does a route given
one (`NeedsPermission::permission`) in an area that does not admit by
permission, which would check no grant. sofie-identity stores
`role` and `role_grant` rows (a grant revoked is deleted softly), and a
membership of a `Permitted` area names its member's role; the grants are
read for each request there, so a revoked grant stops the next request. A
handler reads them as `Permissions` (`can(PERMISSION)`); `create_role`,
`grant` (for seeds and boot), `grant_as` (refusing a permission the granter's
own `Permissions` do not cover), `revoke` and `assign_role(read, clock, account,
area, role)` return what to commit; a new role given to an account whose address
nobody confirmed distrusts it as adopting one does (below). Signing up is closed in every member area unless it sets
`MemberArea::SIGN_UP` (generated only when the area implements
`sofie_identity::SignUp`); a `Permitted` area that opens it lets anyone sign
up, and each new account gets 403 everywhere there until it is given a role.
Staff are added instead with `add_member(read, clock, email, area)` (`.with_role(read, clock, role).await`;
an address without an account gets one with no password, which signs in once
its owner sets one by a reset or sign-in link or connects a provider; an
existing account it adopts into the area, or gives a new role, whose address
is unconfirmed loses its password and every other way in the same way), or by
registering `add_member_task` in the app's tasks:
`Tasks::new().task("sofie_identity.add_member", sofie_identity::add_member_task)`,
then `<app> task sofie_identity.add_member --email … --area … --role …`, the
role one created before (`create_role`, `grant`, `role_named`). With
sofie-identity's `permitted-screens`, `sofie_identity::permitted_routes::<A>` adds
(for a `Permitted` area `A`)
roles (`/roles`) and each role's grants (`/roles/{role}/role-grants`, a grant
the poster's own grants do not cover refused) as resources, with `permitted-flags` a screen targeting flags (`/flags`: on,
environments, areas, principals, percentage and variant weights, each list
comma-separated; needing `sofie_identity.flag.*.read|update`) and one changing
the declared runtime settings (`/settings`, needing
`sofie_identity.setting.*.read|update`) and with `permitted-shield` the
shield's blocks in force, each lifted there (`/shield`, needing
`sofie_identity.shield.*.read|delete`); assigning a role to a member has no
screen yet.

**Contributed by every module, collected by `sofie gen`.** A module lists what it
publishes in an area in that area's folder, as [routes](#routes) and menu
entries. Actions stay free of HTTP; the same action can be published in several
areas, and each listing inherits its area's checks. Everything one audience can
reach in a module is in one file, which is where a security review starts.

Derived from each area:

- The route group: prefix, guard, session and CSRF settings, rate limits.
  The app adds an area-wide limit with `app.throttle::<A>(Rate::per_minute(n))`
  and its own pipeline steps with `app.step::<A>(step)`, or for every area
  with `app.step_all(step)`: a `PipelineStep`'s `check` sees the request's
  head, the client address and the principal, runs after CSRF, the guard and
  the limits (app-wide steps first), before the body is read, and can only
  refuse, with an error answered as a handler's would be. Its `answered`
  (empty by default) sees the answer's status and how long it took and sets
  headers, in reverse order, on every answer the router gives (an app-wide
  step on a not-found too; never on a health, metrics, asset or preflight
  answer), before the framework's own headers: setting
  `Set-Cookie`, a security header or a CORS header is refused, and the
  security headers are set after it regardless, so a step cannot weaken them.
- Cross-origin calls, with the `cors` feature: `app.cors::<A>(Cors::new()
  .origin("https://app.example").methods(…).headers(…).exposed_headers(…)
  .credentials().max_age(…))`, more origins from `SOFIE_CORS_ORIGINS`
  (`api=https://app.example, api=…`; naming an area without a policy refuses
  to start). Origins are exact (`*` only alone and never with credentials);
  a preflight `OPTIONS` for one of the area's routes is answered by the
  framework before routing (a route whose flag is off for a caller without a
  session is missing to it), and an answer to an allowed origin carries the
  CORS headers, `Vary: Origin` whenever origins are listed. A cross-origin
  write passes the CSRF check only when the area's policy allows its
  `Origin`, and, unless the policy allows credentials, only without the
  session cookie.
- Hosts a page may reach besides its own origin, such as a payment
  provider's script and frames: `app.serving(|serving|
  serving.content_security::<A>(ContentSecurity::new()
  .script("https://js.provider.example").frame("https://js.provider.example")
  .connect("https://api.provider.example")))`, or for every area with
  `serving.content_security_all(…)`; also `.style`, `.image`, `.font` and
  `.form_action`. Only the area's pages carry them. A source is
  `https://host[:port]`, its leftmost label `*` for any subdomain; a keyword
  (`'unsafe-inline'`, `'unsafe-eval'`), another scheme, a path or `*` alone
  refuses to start, so the nonce policy cannot be undone. The app's own
  fonts always load; who may frame a page stays `'none'`.
- The principal as a typed handler argument; an action routed to one area
  cannot ask for another area's principal.
- For `Owned`, a `Read` that adds the owner filter to every query and a commit
  that refuses any record the principal does not own. A handler gets a `Read`
  only from its request context, which scopes it; there is no other to ask
  for.
- Island bundles per area: an area never ships another area's islands.
- Layout and menu, from the menu entries of every module's area folder:
  `pub fn menu(menu: &mut Menu<Console>) { menu.entry("Orders",
  routes::console::list_orders()); }`, wired as `app.menu::<Console>(…)`
  (trait `Navigation`, the `menu` feature, before or after `App::gates`; `sofie gen`
  reads each `menu.rs` without compiling it, so its body holds only
  `menu.entry(…)` statements, and `sofie check` refuses an entry's
  `.feature(…)` naming no flag of the module). Each whole
  page of the area starts with its menu; an entry whose route needs a
  permission the viewer lacks, or whose route or own `.feature(…)` is off,
  is left out. An entry must name a page of its area without parameters.
- `area` as a dimension on every span, metric and audit entry, and as an
  OpenAPI tag.

## Routes

The shape is Laravel's: verbs, resources, groups, model binding, named routes.
What Laravel asks you to write by hand — names, binding scopes, middleware,
permission checks — is derived from the area, the schema and the handler's
signature.

```rust
// modules/order/src/areas/customer/routes.rs
pub fn routes(routes: &mut Routes<Customer>) {
    routes.resource::<Order>().only([Index, Show]);
    routes.post("/orders/{order}/cancel", cancel_order);
    routes.screen("/orders/{order}/invoice", "invoice", order_invoice);
}

// modules/order/src/areas/console/routes.rs
pub fn routes(routes: &mut Routes<Console>) {
    routes.resource::<Order>();
    routes.post("/orders/{order}/cancel", cancel_order).throttle(Rate::per_minute(10));
    routes.group("/orders/{order}", |routes| {
        routes.resource::<OrderLine>().only([Index, Create]);
    });
}

// modules/payment/src/areas/web/routes.rs
pub fn routes(routes: &mut Routes<Web>) {
    routes.webhook("/payments/stripe", stripe_callback).verify(StripeSignature);
}
```

| Laravel | Here |
|---|---|
| `Route::get(…)` and friends | `routes.get(…)`; a query or screen only under `get`, an action never under `get`, checked by `sofie gen`; a GET (and HEAD) handler resolves in `ReadContext` and cannot take a write-side argument or return a writable outcome, a compile error and `sofie check`'s `writing-get` (docs/core.md, Handlers) |
| `Route::resource(…)` | `routes.resource::<A>()`: screens from the aggregate's field metadata, writes through the factory and the aggregate's methods |
| Route model binding | A `{order}` segment binds the handler's `Order` argument (an action's) or a read model of `Order` (a GET's), loaded through the area's scope: another owner's record or a child outside its parent is a 404 |
| `Route::view(…)` | `routes.screen(…)` and `routes.island(…)`, each fed by its loader |
| `redirect()->route(…)` after an action | `.then_redirect("/orders/{id}")`: a form posted without JavaScript is redirected, htmx navigates, JSON gets the reply; each `{name}` is the route's path parameter or else the reply's field |
| `->name('…')` | Derived: `<module>.<area>.<handler>`, such as `order.customer.cancel_order`, one per route by rule; builders such as `order_contract::routes::customer::cancel_order(order_id)` return a `RoutePath` (method and filled path) and take a segment named after one of the module's aggregates as that aggregate's `Id` (the contract cannot name the type, so the builder is generic and `sofie::http::require_aggregate` fails the build for any other aggregate's id) and any other segment as `impl Display`; written into the contract's `src/gen/mod.rs` and exported by its `pub use r#gen::routes;` |
| `->middleware('auth')`, `can:` | From the area: guard and access; the permission is derived from the handler (`order.order.*.cancel`) |
| `->middleware('throttle')`, `signed` | Modifiers on the route: `.throttle(…)`, `.signed()`, `.idempotent()`, `.body_limit(…)` (see below) |
| Webhook routes with CSRF exceptions | `routes.webhook(…).verify(…)` in `Web` only: raw body, signature verification, idempotency by delivery id, no session |
| `route:cache` | Nothing to cache: the router is built once at boot |
| `route:list` | `sofie routes`: area, method, path, name, handler, permission (in a `Permitted` area), feature flag, owner scope and modifiers, by area and path. Rows come from app modules and from the framework packages the app depends on (sofie-identity, sofie-tenancy, sofie-content). Each package lists its routes in a `ROUTES` table its root re-exports, read without compiling and filtered by its enabled features. A row is served where the app's code mounts that function (`.area("<crate>", <crate>::<function>::<Area>)`) or where the package's own `Module` mounts it, and is named `<crate>.<area>.<handler>`. Package routes also take part in the route-conflict check |
| Duplicate paths found at runtime, the last one winning | Found by `sofie gen`, across every module in the area |

What the modifiers do:

- `.throttle(rate)` counts requests per signed-in principal, or per client
  address without one (an IPv6 client by its /64), and answers 429 with
  `Retry-After` past the rate. Counting is in-process: behind a load
  balancer each instance allows the full rate, so the effective limit is
  the rate times the instance count. A handler or library limiting by its
  own rule returns `Error::TooManyRequests(wait)`, answered the same way.
- With sofie-http's `shield` feature (sofie's `shield`), the shield watches
  behaviour, never payloads. The pipeline sends small signals into a bounded
  channel that never blocks (a full queue drops and counts): a 404 on an
  unmatched path or a 405 (`scan`; not for the few paths browsers and
  crawlers ask for on their own, such as `/favicon.ico`, `/robots.txt` and
  `/.well-known/security.txt`), a 404 on a matched route (`probe`), a
  403 (`refused`: CSRF, signature, permission), a 400 or 413 (`malformed`),
  a 429 (`throttled`), a failed connection such as one reset too often
  (`protocol`), and, through the `SignIns` handler argument sofie-identity's
  `shield` feature takes, failed sign-ins per caller (`sign_in`), per
  account and caller (`account`), per account (`account_total`) and
  sign-ups (`sign_up`). A `sofie-shield` thread drains
  them every 50 ms, counts them per key (the principal when signed in, else
  the address, IPv6 by /64; the account and the caller's address for
  `account`, the account for `account_total`) in sliding windows,
  and grades each rule against its threshold (`SOFIE_SHIELD_THRESHOLDS`,
  `rule=count` or `area.rule=count`): half reports, the threshold slows the
  caller down to one request a second, twice it blocks it with 429 and
  `Retry-After` for 15 minutes; an account at its `account` threshold has
  signing in locked for 15 minutes for that caller only, so an attacker
  elsewhere cannot lock its owner out, and at its looser `account_total`
  threshold (100) for everyone (while it holds, every password reads as a
  wrong one and counts no further, so no answer tells a guess right); either lock `AreaMembers::sign_in_locked` tells its owner,
  which sofie-identity does by raising `sofie_identity.SignInLocked` for the app
  to listen to. Every block expires and the table holds 65,536 at most. A
  request pays one atomic load while nothing is blocked, else one read of
  the table, after its session opens; a path no route has opens no session
  and meets the caller's address block before its 404. Never blocked by address: a trusted
  proxy, a loopback or unspecified address. `SOFIE_SHIELD` is `on`,
  `dry-run` (reports only; the default of a debug build) or `off` (tests,
  `sofie bench`, `sofie fuzz`). Each decision is one warning with its rule,
  grade, reason and caller (an address as its /24 or /48 only) and, with
  `metrics`, counted in `sofie.shield.decisions` by rule and grade. With
  `SOFIE_SHIELD_SHARED=on` blocks are written to `sofie.shield` and every
  instance follows the table on `NOTIFY sofie_shield` (or every 10 s);
  listening that fails is resumed a second later and the table read right after;
  `ShieldBlocks` lists and lifts them in an action, `ActiveBlocks`
  lists them on a screen; a block's id is a digest of its caller, the same
  on every instance.
- `.signed()` opens the route only through a link `SignedUrls` signed: a
  handler argument whose `sign(path, valid_for)` appends `expires` and an
  HMAC-SHA256 `signature` made with a key derived from the app's
  `SOFIE_SIGNING_KEYS`. A changed, expired or unsigned link is a 403; the
  handler's `Query` and `Listing` read the link without the two. A job
  asks for `SignedUrls` too: the worker's database carries the same keys
  (`Db::with_signing_keys`, given by `sofie::run` and sofie-testing).
- `.idempotent()` (actions only) requires an `Idempotency-Key` header. The
  key is scoped to the route and the principal; the first answer is stored
  in `sofie.idempotency` for a day and replayed to retries, a key reused with
  another body is a 422, and the claim on the key commits in the action's
  own transaction, so retries running alongside write once. On a public
  route nobody scopes the key, so clients must make it unguessable. The
  handler may take the key as an `IdempotencyKey` for its vendor call; only
  the framework builds one. The engine's script sends a key with every htmx
  write and keeps it for that element until an answer arrives, so an
  idempotent HTML form resent after a lost answer writes once (routes not
  marked ignore the header); a plain form post without the script sends none.
- `.body_limit(bytes)` raises the 64 KiB a body may hold.
- `.file_limit(bytes)` (with `uploads`) lets a route take files and holds
  each file of a `multipart/form-data` form below it (elsewhere a file part
  is answered 415); without `.body_limit(…)` the form may hold one such
  file and 64 KiB more. On such a route a body
  that is not multipart is held to 64 KiB. A file field is an
  `Upload` in the route's `Input`, already in the app's `Storage` when the
  handler runs; the handler keeps its `key()` and links to it with
  `Storage::download_url` (see core.md, "Files").
- A webhook's `Verify` gets the headers and raw body and returns the
  delivery id; the delivery runs without a session or the same-origin
  check, and a redelivery of an id gets the first answer.

A resource is reached under its name in the plural (`OrderLine` →
`/order-lines`): `GET /order-lines` lists, `GET /order-lines/{order_line}`
shows, `GET` and `POST /order-lines/new` create, `GET` and `POST
/order-lines/{order_line}/edit` change and, for an aggregate with
`SoftDelete`, `POST /order-lines/{order_line}/delete` deletes. Each form
posts to its own page's path, so a refused form without JavaScript comes
back filled in; a write refreshes the list (or the record) for htmx and
sends a browser to its page. `sofie gen` names the routes
`sofie::resource::list_order_lines`, `show_order_line`, `new_order_line`,
`create_order_line`, `edit_order_line`, `update_order_line`,
`delete_order_line`; in a `Permitted` area each needs
`<module>.<model>.*.read|create|update|delete`. A new record's owner is
the principal (`OwnerOf`); a `Permitted` area creates only records without
an owner. A duplicate of an `#[unique]` field is answered as that field's
`validation.unique` error when its index is named `<table>_<field>` (or
`<table>_<field>_key`, Postgres's name for an inline `UNIQUE`), for any
aggregate's commit.

`routes.group("/orders/{order}", |routes| …)` (the `groups` feature, the
trait `Groups`) puts routes under a parent's path. A record bound below
it must hold the parent's id in its field named like the parameter
(`OrderLine.order`), or it is not found; a resource's list shows only the
parent's records, and its forms take the parent from the path, never from
the post.

`.signed()`, `.idempotent()`, webhooks, `App::step` and `App::cors` are cargo
features of `sofie` (`signed-links`, `idempotency`, `webhooks`, `steps`,
`cors`), off by default,
so an app compiles only the modifiers it uses.

`routes.rs` is plain Rust, so the compiler checks every handler against its
area: `Routes<Customer>` accepts only handlers whose principal is a customer.
`sofie gen` reads the same file without compiling it, so the body is restricted
to route calls with literal paths; anything else is a generation error.

## Rules and how they are enforced

| Rule | Enforced by |
|---|---|
| A module imports another module's contract crate only | `sofie check`: a module depends on no other module's implementation crate, a contract on no module at all (`sofie` and `sofie-identity` are not modules) |
| Dependencies flow Experience → Core → Platform; nothing depends on the app | `sofie check`, reading Cargo metadata and each module's manifest tier |
| Platform never listens to Core (D15) | `sofie check`: a Platform module's listener hears only events of Platform modules |
| Events are declared in contracts | `sofie check` (`event-in-contract`): an event derived in a module's own crate is a finding |
| A mirror copies only what its source exposes | `sofie check` (`mirror-field`) |
| `sofie check` runs on every build | The app's `build.rs` calls it; a finding fails `cargo build`, and Cargo reruns it only when a module, the app's sources or tests, a manifest or the allow-list changes |
| No cycles between modules | Cargo |
| Aggregates are constructed only in the factory | Module privacy: fields are private to the aggregate's folder, and the factory is its child |
| An aggregate changes only through its own methods | The same privacy |
| A query cannot write | The connection type it receives has no write methods |
| No N+1 query can be written | Aggregates load whole; read models declare their joins; there is no lazy relation |
| A write never overwrites a newer version | Version column checked on every update |
| A status moves only along its declared edges | The unit of work checks the enum's transitions on every save |
| An invalid value never enters the app | Value objects are built only through their parsing constructor, deserialization included |
| No float in a money path | `Money` exposes no float in or out |
| A sensitive value is never logged unmasked | Logging accepts values through `LogFields`, which masks them; values are not formatted into messages |
| No transaction is held while a handler runs | The unit of work opens it only to commit what the handler returned |
| Application code never names the database driver | Values encode to `SqlValue`; the driver is reached only through the database seam |
| No foreign key or join across modules | `sofie check` (`cross-module-sql`) on the migrations: no `REFERENCES`, `FROM`, `JOIN`, call or other table reference qualified by another module's schema; raw SQL in code is held by the module's role (`module-roles`) |
| Generated code is current | `sofie check` (`stale-wiring`), run by the framework's CI on the examples |
| Nothing reads the system time but the clock | `sofie check`: no `now`, `now_utc`, `now_local` or `today` of `SystemTime`, `Instant`, jiff, chrono or time types in a module |
| Development tools never reach production | The toolbar and impersonation are compiled into development builds only |
| A removed flag leaves no use behind | Flags are constants; removal breaks the build where they are used |
| Performance does not regress unnoticed | `sofie bench` against the per-route baseline |
| A scaffolded test is never overwritten | Scaffolding only writes new files, named by the moment they were written |
| Documentation matches the code | Generated from it and checked like any generated code |
| A diagram never claims an edge the code does not have | Flows are built only from declarations; unresolved edges are marked, never guessed |
| Schema and migrations agree | `schema:check`: the scaffolded `schema` fact of each aggregate, run against a migrated database in CI, and `<app> schema check` against a deployed one |
| Migrations do not lock a table the running version uses | `sofie check` (`migration-lock`), with an allow comment and its reason for a justified case, or `migrations/migration-allow.toml` for a migration already applied or a table that stays small |
| A module's own SQL reaches only its own schema | Its database role (`0000_module_role.sql`), entered by `read.sql(…)` with `module-roles` |
| The database holds every field constraint and enum of the schema | `sofie check` (`schema-constraint`): a CHECK per constrained or enum column, every variant name listed, a unique index per `#[unique]` field, read from the migrations' SQL; every behavior's columns (`version`, `created_at`, …) on a table the migrations make (`behavior-column`) |
| No type grows into a god type | `sofie check`, for an app that sets `[package.metadata.sofie] structure-ceilings = true` (off by default): every type of the app's crates under the framework's own ratchet ceiling |
| Comments stay sparse | `sofie check`, for an app that opts in with `structure-ceilings = true`: comment lines at most 2.86% of code lines per crate of the app (3 lines free); lowered after pruning passes, never raised |
| The framework stays small and fast against Loco | `bench/loco-baseline.json` holds Loco's numbers and every ceiling: sofie-check's `limits` test holds app code, crates and dependency lines without building, and framework lines are measured and reported; `bench/run.sh --full` fails on binary size, build times, throughput, p99, memory, startup and audit rows |
| No dead code accumulates | `sofie check` and the workspace's structure test: a `pub` item nothing names outside its definition (a `pub use` or a doc comment is not a use; trait impl items, `main` and tests are not counted) and a `[dependencies]` entry the crate's sources never name are findings, unless `dead-code-allow.toml` lists them with a reason; allowed items still count toward a ceiling of 2% of a crate's `pub` items, which is only lowered |
| Translations are complete | `sofie check`: key parity across languages |
| Screens use only catalog components and islands | The template parser accepts nothing else |
| A screen binds only fields its loader provides | `sofie gen` type-checks template expressions |
| A screen never sends what the principal may not see | Permission and flag conditions are evaluated while rendering, per request |
| An endpoint in an owned area cannot reach another principal's records | Its handler's `Read` is made only by the request context, scoped to the principal, and has no way to widen; only the aggregate's own module may ask whether records exist across owners (`across`, crate-private), which answers a yes or a count, is audited as `cross_owner_check`, and can be guarded in the commit; its commit refuses records the principal does not own; `sofie check` (`own-contract`) reports a module reading through its own contract or building a cross-owner check by hand |
| An endpoint receives only its own area's principal | `Routes<A>` accepts only handlers for `A`'s principal |
| Every request leaving the app is an outgoing request through a gateway or contract | `Outgoing` cannot be constructed or resolved by app code; `sofie check` refuses HTTP client crates (a dependency pulling one in, by `Cargo.lock`, too) and raw sockets (a `net::*` glob too) in modules and the app |
| An outgoing request reaches only its vendor's hosts | Egress allowlist per `Outgoing`; private and metadata addresses refused after resolution |
| A `GET` never changes state | `sofie gen`: only queries and screens are routed under `get` |
| No two endpoints share a path in an area | `sofie gen`, across every module |
| Every entry point is traced | Spans are opened by the framework's handler adapter and worker, which every entry point passes through |
| A trace survives async work and a split | `traceparent` stored on outbox rows and jobs, propagated on contract and gateway calls |
| A metric label cannot explode in cardinality | Labels accept only route names, enums and module names |
| Telemetry carries no personal data or secrets | Masking by value type, sensitive schema fields, payload keys and the message scrubber |
| Environment variables are read only through a module's config | `sofie check`: no `env::var`, `env::vars` or `env!` outside the framework |
| A module's environment keys carry its prefix | The loader derives them from the module's name; there is no other way to declare one |
| A declared task can be run | `sofie check` (`task-runner`): a module with `#[task]`s needs an app whose `main` calls `sofie::run_with_tasks` |
| A client area's screens can be served | `sofie check` (`client-feature`): an app with a client area enables sofie's `clients` feature |
