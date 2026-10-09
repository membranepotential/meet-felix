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
| Worker spike scripts | A real `RoomObject` under `wrangler dev`: sockets, broadcast, hibernation | `node crates/worker/spike/ws-clients.mjs` | Spike 1 script exists |
| Web checks | Types and build of the Svelte shell | `pnpm --filter web check`, `pnpm --filter web build` | Exist |
| Device checklist | Real phones, tablets and desktop browsers | manual, phase 4 | Planned |
| Deployed Worker checks | Hibernation and billing on Cloudflare itself | manual, dashboard | Planned |

Run everything that exists before a commit:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
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
is spike 1's script. It creates a meeting through `POST /api/new`, checks the
three-word id, the host token and the meeting lookup, opens ten WebSocket
clients, and checks that a message from one reaches all of them, that the
auto-response answers pings, that malformed frames
are rejected, and that every socket closes cleanly. It prints PASS or FAIL
per check and needs Node 22 or newer. Its result goes to
[spikes/01-do-hibernation.md](spikes/01-do-hibernation.md).

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
