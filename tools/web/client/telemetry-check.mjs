// Run by sofie-cli's slow tests: `node telemetry-check.mjs`. A client area's telemetry, with the
// browser stubbed, sends what `engine.js` sends for an HTML page plus its own navigations.
import assert from "node:assert/strict";

const listeners = {};
const posted = [];
globalThis.addEventListener = (name, listener) => { listeners[name] = listener; };
globalThis.document = {
  visibilityState: "visible",
  addEventListener: (name, listener) => { listeners[name] = listener; },
};
globalThis.performance = { getEntriesByType: () => [{ responseStart: 12 }] };
globalThis.PerformanceObserver = { supportedEntryTypes: [] };
globalThis.fetch = (url, init) => {
  posted.push({ url, init });
  return Promise.resolve();
};

const { Telemetry } = await import("./core/telemetry.js");
const meta = {
  content: "/_sofie/telemetry",
  dataset: { route: "notes.board.list_notes", sample: "1", traceparent: "00-aaaa-bbbb-01" },
};

assert.equal(Telemetry.start(null), null);
assert.equal(Telemetry.start({ ...meta, dataset: { ...meta.dataset, sample: "0" } }), null);
const telemetry = Telemetry.start(meta);
const child = telemetry.traceparent().split("-");
assert.deepEqual([child[0], child[1], child[3]], ["00", "aaaa", "01"]);
assert.match(child[2], /^[0-9a-f]{16}$/);
telemetry.navigated();
listeners.error({ error: new TypeError("x") });
document.visibilityState = "hidden";
listeners.visibilitychange();

assert.equal(posted.length, 1);
assert.equal(posted[0].url, "/_sofie/telemetry");
assert.equal(posted[0].init.headers.traceparent, "00-aaaa-bbbb-01");
const body = JSON.parse(posted[0].init.body);
assert.equal(body.route, "notes.board.list_notes");
assert.deepEqual(
  body.records.map(({ kind, name }) => `${kind} ${name}`),
  ["vital TTFB", "navigation load", "navigation client", "error TypeError", "vital LCP", "vital CLS", "vital INP"],
);
