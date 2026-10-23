// The template runtime: the engine's meaning of paths, truthiness, text and URLs, and the
// formats of the request's language, which the server describes in the envelope.
import currencies from "sofie:currencies";

const SAFE_SCHEMES = ["http", "https", "mailto", "tel"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const NO_BREAK_SPACE = " ";

// Records and lists write nothing, so a template has to pick fields; a local date-time is
// the one record read as a value.
export function text(value) {
  switch (typeof value) {
    case "string":
      return value;
    case "number":
      return Number.isFinite(value) ? String(value) : "";
    case "boolean":
      return String(value);
    case "object":
      return localDateTime(value);
    default:
      return "";
  }
}

// `{local, zone}` as `2026-10-20 09:30 Europe/Istanbul`, as the server writes it.
function localDateTime(value) {
  if (!isLocalDateTime(value)) return "";
  const minute = value.local.length >= 16 && value.local[10] === "T" ? value.local.slice(0, 16) : value.local;
  return `${minute.replace("T", " ")} ${value.zone}`;
}

function isLocalDateTime(value) {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const names = Object.keys(value);
  return names.length === 2 && typeof value.local === "string" && typeof value.zone === "string";
}

export function truthy(value) {
  if (value === null || value === undefined) return false;
  if (Array.isArray(value)) return value.length > 0;
  switch (typeof value) {
    case "boolean":
      return value;
    case "number":
      return Number.isFinite(value) && value !== 0;
    case "string":
      return value.length > 0;
    case "object":
      return Object.keys(value).length > 0;
    default:
      return false;
  }
}

export function items(value) {
  return Array.isArray(value) ? value : [];
}

export function join(parts) {
  return parts.map(text).join("");
}

// RFC 3986's unreserved characters stay; everything else is percent-encoded, as the server does.
export function encode(written) {
  return encodeURIComponent(written).replace(
    /[!'()*]/g,
    (character) => `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

// A `{…}` part of a URL: an integer as its digits, anything else percent-encoded.
export function urlPart(value) {
  return Number.isInteger(value) ? String(value) : encode(text(value));
}

// An allowlist: `javascript:`, `data:` and any scheme invented later fall to `#`.
export function url(value) {
  const written = text(value);
  const end = written.search(/[:/?#]/);
  if (end === -1 || written[end] !== ":") return written;
  const scheme = [...written.slice(0, end)]
    .filter((character) => character > " ")
    .join("")
    .toLowerCase();
  return SAFE_SCHEMES.includes(scheme) ? written : "#";
}

export function gate(view, asks, name) {
  return view.gates?.[asks]?.[name] === true;
}

// A key the language lacks is written as the key, as the server writes it.
export function t(view, key, values) {
  const message = "count" in values ? counted(view, key, values.count) : view.messages?.[key];
  if (message === undefined) return key;
  return message.replace(/\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (_, name) => number(view, values[name]));
}

// `key.one` where the count is one, then `key.other`, then the plain key.
function counted(view, key, count) {
  const messages = view.messages ?? {};
  const one = isOne(view.conventions.plural, count) ? messages[`${key}.one`] : undefined;
  return one ?? messages[`${key}.other`] ?? messages[key];
}

function isOne(rule, count) {
  if (typeof count !== "number") return false;
  const size = Math.abs(count);
  if (rule === "one-is-one") return size === 1;
  if (rule === "one-ends-in-one-not-eleven") return Number.isInteger(size) && size % 10 === 1 && size % 100 !== 11;
  return rule === "one-is-zero-or-one" && size < 2;
}

// As the server's plain message: one with `{…}` parts to fill is not text on its own.
export function message(view, key, fallback) {
  const message = view.messages?.[key];
  return message === undefined || /\{[A-Za-z_][A-Za-z0-9_]*\}/.test(message) ? fallback : message;
}

export { message as label };

export function number(view, value) {
  if (typeof value !== "number") return text(value);
  return writeNumber(view.conventions, text(value));
}

export function money(view, value) {
  const amount = moneyOf(value);
  return amount ? writeMoney(view.conventions, amount) : "";
}

export function date(view, value) {
  if (isLocalDateTime(value)) return writeLocalDateTime(view.conventions, value);
  const written = text(value);
  if (!written) return "";
  return writeDate(view.conventions, written);
}

// Money as the server writes it: `minor` and `currency`, nothing else.
export function moneyOf(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return null;
  const names = Object.keys(value);
  if (names.length !== 2 || !Number.isInteger(value.minor)) return null;
  const currency = currencies[value.currency];
  return currency ? { minor: value.minor, code: value.currency, digits: currency[0], symbol: currency[1] } : null;
}

export function major(amount) {
  const digits = String(Math.abs(amount.minor)).padStart(amount.digits + 1, "0");
  const sign = amount.minor < 0 ? "-" : "";
  if (amount.digits === 0) return sign + digits;
  return `${sign}${digits.slice(0, -amount.digits)}.${digits.slice(-amount.digits)}`;
}

export function writeMoney(conventions, amount) {
  const written = major(amount);
  switch (conventions.money) {
    case "symbol-before": {
      const [sign, digits] = splitSign(written);
      const prefix = amount.symbol ?? amount.code + NO_BREAK_SPACE;
      return sign + prefix + writeNumber(conventions, digits);
    }
    case "symbol-after":
      return writeNumber(conventions, written) + NO_BREAK_SPACE + (amount.symbol ?? amount.code);
    default:
      return `${writeNumber(conventions, written)} ${amount.code}`;
  }
}

export function writeNumber(conventions, plain) {
  if (/[eE]/.test(plain)) return plain;
  const [sign, digits] = splitSign(plain);
  const [whole, fraction] = digits.split(".");
  let grouped = whole;
  if (conventions.group) {
    grouped = "";
    for (let index = 0; index < whole.length; index += 1) {
      if (index > 0 && (whole.length - index) % 3 === 0) grouped += conventions.group;
      grouped += whole[index];
    }
  }
  return sign + grouped + (fraction === undefined ? "" : conventions.decimal + fraction);
}

function splitSign(plain) {
  return plain.startsWith("-") ? ["-", plain.slice(1)] : ["", plain];
}

function writeDate(conventions, written) {
  if (/^\d{4}-\d{2}-\d{2}$/.test(written)) return writeDay(conventions, written);
  // A date-time without an offset names no instant, so it is written on its own wall clock.
  if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:[\d.]*)?$/.test(written)) {
    return writeDayAndMinute(conventions, written.slice(0, 10), written.slice(11, 16));
  }
  const instant = /^\d{4}-\d{2}-\d{2}T/.test(written) ? new Date(written) : null;
  if (!instant || Number.isNaN(instant.getTime())) return written;
  const minute = wallClock(conventions, instant);
  // Where the wall clock repeats, only the offset tells its two instants apart.
  const offset = repeatedOffset(conventions, instant);
  const zone = offset === null ? shownZone(conventions) : `${offset} ${shownZone(conventions)}`;
  return `${writeDayAndMinute(conventions, minute.slice(0, 10), minute.slice(11, 16))} ${zone}`;
}

function zoneOf(conventions) {
  return conventions.zone ?? "UTC";
}

const wallClocks = new Map();

// The records holding instants, by the envelope's `instants`: the data's shape with each instant
// `true` and a list standing for its items. JSON writes an instant as text, so text written like
// one is told apart by this alone.
export function instantFields(data, shape, found = new WeakMap()) {
  if (!shape || typeof shape !== "object" || !data || typeof data !== "object") return found;
  if (Array.isArray(data)) {
    for (const item of data) instantFields(item, shape, found);
    return found;
  }
  const names = Object.keys(shape).filter((name) => shape[name] === true);
  if (names.length > 0) found.set(data, new Set(names));
  for (const [name, inner] of Object.entries(shape)) instantFields(data[name], inner, found);
  return found;
}

// What an instant's text box asks for where the browser cannot place the display zone.
export const offsetPlaceholder = "with an offset, such as 2026-10-11T09:30:00Z";

// A browser whose time zone data predates the display zone shows UTC, named as UTC.
export function knowsZone(conventions) {
  return wallClockFormat(zoneOf(conventions)) !== null;
}

function shownZone(conventions) {
  return knowsZone(conventions) ? zoneOf(conventions) : "UTC";
}

function wallClockFormat(zone) {
  if (!wallClocks.has(zone)) wallClocks.set(zone, minuteFormat(zone));
  return wallClocks.get(zone);
}

function minuteFormat(zone) {
  const minutes = { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" };
  try {
    return new Intl.DateTimeFormat("en-US", { ...minutes, timeZone: zone, hourCycle: "h23" });
  } catch (error) {
    if (error instanceof RangeError) return null;
    throw error;
  }
}

// An instant on the shown zone's wall clock, `YYYY-MM-DDTHH:MM`, as the server shows it.
export function wallClock(conventions, instant) {
  const format = wallClockFormat(shownZone(conventions));
  const parts = Object.fromEntries(format.formatToParts(instant).map((part) => [part.type, part.value]));
  return `${parts.year.padStart(4, "0")}-${parts.month}-${parts.day}T${parts.hour}:${parts.minute}`;
}

// The instant's offset, `+01:00`, where a backward shift repeats its wall clock in the shown zone;
// a shift lasts at most two hours, so the offsets three hours away name the other side.
export function repeatedOffset(conventions, instant) {
  const format = wallClockFormat(shownZone(conventions));
  const time = instant.getTime();
  const own = offsetAt(format, time);
  const hour = 3600000;
  for (const other of [offsetAt(format, time - 3 * hour), offsetAt(format, time + 3 * hour)]) {
    if (other !== own && offsetAt(format, time + own - other) === other) return offsetText(own);
  }
  return null;
}

// The instant's offset in the shown zone, `+02:00`.
export function offsetOf(conventions, instant) {
  return offsetText(offsetAt(wallClockFormat(shownZone(conventions)), instant.getTime()));
}

// Whether a backward shift repeats a `datetime-local` wall clock: two offsets each place it.
export function repeats(conventions, wallClock) {
  const format = wallClockFormat(shownZone(conventions));
  const time = Date.parse(`${wallClock.slice(0, 16)}Z`);
  const hour = 3600000;
  const placing = [offsetAt(format, time - 3 * hour), offsetAt(format, time + 3 * hour)].filter(
    (offset) => offsetAt(format, time - offset) === offset,
  );
  return placing.length === 2 && placing[0] !== placing[1];
}

function offsetAt(format, time) {
  const parts = Object.fromEntries(format.formatToParts(time).map((part) => [part.type, part.value]));
  const minute = time - (((time % 60000) + 60000) % 60000);
  return Date.UTC(Number(parts.year), Number(parts.month) - 1, Number(parts.day), Number(parts.hour), Number(parts.minute)) - minute;
}

function offsetText(milliseconds) {
  const minutes = Math.abs(milliseconds) / 60000;
  const pad = (number) => String(number).padStart(2, "0");
  return `${milliseconds < 0 ? "-" : "+"}${pad(Math.floor(minutes / 60))}:${pad(minutes % 60)}`;
}

// On its own wall clock, so its zone is named rather than UTC.
function writeLocalDateTime(conventions, { local, zone }) {
  const shaped = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}/.test(local);
  const written = shaped ? writeDayAndMinute(conventions, local.slice(0, 10), local.slice(11, 16)) : local;
  return `${written} ${zone}`;
}

function writeDayAndMinute(conventions, day, minute) {
  const separator = conventions.days === "month-name-first" ? ", " : " ";
  return `${writeDay(conventions, day)}${separator}${minute}`;
}

function writeDay(conventions, iso) {
  const [year, month, day] = [iso.slice(0, 4), iso.slice(5, 7), iso.slice(8, 10)];
  switch (conventions.days) {
    case "month-name-first":
      return `${MONTHS[Number(month) - 1] ?? ""} ${day.replace(/^0+/, "")}, ${year}`;
    case "day-first-dotted":
      return `${day}.${month}.${year}`;
    default:
      return iso;
  }
}
