# Testing

meet-felix is tested in layers that follow the code layers
([D2](architecture/README.md#d2-a-pure-rust-core-with-imperative-shells-2026-10-09)):
the pure core carries most of the proof, the shells get smaller checks, and
real devices and the deployed Worker close the gaps that no local test can
reach. Each layer below says what exists on 2026-10-09 and what is planned.

| Layer | What it proves | Command | State on 2026-10-09 |
|---|---|---|---|
| Core unit tests | Single rules of `step`, whole meetings, ids, the word list | `cargo test -p meeting-core` | Exist; growing with the core |
| Core property tests | The invariants hold for any event sequence | `cargo test -p meeting-core` | Exist (`proptest`, 512 cases of up to 200 actions) |
| WASM build of the core | The core compiles for the browser | `cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm` | Exists |
| SFU client tests | Request and response JSON against samples from Cloudflare's docs and a live SFU response (`not_found_track_error` with an offer); core SFU effects to replies, with the negotiation id carried through and a publish answer's per-track errors turned into rejected kinds | `cargo test -p sfu-api` | Exist |
| Worker unit tests | Storage record, socket attachment with the owner flag, alarm (a fired alarm is forgotten unless the core sets it again) and keepalive rules; the ended room's alarm (a pending `/end` is sent again before the storage is deleted); the owner's session cookie (valid, expired, tampered, foreign, missing) and password; `/api/new` and `/api/login` input checks | `cargo test -p meet-worker` | Exist |
| E2E protocol tests | The real `RoomObject` running the core under `wrangler dev`, driven over raw WebSockets, with a mock SFU: owner session, HTTP routes, the meeting lifecycle, consent and recording state, negotiation, host controls, SFU failures and session recovery, the meetings list | `pnpm test:e2e` (project `protocol`) | Exist (Playwright Test, `web/e2e/protocol/`) |
| E2E browser tests | The UI flows in Chromium and Firefox with fake devices against the same local stack: login, pre-join, lobby, tiles, controls, screen share, leave and end, past meetings, recovery | `pnpm test:e2e` (projects `chromium`, `firefox`) | Exist (`web/e2e/browser/`) |
| E2E recorder test | A recorded meeting through the `RecorderObject`: the mock SFU's adapter streams 40 s of real speech into it, the chunks reach the local R2, the real Whisper on Workers AI returns the snippets' keywords, with speaker label, consent record and no gaps | `pnpm test:e2e:recorder` (project `recorder`) | Exists (`web/e2e/protocol/recorder.spec.ts`, tag `@recorder`); needs the network and `wrangler login` |
| E2E real-SFU tests | A three-party call through the real Realtime SFU: media both ways, screen share, a layer change that holds for 25 s, 60 s without reconnects; a five-party call in which everyone decodes everyone else's video for 30 s | `pnpm test:e2e:real` | Exist; need the `CALLS_*` secrets and network |
| Web checks | Types and build of the Svelte shell | `pnpm --filter web check`, `pnpm --filter web build` | Exist |
| Device checklist | Real phones, tablets and desktop browsers | manual, phase 4 | Planned |
| Deployed Worker checks | Hibernation and billing on Cloudflare itself | manual, dashboard | Planned |

Run everything that exists before a commit:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo clippy -p meet-worker -p sfu-api --all-targets --target wasm32-unknown-unknown -- -D warnings
cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm
pnpm --filter web check
pnpm --filter web lint
pnpm --filter web test
pnpm test:e2e
```

`pnpm test:e2e` builds the web app and the Worker itself, so a separate
`pnpm --filter web build` is not needed. It stays offline; after a change to
the recorder, the transcript job or the adapter calls, also run
`pnpm test:e2e:recorder` (about 1 min, needs the network and `wrangler login`,
and costs about 30–50 Workers AI neurons of the 10,000 free per day: 46.63 per
audio minute for `whisper-large-v3-turbo`, see research 11).

## Core: unit and property tests

The core is pure, so its tests need no network, no Cloudflare and no browser.
Tests live in [`crates/meeting-core/src/tests/`](../crates/meeting-core/src/tests/):
`flow.rs` (whole meetings step by step), `rules.rs` (single rules such as who
may lock or record), `props.rs` (property tests), `words.rs` (the word list)
and `support.rs` (helpers that build events and run them through `step`).

The property tests generate random sequences of join, leave, lock, knock,
renegotiation and recording events and check the invariants listed in
[core/](core/README.md#invariants-to-test) after every step: no orphaned SFU
tracks, one offerer per session, no recording without a broadcast, and no
host locked out. They also check that every joined client's last snapshot
equals the current room view, that only consenting participants are
recorded, and that `RoomState` survives a JSON round trip. Since 2026-10-10
the simulator also plays the shell's side of a dropped socket: it delivers
nothing to a participant whose socket closed and sends nothing from them
until they resume, and it checks the grace-period invariants of
[D35](core/README.md#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)
(a drop only marks a joined participant, the mark never moves later, no
grace outlives its alarm, a resume changes nothing else, and the room shows
exactly the dropped participants as reconnecting). Since 2026-10-10 the
simulated SFU also rejects some offered tracks in its publish answers, and
it can hold a call past its negotiation's deadline and answer it later
(`Stall`, `Unstall`); a simulated client offers again after its held offer
timed out (`Reoffer`), so the held answer arrives late for the new offer.
The tests check that every published or pulled track
was accepted by the SFU, and that a reply about a negotiation other than the
running one changes nothing. A second test (1,000 cases)
checks that the random sequences actually reach rooms that receive tracks,
record, end, lose, resume and expire sockets in the grace period, get a
publish answer that rejects some tracks, and get a late publish reply and a
late subscribe reply while a newer negotiation runs, since otherwise the properties would prove little. There is
no CI yet; phase 1 adds it, running these tests on every push.

## End-to-end tests (Playwright)

The end-to-end tests live in [`web/e2e/`](../web/e2e/) and run with
Playwright Test ([`web/playwright.config.ts`](../web/playwright.config.ts)).
They start the real Worker under `wrangler dev` and drive it the way clients
do. They replace the spike script `crates/worker/spike/ws-clients.mjs` (in the
git history) and the throwaway Playwright scripts of spike 2.

### Commands

| Command | What runs |
|---|---|
| `pnpm test:e2e` | Builds, then the `protocol`, `chromium` and `firefox` projects against the mock SFU |
| `pnpm test:e2e:recorder` | The `recorder` project (the test tagged `@recorder`): the mock SFU, and Workers AI remote. The script sets `E2E_RECORDER=1`; without it the test skips, so a `-g` filter cannot run it by accident |
| `pnpm test:e2e:real` | The `real` project (tests tagged `@real`) against the real Realtime SFU; skipped without the secrets |
| `pnpm --filter web exec playwright test e2e/protocol/failures.spec.ts` | One spec file (add `--project=…` for one browser) |
| `pnpm --filter web exec playwright test -g "dropped socket"` | Tests whose title matches |

Environment variables:

- `E2E_SKIP_BUILD=1` reuses `web/build` and `web/e2e/.artifacts/worker/`
  instead of building them again (for repeated local runs).
- `E2E_IDLE_MS` sets the idle phase of the hibernation test (default 15000).
- `E2E_SLOW=1` adds the keepalive test, which waits about two minutes.
- `E2E_HTML_REPORT=1` adds Playwright's HTML report in
  `web/e2e/.artifacts/report/` (off by default; the list reporter always runs).
- `CI` set: one retry per failed test, with a trace of the retry, and
  `test.only` fails the run.

Screenshots of failed tests, traces and each stack's logs land in
`web/e2e/.artifacts/` (git-ignored): `results/` per test, `stacks/<project>-w<n>-<time>/`
with `wrangler.log`, `mock-sfu.log` and the Durable Object and KV state.

### Installing what the tests need

On a clean checkout (CI or a new machine):

1. Install Rust with `rustup`, then add the WebAssembly target:
   `rustup target add wasm32-unknown-unknown`.
2. Install `wasm-pack` (it builds the core for the web app) and `worker-build`:
   `cargo install wasm-pack worker-build`. The build looks for both in
   `$CARGO_HOME/bin` (default `~/.cargo/bin`) even if that is not on `PATH`.
3. Install Node 26 or newer and pnpm, then `pnpm install` at the repo root.
4. Install the browsers of the pinned Playwright version:
   `pnpm --filter web exec playwright install chromium firefox`
   (on a CI runner add `--with-deps` for the system libraries).
5. Run `pnpm test:e2e`. Expected: the build output, then about 60 tests, of
   which 3 are skipped (the keepalive test without `E2E_SLOW=1`, and screen
   sharing and the device switch in Firefox).

For `pnpm test:e2e:real`, put `CREATE_SECRET`, `CALLS_APP_ID` and
`CALLS_APP_SECRET` into the git-ignored `.dev.vars` at the repo root; the
stack passes that file to `wrangler dev --env-file` and never copies it.

### How the stack works

A global setup ([`global-setup.ts`](../web/e2e/global-setup.ts)) runs once per
run: `pnpm run build` (the WASM core and the SvelteKit app into `web/build`)
and `worker-build` into `web/e2e/.artifacts/worker/`.

The worker-scoped fixture `stack` ([`fixtures/stack.ts`](../web/e2e/fixtures/stack.ts))
starts one stack per Playwright worker, so workers run in parallel without
sharing state:

- the mock SFU ([`mock-sfu.ts`](../web/e2e/mock-sfu.ts)) as a child process;
- `wrangler dev` with a copy of `wrangler.toml` in the stack's directory: the
  `[build]` table is dropped and `main` points at the prebuilt bundle, so
  wrangler neither rebuilds nor watches `crates/worker` while tests run, and
  local workerd gets `ROOM_JURISDICTION` empty (no jurisdictions locally,
  [D5](architecture/README.md#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)).

A project picks the stack's kind with the `stackKind` option:

| Kind | SFU | Workers AI | Used by |
|---|---|---|---|
| `mock` | mock | none (`--local` disables remote bindings): no network, no login | `protocol`, `chromium`, `firefox` |
| `recorder` | mock | remote, as `wrangler dev` runs it by default: needs the network and `wrangler login`, billed in neurons | `recorder` |
| `real` | the real Realtime SFU, secrets from `.dev.vars` | remote | `real` |

The mock SFU listens on port 0 and reports the port it got in its log. Against
the mock SFU, `wrangler dev` gets a port picked just before it starts and
`RECORDER_ORIGIN=http://localhost:<port>`, so the recorder's adapter
endpoints name this Worker and the mock's adapters stream into it (the
`wrangler.toml` value is the production host). If another process takes the
port in between, `wrangler dev` fails with an `[ERROR]` line and the stack
fails at once. The `real` stack keeps port 0. Each child runs in its own
process group and is stopped by signalling that group by its PID, never by
matching command lines.

The mock SFU speaks just enough of the Realtime SFU API for the Worker
(`sessions/new`, `tracks/new` for publishing and pulling, `renegotiate`,
`tracks/close`, `tracks/update`, `adapters/websocket/new` and `close`) and writes real SDP
([`sdp.ts`](../web/e2e/sdp.ts)): answers that accept a browser's offer with
its codecs and simulcast layers, and offers that carry the pulled tracks. So
browsers negotiate against it as against the real SFU, but no media flows (the
SDP has no ICE candidates). Like the live SFU before a publisher is
connected, the first pull of each track fails with `not_found_track_error`
and still returns an offer. Its WebSocket adapters work like Cloudflare's: each
opens a WebSocket to the recorder's `/rec/…` endpoint and streams the audio
set with `adapter-audio` in 20 ms protobuf `Packet`s, looping, until
`adapters/websocket/close`; without audio (the default) an adapter connects
and sends nothing, so recorded meetings in the offline suite store no chunks
and never call Workers AI. It connects only to `ws://` endpoints, never to the
production host. Tests control it through `/__control/`:

| Request | Effect |
|---|---|
| `POST /__control/reset` | Clears every fault (sessions and the call log stay) |
| `POST /__control/down` `{"down": true}` | Every SFU call answers 503 |
| `POST /__control/close-delay` `{"ms": 1000}` | Holds each `tracks/close` that long |
| `POST /__control/lose-session` `{"next": "pull"}` or `{"session": "<id>"}` | That session answers this and every later call with HTTP 410 `session_error` |
| `POST /__control/adapter-audio` `{"file": "<path>"}` or `{"file": null}` | Adapters created afterwards stream that file (raw 48 kHz stereo s16le PCM, what an adapter delivers), or nothing; `reset` sets `null` |
| `GET /__control/calls?since=<seq>` | The calls the Worker made after `seq`, with their request bodies |
| `GET /__control/adapters` | Every adapter: id, track, endpoint, socket state (`connecting`, `open`, `closed`, `refused`) and packets sent |

The typed client is `MockSfu` in
[`fixtures/mock-sfu-control.ts`](../web/e2e/fixtures/mock-sfu-control.ts);
the test-scoped `sfu` fixture resets it before and after each test.

### Fixtures

Tests import `test` and `expect` from
[`fixtures/test.ts`](../web/e2e/fixtures/test.ts). Besides `stack` and `sfu`:

- `api`: the Worker's HTTP API ([`fixtures/api.ts`](../web/e2e/fixtures/api.ts)),
  returning raw responses; `loginAsOwner` and `createMeeting` set tests up
  through it.
- `owner`: a fresh owner session cookie for the test.
- `rooms`: opens `RoomClient`s ([`fixtures/room-client.ts`](../web/e2e/fixtures/room-client.ts)),
  typed WebSocket clients that send `ClientMsg` and wait for typed `ServerMsg`s
  (types from `web/src/lib/protocol.ts`, JSON only, no WASM). Helpers in
  [`fixtures/meeting.ts`](../web/e2e/fixtures/meeting.ts) (`joinAs`,
  `liveMeeting`) and [`fixtures/negotiation.ts`](../web/e2e/fixtures/negotiation.ts)
  (`publishOffer`, `receive`) build rooms and play the browser's part in
  negotiations.
- `host`, `guest` and `people.open(label, { owner })`: browser participants
  ([`fixtures/participant.ts`](../web/e2e/fixtures/participant.ts)), each in
  its own context; the owner's context gets the session cookie through the
  API. A participant logs every signalling frame its page sent and received,
  and knows the UI by its roles and texts (`control(name)`, `endForAll()`,
  …) and, where a role or text does not single an element out, by a
  `data-testid` (the getters `meeting`, `tiles`, `tile(name)`, `pill(text)`
  and `transcriptLink`; the testids are listed in "Adding a test").
- [`fixtures/speech.ts`](../web/e2e/fixtures/speech.ts): the snippets of
  [`web/e2e/audio`](../web/e2e/audio/README.md) and `speechPcm()`, which
  reads their WAV files and returns them as an adapter's PCM. [`fixtures/call.ts`](../web/e2e/fixtures/call.ts) has
  `startCall`; [`fixtures/media-stats.ts`](../web/e2e/fixtures/media-stats.ts)
  reads `getStats()` for the real-SFU test.

Chromium runs with `--use-fake-device-for-media-stream=device-count=2` and
`--use-fake-ui-for-media-stream`, Firefox with `media.navigator.streams.fake`
and `media.navigator.permission.disabled`. `device-count=2` gives Chromium two
fake cameras (`fake_device_0`, `fake_device_1`) and three fake microphones
("Fake Default Audio Input", "Fake Audio Input 1", "Fake Audio Input 2"), so
the device switch test has a second device to switch to. Firefox fakes one
device per kind, so that test runs in Chromium only.

### What each spec covers

Protocol layer (`web/e2e/protocol/`, Node only, about 25 s):

| Spec | Covers |
|---|---|
| `owner-session.spec.ts` | `POST /api/login` (401 wrong password, 400 without one, 204 with the cookie `HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000`), `GET /api/me` (cookie, none, forged), `POST /api/logout` |
| `http-routes.spec.ts` | `POST /api/new` (401 without or with a wrong secret, 201 with the secret or the cookie, three words and nothing else), `GET /api/meeting/{words}` (policy, `scheduled`, case-insensitive, 404), 426 without upgrade, `GET /api/ice` (STUN for a known meeting, 404 otherwise) |
| `lifecycle.spec.ts` | A guest waits in a scheduled room despite a forged `x-meet-owner`; the owner joins as host, the room goes live, the guest is let in; ping and pong; a malformed frame; leaving; a dropped socket keeps the participant and its tracks, shown `reconnecting`, until the resume with the token brings it back with the same id and epochs; a knocking guest whose socket closes is gone at once; resume with the token; "join as host" with the cookie; `End` closes every socket, refuses the next one and releases the words; the room keeps its participants through an idle phase |
| `consent-recording.spec.ts` | Recording from the host's arrival in a `fromStart` meeting, the transcript token in `Ended`; a guest without consent waits in `awaitingConsent` until `SetConsent`; `hostDecides`: recording on and off by the host, `ConsentRequired` and `Muted` for a guest without consent; a guest cannot record |
| `negotiation.spec.ts` | `PublishAnswer` from the SFU; a track the SFU rejects (an offered mid missing from the SDP) is listed in `PublishAnswer.rejected`, not shown in the room view and never pulled; the first pull's `not_found_track_error` reaches the guest as a `SubscribeOffer` without tracks, the retry delivers the track; answers reach `renegotiate`; `Unpublish` closes the track on the publish and the subscribe session; a third participant's track reaches everyone |
| `host-controls.spec.ts` | Lock, knock, admit; kick (with the SFU holding `tracks/close` while the kicked client sends `Join` on the same socket, which must not bring it back); a guest cannot kick; a host's mute |
| `failures.spec.ts` | A pull answered with 410 makes the room create a new session, send `SubscribeReset` with epoch 1, deliver the track on the new session and make no further call on the lost one; `ResetSession` answered with `PublishReset` or `SubscribeReset`, a stale epoch ignored; with the SFU down, joining fails with `SfuUnavailable` and the socket closes |
| `meetings-list.spec.ts` | `GET /api/meetings`: 401 without or with a wrong secret, 200 with the cookie; newest first; an ended meeting with its transcript token, a never-joined one as `scheduled` |
| `keepalive.spec.ts` | With `E2E_SLOW=1`: a socket that never pings is closed with 1001, a pinging one stays |

Recorder project (`web/e2e/protocol/recorder.spec.ts`, tag `@recorder`, Node
only, about 50 s; the `protocol` project skips it):

| Spec | Covers |
|---|---|
| `recorder.spec.ts` | A meeting recorded from the start: the host's arrival starts recording; the recorder creates one adapter for the host's published audio (publisher session, track name, `pcm`, an endpoint `ws://…/rec/{recorder}/{secret}/{who}/0`); it accepts the adapter's socket, and refuses a wrong secret (403) and an unknown generation (410); the adapter streams 40 s of the real speech; `Ended` carries the token; the end closes the adapter at the SFU; the status API sends `no-referrer`, `noindex`, `no-store`, names the meeting and its times, and counts the stored chunks as long as the stream (±3 s); an unknown token is 404 with the same headers; the job finishes with every chunk handled and `ready` (the transcript page's parser reads the status); the transcript names the meeting, labels the speaker "Felix (host)", lists the host's consent (notice version 1), has no gaps, and contains every snippet's keywords; `transcript.md` and `transcript.json` download; `/t/{token}` carries the headers from `web/static/_headers` |

