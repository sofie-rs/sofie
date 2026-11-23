// The page's one WebSocket to the app (`import { socket } from "sofie:socket"`): `watch` calls a
// GET and hears its answer again whenever what it read changes; `call` runs any route once.
// Every watch is asked again when the socket reopens, so changes made while it was closed are
// caught up.

const PATH = "/_sofie/socket";
const OPEN = 1;
// The server closes each socket after its lifetime so the current cookie is sent again.
const REOPEN_AT_ONCE = 4000;
const FIRST_DELAY = 500;
const LONGEST_DELAY = 30_000;
const CLOSED = "The socket closed before the call was answered.";

export class Socket {
  constructor({ url, WebSocket = globalThis.WebSocket, schedule = setTimeout } = {}) {
    this.url = url;
    this.WebSocket = WebSocket;
    this.schedule = schedule;
    this.nextId = 1;
    this.watches = new Map();
    this.calls = new Map();
    this.connection = null;
    this.reopening = false;
    this.delay = FIRST_DELAY;
  }

  // Answers arrive as `{ status, body }`; one that is not a success ends the watch.
  watch(path, onAnswer) {
    const id = this.nextId++;
    this.watches.set(id, { path, onAnswer });
    this.send({ id, method: "GET", path });
    return () => {
      if (this.watches.delete(id)) this.sendIfOpen({ stop: id });
    };
  }

  // A call sent but unanswered when the socket closes is rejected rather than sent again, as
  // it may have run.
  call(method, path, body) {
    const id = this.nextId++;
    const message = body === undefined ? { id, method, path } : { id, method, path, body };
    return new Promise((resolve, reject) => {
      this.calls.set(id, { message, resolve, reject, sent: false });
      this.send(message);
    });
  }

  send(message) {
    if (this.connection?.readyState === OPEN) {
      this.transmit(message);
      return;
    }
    this.open();
  }

  sendIfOpen(message) {
    if (this.connection?.readyState === OPEN) this.transmit(message);
  }

  transmit(message) {
    this.connection.send(JSON.stringify(message));
    const call = this.calls.get(message.id);
    if (call) call.sent = true;
  }

  open() {
    if (this.connection || this.reopening) return;
    const connection = new this.WebSocket(this.url ?? defaultUrl());
    this.connection = connection;
    connection.onopen = () => this.opened();
    connection.onmessage = (event) => this.answered(JSON.parse(event.data));
    connection.onclose = (event) => this.closed(event.code);
  }

  opened() {
    this.delay = FIRST_DELAY;
    for (const [id, { path }] of this.watches) this.transmit({ id, method: "GET", path });
    for (const call of this.calls.values()) if (!call.sent) this.transmit(call.message);
  }

  answered({ id, status, body }) {
    const answer = { status, body };
    const call = this.calls.get(id);
    if (call) {
      this.calls.delete(id);
      // A GET is watched by the server; a call wants only its first answer.
      if (call.message.method === "GET" && isSuccess(status)) this.sendIfOpen({ stop: id });
      call.resolve(answer);
      return;
    }
    const watch = this.watches.get(id);
    if (!watch) return;
    if (!isSuccess(status)) this.watches.delete(id);
    watch.onAnswer(answer);
  }

  closed(code) {
    this.connection = null;
    for (const [id, call] of this.calls) {
      if (!call.sent) continue;
      this.calls.delete(id);
      call.reject(new Error(CLOSED));
    }
    if (this.watches.size === 0 && this.calls.size === 0) return;
    const delay = code === REOPEN_AT_ONCE ? 0 : this.delay;
    if (code !== REOPEN_AT_ONCE) this.delay = Math.min(this.delay * 2, LONGEST_DELAY);
    this.reopening = true;
    // Spread out, so the tabs of a restarted server do not all reopen at the same moment.
    this.schedule(() => {
      this.reopening = false;
      this.open();
    }, delay * (0.5 + Math.random() / 2));
  }
}

function isSuccess(status) {
  return status >= 200 && status < 300;
}

function defaultUrl() {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}${PATH}`;
}

export const socket = new Socket();
