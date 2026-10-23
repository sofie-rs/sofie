// A table over one page of a list: sort links, filters and a pager, each loading the screen again
// with its query, as the server answers them.
import * as S from "./format.js";
import { body } from "./table.js";
import { h, withId } from "./tree.js";

// sofie_core::listing::DEFAULT_SIZE; a link keeps `size` only when it differs.
const DEFAULT_SIZE = 20;

export function listed(props) {
  const rows = props.rows;
  const page = rows && typeof rows === "object" && !Array.isArray(rows) && "items" in rows ? rows : null;
  const listing = props.listing ?? { sort: [], filter: [] };
  if (!page && listing.sort.length === 0 && listing.filter.length === 0) return null;
  return {
    items: page ? page.items : rows,
    page: Number.isInteger(page?.page) && page.page >= 0 ? page.page : null,
    size: Number.isInteger(page?.size) && page.size >= 0 && page.size !== DEFAULT_SIZE ? page.size : null,
    hasNext: page?.has_next === true,
    sort: typeof page?.sort === "string" ? page.sort : null,
    filter: page?.filter && typeof page.filter === "object" && !Array.isArray(page.filter) ? page.filter : null,
    query: page?.query && typeof page.query === "object" && !Array.isArray(page.query) ? page.query : {},
    controls: page?.controls && typeof page.controls === "object" && !Array.isArray(page.controls) ? page.controls : {},
    errors: page?.errors && typeof page.errors === "object" && !Array.isArray(page.errors) ? page.errors : {},
    listing,
  };
}

// The first page is left out, so a new order or filter starts there.
function query(state, page, sort) {
  const pairs = [];
  if (page > 1) pairs.push(["page", String(page)]);
  if (state.size !== null) pairs.push(["size", String(state.size)]);
  if (sort) pairs.push(["sort", sort]);
  for (const [name, value] of [...Object.entries(state.query), ...Object.entries(state.filter ?? {})]) {
    if (typeof value === "string") pairs.push([name, value]);
  }
  return `?${pairs.map(([name, value]) => `${S.encode(name)}=${S.encode(value)}`).join("&")}`;
}

function link(state, page, sort, view, ...children) {
  const href = query(state, page, sort);
  return h("a", { href, onclick: view.runtime?.follow(href) }, ...children);
}

export function listingTable(props, state, hasRow, view) {
  return h(
    "div",
    withId({ class: "sofie-listing" }, props.id),
    unplacedErrors(state),
    filters(props, state, view),
    h("table", null, sortedHeader(props, state, hasRow, view), body(props, state.items, hasRow, view)),
    pager(state, view),
  );
}

function sortedHeader(props, state, hasRow, view) {
  return h(
    "thead",
    null,
    h(
      "tr",
      null,
      props.columns.map((column, index) => {
        const sorted = state.sort === column ? "ascending" : state.sort === `-${column}` ? "descending" : null;
        const label = props.labels[index];
        const attributes = sorted ? { scope: "col", "aria-sort": sorted } : { scope: "col" };
        if (!state.listing.sort.includes(column)) return h("th", attributes, label);
        const next = sorted === "ascending" ? `-${column}` : column;
        return h("th", attributes, link(state, 1, next, view, label));
      }),
      hasRow ? h("th", { scope: "col" }) : null,
    ),
  );
}

// The server sends a refused filter as its message's key, arguments and message, the message
// already in the viewer's language; plain text is shown as it is, as the engine does.
function errorText(error) {
  if (typeof error === "string") return error;
  return typeof error?.message === "string" ? error.message : null;
}

// An operator filter, or one a Table offers no field for, has no slot to show its error beside.
function unplacedErrors(state) {
  const slots = new Set(state.listing.filter.map(({ name }) => name));
  return Object.entries(state.errors)
    .map(([key, error]) => [key, errorText(error)])
    .filter(([key, error]) => !slots.has(key) && error !== null)
    .map(([key, error]) => h("p", { class: "sofie-field-error", role: "alert" }, `${key}: ${error}`));
}

function filters(props, state, view) {
  if (state.listing.filter.length === 0) return null;
  return h(
    "form",
    { class: "sofie-filter", method: "get", onsubmit: view.runtime?.filter() },
    state.listing.filter.map(({ name, label }) => {
      const value = state.filter?.[name];
      const asked = typeof value === "string" ? value : null;
      const error = errorText(state.errors[name]);
      return h(
        "label",
        null,
        label,
        " ",
        control(name, asked, state.controls[name], error !== null, view),
        error === null ? null : h("span", { class: "sofie-field-error" }, error),
      );
    }),
    state.size === null ? null : h("input", { type: "hidden", name: "size", value: String(state.size) }),
    state.sort ? h("input", { type: "hidden", name: "sort", value: state.sort }) : null,
    Object.entries(state.query)
      .filter(([, value]) => typeof value === "string")
      .map(([name, value]) => h("input", { type: "hidden", name, value })),
    h("button", { type: "submit" }, S.message(view, "table.filter", "Filter")),
  );
}

// An enum's variants are chosen from; any `type` but `datetime-local` is a search box; an
// instant given with its offset, or refused, is a text box, which holds the offset a picker would
// drop, and so is any instant in a zone this browser cannot place, whose cells show UTC.
function control(name, value, described, refused, view) {
  const invalid = refused ? { "aria-invalid": "true" } : {};
  const values = Array.isArray(described?.values) ? described.values.filter((choice) => typeof choice === "string") : [];
  if (values.length > 0) {
    const options = values.map((choice) =>
      h("option", choice === value ? { value: choice, selected: true } : { value: choice }, choice),
    );
    return h("select", { name, ...invalid }, h("option", { value: "" }), ...options);
  }
  const isWallClock = value === null || /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(value);
  const isPicker = isWallClock && !refused && S.knowsZone(view.conventions);
  const type = described?.type !== "datetime-local" ? "search" : isPicker ? "datetime-local" : "text";
  const input = { type, name };
  if (described?.type === "datetime-local" && !S.knowsZone(view.conventions)) input.placeholder = S.offsetPlaceholder;
  if (value !== null) input.value = value;
  return h("input", { ...input, ...invalid });
}

function pager(state, view) {
  if (state.page === null || (state.page <= 1 && !state.hasNext)) return null;
  return h(
    "nav",
    { class: "sofie-pager" },
    state.page > 1 ? link(state, state.page - 1, state.sort, view, S.message(view, "table.previous", "Previous")) : null,
    h("span", null, String(state.page)),
    state.hasNext ? link(state, state.page + 1, state.sort, view, S.message(view, "table.next", "Next")) : null,
  );
}
