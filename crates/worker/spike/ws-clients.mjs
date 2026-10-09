// Spike 1: ten WebSocket clients against a Room Durable Object on `wrangler dev`.
// Usage: node crates/worker/spike/ws-clients.mjs [base-url]   (default http://localhost:8787)
// Needs Node >= 22 for the global WebSocket.

const BASE = process.argv[2] ?? "http://localhost:8787";
const CLIENTS = 10;
const SENDER = 3; // 1-based, as in "client 3"
// Idle time with all sockets open, long enough for workerd to evict the hibernatable room.
const IDLE_MS = Number(process.env.SPIKE_IDLE_MS ?? 15000);
const TIMEOUT_MS = 5000;
const PING = '{"type":"ping"}';
const PONG = '{"type":"pong"}';

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};

const withTimeout = (promise, what) =>
  Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(`timeout: ${what}`)), TIMEOUT_MS)),
  ]);

/** Resolves with the first text frame that satisfies `predicate`. */
const nextFrame = (ws, predicate, what) =>
  withTimeout(
    new Promise((resolve) => {
      const onMessage = (event) => {
        if (predicate(event.data)) {
          ws.removeEventListener("message", onMessage);
          resolve(event.data);
        }
      };
      ws.addEventListener("message", onMessage);
    }),
    what,
  );

const open = (url) =>
  withTimeout(
    new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.addEventListener("open", () => resolve(ws), { once: true });
      ws.addEventListener("error", () => reject(new Error(`cannot open ${url}`)), { once: true });
    }),
    `open ${url}`,
  );

const closed = (ws) =>
  withTimeout(new Promise((resolve) => ws.addEventListener("close", resolve, { once: true })), "close");

async function main() {
  // HTTP routes
  const created = await fetch(`${BASE}/api/new`, { method: "POST" });
  const { meeting, host_token: hostToken } = await created.json();
  check("POST /api/new returns 201", created.status === 201, `status ${created.status}`);
  check("meeting is three words", /^[a-z]+-[a-z]+-[a-z]+$/.test(meeting), meeting);
  check("host_token is 32 bytes hex", /^[0-9a-f]{64}$/.test(hostToken));

  const known = await fetch(`${BASE}/api/meeting/${meeting}`);
  check("GET /api/meeting/{words} known -> 200", known.status === 200, `status ${known.status}`);
  const unknown = await fetch(`${BASE}/api/meeting/no-such-meeting`);
  check("GET /api/meeting/{words} unknown -> 404", unknown.status === 404, `status ${unknown.status}`);
  const plain = await fetch(`${BASE}/ws/${meeting}`);
  check("GET /ws/{words} without upgrade -> 426", plain.status === 426, `status ${plain.status}`);

  // Ten sockets join and say hello
  const wsUrl = `${BASE.replace(/^http/, "ws")}/ws/${meeting}`;
  const sockets = await Promise.all(Array.from({ length: CLIENTS }, () => open(wsUrl)));
  check(`${CLIENTS} sockets open`, sockets.every((ws) => ws.readyState === WebSocket.OPEN));
  sockets.forEach((ws, i) => ws.send(JSON.stringify({ type: "hello", name: `client-${i + 1}` })));

  // One chat from client 3 reaches everyone, the sender included
  const text = `hello from client ${SENDER} at ${Date.now()}`;
  const expected = JSON.stringify({ type: "chat", from: `client-${SENDER}`, text });
  const received = sockets.map((ws, i) =>
    nextFrame(ws, (data) => data === expected, `chat at client-${i + 1}`).then(
      () => true,
      () => false,
    ),
  );
  sockets[SENDER - 1].send(JSON.stringify({ type: "chat", text }));
  const got = await Promise.all(received);
  check(
    `all ${CLIENTS} clients receive the chat from client-${SENDER}`,
    got.every(Boolean),
    `${got.filter(Boolean).length}/${CLIENTS}`,
  );

  // After an idle phase (room may be evicted), state from SQLite and attachments still works
  console.log(`      idle ${IDLE_MS} ms ...`);
  await new Promise((resolve) => setTimeout(resolve, IDLE_MS));
  const afterIdle = JSON.stringify({ type: "chat", from: "client-7", text: "still here" });
  const receivedAfterIdle = sockets.map((ws) =>
    nextFrame(ws, (data) => data === afterIdle, "chat after idle").then(() => true, () => false),
  );
  sockets[6].send(JSON.stringify({ type: "chat", text: "still here" }));
  const gotAfterIdle = await Promise.all(receivedAfterIdle);
  check(
    `after ${IDLE_MS} ms idle, a chat from client-7 still reaches all ${CLIENTS}`,
    gotAfterIdle.every(Boolean),
    `${gotAfterIdle.filter(Boolean).length}/${CLIENTS}`,
  );

  // Ping: the exact frame is answered by the runtime's auto-response; any other spelling by the room
  const pongs = await Promise.all(
    sockets.map((ws) => {
      const pong = nextFrame(ws, (data) => data === PONG, "auto-response pong");
      ws.send(PING);
      return pong.then(() => true, () => false);
    }),
  );
  check("auto-response answers the exact ping on every socket", pongs.every(Boolean));

  const handled = nextFrame(sockets[0], (data) => data === PONG, "handler pong");
  sockets[0].send('{ "type": "ping" }');
  check("room answers a differently spelled ping", await handled.then(() => true, () => false));

  const malformed = nextFrame(sockets[0], (data) => data.includes('"error"'), "malformed error");
  sockets[0].send("not json");
  check(
    "room rejects a malformed frame",
    (await malformed.catch(() => "")) === '{"type":"error","reason":"malformed"}',
  );

  // Chat before hello is refused
  const late = await open(wsUrl);
  const refused = nextFrame(late, (data) => data.includes('"error"'), "not_joined error");
  late.send(JSON.stringify({ type: "chat", text: "who am I" }));
  check(
    "chat before hello is refused",
    (await refused.catch(() => "")) === '{"type":"error","reason":"not_joined"}',
  );

  // Everyone leaves
  const all = [...sockets, late];
  const closes = all.map(closed);
  all.forEach((ws) => ws.close(1000, "bye"));
  const events = await Promise.all(closes.map((p) => p.catch(() => null)));
  check(
    "every socket completes the close handshake with 1000",
    events.every((event) => event?.code === 1000),
    events.map((event) => event?.code ?? "none").join(","),
  );
}

try {
  await main();
} catch (error) {
  check("spike ran to completion", false, error.message);
}
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${failed === 0 ? "PASS" : "FAIL"}: ${results.length - failed}/${results.length} checks passed`);
process.exit(failed === 0 ? 0 : 1);
