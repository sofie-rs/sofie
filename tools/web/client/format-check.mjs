// Run by sofie-cli's slow tests: `node format-check.mjs`. A client area formats as the server's
// `crates/sofie-engine/tests/translations.rs` expects the engine to.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import * as esbuild from "esbuild";

import { currencies } from "../shared.mjs";

const directory = await mkdtemp(join(tmpdir(), "sofie-format-"));
await esbuild.build({
  entryPoints: ["format", "table", "tree", "field", "listing"].map((name) => fileURLToPath(new URL(`./core/${name}.js`, import.meta.url))),
  bundle: true,
  outdir: directory,
  outExtension: { ".js": ".mjs" },
  format: "esm",
  platform: "node",
  logLevel: "warning",
  plugins: [currencies({ EUR: [2, "€"] })],
});
const load = (name) => import(pathToFileURL(join(directory, `${name}.mjs`)).href);
const [S, table, tree, field, listing] = await Promise.all(
  ["format", "table", "tree", "field", "listing"].map(load),
);
await rm(directory, { recursive: true });

const english = {
  conventions: { decimal: ".", group: ",", money: "symbol-before", days: "month-name-first", plural: "one-is-one" },
  messages: { "cart.items.one": "One item", "cart.items.other": "{count} items" },
};
const german = { conventions: { decimal: ",", group: ".", money: "symbol-after", days: "day-first-dotted" } };
const plain = { conventions: { decimal: ".", group: null, money: "code-after", days: "iso" } };
const french = {
  conventions: { ...plain.conventions, plural: "one-is-zero-or-one" },
  messages: { "cart.items.one": "{count} article", "cart.items.other": "{count} articles" },
};
const japanese = { conventions: { ...plain.conventions, plural: "other-only" }, messages: { "cart.items.other": "{count}件" } };
const russian = {
  conventions: { ...plain.conventions, plural: "one-ends-in-one-not-eleven" },
  messages: { "cart.items.one": "{count} товар", "cart.items.other": "{count} товаров" },
};
const turkish = {
  conventions: { decimal: ",", group: ".", money: "symbol-before", days: "day-first-dotted", plural: "one-is-one" },
  messages: { "orders.count": "{name} için {count} sipariş" },
};

const items = (view, count) => S.t(view, "cart.items", { count });
assert.equal(items(english, 1), "One item");
assert.equal(items(english, 0), "0 items");
assert.equal(items(english, 1.5), "1.5 items");
assert.equal(items(english, 1234), "1,234 items");
assert.equal(items(french, 0), "0 article");
assert.equal(items(french, 2), "2 articles");
assert.equal(items(japanese, 1), "1件");
assert.equal(items(russian, 21), "21 товар");
assert.equal(items(russian, 11), "11 товаров");
assert.equal(items(russian, 1.5), "1.5 товаров");
assert.equal(S.t(turkish, "orders.count", { count: 1234, name: "Ada" }), "Ada için 1.234 sipariş");

const meeting = { local: "2026-10-20T09:30:00", zone: "Europe/Istanbul" };
assert.equal(S.date(english, meeting), "Oct 20, 2026, 09:30 Europe/Istanbul");
assert.equal(S.date(german, meeting), "20.10.2026 09:30 Europe/Istanbul");
assert.equal(S.date(plain, meeting), "2026-10-20 09:30 Europe/Istanbul");
assert.equal(S.date(english, "2026-10-11T09:30:00Z"), "Oct 11, 2026, 09:30 UTC");
assert.equal(S.date(english, meeting.local), "Oct 20, 2026, 09:30");
const inIstanbul = { conventions: { ...english.conventions, zone: "Europe/Istanbul" } };
assert.equal(S.date(inIstanbul, "2026-10-11T22:30:00Z"), "Oct 12, 2026, 01:30 Europe/Istanbul");
assert.equal(S.wallClock(inIstanbul.conventions, new Date("2026-10-11T22:30:15.5Z")), "2026-10-12T01:30");
assert.equal(S.wallClock(english.conventions, new Date("2026-10-11T00:05:00Z")), "2026-10-11T00:05");
const unknownToIntl = { conventions: { ...english.conventions, zone: "Mars/Olympus" } };
assert.equal(S.knowsZone(unknownToIntl.conventions), false);
assert.equal(S.date(unknownToIntl, "2026-10-11T22:30:00Z"), "Oct 11, 2026, 22:30 UTC");
assert.equal(S.wallClock(unknownToIntl.conventions, new Date("2026-10-11T22:30:00Z")), "2026-10-11T22:30");

