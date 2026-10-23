// A form field as the engine renders it: the label, the control its input type describes, and the
// slot its error is written in.
import * as S from "./format.js";
import { h } from "./tree.js";

export function field(described, written, form, view) {
  const id = `${form.id}-${described.name}`;
  const error = form.submitted?.errors?.[described.name];
  const posted = form.submitted && !["password", "file"].includes(described.control);
  const source = posted
    ? { posted: true, value: form.submitted.input[described.name] }
    : { posted: false, present: "value" in written, value: written.value };
  const control = {
    id,
    name: described.name,
    describedBy: `${id}-error`,
    required: described.required,
    constraints: described.constraints,
    invalid: error !== undefined,
  };
  const hidden = described.attributes.some(([name, value]) => name === "type" && value === "hidden");
  return h(
    "div",
    { class: "sofie-field" },
    hidden ? null : h("label", { for: id }, written.label ?? described.label ?? ""),
    controlOf(described, written, control, source, view),
    h("p", { class: "sofie-field-error", id: `${id}-error`, "aria-live": "polite" }, error ?? ""),
  );
}

function controlOf(described, written, control, source, view) {
  switch (described.control) {
    case "password":
      return input([["type", "password"]], control, source);
    case "file":
      return input([["type", "file"]], control, source);
    case "textarea":
      return h("textarea", attributes(control, ""), S.text(source.value));
    case "checkbox":
      return checkbox(control, source);
    case "select":
      return select(described, written, control, source, view);
    case "money":
      return moneyInput(written, control, source);
    case "percentage":
      return percentageInput(control, source);
    case "parts":
      return parts(described, control, source);
    default:
      return input(described.attributes, control, source, view);
  }
}

function attributes(control, suffix, identity = { id: control.id }, constraints = control.constraints) {
  const written = { ...identity, name: control.name + suffix, "aria-describedby": control.describedBy };
  if (control.required) written.required = true;
  for (const [name, bound] of constraints) written[name] = String(bound);
  if (control.invalid) written["aria-invalid"] = "true";
  return written;
}

// Only the first part carries the id the label points to; constraints describe the whole value.
function partAttributes(control, part, first) {
  return attributes(control, `.${part.name}`, first ? { id: control.id } : { "aria-label": part.label }, []);
}

// An instant in a `datetime-local` control, on the display zone's wall clock, which the server
// reads a posted value back in; its seconds come from the text, as `Date` keeps milliseconds only.
function instantControl(value, view) {
  const written = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::(\d{2}(?:\.(\d+))?))?Z$/.exec(value);
  if (!written) return null;
  const instant = new Date(value);
  if (Number.isNaN(instant.getTime())) return null;
  const minute = S.wallClock(view.conventions, instant);
  const seconds = written[1] ?? "00";
  // A text box holds what the control cannot: the offset telling a repeated wall clock's two
  // instants apart, or a fraction finer than milliseconds.
  const offset = S.repeatedOffset(view.conventions, instant);
  if (offset !== null) return { type: "text", value: `${minute}:${seconds}${offset}` };
  if ((written[2] ?? "").length > 3) {
    return { type: "text", value: `${minute}:${seconds}${S.offsetOf(view.conventions, instant)}` };
  }
  // Without `step="any"` the browser refuses the seconds.
  return /^00(\.0*)?$/.test(seconds) ? { value: minute } : { step: "any", value: `${minute}:${seconds}` };
}

// Posted text shown again as typed, in the control that holds it, by the engine's rule: a text box
// holds any text that is no wall clock, and a wall clock the zone repeats or that has a fraction
// finer than milliseconds. The engine also boxes a wall clock it cannot read in the zone.
function postedInstantControl(text, view) {
  const wallClock = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(?:\.(\d+))?)?$/.exec(text);
  if (!wallClock) return { type: "text", value: text };
  const fraction = wallClock[2] ?? "";
  if (fraction.length > 3 || S.repeats(view.conventions, text)) return { type: "text", value: text };
  return wallClock[1] ? { step: "any", value: text } : { value: text };
}

function instantControlOf(inputAttributes, source, view) {
  const isDateTime = inputAttributes.some(([name, kind]) => name === "type" && kind === "datetime-local");
  if (!isDateTime || !S.knowsZone(view.conventions)) return null;
  if (source.posted) return source.value === undefined ? null : postedInstantControl(S.text(source.value), view);
  return source.present ? instantControl(S.text(source.value), view) : null;
}

