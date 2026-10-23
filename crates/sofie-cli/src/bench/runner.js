
import http from 'k6/http';
import { Rate, Trend } from 'k6/metrics';

const TARGET = __ENV.SOFIE_BENCH_TARGET;
const ACCOUNTS = JSON.parse(__ENV.SOFIE_BENCH_ACCOUNTS);
const VUS = 16;
const SECONDS = 10;
const SLOW_MS = 1000;

const measured = Object.fromEntries(
  ENDPOINTS.map((endpoint) => [
    endpoint.name,
    {
      latency: new Trend(`latency_${endpoint.metric}`, true),
      failed: new Rate(`failed_${endpoint.metric}`),
    },
  ]),
);

// One endpoint after another, so each one's p95 is its own.
export const options = {
  scenarios: Object.fromEntries(
    ENDPOINTS.map((endpoint, index) => [
      endpoint.metric,
      {
        executor: 'constant-vus',
        vus: VUS,
        duration: `${SECONDS}s`,
        startTime: `${index * (SECONDS + 1)}s`,
        gracefulStop: '1s',
        exec: 'load',
        env: { SOFIE_BENCH_ENDPOINT: String(index) },
      },
    ]),
  ),
};

export function setup() {
  const sessions = {};
  for (const [area, member] of Object.entries(AREAS)) {
    sessions[area] = signIn(area, member);
  }
  const ids = {};
  for (const collection of COLLECTIONS) {
    ids[collection.key] = firstId(fill(collection.path, collection.ids, ids), sessions[collection.area]);
  }
  return { sessions, ids };
}

export function load(data) {
  const endpoint = ENDPOINTS[Number(__ENV.SOFIE_BENCH_ENDPOINT)];
  const headers = { accept: endpoint.accept, traceparent: traceparent() };
  if (endpoint.area !== null) {
    headers.cookie = data.sessions[endpoint.area];
  }
  const params = { headers, redirects: 0, tags: { name: endpoint.name } };
  const url = TARGET + fill(endpoint.path, endpoint.ids, data.ids);
  let response;
  if (endpoint.method === 'POST') {
    headers['content-type'] = 'application/json';
    headers.origin = TARGET;
    if (endpoint.idempotent) {
      headers['idempotency-key'] = uuid();
    }
    const body = endpoint.bodies[(__VU + __ITER) % endpoint.bodies.length];
    response = http.post(url, JSON.stringify(body), params);
  } else {
    response = http.get(url, params);
  }
  measured[endpoint.name].latency.add(response.timings.duration);
  measured[endpoint.name].failed.add(endpoint.method === 'POST' ? response.status >= 400 : response.status !== 200);
  if (response.timings.duration > SLOW_MS) {
    const trace = headers.traceparent.split('-')[1];
    console.warn(`${endpoint.name} took ${Math.round(response.timings.duration)} ms: trace ${trace}`);
  }
}

export function handleSummary(data) {
  const routes = {};
  for (const endpoint of ENDPOINTS) {
    const latency = data.metrics[`latency_${endpoint.metric}`];
    const failed = data.metrics[`failed_${endpoint.metric}`];
    if (latency !== undefined && failed !== undefined) {
      routes[endpoint.name] = { p95_ms: latency.values['p(95)'], error_rate: failed.values.rate };
    }
  }
  return { [__ENV.SOFIE_BENCH_SUMMARY]: JSON.stringify({ routes }) };
}

// Signing up an address that exists changes nothing, so one local account serves every run.
function signIn(area, member) {
  const account = ACCOUNTS[area];
  const credentials = JSON.stringify({ email: account.email, password: account.password });
  const params = {
    headers: { accept: 'application/json', 'content-type': 'application/json', origin: TARGET },
    redirects: 0,
  };
  if (account.signUp && member.signUp !== null) {
    http.post(TARGET + member.signUp, credentials, params);
  }
  const response = http.post(TARGET + member.signIn, credentials, params);
  const cookie = Object.entries(response.cookies)
    .map(([name, values]) => `${name}=${values[0].value}`)
    .join('; ');
  if (cookie === '') {
    throw new Error(`signing in to ${area} at ${member.signIn} answered ${response.status} without a session`);
  }
  return cookie;
}

function firstId(path, cookie) {
  const headers = { accept: 'application/json' };
  if (cookie !== undefined) {
    headers.cookie = cookie;
  }
  const response = http.get(TARGET + path, { headers, redirects: 0 });
  const rows = response.status === 200 ? response.json() : [];
  if (!Array.isArray(rows) || rows.length === 0 || rows[0].id === undefined) {
    throw new Error(`GET ${path} answered ${response.status} with no record to load; seed the database first`);
  }
  return rows[0].id;
}

// A parameter takes the id of the list its path names, as two lists may fill parameters named alike.
function fill(path, keys, ids) {
  return path.replace(/\{(\w+)\}/g, (_, parameter) => encodeURIComponent(ids[keys[parameter]]));
}

function traceparent() {
  return `00-${hex(32)}-${hex(16)}-01`;
}

function uuid() {
  return `${hex(8)}-${hex(4)}-4${hex(3)}-a${hex(3)}-${hex(12)}`;
}

function hex(length) {
  let text = '';
  for (let index = 0; index < length; index += 1) {
    text += Math.floor(Math.random() * 16).toString(16);
  }
  return text;
}