// A table writes a cell the envelope marks as an instant as `date()` does, as the engine's
// `<Table>` does for a typed instant; text written like one stays text.
const cells = { columns: ["at", "note"] };
const rows = [{ at: "2026-10-11T22:30:00.5Z", note: "2026-10-11T22:30:00Z" }, { at: null, note: "" }];
const marked = { ...inIstanbul, instants: S.instantFields({ rows }, { rows: { at: true } }) };
assert.equal(
  tree.toHtml(table.body(cells, rows.slice(0, 1), false, marked)),
  "<tbody><tr><td>Oct 12, 2026, 01:30 Europe/Istanbul</td><td>2026-10-11T22:30:00Z</td></tr></tbody>",
);
assert.equal(
  tree.toHtml(table.body(cells, rows.slice(0, 1), false, inIstanbul)),
  "<tbody><tr><td>2026-10-11T22:30:00.5Z</td><td>2026-10-11T22:30:00Z</td></tr></tbody>",
);

// An instant's control holds what the engine's `<Field>` writes, which the server reads back.
const instantInput = (value, view) => {
  const described = { name: "at", control: "input", attributes: [["type", "datetime-local"]], required: false, constraints: [] };
  const html = tree.toHtml(field.field(described, { value }, { id: "f" }, view));
  return html.slice(html.indexOf("<input"), html.indexOf(">", html.indexOf("<input")) + 1);
};
assert.equal(
  instantInput("2026-10-11T22:30:00Z", inIstanbul),
  '<input type="datetime-local" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T01:30">',
);
assert.equal(
  instantInput("2026-10-11T22:30:15.5Z", inIstanbul),
  '<input type="datetime-local" step="any" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T01:30:15.5">',
);
const inBerlin = { conventions: { ...english.conventions, zone: "Europe/Berlin" } };
assert.equal(
  instantInput("2026-10-25T00:30:00Z", inBerlin),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-25T02:30:00+02:00">',
);
assert.equal(
  instantInput("2026-10-25T01:30:00Z", inBerlin),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-25T02:30:00+01:00">',
);
assert.equal(
  instantInput("2026-10-25T02:30:00Z", inBerlin),
  '<input type="datetime-local" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-25T03:30">',
);
// A fraction finer than the milliseconds a `datetime-local` control keeps goes in a text box, exactly.
assert.equal(
  instantInput("2026-10-11T22:30:15.123456Z", inBerlin),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T00:30:15.123456+02:00">',
);
assert.equal(
  instantInput("2026-10-11T22:30:15.123Z", inBerlin),
  '<input type="datetime-local" step="any" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T00:30:15.123">',
);
// Posted text shown again after another field failed keeps the control it needs, as the engine's.
const repostedInput = (posted, view) => {
  const described = { name: "at", control: "input", attributes: [["type", "datetime-local"]], required: false, constraints: [] };
  const form = { id: "f", submitted: { input: { at: posted }, errors: { other: "is taken" } } };
  const html = tree.toHtml(field.field(described, {}, form, view));
  return html.slice(html.indexOf("<input"), html.indexOf(">", html.indexOf("<input")) + 1);
};
assert.equal(
  repostedInput("2026-10-25T02:30:00+01:00", inBerlin),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-25T02:30:00+01:00">',
);
assert.equal(
  repostedInput("2026-10-12T00:30:15", inBerlin),
  '<input type="datetime-local" step="any" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T00:30:15">',
);
assert.equal(
  repostedInput("2026-10-12T00:30", inBerlin),
  '<input type="datetime-local" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-12T00:30">',
);
assert.equal(
  repostedInput("2026-10-25T02:30", inBerlin),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-25T02:30">',
);
// Text that is no wall clock keeps a text box, which a `datetime-local` control would empty.
for (const posted of ["yesterday", "2026-10-25T02:30:00+25:00", "2026-10-12T00:30:15.1234"]) {
  assert.equal(
    repostedInput(posted, inBerlin),
    `<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="${posted}">`,
  );
}
// Where the wall clock repeats, the offset is written before the zone, as the engine writes it.
assert.equal(S.date(inBerlin, "2026-10-25T00:30:00Z"), "Oct 25, 2026, 02:30 +02:00 Europe/Berlin");
assert.equal(S.date(inBerlin, "2026-10-25T01:30:00Z"), "Oct 25, 2026, 02:30 +01:00 Europe/Berlin");
assert.equal(S.date(inBerlin, "2026-10-25T02:30:00Z"), "Oct 25, 2026, 03:30 Europe/Berlin");
const inLondon = { conventions: { ...english.conventions, zone: "Europe/London" } };
assert.equal(S.repeatedOffset(inLondon.conventions, new Date("2026-10-25T01:30:15.5Z")), "+00:00");
assert.equal(S.repeatedOffset(inLondon.conventions, new Date("2026-03-29T01:30:00Z")), null);
assert.equal(S.repeatedOffset({ zone: "America/New_York" }, new Date("2026-11-01T05:30:00Z")), "-04:00");

