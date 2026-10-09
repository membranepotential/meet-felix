// Room Durable Object running meeting-core, driven by WebSocket clients on `wrangler dev`.
// Starts a mock Realtime SFU on SPIKE_SFU_PORT (default 8788), so the run needs no network.
//
// Start the Worker first (from the repo root):
//   pnpm exec wrangler dev --port 8797 --var ROOM_JURISDICTION: --var CREATE_SECRET:spike-secret \
//     --var CALLS_APP_ID:spike --var CALLS_APP_SECRET:spike-sfu \
//     --var SFU_API_URL:http://localhost:8788/v1
// Then: node crates/worker/spike/ws-clients.mjs [base-url]   (default http://localhost:8787,
// so pass http://localhost:8797 for the command above)
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

// Mock SFU: just enough of the HTTPS API for the room's calls, a switch to fail them all, and a
// delay for `tracks/close` that holds the room inside an SFU call. Like the live SFU when the
// publisher is not connected yet (docs/spikes/02-first-call.md), the first pull of each track
// fails with `not_found_track_error` and still returns an offer; later pulls succeed.
const sfu = { down: false, closeDelayMs: 0, calls: [], nextMid: 0, pulled: new Set() };
const pull = ({ sessionId, trackName }) => {
  const key = `${sessionId}/${trackName}`;
  if (sfu.pulled.has(key)) return { sessionId, trackName, mid: String(sfu.nextMid++) };
  sfu.pulled.add(key);
  return {
    sessionId,
    trackName,
    errorCode: "not_found_track_error",
    errorDescription:
      "Track not found on remote peer. Make sure the publisher peer is connected and sending packets for this track",
  };
};
const sfuServer = createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  const json = body ? JSON.parse(body) : null;
  const path = new URL(req.url, "http://sfu").pathname;
  const [, session, rest] = path.match(/^\/v1\/apps\/spike\/sessions\/(?:new|([0-9a-f]+)\/(.+))$/) ?? [];
  const op = session ? rest : "new";
  sfu.calls.push({ method: req.method, op, session, json });
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
            tracks: json.tracks.map(pull),
            sessionDescription: { type: "offer", sdp: "mock-offer" },
          });
    case "PUT renegotiate":
      return reply(200, {});
    case "PUT tracks/close":
      await sleep(sfu.closeDelayMs);
      return reply(200, { requiresImmediateRenegotiation: false, tracks: json.tracks });
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
  take(type, predicate = () => true, ms = TIMEOUT_MS) {
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
      ms,
    );
  }

  /** Whether a message of `type` arrives within `ms`, as a boolean. */
  gets(type, predicate, ms) {
    return this.take(type, predicate, ms).then(
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

const listMeetings = (secret = SECRET) =>
  fetch(`${BASE}/api/meetings`, { headers: secret ? { authorization: `Bearer ${secret}` } : {} });
/** The newest entry for `meeting` in `GET /api/meetings` once `done(entry)`, or the last seen. */
const listedMeeting = async (meeting, done) => {
  let entry;
  for (const deadline = Date.now() + TIMEOUT_MS; Date.now() < deadline; await sleep(100)) {
    const { meetings } = await (await listMeetings()).json();
    entry = meetings.find((m) => m.meeting === meeting);
    if (entry && done(entry)) break;
  }
  return entry;
};

const join = (client, fields) => client.send({ type: "join", consent: true, ...fields });
const present = (state, id) => state.participants.some((p) => p.id === id);
/** Resolves with the first mock SFU call matching `predicate`, or null after TIMEOUT_MS. */
const sfuCall = async (predicate) => {
  for (const deadline = Date.now() + TIMEOUT_MS; Date.now() < deadline; await sleep(50)) {
    const call = sfu.calls.find(predicate);
    if (call) return call;
  }
  return null;
};
let answers = 0;
/**
 * Answers every SubscribeOffer `client` gets until one carries a track of `owner`, and waits
 * until the SFU has that answer. Resolves with that offer, the offers without that track
 * before it, and the client's subscribe session; `offer` is null if none came in time.
 */
const receive = async (client, owner, ms = 15000) => {
  const deadline = Date.now() + ms;
  const before = [];
  for (;;) {
    const offer = await client.take("subscribeOffer", undefined, Math.max(deadline - Date.now(), 1)).catch(() => null);
    if (offer === null) return { offer, before, session: undefined };
    const sdp = `client-answer-${answers++}`;
    client.send({ type: "subscribeAnswer", sdp });
    const call = await sfuCall((c) => c.op === "renegotiate" && c.json.sessionDescription.sdp === sdp);
    await sleep(200); // the SFU's reply reaches the room after the mock logged the call
    if (offer.tracks.some((t) => t.track.owner === owner)) return { offer, before, session: call?.session };
    before.push(offer);
  }
};
const closesMid = (session, mid) => (c) =>
  c.op === "tracks/close" && c.session === session && c.json.tracks.some((t) => t.mid === mid);
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
  // The mock fails the first pull with not_found plus an offer; the room passes that offer on
  // without tracks, and the retry after SUBSCRIBE_RETRY_MS delivers the track.
  const { offer, before: emptyOffers, session: guestSubscribe } = await receive(guest, hostId);
  check(
    "a pull whose track is not found still reaches the guest as an empty SubscribeOffer",
    emptyOffers.length === 1 && emptyOffers[0].tracks.length === 0 && emptyOffers[0].sdp === "mock-offer",
    JSON.stringify(emptyOffers.map((o) => o.tracks)),
  );
  check(
    "after the retry the guest gets a SubscribeOffer for the host's audio",
    offer?.tracks?.[0]?.track?.owner === hostId && offer.tracks[0].track.kind === "audio",
    JSON.stringify(offer?.tracks),
  );
  check("the guest's answers reach the SFU's renegotiate", guestSubscribe !== undefined);

  // Unpublish: the room closes the track on the host's publish session and on the guest's
  // subscribe session
  const hostPublish = sfu.calls.find((c) => c.op === "tracks/new" && c.json.tracks[0].location === "local")?.session;
  host.send({ type: "unpublish", kind: "audio" });
  check("Unpublish closes the track on the publish session", (await sfuCall(closesMid(hostPublish, "0"))) !== null);
  check(
    "…and on the subscriber's session",
    (await sfuCall(closesMid(guestSubscribe, offer?.tracks?.[0]?.mid))) !== null,
  );

  // Idle: the room may be evicted; state comes back from storage and attachments
  console.log(`      idle ${IDLE_MS} ms ...`);
  await sleep(IDLE_MS);

  // Leave: a client that closes its socket is gone for the others
  const leaver = await Client.open(meeting);
  join(leaver, { name: "Leaver" });
  const leaverId = (await leaver.take("welcome").catch(() => null))?.you;
  check(
    "host sees the leaver join",
    await host.gets("state", (m) => present(m.state, leaverId)),
    `id=${leaverId}`,
  );
  leaver.ws.close(1000, "bye");
  check("a client-initiated close reaches the others as State", await host.gets("state", (m) => !present(m.state, leaverId)));

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

  // The third client publishes and the others subscribe, so the kick closes tracks on the SFU;
  // the mock holds that call while the kicked client tries to join again on the same socket.
  third.send({ type: "publishOffer", sdp: "third-offer", tracks: [{ mid: "0", kind: "audio", name: "third-mic" }] });
  check("the third client publishes", await third.gets("publishAnswer"));
  const answered = await Promise.all([host, guest].map(async (client) => (await receive(client, knock?.who)).offer !== null));
  check("host and guest receive the third client's track", answered.every(Boolean), answered.join(","));
  sfu.closeDelayMs = 1000;
  third.ws.addEventListener("message", (event) => {
    if (JSON.parse(event.data).type === "kicked") join(third, { name: "Third" });
  });
  const closesBefore = sfu.calls.filter((c) => c.op === "tracks/close").length;
  host.send({ type: "kick", who: knock?.who });
  check("kicked client gets Kicked", await third.gets("kicked"));
  const kickClose = await withTimeout(third.closed, "kick close").catch(() => null);
  check("kicked client's socket closes", kickClose !== null, `code ${kickClose?.code}`);
  const rejoined = await Promise.all([
    host.gets("knock", (m) => m.who === knock?.who, 2000),
    third.gets("waiting", undefined, 2000),
  ]);
  sfu.closeDelayMs = 0;
  check(
    "the kick closed tracks on the SFU",
    sfu.calls.filter((c) => c.op === "tracks/close").length > closesBefore,
  );
  check("a Join right after the kick does not bring the kicked client back", !rejoined.some(Boolean), rejoined.join(","));
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
  const ended = await Promise.all([host, resumed].map((c) => c.take("ended").catch(() => null)));
  check("End reaches everyone", ended.every(Boolean), ended.map(Boolean).join(","));
  const transcript = ended[0]?.transcript;
  check("End of a recorded meeting carries the transcript token", /^[0-9a-f]{32}$/.test(transcript ?? ""), transcript);
  const closes = await Promise.all([host.closed, resumed.closed].map((p) => withTimeout(p, "end close").catch(() => null)));
  check("every socket closes after End", closes.every((c) => c !== null), closes.map((c) => c?.code).join(","));
  const after = await upgradeStatus(meeting);
  check("the next socket is refused", after === 404 || after === 410, `status ${after}`);
  const gone = await fetch(`${BASE}/api/meeting/${meeting}`);
  check("the words are released", gone.status === 404, `status ${gone.status}`);

  // Past meetings: owner-only, and the ended meeting is listed with its transcript token
  check("GET /api/meetings without a secret -> 401", (await listMeetings("")).status === 401);
  check("GET /api/meetings with a wrong secret -> 401", (await listMeetings("wrong")).status === 401);
  const listing = await listMeetings();
  const { meetings } = await listing.json();
  check(
    "GET /api/meetings with the secret -> newest first",
    listing.status === 200 && meetings.every((m, i) => i === 0 || meetings[i - 1].createdAt >= m.createdAt),
    `status ${listing.status}, ${meetings.length} meetings`,
  );
  const past = await listedMeeting(meeting, (m) => m.ended);
  check(
    "GET /api/meetings lists the meeting as ended with its transcript token",
    past?.ended === true && past.transcript === transcript && typeof past.createdAt === "number",
    JSON.stringify(past),
  );

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
