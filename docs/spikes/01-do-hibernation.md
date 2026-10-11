# Spike 1: Rust Durable Object with 10 hibernating WebSockets (local part)

*2026-10-09. Run locally with `wrangler dev`; nothing was deployed. The Free-plan question for the SFU and Workers AI is still open (see "Still to verify on a deployed Worker").*

## Result

The Rust `RoomObject` holds 10 WebSockets through the Hibernation API, keeps its state in SQLite and socket attachments, and survives eviction. In `wrangler dev`, `workerd` evicted the room during a 15 s idle phase. It then rebuilt the room with all 10 sockets still connected, and the next chat reached all 10 clients. Pings matching the auto-response were answered without waking the room. The empty-room alarm deleted all storage after 2 minutes.

Rerun on 2026-10-09 after the `RoomObject` started running `meeting_core::step`: the room keeps its `RoomState` as JSON under one storage key and each socket's participant id in its attachment. With a mock SFU, the script passes 36 checks: create with the creation secret, host and guest join, publish and subscribe, a 15 s idle phase, lock, knock, admit, kick, resume, end, the refusal of later sockets, `SfuUnavailable` while the SFU is down, and the keepalive check that closes a socket without pings. An ended room no longer deletes its storage: it keeps an ended marker so it refuses later sockets. (2026-10-10: since then an ended room deletes its storage an hour after End; see the as-built note under [D8](../links/README.md#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09).)

Rerun on 2026-10-09 after a review of the worker: 43 checks, 45 with the keepalive check. New checks: `Unpublish` closes the track on the publisher's publish session and on the subscriber's session; a client that closes its own socket reaches the others as a `State` without it; a kicked client that sends `Join` on the same socket at once, while the room is still inside the kick's SFU call, does not come back (the mock holds `tracks/close` for one second). That last check fails (41/42) with the old effect order, where `CloseSocket` ran after the SFU calls, and passes now that the room carries out socket effects before SFU calls. The mock SFU now answers the first pull of each track like the live SFU did in [spike 2](02-first-call.md) while the publisher was not connected yet: `not_found_track_error` with an offer. The room passes that offer to the client without tracks, and the retry after `SUBSCRIBE_RETRY_MS` delivers the track.

Two findings need action outside `crates/worker`:

1. **`strip = true` in the workspace `[profile.release]` breaks `worker-build`.** wasm-bindgen fails with `externref table required for catch wrappers`. Tested one setting at a time: `strip = true` fails, while `lto = true`, `opt-level = "s"`, `codegen-units = 1` and `strip = "debuginfo"` all build. Fix: set `strip = "debuginfo"` in the root `Cargo.toml`. Until then, run with `CARGO_PROFILE_RELEASE_STRIP=debuginfo`.
2. **Local workerd has no jurisdictions.** `unique_id_with_jurisdiction("eu")` throws `Jurisdiction restrictions are not implemented in workerd.` The jurisdiction therefore comes from the var `ROOM_JURISDICTION` (`"eu"` in `wrangler.toml`; an empty value means none). Local runs pass `--var ROOM_JURISDICTION:`. A `dev` script in `package.json` should carry that flag.

## Versions

| Component | Version |
|---|---|
| `worker` crate (crates.io) | 0.8.7 |
| `worker-build` | 0.8.7 (output: `build/index.js`, `build/index_bg.wasm`, ≈300 KB wasm) |
| `wasm-bindgen` | 0.2.129 |
| `wrangler` | 4.147.0 |
| `workerd` (bundled) | 1.20261001.1, so `compatibility_date = "2026-10-01"` is the newest date it supports |
| Rust / Node | 1.95.0 / 26.6.0 |

## How to run

Since 2026-10-09 the script drives the real room (`meeting_core::step`) and starts a mock Realtime SFU on port 8788, so it needs no network. The root `Cargo.toml` now has `strip = "debuginfo"`, so the `CARGO_PROFILE_RELEASE_STRIP` override is no longer needed.

1. If `~/.cargo/bin` is not on `PATH`, prepend it: `wrangler` calls `worker-build` by name.
2. From the repo root, start the Worker with the spike's secrets and the mock SFU:
   `pnpm exec wrangler dev --var ROOM_JURISDICTION: --var CREATE_SECRET:spike-secret --var CALLS_APP_ID:spike --var CALLS_APP_SECRET:spike-sfu --var SFU_API_URL:http://localhost:8788/v1`.
   Wait until it prints `Ready on http://localhost:8787`. Add `--port <port>` if 8787 is taken.