// A listing's instant filter holds what was asked, as the engine's `<Table filter>` writes it.
const instantFilter = (asked, view) => {
  const props = {
    columns: ["starts_at"],
    labels: ["Starts at"],
    rows: { items: [], filter: { starts_at: asked }, controls: { starts_at: { type: "datetime-local" } } },
    listing: { sort: [], filter: [{ name: "starts_at", label: "Starts at" }] },
  };
  const html = tree.toHtml(listing.listingTable(props, listing.listed(props), false, view));
  return html.slice(html.indexOf("<input"), html.indexOf(">", html.indexOf("<input")) + 1);
};
assert.equal(instantFilter("2026-10-20T09:30", inBerlin), '<input type="datetime-local" name="starts_at" value="2026-10-20T09:30">');
assert.equal(
  instantFilter("2026-10-25T02:30:00+01:00", inBerlin),
  '<input type="text" name="starts_at" value="2026-10-25T02:30:00+01:00">',
);
assert.equal(
  instantFilter("2026-10-20T09:30", unknownToIntl),
  '<input type="text" name="starts_at" placeholder="with an offset, such as 2026-10-11T09:30:00Z" value="2026-10-20T09:30">',
);
// A filter the server refused, such as a wall clock the zone repeats, is a text box with its error
// beside it, as the engine writes it.
{
  const props = {
    columns: ["starts_at"],
    labels: ["Starts at"],
    rows: {
      items: [],
      filter: { starts_at: "2026-10-25T02:30" },
      controls: { starts_at: { type: "datetime-local" } },
      errors: {
        starts_at: {
          key: "validation.instant.repeated",
          arguments: { earlier: "<+02:00>", later: "<+01:00>" },
          message: "is repeated: <+02:00> or <+01:00>",
        },
      },
    },
    listing: { sort: [], filter: [{ name: "starts_at", label: "Starts at" }] },
  };
  const html = tree.toHtml(listing.listingTable(props, listing.listed(props), false, inBerlin));
  assert.ok(
    html.includes(
      '<input type="text" name="starts_at" value="2026-10-25T02:30" aria-invalid="true"><span class="sofie-field-error">' +
        "is repeated: &lt;+02:00&gt; or &lt;+01:00&gt;</span></label>",
    ),
    html,
  );
}
// An operator filter, or a Table without `filter`, has no slot: its error is told above the table.
{
  const rows = {
    items: [],
    page: 1,
    filter: { status: "x", "total.gt": "a<b" },
    errors: { status: "is no status", "total.gt": "must be a <number>" },
  };
  const table = (filter) => {
    const props = { id: "orders", columns: ["status"], labels: ["Status"], rows, listing: { sort: [], filter } };
    return tree.toHtml(listing.listingTable(props, listing.listed(props), false, inBerlin));
  };
  const told = '<p class="sofie-field-error" role="alert">total.gt: must be a &lt;number&gt;</p>';
  const bare = table([]);
  const withForm = table([{ name: "status", label: "Status" }]);
  assert.ok(bare.includes(`<p class="sofie-field-error" role="alert">status: is no status</p>${told}<table>`), bare);
  assert.ok(withForm.includes(`${told}<form`) && !withForm.includes("status: is no status"), withForm);
}
// Without the zone, an instant's control is a text box asking for the offset the page reads it with.
assert.equal(
  instantInput("2026-10-11T22:30:00Z", unknownToIntl),
  '<input type="text" id="f-at" name="at" aria-describedby="f-at-error" value="2026-10-11T22:30:00Z" placeholder="with an offset, such as 2026-10-11T09:30:00Z">',
);
