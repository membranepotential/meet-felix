# Existing open-source meeting apps: architectures and lessons (as of 2026-10-09)

Method note: ~25 search/fetch calls. Primary docs were fetched where possible. Several sources (Jitsi handbook architecture, LiveKit sizing) were thin, so some questions have Gaps. Items from my own background knowledge are in Gaps/Inferences and labeled UNVERIFIED.

## Jitsi Meet: components, resources, invite links, lobby, captions

### Takeaway
Jitsi is a multi-service stack (Prosody XMPP, Jicofo, JVB, nginx, optional Jigasi/Jibri) that needs a dedicated 4-8 core server; it is heavy because of XMPP signaling plus a bridge fleet design. As of July 2026 Jitsi deprecated Jigasi-based transcription in favor of a JVB-to-WebSocket transcription proxy.

### Cited Findings
- Components: Jitsi Meet (JS client, React), JVB (WebRTC SFU), Jicofo (conference focus; manages media sessions, load-balances participants across bridges), Jigasi (SIP gateway), Jibri (recording/streaming via headless Chrome in a virtual framebuffer + ffmpeg), Prosody (XMPP signaling) — [Jitsi handbook architecture](https://jitsi.github.io/handbook/docs/architecture/)
- Jicofo talks to other components through Prosody; Prosody also acts as a pub/sub queue where bridges publish status; clients reach JVB over UDP 10000 plus a Colibri WebSocket; nginx proxies BOSH/WebSocket. A conference always uses a single bridge. (secondary sources) — [team-collaboration-toolkit](https://team-collaboration-toolkit.readthedocs.io/en/latest/jitsi/architecture.html), [Akamai/Linode guide](https://www.akamai.com/cloud/guides/manually-deploy-jitsi-cluster/)
- Self-host sizing: bandwidth per stream ~200 kbit/s (180p), ~500 kbit/s (360p), ~2.5 Mbit/s (720p), ~10 Mbit/s (4K); 1 Gbit/s may suffice for a small server; RAM 8 GB usual, 4 GB for small meetings, 2 GB for tests; 4 dedicated cores can be enough; Prosody uses one core; avoid shared-CPU hosting; ~20 GB disk — [Jitsi requirements](https://jitsi.github.io/handbook/docs/devops-guide/devops-guide-requirements/)
- Each simultaneous recording needs its own Jibri (8 GB RAM for 720p-class); do not co-locate with Jitsi Meet — same source.
- Docker ports: 80/tcp, 443/tcp, 10000/udp (RTP), 20000-20050/udp only for Jigasi — [Jitsi Docker guide](https://jitsi.github.io/handbook/docs/devops-guide/devops-guide-docker/)
- Auth: `ENABLE_GUESTS` (if off, everyone must authenticate; with guests on, unauthenticated users wait until an authenticated user opens the room), `ENABLE_LOBBY`, `ENABLE_AUTH` + `AUTH_TYPE=jwt` with `JWT_APP_ID`/`JWT_APP_SECRET`, `JWT_ALLOW_EMPTY` for anonymous users alongside JWT — same Docker guide.
- Captions, old design: Jigasi joins as a hidden participant (hidden domain), backends Google STT, Vosk, Whisper, others; JSON results become subtitles, plain text goes to chat — [Jigasi repo](https://github.com/jitsi/jigasi), [search summary of README](https://www.sourcepulse.org/projects/1300165)
- Captions, new design (blog 2026-07-14, Boris Grozev): "Jigasi-based transcription is now deprecated and will be removed in a future release." Jicofo tells JVB the service URL; JVB opens one WebSocket and streams each participant's Opus audio tagged by source; service returns transcription events over the same socket; no hidden participant. Service is `opus-transcriber-proxy` (Jitsi org on GitHub; license not stated in post), self-hostable via Docker or deployable to Cloudflare, supports OpenAI-realtime, Deepgram, Gemini, xAI, any OpenAI-compatible endpoint, with failover — [Jitsi blog](https://jitsi.org/blog/a-new-architecture-for-transcription-and-more/)
- Jigasi complaint: each transcribed meeting needs a stateful instance; scaling means a fleet — same blog.

### Inferences
- Lesson for a small app: server-side transcription is best done by tapping audio at the SFU (or a bot participant) and streaming Opus over WebSocket to an STT proxy. Browser-side Web Speech/on-device STT would avoid all of it.
- Jitsi's cost is dominated by the dedicated-CPU server it recommends, not by 2-10 person meetings; a 2-10 person app needs far less than Jitsi's stack.

### Gaps
- Jitsi handbook architecture page gave no message flow or port detail (fetched page was text-light).
- Lobby behavior details (moderator admits, knocking UX) not retrieved; only the toggle is cited.
- Jitsi room-link model (room name in URL path, no secret by default; JWT passed as `?jwt=`) is UNVERIFIED background knowledge.

## LiveKit: architecture, minimum resources, simulcast/dynacast, client duties

### Takeaway
LiveKit is an Apache-2.0 Go/Pion SFU with JWT-token join, simulcast on by default, and client-side adaptive stream + dynacast driven by server feedback. Official docs give no minimum single-node spec; scale is "bound by CPU and bandwidth".

### Cited Findings
- Server: Go, Pion, Apache-2.0; single binary, Docker, or Kubernetes; features include simulcast, selective subscription, E2EE, SVC (VP9/AV1), SIP, webhooks, JWT auth; "distributed and multi-region" — [livekit/livekit README](https://github.com/livekit/livekit)
- "The scalability of LiveKit is bound by CPU and bandwidth"; recommends 10 Gbps for production, compute-optimized instances; host networking in Docker; domain + CA-signed cert required; built-in TURN needs its own domain/cert (TLS on 443 if no LB); `use_external_ip` for NATed clouds — [LiveKit deployment](https://docs.livekit.io/home/self-hosting/deployment/)
- Ports: 7880 (API), 7881 TCP (RTC), 50000-60000/UDP, 3478/UDP TURN, 443; Ingress adds 1935 (RTMP), 7885/UDP (WHIP). VM guide uses Caddy for TLS, Redis conf generated, Egress/Ingress optional — [LiveKit VM guide](https://docs.livekit.io/home/self-hosting/vm/)
- Redis is "recommended for production deploys" in that doc (config example); the page does not say it is required for multi-node — [deployment](https://docs.livekit.io/home/self-hosting/deployment/)
- Tokens: JWT with grants `roomJoin`, `room`, `canPublish`, `canSubscribe`, `roomAdmin`, `roomCreate`; identity is `sub`; expiry applies only to the initial connection, not reconnects; self-hosted deployments do not revoke tokens, so use short TTL; room config in a token applies only when the room is first created — [LiveKit tokens](https://docs.livekit.io/frontends/authentication/tokens/)
- Simulcast: publisher sends several layers; server picks per recipient. Dynacast: server tells publisher to pause layers nobody consumes (off by default; with SVC codecs only whole streams pause). Adaptive stream: subscriber SDK uses ResizeObserver/IntersectionObserver on video elements and tells the server the needed size; pauses when not visible — [LiveKit advanced tracks](https://docs.livekit.io/home/client/tracks/advanced/), [field guide](https://livekit.io/field-guides/guide/configuring-client-sdk-optimal-video-quality). Sources disagree slightly on default layer count; check per SDK version.
- Agents: Apache-2.0 framework (Python/Node) where an agent server registers with LiveKit, receives dispatch, and runs a job subprocess that joins the room as a participant; STT-LLM-TTS pipeline via plugins — [LiveKit Agents](https://docs.livekit.io/agents/)
- Element Call uses LiveKit with `lk-jwt-service` issuing tokens from Matrix identity — see Element section.

### Inferences
- A custom client that talks to LiveKit would need to replicate: token fetch, signaling WebSocket protocol, publisher/subscriber peer connections, simulcast encodings, adaptive-stream size reporting, reconnect (ICE restart/resume), dynacast handling. This is why using `livekit-client` is far cheaper than a custom client.
- Single-node minimum is therefore empirical; for 2-10 users a small VPS is plausible but UNVERIFIED.

### Gaps
- No official minimum CPU/RAM figure found; LiveKit benchmark page not fetched.
- Exact multi-node role of Redis (room-to-node routing, pub/sub) not confirmed from docs fetched (UNVERIFIED background: Redis used for node discovery and signaling routing).
- JS client reconnect internals (resume vs full reconnect, ICE restart) not retrieved.
- Egress internals (headless Chrome room composite, GStreamer) UNVERIFIED background.

## Other projects: Galene, mediasoup, Janus, ion-sfu, Pion SFUs, Nextcloud HPB, Element Call, BBB

### Takeaway
Galene is the best "small and simple" reference (Go, ~7000 lines server); mediasoup and Janus are toolkits that leave signaling/rooms to you; Nextcloud HPB and BBB are heavy product stacks; Element Call outsources media to LiveKit.

### Cited Findings
- Galene: Go + Pion SFU, forwards without re-encoding; MCU rejected as CPU-expensive; v1 was an adaptive SFU sending the highest quality acceptable to all receivers (slow receiver degrades everyone); later added bandwidth estimation, VP8/VP9 SVC, simulcast; ~7000 lines Go + 4000 lines JS; config in local JSON files, no database; ~1/4 core for a 100-student lecture, ~20 participants/core in a meeting (quadratic); overloaded server freezes video; no E2EE — [galene.org](https://galene.org/), [Galene slides 2021](https://galene.org/galene-20210222.pdf), [LinuxFr](https://linuxfr.org/news/galene-un-serveur-de-videoconference-libre)
- mediasoup: Node.js module or Rust crate; "low level API", not an opinionated server; simulcast, SVC, transport BWE; client libs mediasoup-client (JS), libmediasoupclient (C++) — [mediasoup.org](https://mediasoup.org/). Signaling and rooms are application work (page implies, does not state).
- Janus: modular core (ICE, DTLS-SRTP, SDP, data channels) with plugins; VideoRoom is the SFU plugin; clients attach to a plugin handle; other plugins for streaming, recording, SIP, TextRoom — [WebRTC.ventures Janus post](https://blog.webrtc.ventures/2020/12/janus-webrtc-media-server-video-conference-app), [Janus summary](https://netactuate.com/docs/platform/marketplace/appliances/janus-gateway)
- ion-sfu: Go/Pion SFU; GitHub page showed no archive banner or deprecation notice (1,177 commits; no dates visible) — [ionorg/ion-sfu](https://github.com/ionorg/ion-sfu). The fetched page showed no dates, so "active" is not established.
- Rust options: str0m (sans-I/O, MIT, SFU chat example; used as SFU by Lookback), oxpulse-sfu-kit (rooms + simulcast forwarding on str0m), webrtc-rs `sfu` crate 0.0.3 from Mar 2024 (looks dormant), Rheomesh (SFU on webrtc-rs; temporal-layer filtering only) — [str0m](https://docs.rs/crate/str0m/latest), [oxpulse-sfu-kit](https://docs.rs/oxpulse-sfu-kit), [sfu crate](https://lib.rs/crates/sfu), [Rheomesh](https://rust-digger.code-maven.com/crates/rheomesh)
- mediasoup also ships a Rust crate (see mediasoup.org above).
- Nextcloud Talk HPB: `nextcloud-spreed-signaling` (WebSocket signaling) + Janus (media) + NATS (broker) + coturn; Docker guides use host networking; community note says a role supports only one Janus; some packages are gated to customers — [Nextcloud HPB guide](https://help.nextcloud.com/t/nextcloud-talk-high-performance-backend-hpb-multi-domain-setup-guide/239845), [Nextcloud portal](https://portal.nextcloud.com/article/Scalability/Installation-of-Nextcloud-Talk-High-Performance-Backend)
- Element Call: MatrixRTC framework (applications, slots, membership, transport); Element uses LiveKit as media backend; deployment needs LiveKit + `lk-jwt-service` which mints LiveKit JWTs from a Matrix identity/room; as of 2025-03 supported in Element Web and Element X; guide warns it likely only works with Synapse — [Element blog](https://element.io/blog/element-call-redefining-conferencing-for-privacy-scale-and-sovereignty/), [lk-jwt-service](https://github.com/element-hq/lk-jwt-service), [Matrix RTC setup](https://continuwuity.org/calls/livekit)
- BigBlueButton (production, 3.0-era docs): 16 GB RAM, 8 cores, 250 Mbit/s symmetric, Ubuntu 22.04, UDP 16384-32768; services include nginx, FreeSWITCH (audio), Redis, bbb-webrtc-sfu with mediasoup and Kurento references, Akka apps, GraphQL server, Etherpad, LibreOffice-in-Docker, recording pipeline — [BBB install docs](https://docs.bigbluebutton.org/administration/install/)
- Cloudflare Realtime SFU: no rooms concept, no SDK; app stores participants and track IDs itself; Orange Meets reference app uses Workers + Durable Objects for WebSocket coordination and room state; Orange Meets added MLS E2EE (Rust to WASM) client-side — [Cloudflare SFU docs](https://developers.cloudflare.com/realtime/sfu/introduction/), [Cloudflare blog on Orange Me2eets](https://blog.cloudflare.com/orange-me2eets-we-made-an-end-to-end-encrypted-video-calling-app-and-it-was/)

### Inferences
- The serverless-friendly pattern: a stateful coordination object per room (Durable Object) for signaling + room state, and a managed or self-run SFU for media. Cloudflare's SFU leaves exactly this part to the app.
- Galene's size (7k lines Go) shows a minimal SFU plus client is a small-team scale project; a pure-Rust SFU on str0m is plausible but the ecosystem is young.

### Gaps
- No official 2026 maintenance status for ion-sfu, Janus, mediasoup releases (UNVERIFIED background: ion-sfu development slowed after 2022 and LiveKit grew out of similar ideas; verify GitHub dates before relying).
- BBB architecture one-paragraph rationale (HTML5 client, Meteor to GraphQL migration) only partly covered.
- Galene's current (2026) release status not checked; PDF dated 2025-06-10 exists.

## WebRTC topology: mesh vs SFU for 2-10 participants

### Takeaway
Mesh costs each peer (N-1) uploads and encodes; common guidance is mesh up to about 2-4, SFU beyond. For 2-10 people, SFU download still grows, so simulcast and layer selection matter.

### Cited Findings
- Mesh: each node sends and receives N-1 streams; upload = (N-1) x bitrate; encoding done once per recipient in browser; example 4 people at 1 Mbps = ~3 Mbps up and down each — [Digital Samba](https://www.digitalsamba.com/blog/p2p-sfu-and-mcu-webrtc-architectures-explained), [Ant Media](https://antmedia.io/webrtc-network-topology/), [VideoSDK](https://videosdk.live/blog/sfu-vs-mcu-vs-mesh-webrtc) (vendor blogs)
- SFU: upload roughly one stream (or one simulcast set); download scales with streams subscribed; simulcast lets the SFU choose per recipient — same sources.
- Guidance range: mesh for ~2-4 (some say up to 4-6), SFU for larger — [Forasoft](https://www.forasoft.com/learn/video-streaming/articles-streaming/sfu-mcu-mesh-topologies), [DEV article on building WebRTC apps](https://dev.to/car_oc/do-you-know-what-it-takes-to-create-a-webrtc-video-conferencing-application-1cc1). Thresholds are vendor claims.
- SFU removes the media privacy of mesh: the server sees raw forwarded streams unless E2EE (insertable streams/MLS) is added — [Cloudflare blog](https://blog.cloudflare.com/orange-me2eets-we-made-an-end-to-end-encrypted-video-calling-app-and-it-was/)

### Inferences
- My arithmetic: at 10 people with 1 Mbps video, mesh needs ~9 Mbps up per phone plus 9 encodes (infeasible on phones); SFU needs ~1-2 Mbps up (simulcast adds layers) and download of up to 9 streams unless layer-limited (thumbnails at low layer ~150-300 kbps).
- Hybrid (P2P for 2, SFU for 3+) saves server cost and latency for the common 1:1 case but means two code paths and a mid-call renegotiation/migration when the third person joins. Recommend only if cost matters; otherwise SFU always is simpler. UNVERIFIED design opinion.

### Gaps
- No primary benchmark of phone CPU in mesh found. No source found describing a real hybrid P2P-to-SFU migration implementation.

## Essential client features for smooth meetings

### Takeaway
The cited sources confirm simulcast/adaptive-stream/dynacast and server-side forwarding; the audio-processing and reconnection items are not backed by sources fetched here.

### Cited Findings
- Simulcast + subscriber-driven layer choice via element size/visibility, publisher-side pausing — see LiveKit section ([docs](https://docs.livekit.io/home/client/tracks/advanced/)).
- mediasoup lists transport bandwidth estimation as a built-in; Galene added congestion control and BWE — [mediasoup.org](https://mediasoup.org/), [galene.org](https://galene.org/)
- Galene overload behavior: video freezes — [galene.org](https://galene.org/)

### Inferences
- Using `livekit-client` (or mediasoup-client) gives most of these for free; a custom client would own BWE reaction, layer switching, and reconnect.

### Gaps (all UNVERIFIED background knowledge, not researched)
- Active speaker detection via audio levels (RTP audio-level header extension / `getStats`), browser-built-in `echoCancellation`/`noiseSuppression`/`autoGainControl` constraints vs RNNoise (WASM), ICE restart on network change (`iceRestart`, `navigator.connection`/`online` events), and mobile Safari quirks. No sources gathered; a follow-up pass should cite MDN / webrtcHacks.

## Invite links, auth, lobby: Meet, Jitsi, Whereby

### Takeaway
Meet and Whereby use link-plus-knock with host control; Jitsi uses room name in URL with optional JWT/lobby; LiveKit/Element use signed JWT minted by your backend.

### Cited Findings
- Google Meet: hosts can disable "Anyone with the meeting link can ask to join" for Trusted/Restricted access types (June 2025); guest admit flow with two queues and default-deny for riskier requests (March 2026); removed participants cannot rejoin or knock; rejecting a knock twice blocks the knocker; "Quick access" lets same-domain users skip knocking — [Workspace Updates 2025-06](https://workspaceupdates.googleblog.com/2025/06/google-meet-host-management-control-for-asking-to-join-a-meeting.html), [2026-02 guest admit](https://workspaceupdates.googleblog.com/2026/02/safeguarded-guest-admit-flow-in-google-meet.html), [2020 attendee controls](https://workspaceupdates.googleblog.com/2020/09/meet-participan.html)
- Whereby: rooms can be locked (`isLocked`); guests knock; hosts can let in, reject, or put on hold with a message; a `hostRoomUrl` (carrying a room key) lets hosts enter locked rooms and admit others — [Whereby waiting rooms](https://docs.whereby.com/whereby-product-features/waiting-rooms), [Lock and Knock](https://whereby.helpscoutdocs.com/article/464-lock-knock)
- Jitsi: guests toggle, lobby toggle, optional JWT auth (see Jitsi section).
- LiveKit: JWT per participant with room + publish/subscribe/admin grants, issued by your backend; expiry only checked at connect; no revocation self-hosted — [LiveKit tokens](https://docs.livekit.io/frontends/authentication/tokens/)

### Inferences
- Minimal secure design: unguessable room ID (>=128-bit random) in the URL for guests, a separate host secret (like Whereby's hostRoomUrl) or host JWT, a lobby/knock queue that the host can toggle, and short-TTL join tokens minted by the backend after admit. Link expiry = room TTL in room state.
- Note LiveKit tokens cannot be revoked; removing a user must kick via server API, and short TTL limits rejoin.

### Gaps
- Google Meet link format (xxx-yyyy-zzz code and entropy) not fetched. Whereby's link/secret internals beyond hostRoomUrl not covered.

## Costs to run

### Takeaway
Server resources are driven by bandwidth and CPU for forwarding; official recommendations are conservative (Jitsi 4-8 cores, BBB 8 cores/16 GB), and only Galene gives per-core capacity numbers.

### Cited Findings
- Jitsi: 4 dedicated cores can suffice; 8 GB RAM usual — [Jitsi requirements](https://jitsi.github.io/handbook/docs/devops-guide/devops-guide-requirements/)
- BBB: 16 GB, 8 cores, 250 Mbit/s — [BBB docs](https://docs.bigbluebutton.org/administration/install/)
- Galene: ~20 meeting participants per core, ~1/4 core for 100-student lecture — [galene.org](https://galene.org/)
- LiveKit: bound by CPU and bandwidth; no sizing table — [LiveKit deployment](https://docs.livekit.io/home/self-hosting/deployment/)

### Inferences
- A single 2-4 vCPU VPS with a good network port likely carries many 2-10 person rooms (Galene ratio suggests 20 participants per core in one meeting, quadratic). Egress bandwidth cost is the real limit on metered clouds: a 10-person room with every subscriber on 1 Mbps streams moves ~90 Mbps out.
- Transcription cost lies in STT, not the SFU (Jitsi's new proxy uses external or self-hosted OpenAI-compatible endpoints).

### Gaps
- No LiveKit Cloud pricing figure retrieved (unchecked).

## 2024-2026 write-ups by people building small meeting apps

### Takeaway
I found no first-person Show HN or blog post (Rust or serverless) that I could cite; the best material is Cloudflare's Orange Meets and Rust SFU crates.

### Cited Findings
- Orange Meets (Cloudflare, open source, 2024; E2EE MLS update June 2025) is the main serverless reference: Workers + Durable Objects + Realtime SFU — [Cloudflare blog](https://blog.cloudflare.com/orange-me2eets-we-made-an-end-to-end-encrypted-video-calling-app-and-it-was/), [SFU intro](https://developers.cloudflare.com/realtime/sfu/introduction/)
- Rust SFU building blocks: str0m + oxpulse-sfu-kit; dev.to post on writing an SFU library in Rust — [dev.to](https://dev.to/h3poteto/developing-a-webrtc-sfu-library-in-rust-2g8p) (not read in full)
- Generic lessons from vendor guides: NAT/TURN failures behind corporate firewalls are the usual real-world failure; SFU quality needs substantial effort; recording is separate infrastructure — [DEV article](https://dev.to/car_oc/do-you-know-what-it-takes-to-create-a-webrtc-video-conferencing-application-1cc1) (vendor-ish)

### Inferences
- Plan for TURN (TCP/TLS on 443) from day one; skip recording.

### Gaps
- No 2024-2026 HN Show HN thread found despite two targeted searches; HN search (hn.algolia.com) was not queried directly and should be tried. No webrtcHacks / BlogGeek.me articles were retrieved.
