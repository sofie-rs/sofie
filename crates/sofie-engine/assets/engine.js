// What htmx cannot keep and inline script may not do under the CSP.

// A control is marked invalid while its error slot holds a message: htmx
// swaps the slots out of band but cannot set one attribute.
document.addEventListener("htmx:afterSettle", function () {
  for (const slot of document.querySelectorAll(".sofie-field-error")) {
    const control = document.querySelector('[aria-describedby="' + slot.id + '"]');
    if (!control) continue;
    if (slot.textContent === "") control.removeAttribute("aria-invalid");
    else control.setAttribute("aria-invalid", "true");
  }
});

// Escape closes every open menu and panel.
function toggle(control, open) {
  const panel = document.getElementById(control.getAttribute("aria-controls"));
  if (!panel) return;
  control.setAttribute("aria-expanded", String(open));
  panel.hidden = !open;
}
document.addEventListener("click", function (event) {
  const control = event.target.closest("[aria-controls][aria-expanded]");
  if (control) toggle(control, control.getAttribute("aria-expanded") !== "true");
});
document.addEventListener("keydown", function (event) {
  if (event.key !== "Escape") return;
  for (const control of document.querySelectorAll('[aria-controls][aria-expanded="true"]')) {
    toggle(control, false);
  }
});

// The page writes the notices in its language where that is not English; read once, as a
// notice region swapped in later does not carry them.
const noticeTexts = Object.assign({}, document.getElementById("sofie-notice")?.dataset);
function said(name, english) {
  return noticeTexts[name] || english;
}

// A request with no answer, or an error page from a proxy, gets a notice instead of a broken swap.
function notify(text) {
  const region = document.getElementById("sofie-notice");
  if (!region) return;
  const paragraph = document.createElement("p");
  paragraph.textContent = text;
  region.replaceChildren(paragraph);
}
for (const name of ["htmx:sendError", "htmx:timeout"]) {
  document.addEventListener(name, function () {
    notify(said("unreachable", "The server could not be reached. Try again."));
  });
}
document.addEventListener("htmx:beforeSwap", function (event) {
  const request = event.detail.xhr;
  if (request.status < 500 || request.getResponseHeader("HX-Retarget")) return;
  event.detail.shouldSwap = false;
  notify(said("failed", "Something went wrong. Try again."));
});

// A write keeps its key until an answer arrives, so resending after a lost answer
// is applied once by a route that is `.idempotent()`; others ignore the header.
const unanswered = new WeakMap();
function newKey() {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, function (byte) {
    return byte.toString(16).padStart(2, "0");
  }).join("");
}
document.addEventListener("htmx:configRequest", function (event) {
  if (event.detail.verb === "get") return;
  const element = event.detail.elt;
  if (!unanswered.has(element)) unanswered.set(element, newKey());
  event.detail.headers["Idempotency-Key"] = unanswered.get(element);
});
document.addEventListener("htmx:afterRequest", function (event) {
  if (event.detail.xhr && event.detail.xhr.status > 0) unanswered.delete(event.detail.elt);
});

