// The element trees the catalog builds and every adapter renders: `{ tag, attributes, children }`,
// text as strings, and `{ slot, argument }` where a screen's own content goes.

export function h(tag, attributes, ...children) {
  return { tag, attributes: attributes ?? {}, children: flatten(children) };
}

export function slot(name, argument) {
  return { slot: name, argument };
}

function flatten(children) {
  const flat = [];
  for (const child of children) {
    if (Array.isArray(child)) flat.push(...flatten(child));
    else if (child !== null && child !== undefined && child !== false && child !== "") flat.push(child);
  }
  return flat;
}

export function withId(attributes, id) {
  return id === undefined ? attributes : { ...attributes, id };
}

const VOID = new Set(["area", "br", "col", "hr", "img", "input", "source", "track", "wbr"]);

export function isVoid(tag) {
  return VOID.has(tag);
}

// As the engine writes markup, so a tree can be held to the server's HTML.
export function toHtml(node, slots = {}) {
  if (typeof node === "string") return escape(node);
  if (node.slot) return slots[node.slot]?.(node.argument) ?? "";
  let html = `<${node.tag}`;
  for (const [name, value] of Object.entries(node.attributes)) {
    if (name.startsWith("on") || value === false || value === undefined || value === null) continue;
    html += value === true ? ` ${name}` : ` ${name}="${escape(String(value))}"`;
  }
  html += ">";
  if (isVoid(node.tag)) return html;
  if (node.tag === "textarea") html += "\n";
  const raw = node.tag === "script";
  for (const child of node.children) html += raw && typeof child === "string" ? child : toHtml(child, slots);
  return `${html}</${node.tag}>`;
}

function escape(text) {
  return text.replace(/[&<>"']/g, (character) => ESCAPES[character]);
}

const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
