// Run by sofie-cli's slow tests: `node navigation-check.mjs`. A client area, with the browser
// stubbed, shows the screen of the last navigation even when an earlier one or a post answers
// later, leaves in-page links, and going back or forward over them, to the browser, and posts an
// action's fields as JSON.
import assert from "node:assert/strict";

const pending = [];
globalThis.document = { title: "" };
globalThis.location = { href: "https://shop.test/notes", origin: "https://shop.test" };
globalThis.history = { pushState: (_, __, url) => { location.href = url; } };
globalThis.fetch = (href, init) =>
  new Promise((resolve, reject) => {
    init.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
    pending.push({ href, init, resolve });
  });

function screen(href, title) {
  return {
    status: 200,
    ok: true,
    url: href,
    redirected: false,
    headers: { get: () => "application/json" },
    json: async () => ({ screen: "notes", title }),
  };
}

const { Runtime } = await import("./core/runtime.js");
const shown = [];
const runtime = new Runtime({ notes: {} }, (state) => shown.push(state));
runtime.state = { envelope: { title: "Notes" }, submitted: {}, notice: [] };

const slow = runtime.navigate("https://shop.test/notes?page=2", true);
const fast = runtime.navigate("https://shop.test/notes?page=3", true);
pending[1].resolve(screen(pending[1].href, "Page 3"));
await fast;
pending[0].resolve(screen(pending[0].href, "Page 2"));
await slow;

assert.deepEqual(shown.map((state) => state.envelope.title), ["Page 3"]);
assert.equal(document.title, "Page 3");
assert.equal(location.href, "https://shop.test/notes?page=3");

function click(href) {
  const event = {
    defaultPrevented: false,
    button: 0,
    target: { closest: () => ({ href, target: "", hasAttribute: () => false }) },
    preventDefault() {
      this.defaultPrevented = true;
    },
  };
  runtime.intercept(event);
  return event.defaultPrevented;
}

assert.equal(click("https://shop.test/notes?page=3#reviews"), false, "an in-page link is left to the browser");
assert.equal(click("https://shop.test/notes?page=3#"), false, "an empty fragment is left to the browser");
assert.equal(click("https://shop.test/notes?page=4#reviews"), true, "a fragment on another page still loads that page");

const requested = pending.length;
location.href = "https://shop.test/notes?page=3#reviews";
runtime.popped();
location.href = "https://shop.test/notes?page=3";
runtime.popped();
assert.equal(pending.length, requested, "going back or forward over a hash keeps the screen");
location.href = "https://shop.test/notes?page=2";
runtime.popped();
assert.equal(pending.length, requested + 1, "going back to another page loads it");

const form = { tagName: "FORM", reset() {} };
const posted = runtime.post(form, "/notes/archive", null, null);
const archive = pending[pending.length - 1];
const newer = runtime.navigate("https://shop.test/notes?page=5", true);
const page5 = pending[pending.length - 1];
archive.resolve({ status: 204, ok: true, url: archive.href, redirected: false, headers: { get: () => "" } });
await posted;
assert.equal(pending[pending.length - 1], page5, "a post answered after a newer navigation began is dropped");
page5.resolve(screen(page5.href, "Page 5"));
await newer;
assert.equal(document.title, "Page 5");

const first = runtime.post({ tagName: "FORM", reset() {} }, "/notes/archive", null, null);
const firstPost = pending[pending.length - 1];
const second = { tagName: "FORM", reset() {} };
const secondPosted = runtime.post(second, "/notes", null, { formId: "note", input: {} });
const secondPost = pending[pending.length - 1];
firstPost.resolve({ status: 204, ok: true, url: firstPost.href, redirected: false, headers: { get: () => "" } });
await Promise.resolve();
const refusal = { notice: ["Too long"], errors: [{ field: "body", message: "Too long" }] };
secondPost.resolve({
  status: 422,
  ok: false,
  url: secondPost.href,
  redirected: false,
  headers: { get: () => "application/json" },
  json: async () => refusal,
});
await secondPosted;
assert.deepEqual(runtime.state.notice, ["Too long"], "a post's own reload is no navigation of the user's");
const reload = pending[pending.length - 1];
reload.resolve(screen(reload.href, "Page 5"));
await first;

const actionForm = { tagName: "FORM", reset() {} };
runtime.act("/slots/book", { slot: 7, note: "a&b" })({ currentTarget: actionForm, preventDefault() {} });
const booking = pending[pending.length - 1];
assert.equal(booking.init.method, "POST");
assert.equal(booking.init.body.type, "application/json", "an action's fields post as JSON");
assert.deepEqual(JSON.parse(await booking.init.body.text()), { slot: 7, note: "a&b" });
assert.ok(booking.init.headers["Idempotency-Key"], "an action's post carries its idempotency key");
booking.resolve({
  status: 409,
  ok: false,
  url: booking.href,
  redirected: false,
  headers: { get: () => "application/json" },
  json: async () => ({ error: "The slot is taken." }),
});
await new Promise((resolve) => setTimeout(resolve, 0));
assert.deepEqual(runtime.state.notice, ["The slot is taken."], "a refused action shows its notice");
