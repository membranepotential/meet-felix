# Spike 1: Rust Durable Object with 10 hibernating WebSockets (local part)

*2026-10-09. Run locally with `wrangler dev`; nothing was deployed. The Free-plan question for the SFU and Workers AI is still open (see "Still to verify on a deployed Worker").*

## Result

The Rust `RoomObject` holds 10 WebSockets through the Hibernation API, keeps its state in SQLite and socket attachments, and survives eviction. In `wrangler dev`, `workerd` evicted the room during a 15 s idle phase. It then rebuilt the room with all 10 sockets still connected, and the next chat reached all 10 clients. Pings matching the auto-response were answered without waking the room. The empty-room alarm deleted all storage after 2 minutes.

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

1. If `~/.cargo/bin` is not on `PATH`, prepend it: `wrangler` calls `worker-build` by name.
2. From the repo root, start the Worker: `CARGO_PROFILE_RELEASE_STRIP=debuginfo pnpm exec wrangler dev --var ROOM_JURISDICTION:`. Wait until it prints `Ready on http://localhost:8787`.
3. In a second terminal, run `node crates/worker/spike/ws-clients.mjs` (optional argument: base URL; `SPIKE_IDLE_MS` sets the idle phase, default 15000). It prints one PASS/FAIL line per check, then a summary, and exits 0 on success.

## Output of the spike script (final run, real `wrangler.toml`)

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
| Storage | `state.storage().sql().exec(query, Vec<SqlStorageValue>)` + `SqlCursor::to_array::<T>()`; `storage().set_alarm(Duration)`; `storage().delete_all()` |
| Room id | `ObjectNamespace::unique_id_with_jurisdiction("eu")` exists; locally `unique_id()` |

## Gotchas found

- **Crate name.** The crate cannot be named `worker`, because it depends on the `worker` crate. It is `meet-worker` (directory `crates/worker`), so the clippy command is `cargo clippy -p meet-worker --all-targets --target wasm32-unknown-unknown -- -D warnings`.
- **`wasm-bindgen` dependency.** `#[durable_object]` expands to code that names the `wasm_bindgen` crate directly, so the crate needs `wasm-bindgen` as its own dependency. Without it there are 17 errors such as ``cannot find module or crate `wasm_bindgen` ``.
- **Close handshake.** Cloudflare documents that from compatibility date 2026-04-07 (`web_socket_auto_reply_to_close`) the runtime answers Close frames itself. Local `wrangler dev` did not: without an explicit `ws.close(1000, "bye")` in `websocket_close`, no client got a Close frame within 5 s. The handler now calls `close`, which the docs call safe under the new flag.
- **Template layout.** The current workers-rs template uses `wrangler.toml`, `main = "build/index.js"` (not `build/worker/shim.mjs`) and builds with `worker-build --release`. Here the build runs in `crates/worker` (`build.cwd`) and `--out-dir ../../build` puts the bundle in the git-ignored `/build` at the repo root. A relative `build.cwd` resolves against the directory `wrangler` runs in, so run it from the repo root.
- **KV binding.** It has no `id`. `wrangler dev` uses a local store, and `wrangler deploy` provisions the namespace.

## Still to verify on a deployed Worker (Free plan)

1. Deploy works on the Free plan with `new_sqlite_classes` and the KV namespace provisioned automatically.
2. Dashboard: with 10 sockets open and idle for 10+ minutes, the DO's duration (GB-s) stays near zero, while requests count only real messages. Auto-response pings should add neither.
3. `unique_id_with_jurisdiction("eu")` works, `id_from_string` on the plain namespace reaches the same EU object, and the dashboard shows the EU location.
4. The deployed runtime replies to Close frames on its own, and the explicit `close` call stays harmless (no exception in logs).
5. The alarm fires 2 minutes after the last socket leaves and `delete_all` empties the room.
6. Out of scope here, still part of spike 1: whether the Realtime SFU and Workers AI speech models work on the Free plan.