function input(inputAttributes, control, source, view) {
  const written = Object.fromEntries(inputAttributes);
  const instant = instantControlOf(inputAttributes, source, view);
  if (instant?.type) written.type = instant.type;
  if (instant?.step) written.step = instant.step;
  Object.assign(written, attributes(control, ""));
  if (source.posted ? source.value !== undefined : source.present) {
    written.value = instant?.value ?? S.text(source.value);
  }
  // A wall clock in a zone this browser cannot place would be read back in another; the
  // instant with its offset is read back exactly.
  if (written.type === "datetime-local" && !S.knowsZone(view.conventions)) {
    written.type = "text";
    written.placeholder = S.offsetPlaceholder;
  }
  return h("input", written);
}

// The hidden `false` comes first, so an unchecked box still posts and a checked one overrides it.
function checkbox(control, source) {
  const checked = source.posted ? source.value === "true" : source.value === true;
  const box = { type: "checkbox", ...attributes(control, ""), value: "true" };
  if (checked) box.checked = true;
  return [h("input", { type: "hidden", name: control.name, value: "false" }), h("input", box)];
}

function choiceOf(value) {
  return typeof value === "number" || typeof value === "string" ? S.text(value) : null;
}

function option(value, label, selected) {
  const written = { value };
  if (selected === value) written.selected = true;
  return h("option", written, label);
}

function select(described, written, control, source, view) {
  const selected = choiceOf(source.value);
  const options = written.options;
  const enumeration = typeof options === "string" ? view.enums?.[options] ?? [] : [];
  // A written `options` holding no list offers nothing, as the engine renders it.
  const choices = !("options" in written)
    ? described.options
    : Array.isArray(options)
      ? options.map(choiceOf).filter((choice) => choice !== null)
      : [];
  return h(
    "select",
    attributes(control, ""),
    control.required ? null : h("option", { value: "" }),
    enumeration.map((variant) => option(variant.name, variant.label, selected)),
    choices.map((choice) => option(choice, choice, selected)),
  );
}

function moneyInput(written, control, source) {
  const amount = S.moneyOf(written.value);
  const code = amount?.code ?? (typeof written.currency === "string" ? written.currency : null);
  const typed = (part) => (source.posted ? S.text(source.value?.[part]) : null);
  const amountInput = { type: "text", inputmode: "decimal", ...attributes(control, ".amount") };
  if (typed("amount") !== null) {
    if (typed("amount")) amountInput.value = typed("amount");
  } else if (amount) {
    amountInput.value = S.major(amount);
  }
  return h("div", { class: "sofie-money-input" }, h("input", amountInput), currency(control.name, code, typed("currency")));
}

function currency(name, code, typed) {
  if (!code) {
    const typedCurrency = {
      type: "text",
      class: "sofie-currency",
      maxlength: "3",
      "aria-label": "Currency",
      name: `${name}.currency`,
    };
    if (typed) typedCurrency.value = typed;
    return h("input", typedCurrency);
  }
  return [
    h("span", { class: "sofie-currency" }, code),
    h("input", { type: "hidden", name: `${name}.currency`, value: code }),
  ];
}

function percentageInput(control, source) {
  const written = { type: "text", inputmode: "decimal", ...attributes(control, "") };
  if (!source.posted && Number.isInteger(source.value)) {
    const magnitude = Math.abs(source.value);
    const sign = source.value < 0 ? "-" : "";
    written.value = `${sign}${Math.floor(magnitude / 100)}.${String(magnitude % 100).padStart(2, "0")}`;
  } else if (source.value !== null && source.value !== undefined) {
    written.value = S.text(source.value);
  }
  return h("div", { class: "sofie-percentage-input" }, h("input", written), h("span", { class: "sofie-unit" }, "%"));
}

function parts(described, control, source) {
  return h(
    "div",
    { class: "sofie-parts" },
    described.parts.map((part, index) => {
      const value = source.value?.[part.name];
      const written = partAttributes(control, part, index === 0);
      if (part.options.length === 0) {
        const partInput = { ...Object.fromEntries(part.attributes), ...written };
        if (value !== null && value !== undefined) partInput.value = S.text(value);
        return h("input", partInput);
      }
      const selected = typeof value === "string" ? value : null;
      return h(
        "select",
        written,
        control.required ? null : h("option", { value: "" }),
        part.options.map((choice) => option(choice, choice, selected)),
      );
    }),
  );
}
