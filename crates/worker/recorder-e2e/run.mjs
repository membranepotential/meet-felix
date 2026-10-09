// End-to-end test of the recorder (phase 3) against `wrangler dev`, with a mock SFU whose
// WebSocket adapters really connect to the Worker and stream audio, as Cloudflare's would.
//
// What it checks: a recorded meeting creates an adapter whose endpoint is the recorder's
// `/rec/...` URL; the recorder accepts that stream and refuses a wrong secret; chunks are cut,
// uploaded to the local R2 and counted; ending the meeting closes the adapter; the transcript job
// reaches `ready` (Workers AI runs remotely even in `wrangler dev`, and costs a few cents) with
// at least one paragraph if the audio is speech; the API's privacy headers; the downloads; and
// the `/t/*` headers from web/static/_headers.
//
// Start the Worker first (from the repo root, with ~/.cargo/bin on PATH for worker-build, and
// web/build built by `pnpm --filter web build` so the assets include _headers):
//   pnpm exec wrangler dev --port 8803 --var ROOM_JURISDICTION: --var CREATE_SECRET:e2e-secret \
//     --var CALLS_APP_ID:e2e --var CALLS_APP_SECRET:e2e-sfu \
//     --var SFU_API_URL:http://localhost:8790/v1 --var RECORDER_ORIGIN:http://localhost:8803
// Then: node crates/worker/recorder-e2e/run.mjs [base-url]   (default http://localhost:8803)
//
// Audio: with `espeak-ng` and `ffmpeg` installed, the mock speaks German and English sentences;
// otherwise it streams a 440 Hz tone, which Whisper should leave without text. Set
// RECORDER_E2E_SECONDS (default 34) for the length; over 30 s exercises the hard chunk cut.
// RECORDER_E2E_SFU_PORT (default 8790) moves the mock SFU. Needs Node >= 26.

