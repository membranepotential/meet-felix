# Hosting and media platforms for a serverless Rust video meeting app (as of 2026-10-09)

Notes written 2026-10-09. Every figure is from the cited page as fetched on that date unless marked "inference" or "unverified". Fetching was done through a summarizing tool, so exact wording of some pages was not seen; figures should be re-checked on the page before a purchase decision.

## 1. Cloudflare Realtime SFU and TURN (media path)

### Takeaway
Cloudflare Realtime SFU is a managed SFU at $0.05/GB egress with 1,000 GB/month free (shared with TURN). Clients speak WebRTC to Cloudflare; your own backend (a Worker + Durable Object works) only does signaling and calls a plain HTTPS API with a bearer secret. No Rust-specific SDK was found, the control API is plain HTTP/JSON.

### Cited Findings
- Product is named "Realtime SFU"; it "forwards media tracks and DataChannels between endpoints". The application supplies signaling, state and permissions; there is no room-presence protocol in the SFU, so discovery is the app's job — [Intro](https://developers.cloudflare.com/realtime/sfu/introduction/), [SFU overview page](https://developers.cloudflare.com/realtime/sfu/calls-vs-sfus/)
- Clients do not signal directly to the SFU. They exchange signaling with your backend, which calls the SFU API. Workers is one supported backend; containers or ordinary servers work the same way — [Intro](https://developers.cloudflare.com/realtime/sfu/introduction/)
- Session model: one PeerConnection and one negotiation state machine per session. Remote tracks are identified by publisher session ID + track name. An endpoint can use one bidirectional session or separate publish/subscribe sessions. No SFU region to choose; Cloudflare runs the infrastructure — [Overview](https://developers.cloudflare.com/realtime/sfu/calls-vs-sfus/)
- HTTP API: base `https://rtc.live.cloudflare.com/v1`, `Authorization: Bearer <APP_SECRET>` (keep secret on backend). Endpoints: `POST /apps/{appId}/sessions/new`; `POST .../sessions/{id}/tracks/new` (all entries `local` = publish, or all `remote` = subscribe, per request); `PUT .../renegotiate` (SDP answer); `PUT .../tracks/update`; `PUT .../tracks/close`; `GET .../sessions/{id}`; DataChannel endpoints (`datachannels/establish|new|update|close`); WebSocket adapter `POST /apps/{appId}/adapters/websocket/new|close` — [HTTPS API](https://developers.cloudflare.com/realtime/sfu/https-api/)
- Simulcast: publisher declares RIDs in the SDP; the SFU picks the layer per subscriber from network conditions and device; "handled automatically based on the SDP" — (search-result summary of) [Simulcast docs](https://developers.cloudflare.com/realtime/sfu/simulcast/index.md)
- Recording: no built-in recorder was found in the docs I read; the example-architecture page mentions headless clients that subscribe and record (you build it) — (search-result summary of) [Example architecture](https://developers.cloudflare.com/calls/example-architecture/). The WebSocket adapter (media to/from external services) is documented, [HTTPS API](https://developers.cloudflare.com/realtime/sfu/https-api/)
- Limits: 50 API requests/s per session; up to 64 tracks per request; no fixed upper bound on tracks per session ("practical limits"); track with no packets for 30 s is garbage-collected; codecs H.264, H.265, VP8, VP9, AV1; audio Opus, G.711. Screen share is just another track (no special limit documented) — [Limits](https://developers.cloudflare.com/realtime/sfu/limits/)
- Pricing: $0.05 per GB egress (Cloudflare to client only; publishing is free); first 1,000 GB/month free, shared between SFU and TURN; one billing line; page last updated 2026-09-22 — [SFU pricing](https://developers.cloudflare.com/realtime/sfu/pricing/)
- TURN: $0.05/GB, 1,000 GB free (same pool), STUN at `stun.cloudflare.com` free; traffic between TURN and SFU not charged; relay addresses IPv4 only; TCP relaying (RFC 6062) not supported; credentials valid up to 48 h; anycast, allocation homed at nearest data center; throttling above ~50-100 Mbps per client — [TURN FAQ](https://developers.cloudflare.com/realtime/turn/faq/)
- Regions: pricing page does not mention regions; docs say you cannot choose an SFU region — [Pricing](https://developers.cloudflare.com/realtime/sfu/pricing/), [Overview](https://developers.cloudflare.com/realtime/sfu/calls-vs-sfus/)

### Inferences
- Idle cost is 0; cost is pure egress. With 1,000 GB free a small hobby/team app likely never pays for media (about 150 4-person meeting-hours, see section 6).
- Because the API is plain HTTPS+JSON, Rust integration is `reqwest`/`worker::Fetch` calls plus a browser-side JS WebRTC client; no Rust SDK is needed (none found).
- Server-side recording needs your own headless subscriber (e.g. a Rust WebRTC client on a container) or the WebSocket adapter.

### Gaps
- Max participants per session/room: not documented (rooms are an app concept; only track/rate limits found).
- Whether Cloudflare's example repo has any Rust example: not confirmed.
- EU-only media processing guarantee: not found; anycast routing means the nearest data center, no regional pin documented.

## 2. Workers, Durable Objects and workers-rs

### Takeaway
workers-rs is at `worker` 0.8.7 (2026-09-25) with `#[durable_object]` and SQLite-backed storage. Hibernation support in Rust could not be verified from fetched pages. DOs with hibernated WebSockets cost no duration while idle; incoming messages bill at 20:1.

### Cited Findings
- `worker` crate latest version 0.8.7, released 2026-09-25 — [docs.rs](https://docs.rs/crate/worker/latest)
- Durable Objects in Rust via `#[durable_object]` and `DurableObject` trait, SQLite via `new_sqlite_classes` migration and `state.storage().sql()`; README warns of "rough edges, some unimplemented APIs"; RPC support experimental; Queues beta, D1 alpha; Tokio/async_std not supported; `http` feature for axum types — [workers-rs README](https://github.com/cloudflare/workers-rs), [docs.rs](https://docs.rs/crate/worker/latest)
- Workers Paid: $5/month minimum per account; 10 M requests included then $0.30/M; 30 M CPU-ms included then $0.02/M CPU-ms; no duration charge; no egress charge. Free: 100,000 requests/day, 10 ms CPU per invocation — [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/)
- Durable Objects (paid): 1 M requests/month included, then $0.15/M; duration 400,000 GB-s included then $12.50/M GB-s, billed on 128 MB wall-clock (0.128 GB-s per s); SQLite: 25 B rows read and 50 M rows written included then $0.001/M reads and $1.00/M writes; 5 GB-month stored included then $0.20/GB-month; SQLite billing from 2026-01-07. Free plan: 100,000 requests/day, 13,000 GB-s/day — [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- WebSocket billing: connection = 1 request; incoming messages billed 20:1 (100 messages = 5 requests); outgoing messages and protocol pings free; auto-response (`setWebSocketAutoResponse`) costs no duration; "Idle objects that qualify for hibernation are not billed for duration"; non-hibernating `accept()` bills duration for the whole connection (example: 100 always-on objects ≈ $419/month vs ≈ $20.65/month with hibernation) — [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- Hibernation semantics (JS API): object evicted from memory while sockets stay open; constructor rerun on next message; per-socket state via serialize/deserialize attachment — [WebSockets docs](https://developers.cloudflare.com/durable-objects/best-practices/websockets)

### Inferences
- A hibernating signaling room costs $0 duration when idle; the only costs are the $5/month Workers Paid minimum plus message requests, which are tiny for signaling (a 10-person meeting hour with ~1 msg/s each is ~36 k messages = ~1.8 k billed requests).
- Idle total ≈ $5/month (Workers Paid minimum, which also covers DOs).

### Gaps
- workers-rs support for the WebSocket Hibernation API (`accept_web_socket`, `websocket_message` handlers on the `DurableObject` trait): README fetch did not mention it; docs.rs trait page returned 404; search found only JS docs. Unverified — check docs.rs `worker::State` and `worker::DurableObject` and the repo's `examples/` before committing.
- Fallback if unsupported: write the DO shell in JS/TS and keep Rust as Wasm logic, or use a non-hibernating DO (duration billed while sockets open, about $0.0166/h per always-awake DO by inference: 0.128 GB-s x 3600 x $12.5/M = $0.0058; recompute before use).

## 3. Scale-to-zero container hosting for a self-run Rust SFU

### Takeaway
Only Fly.io Machines (UDP with dedicated IPv4) and API-started VMs (Hetzner, AWS) are realistic for a self-run UDP SFU. Cloud Run, Railway are not (no inbound UDP found). Hetzner is no longer cheap or scale-to-zero friendly in 2026.

### Cited Findings
- Fly.io: Machines billed per second while running; shared-cpu-2x (2 GB) $13.39/month in iad/ewr at continuous runtime (about $0.0186/h); performance-2x (4 GB) $66.00/month; stopped machine rootfs $0.15/GB/month; dedicated IPv4 $2/month (shared IPv4 and IPv6 free); egress in Europe $0.02/GB, inbound free; volumes $0.15/GB-month — [Fly pricing](https://docs.fly.io/about/pricing)
- Fly UDP: needs a dedicated IPv4 (UDP does not work over shared IPv4 or public IPv6); bind UDP socket to `fly-global-services`; external and internal port must match; plan for ~1300-byte MTU — [Fly UDP/TCP](https://docs.fly.io/networking/udp-and-tcp)
- Fly autostop/autostart: `auto_stop_machines = "stop"|"suspend"`; the proxy starts a stopped machine when a request arrives; checks excess capacity every few minutes; only works on existing Machines; page gives no cold-start duration and does not say if it works for UDP; stopped/suspended machines have no CPU/RAM charge — [Fly autostop](https://docs.fly.io/reference/fly-proxy-autostop-autostart)
- Cloud Run: container contract documents HTTP/1, h2c and gRPC only; no mention of UDP — [Cloud Run container contract](https://docs.cloud.google.com/run/docs/container-contract). Direct VPC egress is outbound-only; instance throughput cap 1 Gbps; connection setup can lag by a minute at startup — search summary of [Direct VPC egress docs](https://docs.cloud.google.com/run/docs/configuring/vpc-direct-vpc)
- Railway: TCP proxy only; staff reply (forum) says inbound UDP not supported; request "Under Review" — [Railway TCP proxy](https://docs.railway.com/networking/tcp-proxy), [forum](https://station.railway.com/feedback/adding-inbound-udp-fad19847)
- Hetzner Cloud: billed "until you delete them, independent of their state" (powering off does not stop billing); hourly billing capped at monthly price, partial hours rounded up to a full hour; only outgoing traffic billed, 100 MB blocks; Primary IPv4 billed monthly separately — [Hetzner billing FAQ](https://docs.hetzner.com/cloud/billing/faq/). Price rises 2026-04-01 and 2026-06-15 (new orders/rescales): CX23 EUR 3.99 to 5.49/month, CPX22 7.99 to ~19.49, CCX13 15.99 to 42.99; CX/CAX listed "not available" and provisioning limited since 2026-06-26 per one vendor blog (self-interested, as of 2026-10-01) — [WZ-IT](https://wz-it.com/en/blog/hetzner-price-increase-june-2026-cpx-ccx-alternatives/)
- AWS Fargate: billed per second from image pull to termination; Frankfurt rate not found; US-East 1 vCPU Linux/x86 $0.04048/h (third-party) — [Vantage](https://www.vantage.sh/blog/fargate-pricing). UDP needs an NLB (hourly + NLCU charges), third-party figures only — [Vantage ELB](https://handbook.vantage.sh/aws/services/elb-pricing/)

### Inferences
- Fly cost for a 2-vCPU 1 h meeting: about $0.02 compute (shared-cpu-2x, iad pricing; EU region price not verified) + egress (see section 6) + $2/month IPv4 + ~$0.30/month rootfs for a 2 GB image. Idle ~$2-3/month.
- Hetzner: a created-then-deleted server per meeting is billed one full hour, but boot + provisioning time (tens of seconds to minutes) and 2026 capacity limits make it a poor on-demand fit; idle keep-alive costs full monthly price.
- Fly's proxy autostart may not wake on UDP; a Rust control plane (Worker/DO) should instead call the Fly Machines API to start the machine before clients get the ICE address. Unverified.
- Render, App Runner: not researched (budget); treat as unverified/unlikely (HTTP-only model).

### Gaps
- Cold-start times for Fly (stop vs suspend) not in docs; measure.
- Cloud Run inbound UDP: no explicit statement found either way; absence from the contract suggests no.
- Fargate Frankfurt price, Render and App Runner UDP: not retrieved.
- Exact current Hetzner hourly prices and traffic overage per TB: not retrieved.

## 4. Managed WebRTC alternatives

### Takeaway
LiveKit Cloud, Daily and 100ms charge about $0.004/participant-minute at list (Daily, 100ms) or much less on LiveKit's plan tiers; LiveKit is the only one that is also open-source self-hostable and has Rust SDKs.

### Cited Findings
- LiveKit Cloud (page lists plans; column mapping partly inferred by the fetch tool): Build $0/month with 5,000 WebRTC minutes, 50 GB downstream, 100 concurrent connections; Ship $50/month with 150,000 minutes then $0.0005/min, 250 GB then $0.12/GB, 1,000 connections; Scale $500/month with 1.5 M minutes then $0.0004/min, 3 TB then $0.10/GB. "Region pinning" for Scale and up — [LiveKit pricing](https://livekit.com/pricing)
- Daily: $0.004 per participant-minute list (down to $0.0015 at 50 M+), 10,000 free min/month, recording $0.01349/min, RTMP $0.015/min; EU hosting not stated on page — [Daily pricing](https://www.daily.co/pricing/video-sdk/)
- 100ms: $0.004/participant-minute after 10,000 free; recording $0.0135/min — [100ms pricing](https://www.100ms.live/pricing)
- Agora: from $0.59 per 1,000 minutes; first 10,000 min free; resolution-specific rates not retrieved — [Agora pricing](https://www.agora.io/en/pricing/)
- Twilio Video: the planned 2026-12-05 end-of-life was reversed in a changelog dated 2024-10-21; remains a standalone product (no newer notice found) — [Twilio changelog](https://www.twilio.com/en-us/changelog/-twilio-video-will-remain-a-standalone-product)

### Inferences
- LiveKit is the best "self-hosted enough" option: same server can run on Fly/VM later; Cloud avoids ops now. Rust SDKs exist (livekit/rust-sdks) — not fetched this session, label unverified.
- Daily/100ms at list cost: 4-person hour = 240 pmin = $0.96; 10-person hour = 600 pmin = $2.40.

### Gaps
- Twilio Video current per-minute price, Agora video HD rates, LiveKit EU region list, livekit rust-sdks maturity/license: not retrieved.

## 5. Platform constraints (edge/serverless)

### Cited Findings
- Workers/DOs speak HTTP/WebSocket only; Cloudflare's own design has the SFU as separate product and Workers as the signaling backend — [Intro](https://developers.cloudflare.com/realtime/sfu/introduction/)
- Cloud Run documents only HTTP protocols — [container contract](https://docs.cloud.google.com/run/docs/container-contract)
- Fly requires dedicated IPv4 and `fly-global-services` bind for UDP — [Fly UDP](https://docs.fly.io/networking/udp-and-tcp)
- Railway no inbound UDP — [forum](https://station.railway.com/feedback/adding-inbound-udp-fad19847)

### Inferences
- "Workers cannot do UDP" is consistent with all docs read, but I found no page stating it explicitly (unverified wording). Architecture: DO = signaling and room state, Cloudflare SFU/TURN = media, no UDP in Rust code at all.
- workers-rs has no Tokio, so a Rust SFU (str0m, webrtc-rs) cannot run in Workers; it must run in a container/VM.

## 6. Bandwidth and cost per meeting-hour

### Takeaway
SFU egress dominates: roughly 7 GB per 4-person hour and 30-60 GB per 10-person hour (my estimate). At Cloudflare $0.05/GB, 4-person ≈ $0.33/h, 10-person ≈ $1.6-3.0/h, zero while inside the 1,000 GB monthly free pool.

### Cited Findings
- None for per-participant bitrates; no authoritative source retrieved this session. Rates below are my planning assumptions: 720p simulcast high layer about 1.2-1.5 Mbps, low layers 0.3-0.7 Mbps, audio ~40 kbps, screen share ~1-2 Mbps.

### Inferences (assumptions: a viewer receives each other participant's video; one screen share adds one stream per viewer)
- 4 people, each receives 3 videos at ~1.2 Mbps + audio ≈ 3.7 Mbps, total egress ≈ 15 Mbps ≈ 6.6 GB/h. Cloudflare: $0.33/h (free if under 1,000 GB/month). Fly egress $0.02/GB: $0.13/h plus ~$0.02 compute. Daily/100ms: $0.96/h. LiveKit Ship overage: 240 min x $0.0005 = $0.12 + GB $0.12 x 6.6 = $0.79 (beyond included).
- 10 people, each receives 9 videos at 0.8-1.5 Mbps ≈ 7-14 Mbps, total ≈ 70-135 Mbps ≈ 32-61 GB/h. Cloudflare: $1.6-3.0/h. Fly: $0.65-1.2/h + compute (2 vCPU likely sufficient for forwarding only; unverified). Daily/100ms: $2.40/h.
- Last-N / active-speaker forwarding and lower layers can cut 10-person egress by half or more. Add ~1.5 Mbps x viewers per active screen share.
- Egress at Cloudflare TURN-only usage (users behind strict NAT) is billed the same $0.05/GB.

### Gaps
- No measured bitrates from a source; verify with real traffic.
