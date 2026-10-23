// The catalog's components as element trees, written as the engine writes them; `slots` names the
// content a screen gave a component (`children`, a table's `row`).
import { field } from "./field.js";
import * as S from "./format.js";
import { listed, listingTable } from "./listing.js";
import { body, header } from "./table.js";
import { h, slot, withId } from "./tree.js";

export function component(name, props, view, slots) {
  const build = COMPONENTS[name];
  if (!build) throw new Error(`the client runtime has no component \`${name}\``);
  return build(props, view, slots);
}

export const COMPONENTS = {
  Section: (props) => h("section", withId({}, props.id), h("h2", null, props.title), slot("children")),
  Table: (props, view, slots) => {
    const hasRow = slots.row === true;
    const state = listed(props);
    if (state) return listingTable(props, state, hasRow, view);
    return h("table", withId({}, props.id), header(props, hasRow), body(props, props.rows, hasRow, view));
  },
  Money: (props, view) => h("span", withId({ class: "sofie-money" }, props.id), S.money(view, props.value)),
  Action: (props, view) => {
    if ("when" in props && !S.truthy(props.when)) return null;
    const post = S.url(props.post);
    if (props.with === undefined) {
      return h("button", { type: "button", ...withId({}, props.id), onclick: view.runtime?.act(post) }, props.label);
    }
    // Hidden inputs as the engine writes them; the runtime posts the values themselves as JSON.
    const fields = Object.entries(props.with).map(([name, value]) =>
      h("input", { type: "hidden", name, value: S.text(value) }),
    );
    return h(
      "form",
      { ...withId({}, props.id), method: "post", action: post, onsubmit: view.runtime?.act(post, props.with) },
      fields,
      h("button", { type: "submit" }, props.label),
    );
  },
  Form: (props, view) => form(props, view),
  Badge: (props, view) => {
    const name = typeof props.value === "string" ? props.value : null;
    const variant = (view.enums?.[props.enum] ?? []).find((known) => known.name === name);
    return h(
      "span",
      withId({ class: variant?.class ?? "sofie-badge sofie-tone-neutral" }, props.id),
      variant ? variant.label : S.text(props.value),
    );
  },
  Grid: (props) => h("div", withId({ class: "sofie-grid" }, props.id), slot("children")),
  Card: (props) => {
    const attributes = withId({ class: "sofie-card" }, props.id);
    if (props.href === undefined) return h("div", attributes, slot("children"));
    return h("a", { ...attributes, href: S.url(props.href) }, slot("children"));
  },
  Image: (props) =>
    h("img", { class: "sofie-image", ...withId({}, props.id), src: S.url(props.src), alt: props.alt, loading: "lazy" }),
  Island: (props) =>
    h(props.name, withId({}, props.id), h("script", { type: "application/json" }, JSON.stringify(props.props ?? {}))),
};

// Fields in the order the server describes them, each with what the template wrote of it.
function form(props, view) {
  const description = view.forms?.[props.id] ?? { multipart: false, parts: [] };
  const written = new Map(
    props.parts.filter((part) => part.field).map((part) => [part.field.name, part.field]),
  );
  const submits = props.parts.filter((part) => "submit" in part).map((part) => part.submit);
  const state = { id: props.id, submitted: view.submitted?.[props.id] };
  const attributes = { id: props.id, method: "post" };
  if (description.multipart) attributes.enctype = "multipart/form-data";
  attributes.action = S.url(props.post);
  attributes.onsubmit = view.runtime?.submit(attributes.action, props.id);
  return h(
    "form",
    attributes,
    description.parts.map((part) =>
      part.field
        ? field(part.field, written.get(part.field.name) ?? {}, state, view)
        : h("button", { type: "submit" }, submits[part.submit] ?? ""),
    ),
  );
}
