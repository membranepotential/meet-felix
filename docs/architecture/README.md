# Architecture

meet-felix replaces Google Meet for small meetings of 2 to 10 people. It runs
entirely on Cloudflare, keeps all meeting logic in one pure Rust crate, and
lets thin shells on the server and in the browser carry out what that crate
decides. This page holds what every feature rests on: the goal and non-goals,
the platform, the split into core and shells, the browser stack, the backend
and data location. It also keeps the component map, the phases and the risks.
The feature decisions live in their own folders, listed in the
[docs index](../README.md#design-decisions).

Decisions on this page:

- [D1: Cloudflare is the only platform (2026-10-09)](#d1-cloudflare-is-the-only-platform-2026-10-09)
- [D2: A pure Rust core with imperative shells (2026-10-09)](#d2-a-pure-rust-core-with-imperative-shells-2026-10-09)
- [D3: The browser runs a TypeScript and Svelte shell over the core as WASM (2026-10-09)](#d3-the-browser-runs-a-typescript-and-svelte-shell-over-the-core-as-wasm-2026-10-09)
- [D4: The backend is Rust on Workers, with Durable Objects that hibernate (2026-10-09)](#d4-the-backend-is-rust-on-workers-with-durable-objects-that-hibernate-2026-10-09)
- [D5: Pin data to the EU where Cloudflare allows it (2026-10-09)](#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)
- [D32: One owner is the only host; everyone else is a guest (2026-10-09)](#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)

Code: [`crates/meeting-core/`](../../crates/meeting-core/),
[`crates/worker/`](../../crates/worker/), [`web/`](../../web/),
[`wrangler.toml`](../../wrangler.toml), [`Cargo.toml`](../../Cargo.toml).
Research: [00 — report](../research/00-report-serverless-rust-video-meetings.md),
[01 — hosting and media platforms](../research/01-hosting-and-media-platforms.md),
[02 — Cloudflare deep dive](../research/02-cloudflare-deep-dive.md),
[03 — self-hosted Fly.io path](../research/03-selfhosted-fly-path.md),
[05 — frontend and browser](../research/05-frontend-and-browser.md).

Date: 2026-10-09. Based on the research in [research/](../research/README.md);
prices and versions were checked on that date. "Assumed" marks the plan's own
numbers; "unverified" marks claims without a primary source.

## Goal

A **personal** meeting app: one owner (Felix) is the only host, everyone
else joins as a guest through a link ([D32](#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)).
It replaces Google Meet for small meetings (2–10 people) and:

- costs nothing while no meeting runs, and less per month than Google
  Workspace Business Standard (13.60 € net, ≈16.18 € gross per user; see
  [cost/](../cost/README.md));
- runs in the browser on PC, tablet and phone, with camera, microphone,
  screen share and short invite links;
- records each meeting with everyone's knowledge and delivers a transcript
  afterwards, in German and English;
- is written in Rust with a functional core and an imperative shell, on one
  hosting platform: Cloudflare.

Terms used in all docs:

- **Meeting**: what people see and join through a link. **Room**: the
  server-side state of one meeting (`RoomState`), held by one `RoomObject`.
- **Core**: the `meeting-core` crate, pure functions without I/O
  ([core/](../core/README.md)).
- **Shell**: code that performs the I/O the core asks for: the Worker and its
  Durable Objects on the server, the Svelte app in the browser.
- **Durable Object (DO)**: a Cloudflare object with a single instance per id
  and its own storage. Each meeting has one `RoomObject` and one
  `RecorderObject`.
- **SFU** (selective forwarding unit): the server that receives each
  participant's media and forwards it to the others. Here: Cloudflare
  Realtime SFU ([media/](../media/README.md)).
- **Adapter**: an SFU WebSocket adapter, which streams one SFU audio track as
  PCM to a WebSocket endpoint ([recording/](../recording/README.md)).
- **Owner**: the person who runs this deployment and creates meetings.
  **Host**: a participant whose socket carries the owner's session cookie,
  which the owner's password sets (`POST /api/login`); hosts control the
  meeting. **Guest**: everyone else.

## Non-goals

- **Not a product for other hosts.** There are no accounts, no sign-up, no
  teams and no second host. Anyone who wants their own instance deploys
  their own copy ([D32](#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)).
- **No live captions.** The transcript arrives after the meeting. The
  research on live speech-to-text ([07](../research/07-live-transcription.md))
  informed the post-meeting design but is not built.
- **No video recording.** The SFU has no recorder, and the transcript needs
  only audio.
- **No fallback hosting.** The str0m SFU on Fly.io from the research
  ([03](../research/03-selfhosted-fly-path.md)) was dropped. The core stays
  portable ([D2](#d2-a-pure-rust-core-with-imperative-shells-2026-10-09)), so a
  move would replace only the shell.
- **No screen sharing from phones and tablets.** No phone or tablet browser
  supports `getDisplayMedia`. This is a browser limit, not a choice
  ([D13](../media/README.md#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09)).
- **No knock queue by default.** Rooms are open to anyone with the link; the
  host can lock the room, and late joiners then knock
  ([D6](../links/README.md#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)).

## D32: One owner is the only host; everyone else is a guest (2026-10-09)

meet-felix is a personal meeting app. Exactly one person, the owner, can
create meetings and act as host. Everyone else is a guest: they open a link,
give a name, consent to recording, and join. There is no sign-up, no user
database, no team, no second host, and no "host password" handed to anyone
else.

What follows from this, and is binding for every feature:

- **Authentication is one password, the owner's.** It unlocks the index page
  (new meeting, past meetings) and the "join as host" fallback on a meeting
  page. The Worker checks it against the `CREATE_SECRET` secret and sets a
  signed session cookie for that browser. Nothing else identifies anyone.
- **Guests never authenticate.** A guest's identity is the name they type.
  Guests cannot create, lock, record, admit, mute or end anything.
- **The owner's browsers are the host.** A browser with a valid session joins
  every meeting as host automatically; a browser without one can become the
  host by entering the password. The core learns this from the shell as an
  `owner` flag on each client event, not from a token the client holds.
- **No multi-tenancy, ever.** Do not add accounts, roles beyond host and
  guest, per-user storage, or anything that would make the app usable by
  several hosts. Someone who wants their own meet-felix deploys their own
  copy with their own password and Cloudflare account.
- **Data belongs to the owner.** Past meetings, recordings and transcripts
  are listed only behind the password; guests get exactly the transcript link
  they are shown at the end ([D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09), [D26](../ui/README.md)).

Why: the user said so on 2026-10-09 ("a private meeting app, me as the host
and the only host, nobody else will use it as a host"). Building for one owner
removes whole classes of work (accounts, permissions, billing, abuse) and keeps
the cost model honest ([cost/](../cost/README.md)). It also replaces the
earlier host-token design, where the creating browser held a secret.

## D1: Cloudflare is the only platform (2026-10-09)

Everything runs on Cloudflare: Workers and Durable Objects for signaling and
room state, Realtime SFU and TURN for media, R2 for recordings and
transcripts, KV for the meeting-id lookup, Workers AI for transcription, and
Access for the owner's login. That is one account, one bill and one deploy
tool (`wrangler`).

The media path decides this. A meeting needs a server that receives and
forwards UDP, and no serverless platform hosts that for your own code: Cloud
Run and Railway accept no inbound UDP, and workers-rs runs without Tokio, so
a Rust SFU cannot run as a Worker
([01 §3, §5](../research/01-hosting-and-media-platforms.md)). Cloudflare
Realtime SFU is the only researched media path that idles at $0 without our
code in it. It bills $0.05/GB delivered after 1,000 free GB per month, a pool
shared by SFU, TURN and adapters. Its control API is plain HTTPS with a bearer
secret, so a Rust DO drives it with `worker::Fetch` and needs no SDK. Rust
therefore stays out of the media path and owns what benefits from types and
pure functions: room state, admission, negotiation and transcription control.

Rejected:

- A self-run str0m SFU on Fly.io Machines. It needs a $2/month dedicated
  IPv4, and we would build simulcast handling, bandwidth adaptation and TURN
  integration ourselves ([03](../research/03-selfhosted-fly-path.md)).
- Managed platforms (LiveKit Cloud, Cloudflare RealtimeKit, Daily). They cost
  $0.12–0.24 per participant-hour and tie the client to a vendor SDK
  ([00](../research/00-report-serverless-rust-video-meetings.md)).

Consequences: we cannot choose where media is processed
([D5](#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)), and we
build recording ourselves
([D16](../recording/README.md#d16-capture-each-speakers-audio-on-the-server-in-30-second-chunks-2026-10-09)).

## D2: A pure Rust core with imperative shells (2026-10-09)

The meeting logic lives in `crates/meeting-core`, a crate without I/O: it
reads no clock, no randomness and no network. Its centre is one function,
`step(RoomState, Event) -> (RoomState, Vec<Effect>)`. An event is something
that happened: a client message, an SFU reply, a closed socket, an alarm. An
effect is something the shell must do: send a message, call the SFU, set an
alarm, start recording. The shells load the state, call `step`, store the new
state and carry out the effects in order. Because the core reads no clock, the
shell passes the current time in with each event. Details and the invariants
the tests check: [core/](../core/README.md).

Why:

- `RoomState` is one serializable value, so waking a hibernated DO means
  loading one value from storage.
- `step` is pure, so join, leave, lock, knock, renegotiation and recording
  sequences are tested with property tests and no network.
- The same crate compiles to WASM for the browser, so client and server share
  message types and validation.
- The hosting choice stays reversible: str0m is also sans-IO, so a different
  shell (for example axum and str0m on a VM) could feed the same `step`
  ([00](../research/00-report-serverless-rust-video-meetings.md)).

Recording state is part of `RoomState`, so every client sees a recording start
by construction
([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).

## D3: The browser runs a TypeScript and Svelte shell over the core as WASM (2026-10-09)

`web/` is a SvelteKit static single-page app (Svelte 5 runes, strict
TypeScript). It owns the DOM, `RTCPeerConnection` and the media devices. It
calls `meeting-core`, compiled to WASM, for protocol types, validation and
decisions such as client session state, layout and the simulcast layer to
request. The `wasm` feature of `meeting-core` derives TypeScript declarations
with `tsify` for every type that crosses the WASM boundary, so TypeScript never
restates a Rust type.

Why: every Rust UI framework reaches WebRTC through the same web-sys
bindings. web-sys gates `scalabilityMode`, Wake Lock and
`MediaStreamTrackProcessor` behind `--cfg=web_sys_unstable_apis`, and it seems
to lack `MediaStreamTrack.contentHint`, the setting that keeps screen-share
text sharp ([05 §2](../research/05-frontend-and-browser.md)). TypeScript has
full typings for all of them. With this split the language boundary sits at
the core boundary: Rust decides, TypeScript performs the I/O. The cost is two
toolchains (Cargo and pnpm).

Rejected: an all-Rust UI in Leptos 0.8
([05 §1](../research/05-frontend-and-browser.md)).

## D4: The backend is Rust on Workers, with Durable Objects that hibernate (2026-10-09)

`crates/worker` uses the `worker` crate (0.8.x). It routes requests and holds
two Durable Object classes, one instance of each per meeting: `RoomObject`
(signaling and room state) and `RecorderObject` (recording and the transcript
job, [recording/](../recording/README.md)). The `RoomObject` accepts client
WebSockets through the WebSocket Hibernation API and keeps its state in its
SQLite storage. The runtime can then evict it from memory between messages
while the sockets stay open.

Why: idle DOs that hibernate are not billed for duration, while a socket
accepted without the Hibernation API bills duration for the whole connection.
Each incoming WebSocket message counts as 1/20 of a request, and outgoing
messages are free ([01 §2](../research/01-hosting-and-media-platforms.md)).
workers-rs exposes the Hibernation API: `State::accept_web_socket`, tagged
sockets, `set_websocket_auto_response`, and the handlers `websocket_message`,
`websocket_close`, `websocket_error` and `alarm`
([02 §1](../research/02-cloudflare-deep-dive.md)). After hibernation the
runtime rebuilds the object, so per-socket data must fit in a socket
attachment of at most 16,384 bytes or live in storage.

Open points, checked by spike 1
([spikes/01-do-hibernation.md](../spikes/01-do-hibernation.md)):

- Whether Rust can call `serialize_attachment` (unverified).
- How the open workers-rs PR for eager memory release on hibernation
  ([cloudflare/workers-rs#832](https://github.com/cloudflare/workers-rs/pull/832))
  affects memory under load.
- Whether the Realtime SFU and the Workers AI speech models work on the
  Workers Free plan (unverified). Until then the plan assumes the Workers
  Paid plan ([cost/](../cost/README.md)).

## D5: Pin data to the EU where Cloudflare allows it (2026-10-09)

New Durable Objects are created with `jurisdiction("eu")`, and recordings go
to an EU R2 bucket. The SFU, TURN and Workers AI document no regional
guarantee, so media and the audio sent for transcription may be processed
outside the EU. EU location is a nice-to-have, not a hard requirement. If it
became one, media would have to move off Cloudflare
([03](../research/03-selfhosted-fly-path.md)).

Two mechanics follow:

- A jurisdiction applies only to DO ids from `unique_id()`, not to ids from
  `id_from_name` ([02 §5](../research/02-cloudflare-deep-dive.md)). A meeting
  link therefore cannot derive the DO id from its words; KV maps the words to
  the id ([links/](../links/README.md)).
- The Worker reads the jurisdiction from the `ROOM_JURISDICTION` variable in
  [`wrangler.toml`](../../wrangler.toml). Local `wrangler dev` implements no
  jurisdictions, so run it with `--var ROOM_JURISDICTION:` to create rooms
  without one.

## Component map

```
Browser (Svelte shell + meeting-core.wasm)
   │ WebSocket (signaling)                       │ WebRTC publish + subscribe sessions
   ▼                                             ▼
Worker router ──► Room DO (Rust, hibernating sockets, RoomState in SQLite)
  /api/login (cookie)  │ HTTPS rtc.live.cloudflare.com/v1  (sessions, tracks, renegotiate, adapters)
  /{a-b-c}             ▼
  /t/{token}     Cloudflare Realtime SFU ◄── Cloudflare TURN
                       │ one WebSocket adapter per audio track: 48 kHz stereo s16le PCM
                       ▼
                 Recorder DO (Rust, per meeting): downmix + resample 48 → 16 kHz mono (rubato),
                 energy-gated 30 s chunks cut at silence ──► R2  rec/{meeting}/{speaker}/{n}.wav
                       │ alarm loop after the meeting ends
                       ▼
                 Transcriber (same DO): Workers AI Whisper large-v3-turbo per chunk (`language`)
                 ──► merge segments by wall-clock ──► transcript.md + transcript.json in R2
                 ──► stitch chunks into one WAV per speaker (optional mixed WAV)
```

In the diagram, "Room DO" is the `RoomObject` and "Recorder DO" is the
`RecorderObject`. The routes are the planned page routes; the router in
[`crates/worker/src/lib.rs`](../../crates/worker/src/lib.rs) is the source of
truth for what exists today.

| Component | Where | Role |
|---|---|---|
| `meeting-core` | [`crates/meeting-core`](../../crates/meeting-core/) | Pure Rust library for `wasm32` and native: `RoomState`, participants, tracks, roles, `Recording`, `ClientMsg`/`ServerMsg` (serde + tsify), `Event`, `Effect`, `step()`. Planned: simulcast layer policy, the state machine that cuts chunks and gates silence, transcript merge. |
| `sfu-api` | `crates/sfu-api` (not started) | Typed client for the SFU HTTPS API and the adapter protobuf (`prost`). |
| `worker` | [`crates/worker`](../../crates/worker/) | Router (owner session cookie or bearer on `/api/login`, `/api/new`, `/api/meetings`; words → DO lookup; static assets); `RoomObject`; `RecorderObject` (adapter ingress, resampling, one R2 `put` per chunk, transcript job in the alarm loop, R2 multipart stitch); `Transcriber` trait with `WhisperTurbo` and `Nova3Http` implementations through `worker::Ai`. |
| `web` | [`web/`](../../web/) | SvelteKit static site: new-meeting page, pre-join with consent, meeting grid, device picker, end-of-meeting page, transcript page. Loads the core as WASM built with `wasm-pack`. |
| Realtime SFU | `rtc.live.cloudflare.com/v1` | Forwards media; sessions, tracks, simulcast, adapters ([media/](../media/README.md)). |
| TURN | Cloudflare TURN | Relays media for clients behind strict firewalls. |
| KV | binding `MEETINGS` | Meeting words → `RoomObject` id, with a TTL ([links/](../links/README.md)). |
| R2 | EU bucket | Audio chunks, transcripts, stitched per-speaker WAVs ([recording/](../recording/README.md)). |
| Workers AI | `worker::Ai` | Whisper large-v3-turbo; Nova-3 behind the same trait. |
| Access | Cloudflare Zero Trust | Not used today: the owner logs in with one password (D7, D32). Could protect the owner routes later once a domain is attached. |

## Phases

| Phase | Scope | Done when |
|---|---|---|
| **0. Spikes** | 1. Rust DO with 10 hibernating sockets; try the Free plan for SFU + Workers AI. 2. Adapter → `RecorderObject` → R2 with 10 synthetic tracks: CPU, memory, loss on reconnect. 3. Whisper turbo vs Nova-3 HTTP on German and mixed German/English audio; confirm chunk size limits. 4. Real iPhone/iPad: simulcast, lock screen, app switch, phone call. 5. Measure egress for a real 4-person hour incl. adapters. | Each spike has a written result in [`docs/spikes/`](../spikes/); [cost/](../cost/README.md) updated with measured numbers. |
| **1. Walking skeleton** | Cargo workspace + pnpm `web/`, `meeting-core` with `step`, `RoomObject`, word links with KV, owner password on `/api/new`, 2-person audio+video through the SFU, deploy with `wrangler`. | Two browsers on different networks hold a 10-minute call; the DO hibernates between messages; CI runs the core property tests. |
| **2. Multi-party** | Simulcast + per-tile layer selection, active-speaker detection, desktop screen share with `contentHint: "detail"`, room lock/knock, host controls (mute, kick, end), ICE restart and network-switch reconnect, responsive grid. | 8 participants incl. one phone and one shared screen for 30 min without manual intervention. |
| **3. Recording + transcript** | `RecorderObject`, consent flow and badge, chunking, R2 layout, transcription job, transcript page (`/t/{token}`), stitched per-speaker WAVs. | A recorded 4-person meeting yields a correctly attributed German/English transcript within 15 minutes of ending; every participant saw the notice. |
| **4. Mobile polish** | iOS background handling, Wake Lock, PWA manifest, speaker selection, Picture-in-Picture. | Checklist run on iPhone, iPad, Android, desktop Chrome/Firefox/Safari. |
| **5. Operations and extras** | Cloudflare usage dashboard, monthly budget alert, join rate limits, retention cleanup ([D18](../recording/README.md#d18-keep-recordings-and-transcripts-indefinitely-for-now-2026-10-09)), docs; transcript by email ([D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)); cancel Workspace. | A month of real use stays under 10 €. |

Phases 1–3 are the bulk of the work; phase 3 is mostly a Rust port of
Cloudflare's `ai-tts-stt` example plus R2. Plan v2 had a retention cleanup by
alarm in phase 3; the retention decision of 2026-10-09 (D18) moved it later.
Where the work stands today: [STATUS.md](../../STATUS.md).

## Risks

| Risk | Impact | Mitigation |
|---|---|---|
| workers-rs rough edges (hibernation memory PR open, `serialize_attachment` unverified) | Backend bugs or idle billing | Spike 1; keep per-socket data ≤ 16 KB |
| Whisper turbo German quality, or chunks cut mid-word | Poor transcript | Spike 3; cut chunks at silence; Nova-3 behind the `Transcriber` trait |
| Adapter drops audio during its 15 s retry window | Gaps in the recording | The recorder tracks gaps and marks them in the transcript |
| Assumed bitrates wrong by 2× | Usage leaves the free pool sooner | Spike 5; cap the top simulcast layer above six people |
| iOS behaviour differs from old WebKit bug reports | Dropped media on phones | Spike 4; defensive reacquire logic ([D14](../media/README.md#d14-calls-work-in-a-plain-browser-tab-and-ios-gets-defensive-handling-2026-10-09)) |
| Three-word links guessed | Uninvited joiner | Ephemeral rooms, rate limits, room lock; the host sees every join |
