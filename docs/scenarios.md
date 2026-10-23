# Scenario Walk

> Each scenario is walked through [structure.md](structure.md): what is
> written by hand, what is generated, and where the structure strains. The
> decisions at the end (D1–D15) are the ones `structure.md` refers to.

The example app is a small online shop; it stands for any app, and nothing in
it is part of the framework. Modules used below: `identity`
(Platform: accounts, sign-in, areas), `files` (Platform), `communication`
(Platform), `member`, `catalog`, `order`, `payment`, `delivery` (Core) and
`checkout`, `console` (Experience).

## 1. A CRUD resource

Products are managed in the `Console` area.

**Written**

- `catalog/src/schema.rs`: `Product` with its fields and behaviors.
- `catalog/migrations/0003_products.sql`.
- `catalog/src/areas/console/routes.rs`: `routes.resource::<Product>();`
- `catalog/src/areas/console/menu.rs`: one entry.
- `catalog/lang/<language>.toml`: the screen titles, for each language the app
  declares.

**Generated**: index, show, create, edit and delete screens; the five actions;
permissions `catalog.product.*.{read,create,update,delete}`; input DTOs;
validation; filters and sorting from the field types; telemetry; docs.

**Strain.** A resource needs to create and update without knowing the
module's code. Creation must go through a factory, and an update through the
aggregate, but a plain product has no invariant worth writing.