// Only kinds, names and numbers are sent, never text a person typed or a page showed.
(function () {
  const meta = document.querySelector('meta[name="sofie-telemetry"]');
  if (!meta || Math.random() >= Number(meta.dataset.sample)) return;
  const traceparent = meta.dataset.traceparent;
  const records = [];
  const vitals = { LCP: 0, CLS: 0, INP: 0 };
  let vitalsSent = false;

  function add(kind, name, value) {
    if (records.length < 32) records.push({ kind: kind, name: name, value: value });
  }
  // A lost record is not worth an error a person would see.
  function send() {
    if (records.length === 0) return;
    const headers = { "Content-Type": "application/json" };
    if (traceparent) headers.traceparent = traceparent;
    fetch(meta.content, {
      method: "POST",
      keepalive: true,
      credentials: "same-origin",
      headers: headers,
      body: JSON.stringify({ route: meta.dataset.route, records: records.splice(0) }),
    }).catch(function () {});
  }
  // An entry type a browser does not support is skipped.
  function observe(type, each) {
    if (!(PerformanceObserver.supportedEntryTypes || []).includes(type)) return;
    new PerformanceObserver(function (list) {
      list.getEntries().forEach(each);
    }).observe({ type: type, buffered: true });
  }
  // A child of the page's span, so an htmx request joins the page's trace.
  function childOf(parent) {
    const bytes = crypto.getRandomValues(new Uint8Array(8));
    const span = Array.from(bytes, function (byte) {
      return byte.toString(16).padStart(2, "0");
    }).join("");
    const parts = parent.split("-");
    return [parts[0], parts[1], span, parts[3]].join("-");
  }

  observe("largest-contentful-paint", function (entry) { vitals.LCP = entry.startTime; });
  observe("layout-shift", function (entry) { if (!entry.hadRecentInput) vitals.CLS += entry.value; });
  observe("event", function (entry) { vitals.INP = Math.max(vitals.INP, entry.duration); });
  observe("paint", function (entry) {
    if (entry.name === "first-contentful-paint") add("vital", "FCP", entry.startTime);
  });
  observe("longtask", function (entry) { add("long_task", "longtask", entry.duration); });
  const navigation = performance.getEntriesByType("navigation")[0];
  if (navigation) add("vital", "TTFB", navigation.responseStart);
  add("navigation", "load", 0);

  document.addEventListener("htmx:pushedIntoHistory", function () { add("navigation", "htmx", 0); });
  window.addEventListener("error", function (event) {
    add("error", (event.error && event.error.name) || "Error", 0);
  });
  window.addEventListener("unhandledrejection", function (event) {
    add("error", (event.reason && event.reason.name) || "unhandledrejection", 0);
  });
  if (traceparent) {
    document.addEventListener("htmx:configRequest", function (event) {
      event.detail.headers.traceparent = childOf(traceparent);
    });
  }
  document.addEventListener("visibilitychange", function () {
    if (document.visibilityState !== "hidden") return;
    if (!vitalsSent) {
      vitalsSent = true;
      for (const name in vitals) add("vital", name, vitals[name]);
    }
    send();
  });
})();

// A live screen refetches its fragments through their own route when what it read
// changes, so every guard applies again. A stream refetches once open again, as a change
// may have come in its gap: a lost one reconnects with backoff, one the server ended with
// `end` at once. A stream refused before opening past its seal's `until`, or twice in a
// row (a sign-out, a new key, a skewed clock), refetches the fragments: their marker brings
// a fresh seal, used by the next attempt.
(function () {
  let source = null;
  let stream = "";
  let delay = 1000;
  let missed = false;
  let refusals = 0;

  function listening(url) {
    return url.split("&until=")[0];
  }
  function isSealExpired(url) {
    const until = Number(new URLSearchParams(url.split("?")[1]).get("until"));
    return Date.now() / 1000 >= until;
  }
  // Each fragment is its own source, as htmx keeps only the last request queued on one; and
  // a swap drops what is queued on the swapped element, so a change during a fetch refetches
  // once the answer is in.
  const fetching = new Map();
  function refetchFragment(id) {
    if (fetching.has(id)) {
      fetching.set(id, true);
      return;
    }
    if (!document.getElementById(id)) return;
    fetching.set(id, false);
    const fragment = "#" + id;
    const path = location.pathname + location.search;
    htmx.ajax("GET", path, { source: fragment, target: fragment, swap: "outerHTML" }).finally(function () {
      const changed = fetching.get(id);
      fetching.delete(id);
      if (changed) refetchFragment(id);
    });
  }
  function refetch() {
    const marker = document.querySelector("#sofie-live[data-stream]");
    if (!marker) return;
    for (const id of marker.dataset.fragments.split(" ")) refetchFragment(id);
  }
  function open() {
    const opened = new EventSource(stream);
    let wasOpen = false;
    source = opened;
    opened.onopen = function () {
      wasOpen = true;
      delay = 1000;
      refusals = 0;
      if (missed) refetch();
      missed = false;
    };
    opened.onmessage = refetch;
    opened.addEventListener("end", function () {
      opened.close();
      if (source !== opened) return;
      source = null;
      missed = true;
      open();
    });
    opened.onerror = function () {
      opened.close();
      if (source !== opened) return;
      source = null;
      missed = true;
      if (!wasOpen) refused();
      setTimeout(function () {
        if (!source && stream) open();
      }, delay);
      delay = Math.min(delay * 2, 60000);
    };
  }
  function refused() {
    refusals += 1;
    if (refusals < 2 && !isSealExpired(stream)) return;
    refusals = 0;
    refetch();
  }
  function follow() {
    const marker = document.querySelector("#sofie-live[data-stream]");
    const wanted = marker ? marker.dataset.stream : "";
    const same = listening(wanted) === listening(stream);
    stream = wanted;
    if (same) return;
    if (source) source.close();
    source = null;
    delay = 1000;
    missed = false;
    refusals = 0;
    if (stream) open();
  }
  follow();
  document.addEventListener("htmx:afterSettle", follow);
})();

