// Room Durable Object running meeting-core, driven by WebSocket clients on `wrangler dev`.
// Starts a mock Realtime SFU on SPIKE_SFU_PORT (default 8788), so the run needs no network.
//
// Start the Worker first (from the repo root):
//   pnpm exec wrangler dev --var ROOM_JURISDICTION: --var CREATE_SECRET:spike-secret \
//     --var CALLS_APP_ID:spike --var CALLS_APP_SECRET:spike-sfu \
//     --var SFU_API_URL:http://localhost:8788/v1
// Then: node crates/worker/spike/ws-clients.mjs [base-url]   (default http://localhost:8787)
// SPIKE_IDLE_MS sets the idle phase (default 15000). SPIKE_KEEPALIVE=1 adds the keepalive check,
// which waits about two minutes. Needs Node >= 22 for the global WebSocket.

import { createServer, request } from "node:http";
import { randomBytes } from "node:crypto";

const BASE = process.argv[2] ?? "http://localhost:8787";
const WS_BASE = BASE.replace(/^http/, "ws");
const SECRET = process.env.SPIKE_CREATE_SECRET ?? "spike-secret";
const SFU_PORT = Number(process.env.SPIKE_SFU_PORT ?? 8788);
const SFU_SECRET = "spike-sfu";
const IDLE_MS = Number(process.env.SPIKE_IDLE_MS ?? 15000);
const TIMEOUT_MS = 5000;
const PING = '{"type":"ping"}';

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const withTimeout = (promise, what, ms = TIMEOUT_MS) =>
  Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(`timeout: ${what}`)), ms)),
  ]);

// Mock SFU: just enough of the HTTPS API for the room's calls, plus a switch to fail them all.
const sfu = { down: false, calls: [], nextMid: 0 };
const sfuServer = createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  const json = body ? JSON.parse(body) : null;
  const path = new URL(req.url, "http://sfu").pathname;
  const op = path.replace(/^\/v1\/apps\/spike\/sessions\/(new|[0-9a-f]+\/)/, (_, m) => (m === "new" ? "new" : ""));
  sfu.calls.push({ method: req.method, op, json });
  const reply = (status, value) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  if (req.headers.authorization !== `Bearer ${SFU_SECRET}`) {
    return reply(401, { errorCode: "unauthorized", errorDescription: "bad app secret" });
  }
  if (sfu.down) {
    return reply(503, { errorCode: "temporarily_unavailable_error", errorDescription: "mock SFU is down" });
  }
  switch (`${req.method} ${op}`) {
    case "POST new":
      return reply(201, { sessionId: randomBytes(16).toString("hex") });
    case "POST tracks/new":
      return json.tracks[0].location === "local"
        ? reply(200, {
            requiresImmediateRenegotiation: false,
            tracks: json.tracks.map(({ mid, trackName }) => ({ mid, trackName })),
            sessionDescription: { type: "answer", sdp: "mock-answer" },
          })
        : reply(200, {
            requiresImmediateRenegotiation: true,
            tracks: json.tracks.map(({ sessionId, trackName }) => ({ sessionId, trackName, mid: String(sfu.nextMid++) })),
            sessionDescription: { type: "offer", sdp: "mock-offer" },
          });
    case "PUT renegotiate":
      return reply(200, {});
    case "PUT tracks/close":
    case "PUT tracks/update":
      return reply(200, { requiresImmediateRenegotiation: false, tracks: json.tracks });
    default:
      return reply(404, { errorCode: "not_found", errorDescription: `${req.method} ${path}` });
  }
});

/** A signalling client that keeps every server message until a test takes it. */
class Client {
  constructor(ws) {
    this.ws = ws;
    this.inbox = [];
    this.waiters = [];
    this.closed = new Promise((resolve) => ws.addEventListener("close", resolve, { once: true }));
    ws.addEventListener("message", (event) => {
      this.inbox.push(JSON.parse(event.data));
      this.waiters = this.waiters.filter((waiter) => !waiter());
    });
  }

  static open(meeting) {
    const url = `${WS_BASE}/ws/${meeting}`;
    return withTimeout(
      new Promise((resolve, reject) => {
        const ws = new WebSocket(url);
        ws.addEventListener("open", () => resolve(new Client(ws)), { once: true });
        ws.addEventListener("error", () => reject(new Error(`cannot open ${url}`)), { once: true });
      }),
      `open ${url}`,
    );
  }

  send(msg) {
    this.ws.send(typeof msg === "string" ? msg : JSON.stringify(msg));
  }

  /** Resolves with (and removes) the first message of `type` that satisfies `predicate`. */
  take(type, predicate = () => true) {
    return withTimeout(
      new Promise((resolve) => {
        const attempt = () => {
          const index = this.inbox.findIndex((msg) => msg.type === type && predicate(msg));
          if (index < 0) return false;
          resolve(this.inbox.splice(index, 1)[0]);
          return true;
        };
        if (!attempt()) this.waiters.push(attempt);
      }),
      `${type} message`,
    );
  }

