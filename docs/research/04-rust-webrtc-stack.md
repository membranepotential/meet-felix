# Rust WebRTC backend stack for a small self-hosted meeting app (state as of 2026-10-09)

Method note: version/date facts come from the crates.io API and GitHub API/raw files fetched on 2026-10-09; README claims are fetched from GitHub. Items marked UNVERIFIED are from memory or inference only. Searches for Lookback/Algesten blog posts, HN threads and "who uses str0m in production" returned nothing usable (see Gaps).

## str0m: version, design, features, limitations, users

### Takeaway
str0m is the most actively developed sans-IO WebRTC crate (0.24.1 on 2026-10-03, commits on the day of research), with simulcast, NACK, TWCC, BWE, data channels and ICE lite, but no TURN client, no jitter buffer and no PeerConnection API. It is pre-1.0 with frequent breaking minor releases.

### Cited Findings
- Latest crates.io version 0.24.1 (2026-10-03); 0.24.0 2026-09-25; 0.23.x Aug 2026; 0.22.0 2026-08-08; 0.21.0 2026-06-27; 0.20.0 2026-05-29; 0.19.0 2026-05-04 (roughly one minor release per 2-6 weeks). 2.45M total downloads, ~1.02M recent. Created 2023-03-17. — [crates.io API](https://crates.io/api/v1/crates/str0m)
- Last push 2026-10-09; 634 stars; not archived; 37 open issues; license MIT OR Apache-2.0 (GitHub UI shows MIT; version records say dual). — [GitHub API](https://api.github.com/repos/algesten/str0m), [crates.io](https://crates.io/api/v1/crates/str0m)
- Sans-IO: "`Rtc` instance itself is not doing any network talking", no internal threads or async tasks, never calls `Instant::now()` itself; time is an input, so tests can run faster than realtime. No `Rc/Mutex/mpsc/Arc` internally; all input is `&mut self`. — [README](https://github.com/algesten/str0m)
- API shape: inputs = user operations (write media, data), network packets (`Input::Receive`), timeouts (`Input::Timeout`); outputs = `Output::Event`, `Output::Transmit`, `Output::Timeout`. Run-loop contract ("single-mutation invariant"): after every mutation drain `poll_output` until `Output::Timeout` before the next mutation. — [README run loop](https://github.com/algesten/str0m)
- Feature table (README FAQ): SDP, ICE, data channels, send/recv reports, transport-wide CC, bandwidth estimation, simulcast, NACK, packetize, fixed depacketize buffer all supported. NOT supported: Peer Connection API, adaptive jitter buffer, capture/encode/decode/render, TURN, NIC enumeration. — [README FAQ](https://github.com/algesten/str0m)
- ICE lite: `RtcConfig::set_ice_lite(true)` exists in `src/lib.rs`; test `tests/ice-lite-connectivity.rs`; changelog 0.24.0 "Let `Rtc::accepts` recognize validated remote addresses in ICE-lite mode" — checked in a shallow clone of the repo (HEAD 2026-10-09) and [CHANGELOG](https://github.com/algesten/str0m/blob/main/CHANGELOG.md). ICE lite fits a server with a public IP.
- RTX: source has RTX/resend handling (`p.resend()` in `src/media/mod.rs`, test `tests/rtx-cache-0.rs`, `src/streams/register_nack.rs`); the README table lists only NACK. Treat RTX as supported but not documented in the feature table. — shallow clone of repo
- Other 0.20-0.24 changelog items: RFC 2198 RED for audio (breaking, 0.24), DTMF, G.722, comfort noise, AV1/VP9 fixes, configurable RTCP intervals, configurable video reordering timeout, SCTP send buffer limit, paced-burst limit (~63 kB), `Rtc` shutdown normalization. — [CHANGELOG](https://github.com/algesten/str0m/blob/main/CHANGELOG.md)
- DTLS: own `dimpl` crate (0.7.5, 2026-10-03) plus crypto backends: `aws-lc-rs` (default), `rust-crypto`, `openssl` (DTLS 1.2 only), `openssl-dimpl`, `apple-crypto`, `wincrypto`. SCTP via `sctp-proto` (algesten, 0.10.5, 2026-09-18). — [README crypto section](https://github.com/algesten/str0m), [crates.io dimpl](https://crates.io/api/v1/crates/dimpl), [sctp-proto](https://crates.io/api/v1/crates/sctp-proto)
- Two API levels: frame level (default; `Event::MediaData` full audio/video frames, `Writer::write`) and RTP mode (`set_rtp_mode(true)`; `Event::RtpPacket`, `StreamTx::write_rtp`, `StreamRx::request_keyframe`) for forwarding packets in an SFU. — [README "Frame or RTP level?"](https://github.com/algesten/str0m)
- Codecs listed in changelog/README: Opus, VP8, VP9, H.264, AV1, G.722; negotiated via SDP. SDP is created/consumed with `rtc.sdp_api()` (`accept_offer`, `add_media`, `apply`, `accept_answer`); the author-facing API is deliberately not the W3C PeerConnection API. — [README](https://github.com/algesten/str0m)
- TURN/STUN client and NIC enumeration are out of scope by design: the app obtains sockets (local or TURN-relayed) and registers them as candidates; str0m only runs the ICE agent. — [README "NIC enumeration and TURN"](https://github.com/algesten/str0m)
- Project status per README: developed by Martin Algesten of Lookback "for a specific use case: str0m as a server SFU"; p2p is less tested; "Performance is very good"; chat example "not intended for production use". Panics are used for broken internal invariants; README recommends `catch_unwind` per `Rtc` to isolate a faulty peer. — [README "Project status"/"Panics"](https://github.com/algesten/str0m)
- Platforms tested: Linux x86_64/aarch64, macOS, Windows. Examples: `chat` (SFU, one UDP socket for all traffic, two threads), `http-post` (receive media from browser). Named real-world user: BitWHIP (CLI WHIP agent). — [README](https://github.com/algesten/str0m)
- Test suite in repo is large and in-process: tests for nack, twcc, simulcast-rid-mapping, ice-restart, ice-lite, loss, network-conditions, dtls-retransmit, malformed-packet-injection, srtp-replay-attack, and a `tests/bwe/` group (delay, loss, probes, recovery, ALR). `tests/common.rs` has `progress(l, r)` and `negotiate()` helpers that wire two `Rtc` instances together with virtual time. — shallow clone of repo (tests dir)

### Inferences
- Mapping to functional core / imperative shell: str0m `Rtc` is already the "core" per peer (state + `handle_input` + `poll_output` + deadline). The shell is a loop that owns the UDP socket(s), a clock, and a channel from the signaling layer. Per-room routing (which peer's `Event::MediaData`/`RtpPacket` goes to which other peers' `Writer`) is app logic that can be written as a pure function `(RoomState, Event) -> (RoomState, Vec<Command>)`. Caveat: `Rtc` is mutated in place (`&mut self`), so it is "functional" only in the sans-IO sense (deterministic given inputs), not immutable; wrap it as an opaque state value.
- The "single-mutation invariant" means the shell must drain each `Rtc` after every command; model this as a `step(rtc, input) -> Vec<Output>` function that always drains.
- RTP mode is the better fit for a forwarding SFU (no depacketize/repacketize); frame mode is simpler and is what an audio-transcription tap would use.
- For 2-10 participants a single-threaded loop with one UDP socket (like the `chat` example) is enough.

### Gaps
- No primary source found for named production users beyond Lookback (author's employer) and BitWHIP; Lookback's own engineering posts were not found by search.
- Exact last-release RTX documentation not found; RTX support inferred from source.
- Whether the str0m `chat` example handles simulcast/BWE-driven layer selection was not inspected.

## webrtc-rs (webrtc, rtc crates)

### Takeaway
Active again: `webrtc` 0.21.0 released 2026-09-19 as a thin async layer over a new sans-IO `rtc` core (same org), after a long quiet period on the older 0.17 line (my recollection, UNVERIFIED). Pre-1.0 with breaking minors; simulcast is only evidenced by an example.

### Cited Findings
- `webrtc` crate 0.21.0 (2026-09-19), many alpha/beta/rc pre-releases Aug-Sep 2026, 0.20.x series Aug-Sep 2026; 7.76M total downloads, 2.77M recent. — [crates.io](https://crates.io/api/v1/crates/webrtc)
- GitHub: pushed 2026-10-03, 5,160 stars, Apache-2.0 per GitHub (README says dual MIT/Apache-2.0), only 7 open issues, not archived. Release v0.21.0 on 2026-09-19. — [GitHub API](https://api.github.com/repos/webrtc-rs/webrtc), [releases](https://api.github.com/repos/webrtc-rs/webrtc/releases/latest)
- `rtc` (sans-IO protocol core): crate 0.21.0, 996k downloads (969k recent), repo pushed 2026-10-08, 164 stars, Apache-2.0; claims "95%+ W3C API compliance". `webrtc` runs a background `PeerConnectionDriver` that owns sockets and drives the `rtc` core. — [crates.io rtc](https://crates.io/api/v1/crates/rtc), [GitHub rtc](https://api.github.com/repos/webrtc-rs/rtc), [README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)
- README: 0.21.x is the recommended line, 0.20.x previous; pre-1.0, minor bumps may break; a "path to webrtc 1.0" issue tracks stabilization. 0.21 adds `#[non_exhaustive]`, provider-neutral crypto (`rtc-crypto`; Ring and aws-lc-rs), virtual-clock deterministic time for ICE/DTLS/SCTP in a mock runtime, data channel back-pressure. `video/ulpfec` no longer registered by default. — [README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)
- 39 runnable examples including simulcast, ICE restart, data channels, insertable streams; the README gives no detail on simulcast depth. — [README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)
- Perf work in 0.21: SRTP MAC about 40% faster per RTCP packet; AES-CTR batched ~9-10% faster; DTLS GCM record encrypt 1.015 µs -> 262 ns; per-packet benchmarks at parity or faster than pre-migration baseline (README claim, own benchmarks). — [README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)
- Older sub-crates still published: `webrtc-ice` 0.17.2 and `turn` 0.17.2 (both 2026-07-20; the `turn` crate is a TURN client/server library of the old async stack), `webrtc-util`. — [crates.io turn](https://crates.io/api/v1/crates/turn), [webrtc-ice](https://crates.io/api/v1/crates/webrtc-ice)
- A third-party benchmark claim: rustrtc says it is ~3.3x faster than webrtc-rs and ~52% less memory (vendor claim, version tested unclear; likely the old 0.17 stack since rustrtc depends on `webrtc 0.17.1` in dev-deps). — [rustrtc README](https://github.com/restsend/rustrtc), [Cargo.toml](https://raw.githubusercontent.com/restsend/rustrtc/HEAD/Cargo.toml)
- Projects using it: Live777 (`webrtc` 0.20.1 + `rtc-*` 0.20.1 in its Cargo.toml). Sponsors listed: Recall.ai, Stream Chat, ChannelTalk (sponsors, not necessarily users). — [live777 Cargo.toml](https://raw.githubusercontent.com/binbat/live777/HEAD/Cargo.toml), [README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)

### Inferences
- The new `rtc` crate means webrtc-rs now offers the same sans-IO shape as str0m, but with a PeerConnection-like API and a younger codebase (0.21 is the first line on that core, per README). str0m has more history on the sans-IO core and is SFU-oriented; `rtc` is more browser-API-like. Both are pre-1.0.
- Old criticisms (Arc/Mutex-heavy async design, performance) apply to the 0.17 line, not necessarily 0.21 (UNVERIFIED for 0.21 under SFU load).

### Gaps
- No independent benchmark of webrtc 0.21 or `rtc` under SFU load; simulcast feature completeness in `rtc` not verified; actual date of the older quiet period not verified.

## Existing open-source Rust SFUs / media servers

### Takeaway
No turnkey Rust "meeting server" exists. Candidates are libraries/kits (webrtc-rs/sfu, oxpulse-sfu-kit, rheomesh) or streaming-oriented servers (Live777, PulseBeam, atm0s). For this app, building a small SFU on str0m is realistic; PulseBeam and oxpulse-sfu-kit are the best references.

### Cited Findings
- **PulseBeam** (PulseBeamDev/pulsebeam): general-purpose SFU for browsers/mobile/IoT; thread-per-core shared-nothing; H.264 Baseline up to 4.1 and Opus only; data channel "Planned"; pushed 2026-10-03; release pulsebeam-v0.4.9 (2026-08-26); 67 stars; AGPL-3.0. Depends on a PulseBeam fork of str0m (branch `feat/bwe-6`). HTTP signaling on TCP/7070; Bazel build; has a `meet` target in its build. — [GitHub](https://github.com/PulseBeamDev/pulsebeam), [Cargo.toml](https://raw.githubusercontent.com/PulseBeamDev/pulsebeam/HEAD/Cargo.toml), [releases](https://api.github.com/repos/PulseBeamDev/pulsebeam/releases/latest)
- **oxpulse-sfu-kit** (anatolykoptev): "reusable multi-client SFU kit on top of str0m"; `Client` (per-peer state machine wrapping `Rtc`), `Registry` (UDP routing + event fanout), simulcast layer selection (`BestFitSelector`), optional BWE pacer, AV1 DD, active-speaker detection, Prometheus. crates.io 0.12.2 (2026-07-13), only ~1.3k downloads, 3 stars; pushed 2026-07-17; MIT OR Apache-2.0 (crate), GitHub says Apache-2.0. Single-author, young; useful as design reference. — [GitHub](https://github.com/anatolykoptev/oxpulse-sfu-kit), [crates.io](https://crates.io/api/v1/crates/oxpulse-sfu-kit). Note README says 0.5 in snippet vs 0.12.2 on crates.io (README stale).
- **webrtc-rs/sfu** (`sfu` crate 0.5.0, 2026-09-04; repo pushed 2026-09-19; 82 stars; MIT/Apache-2.0 per README, GitHub says Apache-2.0): "SFU in Rust with Sans-IO" on the `rtc` core. `Sfu`/`Room`/`Client` all implement `sansio::Protocol`; no sockets/threads/clock; signaling is the event plane (`SFUEvent`: Join, SessionDescription, IceCandidate, Leave); includes `chat` example with HTTPS/WebSocket signaling + UDP socket. This is the closest existing match to a functional-core design. ~9.5k downloads. — [GitHub](https://github.com/webrtc-rs/sfu), [README](https://raw.githubusercontent.com/ngr-tc/sfu/HEAD/README.md), [crates.io](https://crates.io/api/v1/crates/sfu)
- **atm0s-media-server** (8xFF): MIT; pushed 2026-10-07; 330 stars; latest GitHub release v0.2.0-alpha.9 (2026-05-18); crate 0.2.0-alpha.8; README says "refactoring-era state around sans-io-runtime"; WHIP/WHEP, protobuf SDK, RTPengine-style API, console, multi-node (gateway/media/connector). Cargo.toml pins `str0m = "0.6"` (very old str0m; verify before relying on it). Alpha, heavy for 2-10 users. — [GitHub](https://github.com/8xFF/atm0s-media-server), [Cargo.toml](https://raw.githubusercontent.com/8xFF/atm0s-media-server/HEAD/Cargo.toml), [releases](https://api.github.com/repos/8xFF/atm0s-media-server/releases/latest)
- **Live777** (binbat): SFU with WHIP/WHEP as first-class, MPL-2.0, v0.10.2 (2026-09-28), pushed 2026-10-08, 311 stars; built on webrtc-rs 0.20.1 + `rtc-*`; recording to fMP4/S3, `liveman` clustering. Streaming (WHIP/WHEP) oriented, not a meeting room model; simulcast/datachannel not mentioned in README. — [GitHub](https://github.com/binbat/live777), [Cargo.toml](https://raw.githubusercontent.com/binbat/live777/HEAD/Cargo.toml)
- **rheomesh** (h3poteto): SFU library, Apache-2.0, crates.io 0.8.1 (2026-09-15), pushed 2026-10-01, 38 stars; simulcast, SVC, data channels, relay, recording, WHIP/WHEP; ships a TypeScript client lib. — [GitHub](https://github.com/h3poteto/rheomesh), [crates.io](https://crates.io/api/v1/crates/rheomesh)
- **rustrtc** (restsend): its own from-scratch WebRTC stack with full ICE (TURN, ICE lite), GCC BWE, NACK/RTX, `no_std` mode; crate 0.4.4 (2026-10-08), 112 stars, MIT; 15k downloads. Young, vendor benchmarks. — [GitHub](https://github.com/restsend/rustrtc), [crates.io](https://crates.io/api/v1/crates/rustrtc)
- **mediasoup** Rust crate 0.30.0 (2026-10-08, ~335k downloads): Rust API over the C++ mediasoup worker (C++ repo pushed 2026-10-09, 7.4k stars, ISC). Mature SFU with simulcast/SVC but needs the C++ worker subprocess. — [crates.io](https://crates.io/api/v1/crates/mediasoup), [GitHub](https://api.github.com/repos/versatica/mediasoup)
- Stale/inactive: harlanc/rsfu (last push 2023-08-09), billylindeman/switchboard (2025-02-12, webrtc-rs based), waterbustech/waterbus-rs (2025-08-12), weever-streaming (2023-03-31). — [GitHub search](https://api.github.com/search/repositories?q=sfu+webrtc+language:rust)
- Non-Rust: LiveKit server is Go (Apache-2.0, pushed 2026-10-09, 21.3k stars); `livekit/rust-sdks` is the Rust client SDK (Apache-2.0, pushed 2026-10-09); Galene is Go (MIT, pushed 2026-10-04, 1.4k stars; designed for small/medium conferencing — description from memory, UNVERIFIED). — [LiveKit](https://api.github.com/repos/livekit/livekit), [rust-sdks](https://api.github.com/repos/livekit/rust-sdks), [Galene](https://api.github.com/repos/jech/galene)
- Other str0m-based small repos: `Teyk0o/str0m-stress` (SFU stress tool, 2026-04), `kimsun721/stream` (2026-10-08). `AreevAI/flowcat` (voice agents, uses str0m) appears in README search. Not evaluated. — [GitHub search](https://api.github.com/search/repositories?q=str0m+in:readme+language:rust)

### Inferences
- Licensing: AGPL (PulseBeam) rules out copying code unless the app is also AGPL; MPL-2.0 (Live777) is file-level copyleft; others permissive.
- None of these are proven at "Google Meet" feature level; for 2-10 users the hard parts are the app-level policy (who subscribes to what, simulcast layer choice, screen share as separate track, active speaker), which oxpulse-sfu-kit and webrtc-rs/sfu sketch.
- Choosing a "functional core" base: `sfu` (webrtc-rs) is explicitly structured that way but sits on the younger `rtc` core; str0m is proven but you write the room layer yourself (oxpulse-sfu-kit as reference).

### Gaps
- Last-commit dates are `pushed_at` (any branch), not necessarily main. Real-world scale/production references for oxpulse-sfu-kit, PulseBeam and `sfu` not found. Whether atm0s still uses str0m 0.6 in main workspace (versus a newer one) not verified beyond one Cargo.toml line.

## TURN

### Takeaway
Use a managed TURN (Cloudflare) or run coturn; the pure-Rust `turn-rs` server is usable and recent but a single-maintainer project. An SFU with a public IP and ICE lite mostly needs TURN only for clients behind UDP-blocking firewalls (TURN over TLS/443).

### Cited Findings
- **turn-rs** (mycrl/turn-rs; crate `turn-server` 4.1.5-4.1.6, 2026-08-11; repo pushed 2026-09-14; 516 stars; MIT): pure Rust TURN server; long-term credentials only; static credential list in config; UDP+TCP and TLS on TCP; gRPC API for events/session control; Prometheus; RFCs 5389/5766/6062/6156 and TURN REST API draft. Author states coturn is "much more mature". Only ~12.7k downloads total. — [GitHub](https://github.com/mycrl/turn-rs), [crates.io](https://crates.io/api/v1/crates/turn-server)
- `turn` crate 0.17.2 (webrtc-rs, 2026-07-20, 7.4M downloads): TURN client and server library from the older async stack. — [crates.io](https://crates.io/api/v1/crates/turn)
- `rusturn` last release 0.0.7 (2024-02-23), rusturn repo pushed 2026-05-21 (111 stars): low maturity. — [crates.io](https://crates.io/api/v1/crates/rusturn), [GitHub search](https://api.github.com/search/repositories?q=turn+server+language:rust)
- **Cloudflare Realtime TURN**: STUN stun.cloudflare.com:3478/udp; TURN over UDP 3478 (alt 443), TCP 3478 (alt 80), TLS 5349 (alt 443); anycast; China network excluded; free when used with Cloudflare's Realtime SFU, otherwise $0.05 per GB egress to the client; per-allocation limits (~50-100 Mbps, 5-10 kpps). Credential API and Worker integration not covered on the fetched page. — [Cloudflare docs](https://developers.cloudflare.com/realtime/turn/)
- str0m has no TURN; Sans-IO design expects the app to supply TURN-relayed sockets/candidates. — [str0m README](https://github.com/algesten/str0m)
- Twilio and Metered TURN: not researched (no source fetched).

### Inferences
- If the SFU has a public IP/UDP port, browsers connect to it directly, so TURN is needed only for blocked-UDP clients. Then a TURN-over-TLS-on-443 fallback is what matters; Cloudflare's 443 TLS offering covers it at low cost for 2-10 users. Self-host alternative: coturn (C, not Rust) is the safe default; turn-rs if a pure-Rust deployment is a goal (accept its lower maturity).
- An SFU that sits behind firewall-restricted hosting (only TCP 443) cannot be served by str0m alone unless it supports ICE-TCP (str0m README says it supports UDP and TCP).

### Gaps
- Pricing/free-tier numbers for Twilio, Metered, and the Cloudflare free monthly allowance were not verified. Credential-generation API of Cloudflare TURN not verified.

## Signaling design

### Takeaway
For an SFU, signaling is a small WebSocket protocol per room carrying join/leave, SDP offer/answer per peer connection and (optionally) trickle ICE; the server is a peer in negotiation. Axum + tokio-tungstenite is the standard Rust stack. Cloudflare Durable Objects can host the signaling room (and hibernating WebSockets) but cannot run the media plane.

### Cited Findings
- axum 0.8.9 (2026-04-14) and tokio-tungstenite 0.30.0 (2026-07-11) are current, very widely used (510M and 302M downloads). — [axum](https://crates.io/api/v1/crates/axum), [tokio-tungstenite](https://crates.io/api/v1/crates/tokio-tungstenite)
- Existing signaling model in a sans-IO SFU: `SFUEvent` = Join, SessionDescription, IceCandidate, Leave, with Ok/Err replies; each carries `request_id`, `room_id`, `client_id`; example uses HTTPS + WebSocket. — [webrtc-rs/sfu README](https://raw.githubusercontent.com/ngr-tc/sfu/HEAD/README.md)
- PulseBeam uses plain HTTP signaling (TCP/7070) and also exposes WHIP; Live777/atm0s use WHIP/WHEP (HTTP POST of SDP offer, response SDP answer) rather than WebSocket. — [PulseBeam README](https://github.com/PulseBeamDev/pulsebeam), [Live777](https://github.com/binbat/live777)
- str0m `http-post` example shows receiving media via an HTTP POST of an SDP offer; `chat` example serves a page and signals over HTTP(S). — [str0m README](https://github.com/algesten/str0m)
- Cloudflare Durable Objects: WebSocket Hibernation API lets an idle object be evicted while client WebSockets stay connected; in-memory state is lost on wake-up, so per-connection state must be persisted via `serializeAttachment`; one object per room can coordinate many clients. `workers-rs` (`worker` crate 0.8.7, 2026-09-25; repo pushed 2026-10-03, 3.7k stars) is the Rust SDK. — [CF docs](https://developers.cloudflare.com/durable-objects/best-practices/websockets), [crates.io worker](https://crates.io/api/v1/crates/worker), [workers-rs](https://api.github.com/repos/cloudflare/workers-rs)
- Rust-specific Durable Object hibernation API names were not verified (search returned TS examples only).

### Inferences
- Minimal SFU signaling protocol (my design, not a cited standard), JSON or CBOR over one WebSocket per participant, URL carries room id/invite token:
  - Client -> server: `join {room, token, display_name}`, `offer {sdp}` (client-initiated, includes its camera/mic/screen transceivers), `answer {sdp}` (reply to a server offer), `ice {candidate}` (optional; with a server using ICE lite and a fixed public candidate, trickle from server is unnecessary), `leave`, `mute/unmute`, `subscribe {track_id, layer}`.
  - Server -> client: `joined {self_id, peers[]}`, `peer_joined/peer_left`, `answer {sdp}`, `offer {sdp}` (server-initiated when a new remote track must be added to the subscriber's connection), `tracks {peer_id, mid, kind, source: camera|mic|screen}`, `error`.
  - Negotiation choice: one PeerConnection per client carrying both published and subscribed tracks (server offers on topology change; needs glare handling = "perfect negotiation" with server as the polite or impolite side) versus two PeerConnections (publisher: client offers; subscriber: server offers) which removes glare. The two-connection layout is simpler to make deterministic; one-connection is leaner. Recommendation: two connections or a strict server-owned offer queue.
- With an SFU running on a VM, the signaling server can live in the same process (axum + a message channel into the SFU loop) so that signaling messages are just more inputs to the functional core.
- Cloudflare Workers cannot expose UDP sockets (UNVERIFIED, from memory), so the SFU media loop must run on a VM/container; a Durable Object could still hold room metadata/invites. Cloudflare's own managed SFU (Realtime SFU) is a non-Rust alternative.

### Gaps
- No authoritative source found for "perfect negotiation" applied to SFUs (only the W3C pattern from memory). No benchmark or example of a Rust DO signaling server for WebRTC found.

## Audio tap for transcription

### Takeaway
Both str0m and webrtc-rs expose incoming Opus; with str0m frame mode you get depacketized Opus frames per `MediaData` event, which you can fork into a channel to a transcription task and decode with libopus bindings (`opus` crate) or a pure-Rust decoder.

### Cited Findings
- str0m frame mode: `Event::MediaData` delivers full audio frames (Opus payload) with media time; RTP mode: `Event::RtpPacket`. str0m does no decoding (no decode/encode in feature table). So the SFU shell forwards the same frames to subscribers and to a transcription channel. — [str0m README](https://github.com/algesten/str0m)
- README warns str0m has a fixed depacketize buffer but no adaptive jitter buffer; for transcription you may need reordering/loss concealment handled yourself (Opus PLC via decoder). — [str0m README FAQ](https://github.com/algesten/str0m)
- `opus` crate 0.4.0 (2026-08-23): safe Rust bindings to libopus (2.7M downloads) — requires libopus (C). — [crates.io](https://crates.io/api/v1/crates/opus)
- `audiopus` last release 0.3.0-rc.0 (2021-04-22), `audiopus_sys` 0.2.2 (2021): effectively unmaintained. — [crates.io audiopus](https://crates.io/api/v1/crates/audiopus)
- `opus-decoder` 0.1.1 (2026-03-12): pure-Rust Opus decoder "RFC 8251 conformant, no unsafe, no FFI"; new, single-author (162k downloads). — [crates.io](https://crates.io/api/v1/crates/opus-decoder)
- Symphonia 0.6.1 (2026-08-13): README lists an Opus codec crate `symphonia-codec-opus` with feature `opus`, default-on, status "-" (not rated); `docs.rs` lists a libopus adapter `symphonia-adapter-libopus` for 0.6; `symphonia-codec-opus` did not resolve on crates.io API in my check (the crate may be unpublished or named differently). Symphonia is a file/container demuxer-decoder, not an RTP tool; Ogg container only matters if saving recordings. — [Symphonia README](https://github.com/pdeljanov/Symphonia), [search summary](https://docs.rs/crate/symphonia/0.6.1), [crates.io symphonia](https://crates.io/api/v1/crates/symphonia)
- `ogg` crate 0.9.2 (2025-01-12) for writing Ogg containers. — [crates.io](https://crates.io/api/v1/crates/ogg)
- Cited Rust sans-IO SFU designs keep forwarding inside the engine ("no payload above the SFU") — a tap therefore needs a hook/event in the engine. — [webrtc-rs/sfu README](https://raw.githubusercontent.com/ngr-tc/sfu/HEAD/README.md)

### Inferences
- Recommended pipeline: shell receives `Event::MediaData` (audio, Opus 48 kHz) per peer -> emits a pure command `ForkAudio{peer, media_time, bytes}` -> bounded channel to a transcription worker -> `opus` crate decode to 48 kHz PCM (resample to 16 kHz for Whisper-class models) -> ASR. Keep decoding out of the core (it is a side-effecting adapter); the core only decides what to emit. Use `bounded` channel with drop-oldest to protect the SFU loop.
- Attribute audio per participant (one stream per SSRC/mid), which an SFU gives for free.
- Pure-Rust `opus-decoder` would avoid the C dependency but is very new; choose `opus` (libopus) for correctness.

### Gaps
- Not verified: whether `symphonia-codec-opus` is published at 0.6; the ASR side (whisper-rs, etc.) was out of scope.

## Testing sans-IO WebRTC logic in Rust

### Takeaway
Sans-IO makes deterministic tests straightforward: str0m ships an in-process two-peer harness with virtual time; webrtc-rs `rtc` has a virtual-clock mock runtime and `sfu` has no clock at all. Application-level tests can drive the same pattern.

### Cited Findings
- str0m never calls the system clock; README says this allows driving instances "faster than realtime (see the integration tests)". Tests use helpers `progress(l, r)` and `negotiate()` in `tests/common.rs`, plus scenarios for loss, network conditions, BWE, DTLS retransmit, malformed packets. — [str0m README](https://github.com/algesten/str0m), shallow clone of repo
- `rtc` 0.21: deterministic time; the mock runtime's virtual clock drives ICE, DTLS, SCTP timers; README says separation of protocol logic and I/O enables deterministic tests without real network. — [webrtc-rs README](https://github.com/webrtc-rs/webrtc/blob/master/README.md)
- webrtc-rs/sfu: "no sockets, no threads, and no clock of its own"; the caller feeds datagrams/signaling and drains outputs. — [sfu README](https://raw.githubusercontent.com/ngr-tc/sfu/HEAD/README.md)
- str0m-stress is an SFU stress tool built on str0m (2026-04). — [GitHub search](https://api.github.com/search/repositories?q=str0m+sfu+language:rust)

### Inferences
- Suggested layers: (1) pure unit/property tests of the room/routing function with fake events (proptest); (2) simulation tests with several real `Rtc` instances (as clients) connected to the SFU core through a virtual network with a seeded RNG for loss/delay and a virtual `Instant`; (3) a few real-browser end-to-end tests (Playwright with fake media devices) for SDP compatibility with Chrome/Firefox/Safari, which sans-IO tests can't cover.
- Caveat: `Instant` is opaque in std; create virtual time as `start + Duration`, which str0m tests already do.

### Gaps
- Did not find a published guide for deterministic-simulation tests of a Rust SFU (e.g. turmoil/madsim usage); no HN/blog threads from 2024-2026 were located.