// A passkey ceremony: a button with `data-sofie-passkey` ("create" or "get") holds the
// server's options in `data-options`; the browser's answer goes to `data-post` as JSON,
// and the page follows the reply's `location`, or reloads.
(function () {
  function bytes(text) {
    const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
    return Uint8Array.from(binary, function (character) { return character.charCodeAt(0); });
  }
  function text(buffer) {
    const binary = String.fromCharCode.apply(null, new Uint8Array(buffer));
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function credentials(list) {
    return (list || []).map(function (credential) {
      return { type: credential.type, id: bytes(credential.id) };
    });
  }
  // `https:` or this site, so a replied value cannot run a script.
  function isFollowed(target) {
    if (typeof target !== "string") return false;
    try {
      const url = new URL(target, location.href);
      return url.protocol === "https:" || url.origin === location.origin;
    } catch (error) {
      return false;
    }
  }
  async function ceremony(button) {
    const options = JSON.parse(button.dataset.options);
    options.challenge = bytes(options.challenge);
    let credential;
    if (button.dataset.sofiePasskey === "create") {
      options.user.id = bytes(options.user.id);
      options.excludeCredentials = credentials(options.excludeCredentials);
      credential = await navigator.credentials.create({ publicKey: options });
    } else {
      options.allowCredentials = credentials(options.allowCredentials);
      credential = await navigator.credentials.get({ publicKey: options });
    }
    const response = credential.response;
    const body = { id: credential.id, client_data: text(response.clientDataJSON) };
    if (response.attestationObject) body.attestation_object = text(response.attestationObject);
    if (response.authenticatorData) body.authenticator_data = text(response.authenticatorData);
    if (response.signature) body.signature = text(response.signature);
    const answer = await fetch(button.dataset.post, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", Accept: "application/json" },
      body: JSON.stringify(body),
    });
    const reply = await answer.json().catch(function () { return {}; });
    if (!answer.ok) {
      notify(reply.error || said("passkeyRefused", "The passkey was not accepted. Try again."));
      return;
    }
    if (isFollowed(reply.location)) location.assign(reply.location);
    else location.reload();
  }
  document.addEventListener("click", function (event) {
    const button = event.target.closest("[data-sofie-passkey]");
    if (!button) return;
    event.preventDefault();
    if (!window.PublicKeyCredential) {
      notify(said("passkeyUnsupported", "This browser cannot use passkeys."));
      return;
    }
    ceremony(button).catch(function () {
      notify(said("passkeyUnused", "The passkey was not used. Try again."));
    });
  });
})();

// A page that only passes on what another site sent back, such as an OAuth callback,
// posts it at once; without script it waits for a click.
for (const form of document.querySelectorAll("form[data-sofie-autosubmit]")) form.requestSubmit();