3. In a second terminal, run `node crates/worker/spike/ws-clients.mjs` (optional argument: base URL, such as `http://localhost:8797` for `--port 8797`; `SPIKE_IDLE_MS` sets the idle phase, default 15000; `SPIKE_KEEPALIVE=1` adds the keepalive check, about two more minutes). It prints one PASS/FAIL line per check, then a summary, and exits 0 on success.

   The script was replaced on 2026-10-09 by the protocol tests in `web/e2e/protocol/` (`pnpm test:e2e`; see [TESTING.md](../TESTING.md#end-to-end-tests-playwright)) and is in the git history.

Local storage from older runs persists under `.wrangler/state`. Rooms stored by the first spike script (a record with a `status` field) fail to load when their alarm fires (`unknown variant status, expected open or ended`); the errors do not affect new rooms.

## Output of the spike script after the worker review (2026-10-09, `--port 8797`)

```
PASS  POST /api/new without a secret -> 401
PASS  POST /api/new with a wrong secret -> 401
PASS  POST /api/new with the secret -> 201  (status 201)
PASS  meeting is three words  (polo-slang-otter)
PASS  hostToken is 32 bytes hex
PASS  GET /api/meeting/{words} -> recording policy, not ended  ({"recordingPolicy":"fromStart","ended":false})
PASS  GET /api/meeting/{WORDS} matches case-insensitively  (status 200)
PASS  GET /api/meeting/{words} unknown -> 404  (status 404)
PASS  GET /ws/{words} without upgrade -> 426  (status 426)
PASS  GET /api/ice returns Cloudflare STUN
PASS  host gets Welcome as host, recording on  (you=0)
PASS  auto-response answers the exact ping
PASS  room rejects a malformed frame
PASS  guest with consent gets Welcome  (Host,Guest)
PASS  host gets the State broadcast with the guest joined
PASS  host gets the SFU's PublishAnswer
PASS  a pull whose track is not found still reaches the guest as an empty SubscribeOffer  ([[]])
PASS  after the retry the guest gets a SubscribeOffer for the host's audio  ([{"mid":"0","track":{"owner":0,"kind":"audio"}}])
PASS  the guest's answers reach the SFU's renegotiate
PASS  Unpublish closes the track on the publish session
PASS  …and on the subscriber's session
      idle 15000 ms ...
PASS  host sees the leaver join  (id=2)
PASS  a client-initiated close reaches the others as State
PASS  Lock reaches the guest as State
PASS  a third client in the locked room gets Waiting
PASS  host gets the Knock  (who=3)
PASS  admitted client gets Welcome
PASS  the third client publishes
PASS  host and guest receive the third client's track  (true,true)
PASS  kicked client gets Kicked
PASS  kicked client's socket closes  (code 1000)
PASS  the kick closed tracks on the SFU
PASS  a Join right after the kick does not bring the kicked client back  (false,false)
PASS  a guest cannot kick
PASS  resume keeps the participant id  (you=1)
PASS  the room closes the old socket  (code 1000)
PASS  messages for the participant reach the new socket
PASS  End reaches everyone  (true,true)
PASS  every socket closes after End  (1000,1000)
PASS  the next socket is refused  (status 404)
PASS  the words are released  (status 404)
PASS  with the SFU down, joining yields Error{SfuUnavailable}
PASS  …and the room closes the socket  (code 1000)

PASS: 43/43 checks passed
```

The keepalive check, from a second run with `SPIKE_IDLE_MS=1000 SPIKE_KEEPALIVE=1` (last lines):

```
      waiting up to 130 s for the keepalive check ...
PASS  keepalive closes a socket that never pings (1001)  (code 1001)
PASS  keepalive keeps a pinging socket

PASS: 45/45 checks passed
```

## Output of the spike script with the core (2026-10-09, `SPIKE_KEEPALIVE=1`), before the review

```
PASS  POST /api/new without a secret -> 401
PASS  POST /api/new with a wrong secret -> 401
PASS  POST /api/new with the secret -> 201  (status 201)
PASS  meeting is three words  (cringe-shine-region)
PASS  hostToken is 32 bytes hex
PASS  GET /api/meeting/{words} -> recording policy, not ended  ({"recordingPolicy":"fromStart","ended":false})
PASS  GET /api/meeting/{WORDS} matches case-insensitively  (status 200)
PASS  GET /api/meeting/{words} unknown -> 404  (status 404)
PASS  GET /ws/{words} without upgrade -> 426  (status 426)
PASS  GET /api/ice returns Cloudflare STUN
PASS  host gets Welcome as host, recording on  (you=0)
PASS  auto-response answers the exact ping
PASS  room rejects a malformed frame
PASS  guest with consent gets Welcome  (Host,Guest)
PASS  host gets the State broadcast with the guest joined
PASS  host gets the SFU's PublishAnswer
PASS  guest gets a SubscribeOffer for the host's audio  ([{"mid":"0","track":{"owner":0,"kind":"audio"}}])
PASS  the guest's answer reaches the SFU's renegotiate
      idle 15000 ms ...
PASS  Lock reaches the guest as State
PASS  a third client in the locked room gets Waiting
PASS  host gets the Knock  (who=2)
PASS  admitted client gets Welcome
PASS  kicked client gets Kicked
PASS  kicked client's socket closes  (code 1000)
PASS  a guest cannot kick
PASS  resume keeps the participant id  (you=1)
PASS  the room closes the old socket  (code 1000)
PASS  messages for the participant reach the new socket
PASS  End reaches everyone  (true,true)
PASS  every socket closes after End  (1000,1000)
PASS  the next socket is refused  (status 404)
PASS  the words are released  (status 404)
PASS  with the SFU down, joining yields Error{SfuUnavailable}
PASS  …and the room closes the socket  (code 1000)
      waiting up to 130 s for the keepalive check ...
PASS  keepalive closes a socket that never pings (1001)  (code 1001)
PASS  keepalive keeps a pinging socket

PASS: 36/36 checks passed
```

## Output of the first spike script (placeholder protocol, real `wrangler.toml`)

```
PASS  POST /api/new returns 201  (status 201)
PASS  meeting is three words  (stone-juniper-iris)
PASS  host_token is 32 bytes hex
PASS  GET /api/meeting/{words} known -> 200  (status 200)
PASS  GET /api/meeting/{words} unknown -> 404  (status 404)
PASS  GET /ws/{words} without upgrade -> 426  (status 426)
PASS  10 sockets open
PASS  all 10 clients receive the chat from client-3  (10/10)
      idle 15000 ms ...
PASS  after 15000 ms idle, a chat from client-7 still reaches all 10  (10/10)
PASS  auto-response answers the exact ping on every socket
PASS  room answers a differently spelled ping
PASS  room rejects a malformed frame
PASS  chat before hello is refused
PASS  every socket completes the close handshake with 1000  (1000,1000,1000,1000,1000,1000,1000,1000,1000,1000,1000)

PASS: 14/14 checks passed
```

## What the local run proves, and what it does not

Proven locally:

- **Eviction and wake keep state.** In a separate run, a temporary log line in `RoomObject::new` (removed afterwards) printed `RoomObject constructed, sockets=10` during the idle phase. So workerd dropped the instance while 10 hibernatable sockets stayed open, and rebuilt it for the next message. That message needed the sender's name (SQLite) and participant id (attachment); both survived.
- **Auto-response does not wake the room.** An ad-hoc client sent `Hello`, idled 15 s, then sent 5 exact pings. All 5 pongs arrived and the constructor count did not change. The chat that followed constructed the room once.
- **Storage holds the state.** During the idle phase, the room's local SQLite file held 10 rows in `participants`. Each close deleted that socket's row. About 2 minutes after the last socket left, every room database was empty (no tables) and `_cf_ALARM` held no pending alarm.
- **Routing and protocol.** These work: KV with TTL, `unique_id` plus `id_from_string`, forwarding the upgrade to the DO, the typed JSON protocol, and broadcast to all sockets.

Not proven locally:

- **Billing.** Whether duration billing stops while the room hibernates. `wrangler dev` has no billing; only the dashboard can show it.
- **Platform limits and timing.** Production eviction timing, the Free-plan limits, and EU jurisdiction placement.

## Hibernation API as used from Rust (`worker` 0.8.7)

| Purpose | Rust API |
|---|---|
| Define the object | `#[durable_object] pub struct RoomObject { state: State }` + `impl DurableObject for RoomObject` |
| Accept a socket for hibernation | `State::accept_web_socket(&WebSocket)` (`accept_websocket_with_tags(&ws, &[&str])` exists, unused) |
| Per-socket data | `WebSocket::serialize_attachment<T: Serialize>(T)` / `deserialize_attachment<T: DeserializeOwned>() -> Result<Option<T>>`, both present in Rust. The attachment is still readable inside `websocket_close`. Limit 16 KiB. |
| All sockets | `State::get_websockets() -> Vec<WebSocket>` |
| Ping without waking | `State::set_websocket_auto_response(&WebSocketRequestResponsePair::new(PING, PONG)?)`, set in `new`; matches the raw frame byte for byte |
| Handlers | `websocket_message(ws, WebSocketIncomingMessage)`, `websocket_close(ws, code: usize, reason: String, was_clean: bool)`, `websocket_error(ws, Error)`, `alarm() -> Result<Response>` |
| Storage | `state.storage().sql().exec(query, Vec<SqlStorageValue>)` + `SqlCursor::to_array::<T>()`; `storage().set_alarm(Duration)`; `storage().delete_all()`. Since the core runs, the room keeps one JSON string under one key with `storage().get::<String>` / `put` (SQLite-backed objects allow 2 MB per key plus value). |
| Last auto-response | `getWebSocketAutoResponseTimestamp(ws)` has no binding in 0.8.7; `js_sys::Reflect` on `state.as_raw()` calls it. The keepalive check proves it works locally. |
| Room id | `ObjectNamespace::unique_id_with_jurisdiction("eu")` exists; locally `unique_id()` |

## Gotchas found

- **Crate name.** The crate cannot be named `worker`, because it depends on the `worker` crate. It is `meet-worker` (directory `crates/worker`), so the clippy command is `cargo clippy -p meet-worker --all-targets --target wasm32-unknown-unknown -- -D warnings`.
- **`wasm-bindgen` dependency.** `#[durable_object]` expands to code that names the `wasm_bindgen` crate directly, so the crate needs `wasm-bindgen` as its own dependency. Without it there are 17 errors such as ``cannot find module or crate `wasm_bindgen` ``.
- **Close handshake.** Cloudflare documents that from compatibility date 2026-04-07 (`web_socket_auto_reply_to_close`) the runtime answers Close frames itself. Local `wrangler dev` did not: without an explicit `ws.close(1000, "bye")` in `websocket_close`, no client got a Close frame within 5 s. The handler now calls `close`, which the docs call safe under the new flag.
- **Template layout.** The current workers-rs template uses `wrangler.toml`, `main = "build/index.js"` (not `build/worker/shim.mjs`) and builds with `worker-build --release`. Here the build runs in `crates/worker` (`build.cwd`) and `--out-dir ../../build` puts the bundle in the git-ignored `/build` at the repo root. A relative `build.cwd` resolves against the directory `wrangler` runs in, so run it from the repo root.
- **KV binding.** The namespace `MEETINGS` was created on 2026-10-09 and its `id` is in the git-ignored `.env` (`MEETINGS_KV_ID`); `wrangler dev` uses a local store.
- **Method names.** `#[durable_object]` generates a `connect` method, so a `RoomObject` method of that name does not compile (`duplicate definitions with name connect`).
- **Integer map keys in internally tagged enums.** `RoomState` keys maps by `ParticipantId(u32)`, which JSON writes as `"0"`. Wrapped in an enum with `#[serde(tag = ...)]`, the stored record failed to load (`invalid type: string "0", expected u32`), because serde buffers internally tagged content. The record enum is externally tagged.
- **Upgrade from Node's `fetch`.** Node's `fetch` refuses an `Upgrade` header (`fetch failed`); the script reads a refused upgrade's status with `node:http`.

## Still to verify on a deployed Worker (Free plan)

1. Deploy works on the Free plan with `new_sqlite_classes` and the KV namespace provisioned automatically.
2. Dashboard: with 10 sockets open and idle for 10+ minutes, the DO's duration (GB-s) stays near zero, while requests count only real messages. Auto-response pings should add neither.
3. `unique_id_with_jurisdiction("eu")` works, `id_from_string` on the plain namespace reaches the same EU object, and the dashboard shows the EU location.
4. The deployed runtime replies to Close frames on its own, and the explicit `close` call stays harmless (no exception in logs).
5. The room ends 2 minutes after the last admitted participant leaves (the core's alarm), keeps only its ended marker, and the words leave KV.
6. Out of scope here, still part of spike 1: whether the Realtime SFU and Workers AI speech models work on the Free plan.
