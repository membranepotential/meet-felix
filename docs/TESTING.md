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
| SFU client tests | Request and response JSON against samples from Cloudflare's docs and a live SFU response (`not_found_track_error` with an offer); core SFU effects to replies | `cargo test -p sfu-api` | Exist |
| Worker unit tests | Storage record, socket attachment with the owner flag, alarm (a fired alarm is forgotten unless the core sets it again) and keepalive rules; the owner's session cookie (valid, expired, tampered, foreign, missing) and password; `/api/new` and `/api/login` input checks | `cargo test -p meet-worker` | Exist |
| Worker spike scripts | The real `RoomObject` running the core under `wrangler dev`, with a mock SFU | `node crates/worker/spike/ws-clients.mjs` | Exists; covers login, scheduled and live rooms, the protocol end to end and the owner's meeting list |
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
pnpm --filter web build
```

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
recorded, and that `RoomState` survives a JSON round trip. A second test
checks that the random sequences actually reach rooms that receive tracks,
record and end, since otherwise the properties would prove little. There is
no CI yet; phase 1 adds it, running these tests on every push.

## Worker: local spike scripts under `wrangler dev`

The Worker runs locally with `pnpm exec wrangler dev --var ROOM_JURISDICTION:`
(local workerd implements no jurisdictions;
[D5](architecture/README.md#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)).
It needs `worker-build` (`cargo install worker-build`).

[`crates/worker/spike/ws-clients.mjs`](../crates/worker/spike/ws-clients.mjs)
started as spike 1's script and now speaks the real protocol
(`meeting_core::ClientMsg`/`ServerMsg`). It starts a mock Realtime SFU on
port 8788, so it needs no network and no SFU credentials; start the Worker
with the `--var` flags given in its header comment (creation secret, SFU
credentials, `SFU_API_URL` pointing at the mock). It checks:

- the owner's session: `POST /api/login` answers 401 to a wrong password,
  400 to a body without one, and 204 with the `owner` cookie
  (`HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000`) to the right
  one; `GET /api/me` answers 204 with the cookie and 401 without it or with
  a forged one; `POST /api/logout` clears the cookie;
- `POST /api/new`: 401 without or with a wrong secret, 201 with the bearer
  secret or the owner's cookie, three words and nothing else in the
  response; `GET /api/meeting/:words` with the recording policy and
  `phase: "scheduled"`, case-insensitive; 404 and 426 cases; `GET
  /api/ice?meeting=` answers STUN for a known meeting and 404 for an unknown
  one or none;
- a guest who joins the scheduled room gets `Waiting`, even with a forged
  `x-meet-owner` header; the owner's socket (session cookie on the upgrade)
  joins as host, the room goes live (`phase: "live"`) and records from the
  start, and the waiting guest gets `Welcome`; `State` broadcasts, the
  auto-response ping, a malformed frame;
- publish (`PublishAnswer` from the SFU) and subscribe: like the live SFU
  before the publisher is connected, the mock answers the first pull of each
  track with `not_found_track_error` and an offer, which reaches the guest as
  a `SubscribeOffer` without tracks; the retry after `SUBSCRIBE_RETRY_MS`
  offers the track, and the guest's answers reach `renegotiate`;
- `Unpublish`: the track is closed on the publish session and on the
  subscriber's session;
- after an idle phase (`SPIKE_IDLE_MS`, default 15 s): a client that closes
  its socket reaches the others as `State`; lock, a third client gets
  `Waiting`, knock, admit; the third client publishes and the others receive
  the track; kick, with the mock holding the kick's `tracks/close` while the
  kicked client sends `Join` on the same socket, which must not bring it
  back; a guest's kick refused;
- resume: a new socket with the resume token keeps the participant id, the
  old socket is closed, and messages reach the new one;
- "join as host": the guest reconnects with the owner's cookie and its
  resume token, keeps its id, becomes host in everyone's view, and its old
  socket is closed;
- `End`: everyone gets `Ended` with the transcript token (the meeting was
  recorded), the sockets close, the next socket and the meeting lookup get
  404;
- `GET /api/meetings`: 401 without or with a wrong secret; 200 with the
  owner's cookie; with the secret, the list is newest first and shows the ended meeting with `phase: "ended"`,
  `ended: true` and the transcript token from `Ended`, and a never-joined meeting with `phase: "scheduled"`;
- session recovery ([D33](media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09)): the mock answers a guest's
  pull with HTTP 410 `session_error` and every later call on that session
  too; the room creates a new session, sends `SubscribeReset`, delivers the
  host's track on the new session and makes no further call on the lost one;
  `ResetSession` for either kind is answered with `PublishReset` or
  `SubscribeReset` and the new epoch, and a `ResetSession` with an old epoch
  gets no answer;
- with the mock SFU down, joining yields `Error{SfuUnavailable}` and the
  socket closes;
- with `SPIKE_KEEPALIVE=1` (two more minutes): a socket that never pings is
  closed with 1001, a pinging one stays.

It prints PASS or FAIL per check and needs Node 26 or newer, whose global
`WebSocket` sends the session cookie through its non-standard `headers`
option. Its result goes
to [spikes/01-do-hibernation.md](spikes/01-do-hibernation.md). Clients must
send the exact frame `{"type":"ping"}` at least every 60 seconds, or the
keepalive check closes their socket.

For a local run against the real SFU, put the secrets into the git-ignored
`.dev.vars` at the repo root instead of `--var` flags:

```sh
CREATE_SECRET=...
CALLS_APP_ID=...
CALLS_APP_SECRET=...
```

Local `wrangler dev` cannot prove that a DO hibernates or what Cloudflare
bills; that needs the deployed checks below.

## Web: check and build

`pnpm --filter web check` runs `svelte-check` with strict TypeScript;
`pnpm --filter web build` builds the static site. `pnpm --filter web lint`
runs Prettier and ESLint. There are no browser tests yet.

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
