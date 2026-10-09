# Cloudflare platform deep dive for a Rust serverless video meeting app (checked 2026-10-09)

Source note: Cloudflare docs pages quoted below were fetched on 2026-10-09 (several via their `index.md` raw variants). Where WebFetch summarised a page, I flag that the wording is a paraphrase. Anything not backed by a URL is under Gaps.

## 1. workers-rs: Durable Object WebSocket Hibernation API

### Takeaway
Yes. workers-rs (crate `worker`, latest stable 0.8.7 on crates.io) exposes the full hibernation API in Rust: `State::accept_web_socket`, `get_websockets`, `set_websocket_auto_response`, and the `DurableObject` trait handlers `websocket_message` / `websocket_close` / `websocket_error` plus `alarm`. It was merged in Feb 2024; one memory-related hibernation PR is still open.

### Cited Findings
- `DurableObject` trait handlers in `worker/src/durable.rs` (main): `async fn alarm(&self) -> Result<Response>`; `async fn websocket_message(&self, ws: WebSocket, message: WebSocketIncomingMessage) -> Result<()>`; `async fn websocket_close(&self, ws: WebSocket, code: usize, reason: String, was_clean: bool) -> Result<()>`; `async fn websocket_error(&self, ws: WebSocket, error: Error) -> Result<()>`. Default bodies are `unimplemented!()`, so you override only what you need. — [durable.rs](https://raw.githubusercontent.com/cloudflare/workers-rs/main/worker/src/durable.rs)
- `State` methods in the same file: `accept_web_socket(&self, ws: &WebSocket)`, `accept_websocket_with_tags(&self, ws, tags: &[&str])`, `get_websockets() -> Vec<WebSocket>`, `get_websockets_with_tag(&str)`, `get_tags(&WebSocket)`, `set_websocket_auto_response(&self, pair: &worker_sys::WebSocketRequestResponsePair)`, `get_websocket_auto_response()`. — [durable.rs](https://raw.githubusercontent.com/cloudflare/workers-rs/main/worker/src/durable.rs)
- crates.io reports `max_stable_version` 0.8.7 for `worker`. — [crates.io API](https://crates.io/api/v1/crates/worker)
- PR #436 "Implement Hibernatable Web Sockets API" merged 2024-02-28; PR #495 "Add tests for Durable + Websocket + Hibernation API" closed; issue #722 "Memory leak in Rust Durable Object eviction" closed; PR #832 "Support eager memory deallocation on hibernation" still open (as of check). — [PR 436](https://github.com/cloudflare/workers-rs/pull/436), [PR 495](https://github.com/cloudflare/workers-rs/pull/495), [issue 722](https://github.com/cloudflare/workers-rs/issues/722), [PR 832](https://github.com/cloudflare/workers-rs/pull/832)
- workers-rs `examples/` directory has no websocket/hibernation example (list: abort-signal, ai-image, axum, coredump, custom-req, digest, emscripten*, fetcher, kv, queue, receive-email, router, rpc-*, send-email, signal, tcp-socket, tokio-postgres, tracing, udp-socket). Tests live in `test/tests/websocket.spec.ts` and `durable.spec.ts`. — [examples dir (GitHub API)](https://api.github.com/repos/cloudflare/workers-rs/contents/examples), [repo tree](https://github.com/cloudflare/workers-rs)
- Platform behaviour (applies to Rust too): the runtime re-runs the constructor after hibernation, so per-connection data goes in `serializeAttachment` (max 16,384 bytes); ping/pong auto-response does not wake the object. — [DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)

### Inferences
- The Rust API surface is complete enough to build a hibernating signaling room. Whether Rust-side attachment (`serialize_attachment`) is exposed was not verified by me; the `WebSocket` type in `worker` is a separate file I did not read.
- The open PR #832 and closed issue #722 suggest memory-on-eviction behaviour in Rust DOs needs testing under load.

### Gaps
- docs.rs fetch returned no parsable method list; I relied on the GitHub `main` source, not the 0.8.7 tag. Verify the 0.8.7 tag has the same signatures.
- Not checked: `serialize_attachment` / `deserialize_attachment` on `worker::WebSocket`.

## 2. Cloudflare Realtime SFU: simulcast, DataChannels, limits, recording, codecs, locations

### Takeaway
SFU supports simulcast, DataChannels, and H.264/H.265/VP8/VP9/AV1 + Opus/G.711. It has no room concept and no fixed track limit. It has no server-side recording; recording exists only in RealtimeKit. Docs do not state SFU locations or any EU pinning.

### Cited Findings
- Simulcast: publisher sends multiple encodings with RIDs via `addTransceiver()`; each subscriber picks a layer with a `simulcast` object (`preferredRid`, `priorityOrdering`, `ridNotAvailable`) on `POST .../tracks/new`, and switches via `PUT .../tracks/update`. (WebFetch paraphrase) — [Simulcast](https://developers.cloudflare.com/realtime/sfu/features/simulcast/) (the URL I fetched was `/realtime/sfu/simulcast/`; the llms.txt index lists the page at the /features/ path)
- DataChannels: named channels, one publisher to many subscribers, per-channel ordering/retransmit policy (added 2026-08-13), `canReply` for one subscriber to reply to the publisher (2026-07-23), `waitForAck` gate (2026-06-10). Reliable delivery is not durable storage. — [DataChannels](https://developers.cloudflare.com/realtime/sfu/features/datachannels/), [Changelog](https://developers.cloudflare.com/realtime/sfu/platform/changelog/)
- Limits: 50 API requests/s per session (not per app); up to 64 tracks per API request; tracks per session "No fixed upper bound. Endpoint and connection capacity impose practical limits"; 30 s media inactivity garbage-collects a track; 30 s session reuse window after connectivity loss. "Application limits such as room admission ... are separate policies." — [Limits](https://developers.cloudflare.com/realtime/sfu/platform/limits/) (last updated Oct 6, 2026)
- Codecs: Video H.264, H.265, VP8, VP9, AV1; Audio Opus, G.711 A-law, G.711 µ-law; "Endpoint support varies by WebRTC implementation." — [Limits](https://developers.cloudflare.com/realtime/sfu/platform/limits/)
- No rooms: "Rooms, participants ... are application concepts"; the video-room example uses "one Durable Object per room for membership and track discovery." (WebFetch quote of introduction page) — [SFU introduction](https://developers.cloudflare.com/realtime/sfu/introduction/), [Custom video room example](https://developers.cloudflare.com/realtime/sfu/examples/video-room/)
- Locations: "your application does not deploy SFU servers or select an SFU region." No location list, anycast statement, or EU statement on the SFU intro page. — [SFU introduction](https://developers.cloudflare.com/realtime/sfu/introduction/)
- Related fact: TURN "allocations are homed in the nearest available Cloudflare data center to the TURN client via anycast routing." This is TURN, not SFU. — [TURN FAQ](https://developers.cloudflare.com/realtime/turn/faq/)
- Recording: I found no SFU recording feature in the SFU doc index (pages: API, best practices, concepts, examples, DataChannels, media adapters, simulcast, limits, pricing). Recording exists in RealtimeKit (composite or separate participant audio tracks, stored in your R2 bucket). — [SFU llms.txt index](https://developers.cloudflare.com/realtime/llms.txt), [RealtimeKit overview](https://developers.cloudflare.com/realtime/realtimekit/)
- Media adapters: SFU "WebSocket adapter" is GA since 2026-09-30; "Any server that accepts WebSockets, such as a Durable Object, can receive live call audio as PCM samples and video as JPEG frames"; you can send PCM into the SFU. — [Changelog](https://developers.cloudflare.com/realtime/sfu/platform/changelog/)
- Orange Meets (now "Meet") README does not mention transcription, simulcast, DataChannels or room limits; it mentions an optional OpenAI Realtime "invite AI" integration. — [cloudflare/orange](https://github.com/cloudflare/orange)

### Inferences
- A DO per room (as the docs recommend) must track which sessions and track names each participant publishes; the SFU provides no participant list.
- With "no fixed upper bound", the practical participant count in one mesh-of-subscriptions is bounded by per-session API rate (50 req/s) and client download capacity; no official number exists.

### Gaps
- No documented participant-per-room figure; no documented SFU city list; no EU-only routing option for SFU.
- No documented SFU server-side recording. (Absence in the index, not an explicit "unsupported" statement.)
- The simulcast docs URL path differs between my fetch and the index listing; I did not re-verify which is canonical.

## 3. Realtime Agents and RealtimeKit

### Takeaway
RealtimeKit (Dyte-derived) is a managed SDK + REST product (UI kit, recording, transcription) with published per-minute pricing. "Realtime Agents" was announced Aug 2025 as an open-beta voice-AI runtime, free during beta; the current docs surface it as `@cloudflare/voice` in the Agents SDK (Beta). The route that fits a Rust app is the SFU WebSocket adapter, which is plain HTTP + WebSocket.

### Cited Findings
- Realtime Agents: blog of 2025-08-29: "During the open beta starting today, Cloudflare Realtime Agents runtime is free to use." No post-beta pricing stated. — [Cloudflare blog](https://blog.cloudflare.com/cloudflare-realtime-voice-ai/)
- Same post: SFU converts Opus audio to PCM and streams it to a WebSocket endpoint you name; the socket is bidirectional (data sent back can be published as a WebRTC track). — [Cloudflare blog](https://blog.cloudflare.com/cloudflare-realtime-voice-ai/)
- Current Agents voice docs: labelled "Beta"; package `@cloudflare/voice` (`withVoice`, `withVoiceInput`), STT models `WorkersAIFluxSTT` (@cf/deepgram/flux) and `WorkersAINova3STT` (@cf/deepgram/nova-3); transport is "Audio streams over WebSocket — no SFU or meeting infrastructure required." JS/TS packages. — [Agents voice](https://developers.cloudflare.com/agents/communication-channels/voice/)
- Whether the `@cloudflare/voice` package is the same product as the 2025 "Realtime Agents" is not stated in either source (a WebSearch summary linked them but I could not confirm it).
- RealtimeKit pricing (updated Sep 3, 2026): audio/video participant $0.002/min; audio-only $0.0005/min; export (recording, RTMP, HLS) $0.010/min, audio-only $0.003/min; raw RTP export to R2 $0.0005/min; real-time transcription "Standard model pricing via Workers AI". — [RealtimeKit pricing](https://developers.cloudflare.com/realtime/realtimekit/pricing/)
- RealtimeKit features: prebuilt UI Kit, Core SDK, REST API for meetings/participants/tokens, recording to your R2, real-time and post-meeting transcription on Workers AI, AI summary. GA status is not stated on the page. — [RealtimeKit overview](https://developers.cloudflare.com/realtime/realtimekit/)
- Pulling audio out of an SFU session: documented, official, plain-HTTP. `POST https://rtc.live.cloudflare.com/v1/apps/{appId}/adapters/websocket/new` with `Authorization: Bearer <APP_SECRET>`; 1-4 track entries; ingest (`location: "local"`) takes 48 kHz stereo PCM (s16le, interleaved) published as Opus; egress (`"remote"`) streams Opus as 48 kHz stereo PCM or video as JPEG (default 1 fps; H.264/H.265/VP8/VP9 input). One direction per adapter. Each message is a protobuf `Packet` (`sequenceNumber`, `timestamp`, `payload`), at most 32 KB incl. overhead. Endpoint must be public `wss://`, no redirects, no custom headers (put auth in URL). Egress retries 15 s; ingest does not reconnect. — [WebSocket adapter](https://developers.cloudflare.com/realtime/sfu/media-transport-adapters/websocket-adapter/) (llms.txt lists it under `/features/media-transport-adapters/`)
- Official end-to-end example "ai-tts-stt": browser mic -> SFU -> WebSocket adapter -> STT Durable Object -> Workers AI nova-3 -> transcript WebSocket to browsers; marked "experimental", with unauthenticated endpoints. Resamples 48 kHz stereo to 16 kHz mono (SpeexDSP WASM) before sending to Nova. — [AI audio pipelines](https://developers.cloudflare.com/realtime/sfu/examples/ai-audio/), [stt-adapter.ts](https://raw.githubusercontent.com/cloudflare/realtime-examples/main/ai-tts-stt/src/stt-adapter.ts), [config.ts](https://raw.githubusercontent.com/cloudflare/realtime-examples/main/ai-tts-stt/src/shared/config.ts)

### Inferences
- Rust is feasible without any TS SDK: SFU adapter creation is a REST call, adapter transport is WebSocket + protobuf, STT is a WebSocket (see section 4). A Rust DO needs a protobuf decoder (e.g. prost) for `Packet` and a resampler (48 kHz stereo to 16 kHz mono) in WASM.
- For the user's goal, RealtimeKit would replace the custom SFU+signaling layer entirely but ties the app to its TS/JS client SDKs and per-minute billing.

### Gaps
- RealtimeKit GA status and data location: not stated in pages I read.
- Realtime Agents post-beta pricing: not published in sources found.
- Protobuf schema (`packet.proto`) exists at `ai-tts-stt/src/packet.proto` in the example repo; I did not read it.

## 4. Workers AI streaming STT (nova-3, flux) over WebSocket

### Takeaway
Both models stream over WebSocket. A DO opens it with an outbound `fetch()` carrying `Upgrade: websocket`; this works and is demonstrated in Cloudflare's own example. Nova-3 supports German. The DO can hold the socket across a meeting only with keep-alive and reconnect logic, because outgoing WebSockets do not hibernate and the model side may close idle connections.

### Cited Findings
- Connection flow (official example): `fetch("https://api.cloudflare.com/client/v4/accounts/{CF_ACCOUNT}/ai/run/@cf/deepgram/nova-3?encoding=linear16&sample_rate=16000", {headers: {Upgrade: "websocket", Authorization: "Bearer <CF_API_TOKEN>"}})`, then `resp.webSocket.accept()`. — [stt-adapter.ts](https://raw.githubusercontent.com/cloudflare/realtime-examples/main/ai-tts-stt/src/stt-adapter.ts)
- Alternative via AI Gateway: `wss://gateway.ai.cloudflare.com/v1/<account_id>/<gateway>/workers-ai?model=@cf/deepgram/nova-3&encoding=linear16&sample_rate=16000&interim_results=true`, header `cf-aig-authorization`, raw binary frames, JSON responses with `channel.alternatives[0].transcript` and `is_final`. — [AI Gateway realtime API](https://developers.cloudflare.com/ai-gateway/usage/websockets-api/realtime-api/)
- Nova-3 params (WebSocket-only): `interim_results`, `vad_events`, `utterance_end_ms`, `endpointing`; also `encoding` (linear16, flac, mulaw, opus, ...), `channels`, `language` (BCP-47), `diarize`, `punctuate`, `smart_format`, `keyterm`, `detect_language`. — [nova-3 model page](https://developers.cloudflare.com/workers-ai/models/nova-3/)
- Nova-3 price: WebSocket $0.0092 per audio minute (836.36 neurons/min); HTTP $0.0052 per audio minute. — [nova-3 model page](https://developers.cloudflare.com/workers-ai/models/nova-3/)
- Flux: "Currently only supports raw signed little-endian 16-bit PCM"; sample rate is a numeric Hz string; events Update, StartOfTurn, EagerEndOfTurn, TurnResumed, EndOfTurn; `eot_threshold` default 0.7, `eot_timeout_ms` default 5000; price $0.0077 per audio minute (WebSocket). No language list on the page. — [flux model page](https://developers.cloudflare.com/workers-ai/models/flux/)
- German: Cloudflare's nova-3 page lists no languages. Deepgram's language overview lists German `de` and `de-CH` for Nova-3 and German within the `multi` code-switching set (WebFetch paraphrase; it did not distinguish streaming vs batch). — [Deepgram models and languages](https://developers.deepgram.com/docs/models-languages-overview)
- Chunking in the official example: batches of 3,200 to 16,000 bytes (about 100 to 500 ms of 16 kHz mono 16-bit); KeepAlive message every 5 s before audio flows; reconnect with backoff via DO alarms; 2 MB send-queue cap. — [config.ts](https://raw.githubusercontent.com/cloudflare/realtime-examples/main/ai-tts-stt/src/shared/config.ts)
- DO behaviour: "Outgoing WebSockets do not hibernate"; an open outbound connection keeps the object from being evicted for up to 15 minutes, and the connection itself may stay open after that. DO limit "Simultaneous outgoing connections/request: 6". — [DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/), [DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/)
- Workers AI ASR limit listed: "720 requests per minute" (unclear whether it applies to streaming). — [Workers AI limits](https://developers.cloudflare.com/workers-ai/platform/limits/)

### Inferences
- Cost at ~$0.0092/min/speaker-stream: a 10-person, 60-minute meeting transcribing everyone separately is about $5.50. Per-speaker streams (one adapter per participant track) are the natural design; this is arithmetic on the cited price.
- The example's hybrid (hibernating server sockets from the SFU + non-hibernating outbound Nova socket) means the DO stays resident during active transcription, so DO duration charges apply for the meeting length.
- Whether an outgoing socket survives a DO eviction after 15 min: docs say the connection "may" stay open, not guaranteed; plan reconnect.

### Gaps
- The exact JSON response schema of Workers AI nova-3 and flux over WebSocket is not documented in pages I read beyond `is_final`/`transcript`.
- Whether Workers AI WebSocket STT has a max session duration or concurrency cap: not found.
- Flux language support (likely English-only) is not stated in Cloudflare docs; unverified.

## 5. Data residency and GDPR

### Takeaway
DO `jurisdiction: "eu"` is documented and pins where the object runs and stores data. Workers AI and Realtime SFU pages state no regional guarantee. Cloudflare states general GDPR compliance (EU Cloud CoC verified).

### Cited Findings
- DO jurisdictions `eu`, `us`, `fedramp`: use `env.NS.jurisdiction("eu").newUniqueId()` (recommended over passing the option to `newUniqueId()`). The constraint "only controls where the Durable Object itself runs and persists data"; Workers can reach it from anywhere; "A DurableObjectId will be logged outside of the specified jurisdiction for billing and debugging purposes." Jurisdiction constraints work only with ids from `unique_id()` (workers-rs source comment, not `id_from_name`). — [DO data location](https://developers.cloudflare.com/durable-objects/reference/data-location/), [durable.rs](https://raw.githubusercontent.com/cloudflare/workers-rs/main/worker/src/durable.rs)
- Workers AI data usage: customer content is not used to train models or improve services without explicit consent; not made available to other customers. The page says nothing on data location or GDPR. — [Workers AI data usage](https://developers.cloudflare.com/workers-ai/platform/data-usage/)
- Cloudflare data-localization FAQ: "Cloudflare services are designed to satisfy the requirements of the GDPR ... verified compliant with the EU Cloud Code of Conduct (EU Cloud CoC), Verification-ID: 2023LVL02SCOPE4316." — [Data localization FAQ](https://developers.cloudflare.com/data-localization/faq/)
- The data-localization index lists a Durable Objects how-to (Regional Services, Customer Metadata Boundary); I saw no Realtime or Workers AI entries in the lines I grepped. — [data-localization llms.txt](https://developers.cloudflare.com/data-localization/llms.txt)
- SFU: "your application does not deploy SFU servers or select an SFU region." — [SFU introduction](https://developers.cloudflare.com/realtime/sfu/introduction/)

### Inferences
- Signaling state and chat can be EU-pinned via DO jurisdiction, but media (SFU) and STT audio (Workers AI) have no documented EU pinning. A strict "EU-only" claim is not supportable from these docs.

### Gaps
- DPA / subprocessor specifics for Realtime and Workers AI: not found in official docs. Needs Cloudflare's DPA page or sales confirmation.
- WebSocket connections into a jurisdiction-restricted DO: page does not address where the connection terminates.

## 6. Durable Object limits and plan differences

### Takeaway
No hard per-DO WebSocket connection cap is documented. Messages up to 32 MiB received, 30 s CPU per invocation (configurable to 5 min). Alarms exist. DOs are available on the Free plan (SQLite-backed only).

### Cited Findings
- WebSocket message size: 32 MiB, received messages only. CPU: 30 s per invocation (HTTP request, WebSocket message or alarm), each incoming message resets it, raisable via `limits.cpu_ms` up to 300,000 ms. — [DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/)
- Connections per DO: not limited on the limits page; the WebSocket guide says a DO can "connect thousands of clients per instance." (WebFetch paraphrase) — [DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)
- Alarms: "Alarm handler invocations have a maximum wall time of 15 minutes"; no documented alarm-count limit. The official STT example uses one alarm for keepalive, reconnect, inactivity and cleanup. — [DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/), [stt-adapter.ts](https://raw.githubusercontent.com/cloudflare/realtime-examples/main/ai-tts-stt/src/stt-adapter.ts)
- Free plan: only SQLite-backed DOs; 100 classes (Paid 500); 5 GB total storage on Free. Free: 100,000 requests/day, 13,000 GB-s/day. Paid: 1M requests/month and 400,000 GB-s/month included, then $0.15/M requests and $12.50/M GB-s. — [DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/), [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- Billing: incoming WebSocket messages count 20:1 toward requests; outgoing messages are free; idle hibernatable DOs "are not billed for duration"; `accept()` without the Hibernation API bills duration while connected. — [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- Hibernation attachments limited to 16,384 bytes per socket. — [DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)

### Inferences
- Free-plan 100,000 requests/day equals 2 M WebSocket messages/day at 20:1, which a small meeting app would exhaust quickly (chat, presence, cursor events). Realtime SFU egress and Workers AI would still need an account; the earlier-established Paid plan is the sensible baseline.
- For a signaling DO, the per-DO connection ceiling is likely governed by the 128 MB DO memory limit; I did not verify that figure.

### Gaps
- No explicit max concurrent WebSocket connections per DO in current docs I fetched (the older ~32,768 figure circulates; unverified and not found in the current limits page).
- Free-plan per-object storage: the limits page contradicts itself (a "1 GB on the Free plan" mention vs a 5 GB account total).