  /** Whether a message of `type` arrives, as a boolean. */
  gets(type, predicate) {
    return this.take(type, predicate).then(
      () => true,
      () => false,
    );
  }
}

const createMeeting = (body, secret = SECRET) =>
  fetch(`${BASE}/api/new`, {
    method: "POST",
    headers: secret ? { authorization: `Bearer ${secret}` } : {},
    body: body ? JSON.stringify(body) : undefined,
  });

/** The status of a WebSocket upgrade request: 101 if accepted. (`fetch` forbids `Upgrade`.) */
const upgradeStatus = (meeting) =>
  withTimeout(
    new Promise((resolve, reject) => {
      const req = request(`${BASE}/ws/${meeting}`, {
        headers: {
          connection: "Upgrade",
          upgrade: "websocket",
          "sec-websocket-version": "13",
          "sec-websocket-key": randomBytes(16).toString("base64"),
        },
      });
      req.on("upgrade", (_res, socket) => {
        socket.destroy();
        resolve(101);
      });
      req.on("response", (res) => {
        res.resume();
        resolve(res.statusCode);
      });
      req.on("error", reject);
      req.end();
    }),
    "upgrade status",
  );

const join = (client, fields) => client.send({ type: "join", consent: true, ...fields });
const names = (state) => state.participants.map((p) => p.name).join(",");