Browser layer (`web/e2e/browser/`, Chromium and Firefox, about 1 min each):

| Spec | Covers |
|---|---|
| `owner.spec.ts` | Login on `/` (wrong password, then the owner area), creating a meeting, logout; `/meetings` shows Scheduled, Live and Ended with the transcript link; "Join as host" inside a live meeting: the owner, joined as a guest from a browser without the cookie while their own host browser is in the call, logs in through the control bar's dialog, gets the host controls, keeps the participant id, the typed name and both peer connections, and the host never sees them leave |
| `call.spec.ts` | The owner's pre-join (name prefilled, "You join as host"); a guest's pre-join (recording notice, "not started yet") and lobby, let in when the host joins; tiles with names, the recording badge, host controls only for the host; mute, camera off and on (the guest's view of the host follows the room), a host muting a guest; the device pickers on the pre-join; switching camera and microphone in the "Devices" dialog swaps the tracks on the publish connection's senders without an unpublish or a new publish offer, and the guest still sees the host's video (Chromium only); screen share with the expand button, control bar visible while expanded for both sides (Chromium only); leave, end for all, the ended page with the transcript link, a late join |
| `consent.spec.ts` | `hostDecides`: the host starts recording, the guest in the call gets the consent dialog, the host sees "Guest has not accepted the recording yet" and "Not recorded" on the guest's tile, guests see neither; a late guest without consent waits outside and joins the notice ("Guest and Late have not…"); each accept shortens it, and it disappears when both accepted |
| `recovery.spec.ts` | The SFU loses the guest's subscribe session: `SubscribeReset`, a new session, negotiation on it; a failed peer connection (simulated in the page): "Reconnecting to the others…", `ResetSession`, a new session; a dropped socket (through `page.routeWebSocket`, closed with code 4000 because the browser rejects 1001 there): the "Reconnecting…" pill for the guest and a dimmed "Reconnecting…" tile on the host's side, then a `Join` with the resume token, the same participant, and both marks gone |
| `real-sfu.spec.ts` (`@real`) | Three people through the real SFU: each receives the others' video (bytes and decoded frames per tile) and audio; a small window gets a smaller simulcast layer, and the decoded width stays at most half the full layer's at every 1 s sample for 25 s, with no new `SetLayer` (the run prints the samples); a screen share reaches the others; 60 s without a new socket or session reset while media keeps flowing. Five people (host and four guests): every tile's video grows in bytes and decoded frames in each of three 10 s rounds |

### Adding a test

1. Pick the layer: a rule of the room or the HTTP API goes into
   `web/e2e/protocol/`; something a person sees or does goes into
   `web/e2e/browser/`; something that needs real media goes into
   `real-sfu.spec.ts` (tag `@real`); something that needs the real Workers AI
   goes into `recorder.spec.ts` (tag `@recorder`).
2. Import `test` and `expect` from `../fixtures/test.ts` and ask for the
   fixtures you need; never start processes or log in by hand.
3. Create a meeting of its own per test (`createMeeting`, `liveMeeting`,
   `startCall`), so tests stay independent and can run in any order.
4. Wait for messages and UI states (`take`, `waitForReceived`, `expect(…)`
   with its retries, `sfu.waitForCall`), never for fixed times. To check that
   something does not happen, use `gets(…, timeoutMs)` with a short window.
5. Find UI elements by role and visible text, as a person does. If a test
   needs a hook the UI lacks, add a `data-testid` in the component rather than
   a CSS path, and a getter for it on `Participant`. The testids so far:
   `meeting` (the meeting view, `Meeting.svelte`), `tile` and `tile-name`
   (every participant's tile in any layout and the name in its caption,
   `Tile.svelte`), `pill` (each notification pill above the control bar,
   `Toasts.svelte`) and `transcript-link` (the link on the page after a
   recorded meeting ended, `MeetingFlow.svelte`).
6. Run `pnpm --filter web check` and `pnpm --filter web lint`: both cover
   `web/e2e/`.

Local `wrangler dev` cannot prove that a DO hibernates or what Cloudflare
bills; that needs the deployed checks below.

## Web: check and build

`pnpm --filter web check` runs `svelte-check` with strict TypeScript, then
`tsc` over `web/e2e/` (its own `tsconfig.json`, with Node types);
`pnpm --filter web build` builds the static site. `pnpm --filter web lint`
runs Prettier and ESLint over the app and the e2e tests. `pnpm --filter web
test` runs the vitest unit tests (`src/**/*.test.ts`); the browser tests are
the end-to-end layer above.

## Planned: device checklist (phase 4)

A written checklist, run on an iPhone, an iPad, an Android phone, and
desktop Chrome, Firefox and Safari: join from a plain browser tab, camera and
microphone, simulcast from iOS, lock screen, app switch, an incoming phone
call, screen share from desktop and its absence on phones, Wake Lock,
Picture-in-Picture and speaker selection. Spike 4 runs the first part of it
early ([media/](media/README.md)).

## Planned: deployed Worker checks

Some facts only show on Cloudflare itself:

- **Hibernation**: after deploy, the dashboard must show no DO duration for
  a room whose sockets are open but idle.
- **Free plan**: whether the SFU and Workers AI speech models work on the
  Workers Free plan (spike 1).
- **Egress**: Cloudflare's usage numbers after a real four-person hour
  (spike 5) replace the assumed bitrates in [cost/](cost/README.md).

## Recorder: unit tests and the end-to-end test

Pure parts run natively under `cargo test` (2026-10-09):
`meeting-core` tests the consent record and roster, speaker labels, the
silence gate, the chunk cuts, the hole detection on sequence numbers,
timestamps and arrival times, the paragraph merge, Europe/Berlin times and
`transcript.md` (`src/tests/recording.rs`); `sfu-api` tests the adapter wire
shapes and the per-entry results of `adapters/websocket/new` and `close`;
the worker tests the protobuf reader, the downmix and FIR decimation (1 kHz
passes, 12 and 20 kHz are attenuated more than 40 dB, a constant stays
constant), the Opus encoder (bitrate cap, pre-skip measured with a click),
the Ogg writer (read back with `ogg::PacketReader`), the capture pipeline
(30 s cut, spooling every 1.5 s, holes, silent chunks), the job's retry rules
and gaps, Whisper's reply filter, and the versioned room record (including
the grace alarm a dropped socket arms, 2026-10-10).

The end-to-end test is
[`web/e2e/protocol/recorder.spec.ts`](../web/e2e/protocol/recorder.spec.ts),
run with `pnpm test:e2e:recorder` (spec table above). It replaced the
standalone script `crates/worker/recorder-e2e/run.mjs` (in the git history)
on 2026-10-10 and keeps its 22 checks, with these differences:

- The owner's login and the meeting's creation are no checks of their own:
  the `owner` fixture and `createMeeting` fail the test if either fails, and
  `owner-session.spec.ts` and `http-routes.spec.ts` check both.
- The refusals check the status: 403 for a wrong secret, 410 for an unknown
  generation (the script only saw "refused").
- The test waits until the adapter has sent 40 s of packets instead of
  sleeping 40 s, and compares the recorded length with what the adapter sent.
- Added: the adapter belongs to the host's publish session and track, the
  transcript has no gaps, and the transcript page's parser reads the status.
- No fallback to a 440 Hz tone: the test reads the WAV files itself, so it
  needs no `ffmpeg`.

First run on 2026-10-10: passed in 46 s; Whisper returned all four snippets
with every keyword.