import { createServer } from "node:http";
import { randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const BASE = process.argv[2] ?? "http://localhost:8803";
const WS_BASE = BASE.replace(/^http/, "ws");
const SECRET = process.env.RECORDER_E2E_CREATE_SECRET ?? "e2e-secret";
const SFU_PORT = Number(process.env.RECORDER_E2E_SFU_PORT ?? 8790);
const SFU_SECRET = "e2e-sfu";
const SECONDS = Number(process.env.RECORDER_E2E_SECONDS ?? 34);
const JOB_TIMEOUT_MS = Number(process.env.RECORDER_E2E_JOB_TIMEOUT_MS ?? 180_000);

const results = [];
const check = (name, ok, detail = "") => {
  results.push({ name, ok });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
};
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const until = async (what, predicate, ms = 10_000, every = 200) => {
  for (const deadline = Date.now() + ms; Date.now() < deadline; await sleep(every)) {
    const value = await predicate();
    if (value) return value;
  }
  throw new Error(`timeout: ${what}`);
};

// --- Audio: 48 kHz stereo s16le, as the adapter delivers it ------------------------------------

const FRAME_BYTES = 960 * 4; // 20 ms of 48 kHz stereo 16-bit

function speech() {
  try {
    const dir = mkdtempSync(join(tmpdir(), "recorder-e2e-"));
    const lines = [
      ["de", "Guten Tag zusammen. Wir besprechen heute den Plan für die nächste Woche."],
      ["en", "Thanks everyone. Let us start with the budget and then talk about the release."],
    ];
    const parts = lines.map(([voice, text], i) => {
      const wav = join(dir, `${i}.wav`);
      execFileSync("espeak-ng", ["-v", voice, "-s", "150", "-w", wav, text]);
      return execFileSync("ffmpeg", ["-v", "error", "-i", wav, "-ar", "48000", "-ac", "2", "-f", "s16le", "-"], {
        maxBuffer: 64 << 20,
      });
    });
    const pause = Buffer.alloc(48_000 * 4); // one second of silence between sentences
    return { kind: "speech", pcm: Buffer.concat(parts.flatMap((p) => [p, pause])) };
  } catch {
    return null;
  }
}

function tone() {
  const pcm = Buffer.alloc(48_000 * 4);
  for (let i = 0; i < 48_000; i++) {
    const s = Math.round(6000 * Math.sin((2 * Math.PI * 440 * i) / 48_000));
    pcm.writeInt16LE(s, i * 4);
    pcm.writeInt16LE(s, i * 4 + 2);
  }
  return { kind: "tone", pcm };
}

const AUDIO = speech() ?? tone();

/** Protobuf `Packet { uint32 sequenceNumber = 1; uint32 timestamp = 2; bytes payload = 5; }`. */
function packet(sequenceNumber, timestamp, payload) {
  const out = [];
  const varint = (v) => {
    let n = BigInt(v);
    do {
      let byte = Number(n & 0x7fn);
      n >>= 7n;
      if (n > 0n) byte |= 0x80;
      out.push(byte);
    } while (n > 0n);
  };
  varint(1 << 3);
  varint(sequenceNumber);
  varint(2 << 3);
  varint(timestamp);
  varint((5 << 3) | 2);
  varint(payload.length);
  return Buffer.concat([Buffer.from(out), payload]);
}

// --- Mock SFU: sessions, tracks, and WebSocket adapters that stream ----------------------------

const sfu = { calls: [], adapters: new Map(), opened: 0, sent: 0 };

/** Connects to the adapter's endpoint and streams the audio in 20 ms packets until closed. */
function startAdapter(adapterId, endpoint) {
  const ws = new WebSocket(endpoint);
  ws.binaryType = "arraybuffer";
  const adapter = { ws, endpoint, timer: undefined, closed: false };
  sfu.adapters.set(adapterId, adapter);
  ws.addEventListener("open", () => {
    sfu.opened++;
    let seq = 1000;
    let offset = 0;
    adapter.timer = setInterval(() => {
      if (ws.readyState !== WebSocket.OPEN) return;
      let payload = AUDIO.pcm.subarray(offset, offset + FRAME_BYTES);
      if (payload.length < FRAME_BYTES) {
        offset = 0;
        payload = AUDIO.pcm.subarray(0, FRAME_BYTES);
      }
      offset += FRAME_BYTES;
      ws.send(packet(seq, seq * 960, payload));
      seq++;
      sfu.sent++;
    }, 20);
  });
  ws.addEventListener("close", () => {
    adapter.closed = true;
    clearInterval(adapter.timer);
  });
  ws.addEventListener("error", () => {});
}

const sfuServer = createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  const json = body ? JSON.parse(body) : null;
  const path = new URL(req.url, "http://sfu").pathname.replace(/^\/v1\/apps\/e2e\//, "");
  sfu.calls.push({ method: req.method, path, json });
  const reply = (status, value) => {
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(value));
  };
  if (req.headers.authorization !== `Bearer ${SFU_SECRET}`) {
    return reply(401, { errorCode: "unauthorized", errorDescription: "bad app secret" });
  }
  if (path === "sessions/new") return reply(201, { sessionId: randomBytes(16).toString("hex") });
  if (path === "adapters/websocket/new") {
    const tracks = json.tracks.map((t) => {
      const adapterId = randomBytes(8).toString("hex");
      startAdapter(adapterId, t.endpoint);
      return { trackName: t.trackName, adapterId, endpoint: t.endpoint };
    });
    return reply(200, { tracks });
  }
  if (path === "adapters/websocket/close") {
    for (const { adapterId } of json.tracks) sfu.adapters.get(adapterId)?.ws.close();
    return reply(200, { tracks: json.tracks.map(({ adapterId }) => ({ adapterId })) });
  }
  const op = path.replace(/^sessions\/[0-9a-f]+\//, "");
  switch (`${req.method} ${op}`) {
    case "POST tracks/new":
      return json.tracks[0].location === "local"
        ? reply(200, {
            tracks: json.tracks.map(({ mid, trackName }) => ({ mid, trackName })),
            sessionDescription: { type: "answer", sdp: "mock-answer" },
          })
        : reply(200, {
            requiresImmediateRenegotiation: true,
            tracks: json.tracks.map(({ sessionId, trackName }, i) => ({ sessionId, trackName, mid: String(i) })),
            sessionDescription: { type: "offer", sdp: "mock-offer" },
          });
    case "PUT renegotiate":
      return reply(200, {});
    case "PUT tracks/close":
    case "PUT tracks/update":
      return reply(200, { tracks: json.tracks });
    default:
      return reply(404, { errorCode: "not_found", errorDescription: `${req.method} ${path}` });
  }
});

