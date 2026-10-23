// A client area in the browser: the envelope being shown, navigation and posting, and what a
// refused form keeps. The adapter re-renders whenever `onChange` is called.

import { isFollowed, passkey } from "./passkey.js";
import { Telemetry } from "./telemetry.js";

export const SCREEN = "application/vnd.sofie.screen+json";

const UNREACHABLE = "The server could not be reached. Try again.";
const FAILED = "Something went wrong. Try again.";

export class Runtime {
  constructor(screens, onChange) {
    this.screens = screens;
    this.onChange = onChange;
    this.state = null;
    // A write keeps its key until an answer arrives, so resending after a lost answer is applied
    // once by a route that is `.idempotent()`.
    this.unanswered = new WeakMap();
    this.telemetry = null;
    this.autosubmitted = null;
    this.navigation = null;
    this.shownHref = null;
    this.navigations = 0;
  }

  start(element = document.getElementById("sofie-screen")) {
    const envelope = JSON.parse(element.textContent);
    this.telemetry = Telemetry.start();
    history.replaceState({ sofie: true }, "", location.href);
    this.shownHref = location.href;
    addEventListener("popstate", () => this.popped());
    document.addEventListener("click", (event) => this.intercept(event));
    document.addEventListener("click", (event) => passkey(event, this));
    this.show(envelope, {}, []);
  }

  show(envelope, submitted, notice) {
    this.state = { envelope, submitted, notice };
    document.title = envelope.title;
    this.onChange(this.state);
  }

  // Called by the adapter once a screen is in the document. A screen that only passes on what
  // another site sent back, such as an OAuth callback, posts it at once, as on an HTML page.
  rendered(envelope) {
    if (this.autosubmitted === envelope) return;
    this.autosubmitted = envelope;
    for (const form of document.querySelectorAll("form[data-sofie-autosubmit]")) form.requestSubmit();
  }

  // Going back or forward over an in-page link fires `popstate` too; reloading then would drop
  // a refused form's errors and input.
  popped() {
    if (isSamePage(location.href, this.shownHref)) return;
    this.shownHref = location.href;
    this.navigate(location.href, false);
  }

  intercept(event) {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest?.("a[href]");
    if (!link || link.target || link.hasAttribute("download")) return;
    const target = new URL(link.href);
    if (target.origin !== location.origin || isInPage(link.href)) return;
    event.preventDefault();
    this.navigate(target.href, true);
  }

  follow(href) {
    return (event) => {
      event.preventDefault();
      this.navigate(new URL(href, location.href).href, true);
    };
  }

  filter() {
    return (event) => {
      event.preventDefault();
      const query = new URLSearchParams(new FormData(event.currentTarget));
      this.navigate(new URL(`?${query}`, location.href).href, true);
    };
  }

  // An action's fields keep the types its screen was sent, so a number posts as a number.
  act(post, fields) {
    return (event) => {
      event.preventDefault();
      const body = fields === undefined ? null : new Blob([JSON.stringify(fields)], { type: "application/json" });
      this.post(event.currentTarget, post, body, null);
    };
  }

  submit(post, formId) {
    return (event) => {
      event.preventDefault();
      const form = event.currentTarget;
      const data = new FormData(form);
      const body = form.enctype === "multipart/form-data" ? data : new URLSearchParams(data);
      this.post(form, post, body, { formId, input: nested(data) });
    };
  }

  // Only what the user starts counts: a post's own reload must not drop another post's answer.
  navigate(href, push) {
    this.navigations += 1;
    return this.load(href, push);
  }

  // A newer load cancels one still loading, so a slow earlier answer cannot win.
  async load(href, push) {
    this.navigation?.abort();
    const navigation = new AbortController();
    this.navigation = navigation;
    const response = await this.request(href, { method: "GET", signal: navigation.signal });
    if (!response) return;
    try {
      await this.answer(response, { push, href });
    } catch (error) {
      if (!navigation.signal.aborted) throw error;
    }
  }

  async post(element, url, body, refused) {
    if (!this.unanswered.has(element)) this.unanswered.set(element, newKey());
    const headers = { "Idempotency-Key": this.unanswered.get(element) };
    const navigations = this.navigations;
    const response = await this.request(new URL(url, location.href).href, { method: "POST", body, headers });
    if (!response) return;
    this.unanswered.delete(element);
    // Its reload or redirect would replace the screen the user has moved on to.
    if (this.navigations !== navigations) return;
    const shown = await this.answer(response, { push: response.redirected, href: location.href, refused });
    if (shown && element.tagName === "FORM") element.reset();
  }

  async request(href, init) {
    const headers = { ...init.headers, Accept: SCREEN };
    const traceparent = this.telemetry?.traceparent();
    if (traceparent) headers.traceparent = traceparent;
    try {
      return await fetch(href, { ...init, credentials: "same-origin", headers });
    } catch {
      if (init.signal?.aborted) return null;
      this.notify([UNREACHABLE]);
      return null;
    }
  }

  // A screen this bundle does not hold, or a refusal to sign in first, loads as a whole page.
  async answer(response, { push, href, refused }) {
    if (response.status === 401) {
      location.assign(href);
      return false;
    }
    if (response.status === 204) {
      await this.load(location.href, false);
      return true;
    }
    const isJson = (response.headers.get("content-type") ?? "").startsWith("application/json");
    if (!isJson) {
      if (response.ok) location.assign(response.url);
      else this.notify([FAILED]);
      return false;
    }
    const body = await response.json();
    // Another site, such as a payment or sign-in provider, which fetch cannot follow to.
    if (response.ok && isFollowed(body.location)) {
      location.assign(body.location);
      return false;
    }
    if (response.ok && body.screen) {
      if (!this.screens[body.screen]) {
        location.assign(response.url);
        return false;
      }
      if (push) {
        history.pushState({ sofie: true }, "", response.url);
        this.shownHref = response.url;
        this.telemetry?.navigated();
      }
      this.show(body, {}, []);
      return true;
    }
    const notice = body.notice ?? (body.error ? [body.error] : [FAILED]);
    const submitted = { ...this.state.submitted };
    if (refused && Array.isArray(body.errors)) {
      const errors = Object.fromEntries(body.errors.map(({ field, message }) => [field, message]));
      submitted[refused.formId] = { input: refused.input, errors };
    }
    this.show(this.state.envelope, submitted, notice);
    return false;
  }

  notify(notice) {
    this.show(this.state.envelope, this.state.submitted, notice);
  }
}

// `amount.currency=EUR` posts as `{ amount: { currency: "EUR" } }`, as the server reads a form.
function nested(data) {
  const input = {};
  for (const [name, value] of data) {
    if (typeof value !== "string") continue;
    const path = name.split(".");
    let into = input;
    for (const part of path.slice(0, -1)) {
      into[part] = typeof into[part] === "object" ? into[part] : {};
      into = into[part];
    }
    into[path[path.length - 1]] = value;
  }
  return input;
}

function newKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function isInPage(href) {
  return href.includes("#") && isSamePage(href, location.href);
}

function isSamePage(href, other) {
  if (!other) return false;
  const target = new URL(href);
  const current = new URL(other);
  return target.pathname === current.pathname && target.search === current.search;
}