**Finding.** When `factory.rs` is absent, a default factory is generated from
the schema: it takes a `CreateProduct` built from the schema and constructs the
aggregate. When an aggregate declares no `update`, a default one applies an
`UpdateProduct`. Writing either replaces the default. Creation still goes
through a factory; the module writes one only when there is a rule to hold.
→ [D6](#d6-default-factory-and-update)

## 2. Checkout across modules

A member checks out a cart, pays with 3-D Secure, and the order is confirmed,
scheduled for delivery and mailed.

**The walk**

1. `POST /checkout` in the `Member` area, owned by the Experience module
   `checkout`. Its action calls three contracts:
   - `catalog.prices(lines)`: a read.
   - `order.place(member, priced_lines)`: a write. The order is born in
     `OrderFactory::place`, status `Pending`, raising `OrderPlaced`.
   - `payment.start(order_id, total, key)`: a write that opens a 3-D Secure
     session with the vendor and returns its redirect.
   The action answers with the redirect.
2. The bank posts back to `POST /payments/3ds` in the `Web` area of
   `payment`, a webhook with signature verification. `complete_3ds` marks the
   payment captured and raises `PaymentCaptured`.
3. `order` listens: `#[on] mark_paid(event: PaymentCaptured, …)` moves the order to
   `Paid` and raises `OrderPaid`.
4. `delivery` listens to `OrderPaid` and schedules the shipment; `order` listens to
   its own `OrderPaid` and sends the confirmation through the `communication`
   contract.
5. The member's browser is redirected from the callback to the order's
   confirmation screen.

**Strains and findings**

- *Where does the process live?* Step 1 is synchronous — the member waits
  for the redirect — so it cannot be a chain of events through the outbox. It
  is an action in an Experience module calling contracts. Steps 2–4 are
  asynchronous and are events. Nothing needs a separate process object: the
  order's status enum is the state, `#[to]` guards it, and a scheduled action
  expires orders left `Pending` (scenario 5). The generated flow diagram shows
  the whole chain. → [D5](#d5-no-process-role)
- *Transactions across modules.* `order.place` and `payment.start` cannot share
  a transaction once the modules are split, so they must not share one now:
  each contract write commits its own unit of work. If `payment.start` fails,
  the order stays `Pending` and expires. → [D5](#d5-no-process-role)
- *A vendor call inside a write.* `payment.start` calls the vendor and must
  return its answer to the member, so it cannot be moved to a listener. It is
  safe because no transaction is open while a handler body runs — loads read
  without locks and the unit of work commits in one short transaction at the
  end — and because the vendor call carries an idempotency key, so a retry
  opens the same session. This replaces the rule "an action that calls a
  gateway is non-transactional". → [D3](#d3-no-transaction-during-the-body),
  [D4](#d4-idempotency-keys-for-effects)
- *How does the action get its dependencies?* `checkout`'s action needs three
  contracts, the principal and the input; `payment.start` needs a gateway.
  All are resolved from the handler's signature by type.
  → [D1](#d1-dependencies-by-signature)
- *Linking across modules.* The callback in `payment` redirects to a screen in
  `order`. Route builders must therefore be reachable from another module:
  each contract exports the builders of its own module's endpoints.
  → [D10](#d10-route-builders-in-contracts)
- *Who sends the mail?* `communication` is Platform and may not import
  `order`'s contract, so it cannot listen to `OrderPaid`. `order` listens to
  its own event and calls `communication`'s contract. Platform reacts to Core
  only by being called. → [D15](#d15-platform-never-listens-to-core)

## 3. An incoming webhook

Covered in scenario 2, step 2. `routes.webhook(...)` gives the raw body,
signature verification and idempotency by delivery id, in the `Web` area.
The handler is an ordinary action. No strain.

## 4. A payment through a gateway

`payment/src/gateways/payment.rs` defines `PaymentGateway`;
`gateways/stripe/` holds the adapter, client and vendor types. The vendor is
chosen by config; the registry is generated as an enum, so dispatch is static.

**Finding.** Methods with side effects at the vendor (`start`, `capture`,
`refund`) take an `IdempotencyKey`, a type only the framework constructs from
an event id, a delivery id or an explicit key. A side-effecting method without
one does not compile into a gateway. → [D4](#d4-idempotency-keys-for-effects)

## 5. A scheduled job

Expire orders left `Pending` for more than an hour.

**Strain.** The natural code loads every pending order and cancels each in one
action, but that is one unit of work over thousands of aggregates: one failure
undoes all, and nothing is retried per order.

**Finding.** The schedule names a query and a per-item action:

```rust
// order/src/actions/expire_order.rs
#[every("15m", each = stale_pending_orders)]
pub async fn expire_order(order: &mut Order) -> Result<()> {
    order.expire()
}
```

The framework runs the query, enqueues one job per id and runs the action per
order in its own unit of work, with its own retries and its own span. Only one
instance fires a schedule: firing it and enqueuing its job is one atomic
statement on `sofie.schedules`, and no lock is held.
→ [D7](#d7-scheduled-per-item-actions)

## 6. A file upload

A member attaches a photo to a support ticket.

**The walk.** `files` (Platform) owns blobs and their storage gateway. The
browser uploads to `files`' endpoint and receives a `FileId`, pending a virus
scan. The ticket form posts the `FileId`; the schema field is
`photo: FileRef`, a value from `files`' contract whose surfaces give the form a
file picker and the detail screen a thumbnail.

**Strain.** `files` must delete blobs nothing references, but it cannot query
other modules' tables.

**Finding.** When a `FileRef` field changes, the unit of work raises a
generated `FileAttached` or `FileDetached` event; `files` keeps reference
counts from them. More generally, aggregates raise generated change events that
the framework's own features consume. → [D9](#d9-generated-change-events)

## 7. Search

The `Console` area searches products by name; a global search spans modules.

**The walk.** `#[searchable]` on schema fields defines the index. The index is
kept current from the generated change events (D9) through the search plugin's
gateway. A module's search is a query; the global search is a view in
`console` calling each contract's generated `search`.

**Finding.** Search is not core: it is a runtime crate plus a generator plugin
that reads `#[searchable]`. Without that mechanism `sofie gen` would have to know
every feature. → [D12](#d12-generator-plugins)

## 8. A member acting on their own records

A member lists their wish lists and each list's items.

**The walk.** The `Member` area is `Owned`. `WishList` declares
`#[owner] account: AccountId`. `WishListItem` belongs to `WishList` in the same
module.

**Strain.** `WishListItem` has no owner column of its own.

**Finding.** Ownership may run through a parent in the same module:
`#[owner(through = wish_list)]`, enforced with a join inside the module. Across
modules there is no join, so a record owned through another module's record
stores the owner id itself. → [D11](#d11-ownership-through-a-parent)

## 9. A console screen that is not a resource

The `Console` dashboard shows due orders with member names and cities,
filterable by city.

**The walk.** A view in `console` (Experience) with loaders calling
`order`'s and `member`'s contracts. Names resolve with
`with_remote(Order::MEMBER)`, which needs a batch lookup on the contract.
→ [D13](#d13-generated-batch-lookups)

**Strain.** Filtering and sorting orders by the member's city needs a join
across modules, which is forbidden.

**Finding.** A module may keep a mirror of fields another module exposes:

```rust
#[mirror(member::Member, fields(name, city))]
pub struct OrderMember;
```

The mirror is a local table kept current from the generated change events
(D9), so `order` filters and sorts by city locally, before and after a split.
This is the cost of split-readiness, and generating it keeps the cost to one
declaration. → [D8](#d8-mirrors)

## 10. Splitting a module out

`sofie split payment`.

**The walk.** The new app contains `payment`; its contract is served over HTTP
by a generated server and reached by a generated client; `PAYMENT_DATABASE_URL`
points at its own database; migrations, translations, config and area
contributions move with it.

**Strains and findings**

- *Events across apps.* The outbox relay dispatches in process today. After a
  split each app publishes an event feed that subscribers pull with a cursor,
  so no broker is required. → [D14](#d14-split-mechanics)
- *One area, two apps.* `payment` contributes to the `Member` and `Web`
  areas, which now span two apps. Paths are already unique per area across
  modules, so a generated reverse-proxy table routes each path to its app.
  → [D14](#d14-split-mechanics)
- *Sign-in across apps.* A session in one app's store is invisible to the
  other. Principals are carried as short-lived signed tokens every app can
  verify, or the session store is shared. → [D14](#d14-split-mechanics)
- Mirrors (D8) and batch lookups (D13) already avoid joins, so no query
  changes.

## 11. Sign-in and areas

**Strain.** Areas were to be defined by the module that owns their principal:
`Member` by `member`, a Core module. Platform modules — `communication`'s
unsubscribe screen, `files`' upload endpoint — cannot import Core, so they could
not contribute to the `Member` area at all.

**Finding.** `identity` (Platform) owns accounts, credentials, guards and every
area. An area's principal is the `Account` aggregate; the Core `member` module keeps
the business profile keyed by that id. Every module can then contribute to
every area its tier allows, which is all of them.
→ [D2](#d2-identity-owns-areas)

## Decisions

### D1. Dependencies by signature

A handler receives what its signature asks for, resolved by type: a bound
aggregate (`Order`, owned, and tracked by the unit of work in an action), the
input (any other deserializable argument), the principal, `Read`, its module's
config, a contract or gateway as `Dep<dyn SomeContract>`, the clock, settings
and features. An argument of any other type does not compile. A module
implements its own contract in `src/contract.rs`. Contracts and gateways are
trait objects chosen at boot, so dispatch is dynamic only at those boundaries.

### D2. Identity owns areas

**Resolved.** Identity is built into the framework as the opt-in crate
`sofie-identity`: it owns accounts, password hashes, area memberships, the
sign-up, sign-in and sign-out actions and screens, and its migrations; `sofie gen`
wires it into an app that depends on it. Areas stay the app's, defined in an
app module's contract, with `sofie_identity::Account` as their principal. An
account signs in to an area only as its member; signing up at the area's
`SIGN_UP` path makes it one. Replaces "an area is defined by the module that
owns its principal".

### D3. No transaction during the body

A handler body runs with no transaction open: loads read without locks, and the
unit of work commits in one short transaction at the end, checking versions.
Vendor and contract calls in the body never hold a database transaction.
Replaces the rule on non-transactional gateway actions.

### D4. Idempotency keys for effects

Gateway and contract methods with side effects take an `IdempotencyKey`, which
only the framework constructs (from an event id, a delivery id or an explicit
request key). Enforced by the type.

### D5. No process role

Synchronous steps across modules are an action in an Experience module calling
contracts; each contract write commits its own unit of work. Asynchronous steps
are events. Compensation is an aggregate's state plus a scheduled action. No
process object; revisit if a scenario needs multi-step compensation that this
cannot express.

### D6. Default factory and update

Absent `factory.rs`, a factory is generated from the schema; absent `update`,
a default applies the generated `Update*` DTO. Writing either replaces it.

### D7. Scheduled per-item actions

`#[every(interval, each = query)]` runs an action per item in its own unit of
work and job. One instance fires each schedule: firing a due schedule and
queueing its job is one atomic statement on `sofie.schedules`, so no lock is
held (core.md, "Single-runner schedules").

### D8. Mirrors

`#[mirror(module::Aggregate, fields(...))]` keeps a local, read-only copy of
another module's exposed fields, updated from its change events, for filtering,
sorting and display without joins.

### D9. Generated change events

Each aggregate with exposed or framework-consumed fields raises generated
change events in its unit of work. Mirrors, search indexing, file references,
broadcasts and cache invalidation are built on them; modules' own domain
events stay hand-raised.

### D10. Route builders in contracts

Each contract crate exports the typed route builders of its module's endpoints,
so modules link to each other without depending on implementations.

### D11. Ownership through a parent

`#[owner(through = parent)]` within a module; across modules the owner id is
stored on the record.

### D12. Generator plugins

Each opt-in feature is a runtime crate plus a generator plugin that reads its
own annotations. Core `sofie gen` knows the schema, routes, wiring, screens and
docs only.

### D13. Generated batch lookups

Every contract gets `get_many(ids)` for the aggregates it exposes, which
`with_remote` uses.

### D14. Split mechanics

Events cross apps as a pull feed per app; a generated reverse-proxy table
routes each area path to its app; principals travel as short-lived signed
tokens. Decided now because paths, events and sign-in must be shaped for it
from the start, even though `split` is built later.

### D15. Platform never listens to Core

A Platform module reacts to Core only by being called through its contract or
through an extension point it declares. Its listeners subscribe to Platform
events only.