// --- The meeting -------------------------------------------------------------------------------

/** A signalling client that keeps every server message until the test takes it. */
async function openClient(meeting, headers) {
  const ws = new WebSocket(`${WS_BASE}/ws/${meeting}`, { headers });
  const inbox = [];
  ws.addEventListener("message", (event) => inbox.push(JSON.parse(event.data)));
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", () => reject(new Error("cannot open the room socket")), { once: true });
  });
  const take = (type, ms = 10_000) =>
    until(`${type} message`, () => {
      const i = inbox.findIndex((m) => m.type === type);
      return i < 0 ? null : inbox.splice(i, 1)[0];
    }, ms, 50);
  return { ws, send: (msg) => ws.send(JSON.stringify(msg)), take };
}

/** The HTTP status of a WebSocket upgrade to `url`: 101 if accepted. */
const upgradeStatus = (url) =>
  new Promise((resolve) => {
    const ws = new WebSocket(url);
    ws.addEventListener("open", () => {
      ws.close();
      resolve(101);
    });
    ws.addEventListener("error", () => resolve("refused"));
  });

const transcriptStatus = async (token) => {
  const response = await fetch(`${BASE}/api/transcript/${token}`);
  return { response, body: response.ok ? await response.json() : null };
};

async function main() {
  await new Promise((resolve) => sfuServer.listen(SFU_PORT, resolve));
  console.log(`mock SFU on :${SFU_PORT}; audio: ${AUDIO.kind}, ${SECONDS} s`);

  const login = await fetch(`${BASE}/api/login`, { method: "POST", body: JSON.stringify({ password: SECRET }) });
  const cookie = login.headers.getSetCookie()[0]?.split(";")[0] ?? "";
  check("owner logs in", login.status === 204);
  const created = await fetch(`${BASE}/api/new`, {
    method: "POST",
    headers: { authorization: `Bearer ${SECRET}` },
    body: JSON.stringify({ recording: "fromStart" }),
  });
  const { meeting } = await created.json();
  check("a meeting that records from the start is created", created.status === 201, meeting);

  const host = await openClient(meeting, { cookie });
  host.send({ type: "join", name: "Felix", consent: true });
  const welcome = await host.take("welcome");
  check("the host joins and recording starts", welcome.state.recording !== undefined);
  host.send({ type: "publishOffer", sdp: "offer", tracks: [{ mid: "0", kind: "audio", name: "mic" }] });
  await host.take("publishAnswer");

  const create = await until("adapter creation", () => sfu.calls.find((c) => c.path === "adapters/websocket/new"));
  const track = create.json.tracks[0];
  const endpoint = new RegExp(`^${WS_BASE}/rec/[0-9a-f]{64}/[0-9a-f]{32}/${welcome.you}/0$`);
  check(
    "the recorder creates an adapter for the host's audio at its /rec endpoint",
    track.location === "remote" && track.outputCodec === "pcm" && track.trackName === "mic" && endpoint.test(track.endpoint),
    track.endpoint.replace(/\/[0-9a-f]{32}\//, "/<secret>/"),
  );
  await until("the adapter's socket to open", () => sfu.opened > 0);
  check("the recorder accepts the adapter's WebSocket", sfu.opened === 1);
  const wrongSecret = track.endpoint.replace(/\/[0-9a-f]{32}\//, `/${"0".repeat(32)}/`);
  check("a wrong secret is refused", (await upgradeStatus(wrongSecret)) === "refused");
  const wrongGeneration = track.endpoint.replace(/\/0$/, "/7");
  check("an unknown adapter generation is refused", (await upgradeStatus(wrongGeneration)) === "refused");

  await sleep(SECONDS * 1000);
  check("the mock streamed 20 ms packets", sfu.sent > SECONDS * 40, `${sfu.sent} packets`);

  host.send({ type: "end" });
  const ended = await host.take("ended");
  const token = ended.transcript;
  check("the end message carries the transcript token", /^[0-9a-f]{32}$/.test(token ?? ""));
  await until("the adapter close", () => sfu.calls.find((c) => c.path === "adapters/websocket/close"), 10_000);
  check("ending closes the adapter at the SFU", [...sfu.adapters.values()].every((a) => a.closed));

  const first = await transcriptStatus(token);
  const headers = first.response.headers;
  check(
    "the status API sends no-referrer, noindex and no-store",
    headers.get("referrer-policy") === "no-referrer" &&
      headers.get("x-robots-tag") === "noindex" &&
      headers.get("cache-control") === "no-store",
  );
  check(
    "the recorded audio is about as long as the stream (progress in ms)",
    first.body?.progress.total >= (SECONDS - 3) * 1000 && first.body?.progress.total <= (SECONDS + 3) * 1000,
    JSON.stringify(first.body?.progress),
  );
  check("the status names the meeting and its times", first.body?.meeting === meeting && first.body?.endedAt >= first.body?.startedAt);
  const unknown = await fetch(`${BASE}/api/transcript/${"f".repeat(32)}`);
  check("an unknown token is 404 with the same headers", unknown.status === 404 && unknown.headers.get("x-robots-tag") === "noindex");

  let last = first.body;
  try {
    last = await until(
      "the transcript job",
      async () => {
        const { body } = await transcriptStatus(token);
        if (body && JSON.stringify(body.progress) !== JSON.stringify(last?.progress)) {
          console.log(`      status ${body.status}, progress ${body.progress.done}/${body.progress.total}`);
        }
        last = body ?? last;
        return body && (body.status === "ready" || body.status === "failed" || body.retryAt) ? body : null;
      },
      JOB_TIMEOUT_MS,
      2_000,
    );
  } catch (e) {
    console.log(`      ${e.message}; last status ${JSON.stringify(last && { ...last, transcript: undefined })}`);
  }
  check("every chunk was stored in R2 and handled by the job", last?.progress.total > 0 && last.progress.done === last.progress.total, JSON.stringify(last?.progress));
  check("the transcript is ready", last?.status === "ready", last?.status);
  if (last?.status === "ready") {
    const t = last.transcript;
    check("the transcript names the meeting and the speaker", t.meeting === meeting && t.speakers.every((s) => s.label === "Felix (host)"));
    check("the consent record lists the host", t.consents.some((c) => c.name === "Felix" && c.noticeVersion === 1));
    if (AUDIO.kind === "speech") {
      check("the speech came back as at least one paragraph", t.paragraphs.length > 0, t.paragraphs.map((p) => p.text).join(" | ").slice(0, 200));
    }
    const md = await fetch(`${BASE}/api/transcript/${token}/transcript.md`);
    check(
      "transcript.md downloads as markdown named after the meeting",
      md.status === 200 &&
        md.headers.get("content-type")?.startsWith("text/markdown") &&
        md.headers.get("content-disposition") === `attachment; filename="${meeting}.md"` &&
        (await md.text()).startsWith(`# Transcript: ${meeting}`),
    );
    const json = await fetch(`${BASE}/api/transcript/${token}/transcript.json`);
    check("transcript.json downloads", json.status === 200 && (await json.json()).meeting === meeting);
  }

  const page = await fetch(`${BASE}/t/${token}`);
  check(
    "/t/{token} is served with no-referrer and noindex (web/static/_headers)",
    page.status === 200 && page.headers.get("referrer-policy") === "no-referrer" && page.headers.get("x-robots-tag") === "noindex",
    `status ${page.status}, referrer-policy ${page.headers.get("referrer-policy")}`,
  );
}

try {
  await main();
} catch (e) {
  check("the run finished", false, e.message);
} finally {
  for (const adapter of sfu.adapters.values()) adapter.ws.close();
  sfuServer.close();
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} checks passed`);
  process.exit(failed === 0 ? 0 : 1);
}
