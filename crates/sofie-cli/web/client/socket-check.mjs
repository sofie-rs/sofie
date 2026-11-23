// Run by sofie-cli's slow tests: `node socket-check.mjs`. The socket helper, against a stubbed
// WebSocket, speaks `/_sofie/socket`'s protocol and reopens as the server asks.
import assert from "node:assert/strict";

import { Socket } from "./core/socket.js";

class FakeWebSocket {
  static opened = [];

  constructor(url) {
    this.url = url;
    this.readyState = 0;
    this.sent = [];
    FakeWebSocket.opened.push(this);
  }

  send(text) {
    this.sent.push(JSON.parse(text));
  }

  open() {
    this.readyState = 1;
    this.onopen();
  }

  answer(message) {
    this.onmessage({ data: JSON.stringify(message) });
  }

  close(code) {
    this.readyState = 3;
    this.onclose({ code });
  }
}

const scheduled = [];
const schedule = (run, delay) => scheduled.push({ run, delay });
const socket = new Socket({ url: "ws://shop.test/_sofie/socket", WebSocket: FakeWebSocket, schedule });
const latest = () => FakeWebSocket.opened.at(-1);

// Nothing opens until something is asked; what is asked before it opens is sent once it does.
assert.equal(FakeWebSocket.opened.length, 0);
const heard = [];
const stop = socket.watch("/members/notes", (answer) => heard.push(answer));
const written = socket.call("POST", "/members/notes", { text: "hi" });
assert.equal(FakeWebSocket.opened.length, 1);
assert.equal(latest().url, "ws://shop.test/_sofie/socket");
latest().open();
assert.deepEqual(latest().sent, [
  { id: 1, method: "GET", path: "/members/notes" },
  { id: 2, method: "POST", path: "/members/notes", body: { text: "hi" } },
]);

// A watch hears each answer; a call resolves with its own.
latest().answer({ id: 1, status: 200, body: { count: 0 } });
latest().answer({ id: 2, status: 200, body: { id: "n1" } });
latest().answer({ id: 1, status: 200, body: { count: 1 } });
assert.deepEqual(await written, { status: 200, body: { id: "n1" } });
assert.deepEqual(heard.map((answer) => answer.body.count), [0, 1]);

// A GET call wants one answer, so its watch is stopped once answered.
const once = socket.call("GET", "/members/notes");
latest().answer({ id: 3, status: 200, body: { count: 1 } });
assert.deepEqual(await once, { status: 200, body: { count: 1 } });
assert.deepEqual(latest().sent.slice(-2), [{ id: 3, method: "GET", path: "/members/notes" }, { stop: 3 }]);

// The server's lifetime close reopens at once and asks every watch again; a call it left
// unanswered is rejected, as it may have run.
const unanswered = socket.call("DELETE", "/members/notes/n1");
latest().close(4000);
await assert.rejects(unanswered, /closed before the call was answered/);
assert.equal(scheduled.length, 1);
assert.equal(scheduled[0].delay, 0);
scheduled.shift().run();
latest().open();
assert.deepEqual(latest().sent, [{ id: 1, method: "GET", path: "/members/notes" }]);

// A lost connection reopens later each time, and an opened one starts over.
latest().close(1006);
const first = scheduled.shift();
first.run();
latest().close(1006);
const second = scheduled.shift();
assert.ok(first.delay >= 250 && first.delay <= 500, `${first.delay}`);
assert.ok(second.delay >= 500 && second.delay <= 1000, `${second.delay}`);
second.run();
latest().open();
latest().close(1006);
const afterOpening = scheduled.shift();
assert.ok(afterOpening.delay >= 250 && afterOpening.delay <= 500, `${afterOpening.delay}`);
afterOpening.run();
latest().open();

// A refused watch ends; a stopped one is told to the server and closes nothing.
const refused = [];
socket.watch("/members/secret", (answer) => refused.push(answer));
latest().answer({ id: 5, status: 403, body: "Forbidden." });
latest().answer({ id: 5, status: 200, body: "late" });
assert.deepEqual(refused, [{ status: 403, body: "Forbidden." }]);
stop();
assert.deepEqual(latest().sent.at(-1), { stop: 1 });

// With nothing left to hear, a closed socket stays closed until something is asked.
const opened = FakeWebSocket.opened.length;
latest().close(1006);
assert.equal(scheduled.length, 0);
assert.equal(FakeWebSocket.opened.length, opened);
