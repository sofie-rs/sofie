// Run by sofie-cli's slow tests: `node engine-check.mjs`. `engine.js`, with the browser and htmx
// stubbed, refetches every fragment of a live screen when what it read changes, and on a
// reconnect only when a change may have been missed or the stream keeps being refused.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { runInNewContext } from "node:vm";

const fragments = ["summary", "orders", "totals"];
const sealed = (until) => `/_sofie/live/area?topics=*&until=${until}&seal=s`;
const inAnHour = Math.floor(Date.now() / 1000) + 3600;
const marker = { dataset: { stream: sealed(inAnHour), fragments: fragments.join(" ") } };
// A page's own element may carry the marker's id too; the marker is the one with a stream.
const elements = {
  "sofie-live": { id: "sofie-live", dataset: {} },
  ...Object.fromEntries(fragments.map((id) => [id, { id }])),
};
const requests = [];
const streams = [];
const timers = [];
let settledSwaps;
const context = {
  location: { pathname: "/orders", search: "?page=2" },
  document: {
    addEventListener(name, listener) {
      if (name === "htmx:afterSettle") settledSwaps = listener;
    },
    getElementById: (id) => elements[id] ?? null,
    querySelector: (selector) => (selector === "#sofie-live[data-stream]" ? marker : null),
    querySelectorAll: () => [],
  },
  htmx: {
    ajax(verb, path, options) {
      let answer;
      const answered = new Promise((resolve) => { answer = resolve; });
      requests.push({ verb, path, options, answer });
      return answered;
    },
  },
  EventSource: class {
    constructor(url) {
      this.url = url;
      this.listeners = {};
      streams.push(this);
    }
    addEventListener(name, listener) {
      this.listeners[name] = listener;
    }
    close() {
      this.closed = true;
    }
  },
  setTimeout: (callback) => timers.push(callback),
  URLSearchParams,
};
const engine = new URL("../../../crates/sofie-engine/assets/engine.js", import.meta.url);
runInNewContext(await readFile(engine, "utf8"), context);

assert.equal(streams.length, 1);
streams[0].onmessage();
assert.deepEqual(
  requests.map(({ verb, path, options }) => [verb, path, options.source, options.target, options.swap]),
  fragments.map((id) => ["GET", "/orders?page=2", `#${id}`, `#${id}`, "outerHTML"]),
);

// A change while a fragment is being fetched may postdate its answer: one more fetch follows it.
streams[0].onmessage();
streams[0].onmessage();
assert.equal(requests.length, fragments.length);
requests[1].answer();
await new Promise((resolve) => setImmediate(resolve));
assert.equal(requests.length, fragments.length + 1);
assert.equal(requests.at(-1).options.target, "#orders");

async function settle() {
  for (let answered = 0; answered < requests.length; answered += 1) {
    requests[answered].answer();
    await new Promise((resolve) => setImmediate(resolve));
  }
}
await settle();
const settled = requests.length;

streams[0].onopen();
streams[0].listeners.end();
assert.ok(streams[0].closed);
assert.equal(streams.length, 2, "a stream that ended for its lifetime reconnects at once");
streams[1].onopen();
assert.equal(requests.length, settled + fragments.length, "and refetches once open, as commits in its gap went unheard");
await settle();

const refusing = requests.length;
streams[1].onerror();
timers.shift()();
streams[2].onerror();
assert.equal(requests.length, refusing, "a refused reconnect within the seal refetches nothing");
timers.shift()();
streams[3].onerror();
assert.equal(requests.length, refusing + fragments.length, "a second refusal in a row refetches");
await settle();
timers.shift()();
streams[4].onopen();
assert.equal(requests.length, refusing + 2 * fragments.length, "a lost stream refetches once open again");
await settle();

marker.dataset.stream = sealed(inAnHour - 7200);
settledSwaps();
const expired = requests.length;
streams[4].onerror();
timers.shift()();
assert.equal(streams[5].url, marker.dataset.stream);
streams[5].onerror();
assert.equal(requests.length, expired + fragments.length, "a refused reconnect past the seal refetches");
