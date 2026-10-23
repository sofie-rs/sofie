import * as S from "./format.js";
import { h, slot } from "./tree.js";

export function header(props, hasRow) {
  return h(
    "thead",
    null,
    h(
      "tr",
      null,
      props.labels.map((label) => h("th", { scope: "col" }, label)),
      hasRow ? h("th", { scope: "col" }) : null,
    ),
  );
}

export function body(props, rows, hasRow, view) {
  const items = S.items(rows);
  if (items.length === 0) {
    const width = props.columns.length + (hasRow ? 1 : 0);
    return h(
      "tbody",
      null,
      h("tr", { class: "sofie-empty" }, h("td", { colspan: String(width) }, S.message(view, "table.empty", "No entries"))),
    );
  }
  return h(
    "tbody",
    null,
    items.map((row) =>
      h(
        "tr",
        null,
        props.columns.map((column, index) => cell(row, column, index === 0 ? link(props, row) : null, view)),
        hasRow ? h("td", { class: "sofie-actions" }, slot("row", row)) : null,
      ),
    ),
  );
}

function link(props, row) {
  return props.link ? S.url(props.link(row)) : null;
}

function cell(row, column, href, view) {
  const value = row?.[column];
  const amount = S.moneyOf(value);
  const shown = amount ? S.writeMoney(view.conventions, amount) : shownText(row, column, view);
  const content = href === null ? shown : h("a", { href }, shown);
  return h("td", amount ? { class: "sofie-money" } : null, content);
}

function shownText(row, column, view) {
  const value = row?.[column];
  const isInstant = typeof value === "string" && view.instants?.get(row)?.has(column) === true;
  return isInstant ? S.date(view, value) : S.text(value);
}