async function main() {
  // HTTP routes
  check("POST /api/new without a secret -> 401", (await createMeeting(null, "")).status === 401);
  check("POST /api/new with a wrong secret -> 401", (await createMeeting(null, "wrong")).status === 401);
  const created = await createMeeting({ recording: "fromStart" });
  const { meeting, hostToken } = await created.json();
  check("POST /api/new with the secret -> 201", created.status === 201, `status ${created.status}`);
  check("meeting is three words", /^[a-z]+-[a-z]+-[a-z]+$/.test(meeting), meeting);
  check("hostToken is 32 bytes hex", /^[0-9a-f]{64}$/.test(hostToken));

  const info = await fetch(`${BASE}/api/meeting/${meeting}`);
  const infoBody = await info.json();
  check(
    "GET /api/meeting/{words} -> recording policy, not ended",
    info.status === 200 && infoBody.recordingPolicy === "fromStart" && infoBody.ended === false,
    JSON.stringify(infoBody),
  );
  const upper = await fetch(`${BASE}/api/meeting/${meeting.toUpperCase()}`);
  check("GET /api/meeting/{WORDS} matches case-insensitively", upper.status === 200, `status ${upper.status}`);
  const unknown = await fetch(`${BASE}/api/meeting/no-such-meeting`);
  check("GET /api/meeting/{words} unknown -> 404", unknown.status === 404, `status ${unknown.status}`);
  const plain = await fetch(`${BASE}/ws/${meeting}`);
  check("GET /ws/{words} without upgrade -> 426", plain.status === 426, `status ${plain.status}`);
  const ice = await (await fetch(`${BASE}/api/ice`)).json();
  check("GET /api/ice returns Cloudflare STUN", ice.iceServers?.[0]?.urls?.[0] === "stun:stun.cloudflare.com:3478");

  // Host joins with the token; the room records from the start
  const host = await Client.open(meeting);
  join(host, { name: "Host", hostToken });
  const hostWelcome = await host.take("welcome");
  const hostId = hostWelcome.you;
  check(
    "host gets Welcome as host, recording on",
    hostWelcome.state.participants[0]?.role === "host" && hostWelcome.state.recording?.by === hostId,
    `you=${hostId}`,
  );
  host.send(PING);
  check("auto-response answers the exact ping", await host.gets("pong"));
  host.send("not json");
  check(
    "room rejects a malformed frame",
    await host.gets("error", (m) => m.code === "malformedMessage"),
  );

  // A guest joins with consent; everyone sees the new state
  const guest = await Client.open(meeting);
  join(guest, { name: "Guest" });
  const guestWelcome = await guest.take("welcome");
  const guestId = guestWelcome.you;
  check("guest with consent gets Welcome", guestWelcome.state.participants.length === 2, names(guestWelcome.state));
  check(
    "host gets the State broadcast with the guest joined",
    await host.gets("state", (m) => m.state.participants.some((p) => p.id === guestId && p.presence === "joined")),
  );

  // Publish through the mock SFU; the guest is offered the host's track and answers
  host.send({ type: "publishOffer", sdp: "client-offer", tracks: [{ mid: "0", kind: "audio", name: "mic" }] });
  check("host gets the SFU's PublishAnswer", await host.gets("publishAnswer", (m) => m.sdp === "mock-answer"));
  const offer = await guest.take("subscribeOffer").catch(() => null);
  check(
    "guest gets a SubscribeOffer for the host's audio",
    offer?.tracks?.[0]?.track?.owner === hostId && offer.tracks[0].track.kind === "audio",
    JSON.stringify(offer?.tracks),
  );
  guest.send({ type: "subscribeAnswer", sdp: "client-answer" });
  await sleep(500);
  check("the guest's answer reaches the SFU's renegotiate", sfu.calls.some((c) => c.op === "renegotiate"));

  // Idle: the room may be evicted; state comes back from storage and attachments
  console.log(`      idle ${IDLE_MS} ms ...`);
  await sleep(IDLE_MS);

  // Lock: a third client knocks and waits; the host admits, then kicks them
  host.send({ type: "lock", on: true });
  check("Lock reaches the guest as State", await guest.gets("state", (m) => m.state.locked));
  const third = await Client.open(meeting);
  join(third, { name: "Third" });
  check("a third client in the locked room gets Waiting", await third.gets("waiting"));
  const knock = await host.take("knock").catch(() => null);
  check("host gets the Knock", knock !== null, `who=${knock?.who}`);
  host.send({ type: "admit", who: knock?.who });
  const thirdWelcome = await third.take("welcome").catch(() => null);
  check("admitted client gets Welcome", thirdWelcome?.you === knock?.who);
  host.send({ type: "kick", who: knock?.who });
  check("kicked client gets Kicked", await third.gets("kicked"));
  const kickClose = await withTimeout(third.closed, "kick close").catch(() => null);
  check("kicked client's socket closes", kickClose !== null, `code ${kickClose?.code}`);
  guest.send({ type: "kick", who: hostId });
  check("a guest cannot kick", await guest.gets("error", (m) => m.code === "notHost"));

  // Resume: a second socket with the resume token takes the guest's place
  const resumed = await Client.open(meeting);
  resumed.send({ type: "join", name: "Guest", consent: true, resume: guestWelcome.resume });
  const resumedWelcome = await resumed.take("welcome").catch(() => null);
  check("resume keeps the participant id", resumedWelcome?.you === guestId, `you=${resumedWelcome?.you}`);
  const oldClose = await withTimeout(guest.closed, "old socket close").catch(() => null);
  check("the room closes the old socket", oldClose !== null, `code ${oldClose?.code}`);
  host.send({ type: "mute", who: guestId });
  check("messages for the participant reach the new socket", await resumed.gets("muted"));

  // End: everyone gets Ended, and the room refuses the next socket
  host.send({ type: "end" });
  const ended = await Promise.all([host.gets("ended"), resumed.gets("ended")]);
  check("End reaches everyone", ended.every(Boolean), ended.join(","));
  const closes = await Promise.all([host.closed, resumed.closed].map((p) => withTimeout(p, "end close").catch(() => null)));
  check("every socket closes after End", closes.every((c) => c !== null), closes.map((c) => c?.code).join(","));
  const after = await upgradeStatus(meeting);
  check("the next socket is refused", after === 404 || after === 410, `status ${after}`);
  const gone = await fetch(`${BASE}/api/meeting/${meeting}`);
  check("the words are released", gone.status === 404, `status ${gone.status}`);

  // SFU down: joining fails with SfuUnavailable and the socket closes
  const second = await (await createMeeting({ recording: "hostDecides" })).json();
  sfu.down = true;
  const lonely = await Client.open(second.meeting);
  join(lonely, { name: "Host", hostToken: second.hostToken });
  check("with the SFU down, joining yields Error{SfuUnavailable}", await lonely.gets("error", (m) => m.code === "sfuUnavailable"));
  const lonelyClose = await withTimeout(lonely.closed, "sfu down close").catch(() => null);
  check("…and the room closes the socket", lonelyClose !== null, `code ${lonelyClose?.code}`);
  sfu.down = false;

  if (process.env.SPIKE_KEEPALIVE === "1") {
    // Keepalive: a socket that never pings is closed within two minutes; a pinging one stays
    const third = await (await createMeeting({ recording: "hostDecides" })).json();
    const silent = await Client.open(third.meeting);
    const pinging = await Client.open(third.meeting);
    const timer = setInterval(() => pinging.send(PING), 20000);
    console.log("      waiting up to 130 s for the keepalive check ...");
    const silentClose = await withTimeout(silent.closed, "keepalive close", 130000).catch(() => null);
    clearInterval(timer);
    check("keepalive closes a socket that never pings (1001)", silentClose?.code === 1001, `code ${silentClose?.code}`);
    check("keepalive keeps a pinging socket", pinging.ws.readyState === WebSocket.OPEN);
    pinging.ws.close(1000, "bye");
  }
}

await new Promise((resolve) => sfuServer.listen(SFU_PORT, resolve));
try {
  await main();
} catch (error) {
  check("spike ran to completion", false, error.message);
}
sfuServer.close();
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${failed === 0 ? "PASS" : "FAIL"}: ${results.length - failed}/${results.length} checks passed`);
process.exit(failed === 0 ? 0 : 1);
