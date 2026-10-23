// A client area's browser telemetry, sent as `engine.js` sends an HTML page's: web vitals, long
// tasks, navigations and errors. Only kinds, names and numbers are sent, never text a person
// typed or a screen showed.

const LIMIT = 32;

export class Telemetry {
  static start(meta = document.querySelector('meta[name="sofie-telemetry"]')) {
    if (!meta || Math.random() >= Number(meta.dataset.sample)) return null;
    return new Telemetry(meta);
  }

  constructor(meta) {
    this.meta = meta;
    this.records = [];
    this.vitals = { LCP: 0, CLS: 0, INP: 0 };
    this.vitalsSent = false;
    this.observe("largest-contentful-paint", (entry) => { this.vitals.LCP = entry.startTime; });
    this.observe("layout-shift", (entry) => { if (!entry.hadRecentInput) this.vitals.CLS += entry.value; });
    this.observe("event", (entry) => { this.vitals.INP = Math.max(this.vitals.INP, entry.duration); });
    this.observe("paint", (entry) => {
      if (entry.name === "first-contentful-paint") this.add("vital", "FCP", entry.startTime);
    });
    this.observe("longtask", (entry) => this.add("long_task", "longtask", entry.duration));
    const navigation = performance.getEntriesByType("navigation")[0];
    if (navigation) this.add("vital", "TTFB", navigation.responseStart);
    this.add("navigation", "load", 0);
    addEventListener("error", (event) => this.add("error", event.error?.name || "Error", 0));
    addEventListener("unhandledrejection", (event) => {
      this.add("error", event.reason?.name || "unhandledrejection", 0);
    });
    document.addEventListener("visibilitychange", () => this.flush());
  }

  navigated() {
    this.add("navigation", "client", 0);
  }

  // A child of the page's span, so the area's requests join the page's trace.
  traceparent() {
    const parent = this.meta.dataset.traceparent;
    if (!parent) return null;
    const bytes = crypto.getRandomValues(new Uint8Array(8));
    const span = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
    const [version, trace, , flags] = parent.split("-");
    return [version, trace, span, flags].join("-");
  }

  add(kind, name, value) {
    if (this.records.length < LIMIT) this.records.push({ kind, name, value });
  }

  // An entry type a browser does not support is skipped.
  observe(type, each) {
    if (!(PerformanceObserver.supportedEntryTypes || []).includes(type)) return;
    new PerformanceObserver((list) => list.getEntries().forEach(each)).observe({ type, buffered: true });
  }

  flush() {
    if (document.visibilityState !== "hidden") return;
    if (!this.vitalsSent) {
      this.vitalsSent = true;
      for (const name in this.vitals) this.add("vital", name, this.vitals[name]);
    }
    this.send();
  }

  // A lost record is not worth an error a person would see.
  send() {
    if (this.records.length === 0) return;
    const headers = { "Content-Type": "application/json" };
    const traceparent = this.meta.dataset.traceparent;
    if (traceparent) headers.traceparent = traceparent;
    fetch(this.meta.content, {
      method: "POST",
      keepalive: true,
      credentials: "same-origin",
      headers,
      body: JSON.stringify({ route: this.meta.dataset.route, records: this.records.splice(0) }),
    }).catch(() => {});
  }
}
