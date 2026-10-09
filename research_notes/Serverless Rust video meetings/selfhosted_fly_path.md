# Self-hosted scale-to-zero fallback: str0m SFU on Fly.io Machines (checked 2026-10-09)

## 1. Fly.io Machines lifecycle, start/stop API, autostart semantics, billing

### Takeaway
A stopped Fly Machine restarts in "well under a second" per Fly's docs (the image is already assembled on the host), and suspend/resume is documented at "a few hundred ms" vs about 2+ s cold start, but suspend is not guaranteed and is a poor fit for an SFU. Start/stop is a plain REST call (`POST /v1/apps/{app}/machines/{id}/start|stop`, Bearer token), so a Cloudflare Worker can start it. Fly's autostart is a Fly Proxy feature; no source says UDP triggers it, so an explicit API start is the only safe design.

### Cited Findings
- Stopped Machine: VM shut down but components stay assembled on the worker host; booting an assembled Machine "usually takes well under a second"; start from stopped has the same timing. Creating a new Machine ("created" state) can take "low double digit seconds"; destroyed -> new is slower than restart. — [Fly Machines overview](https://docs.fly.io/machines/overview)
- Start: `POST /v1/apps/{app_name}/machines/{machine_id}/start`; Stop: `POST .../stop` (optional body `signal`, `timeout`); Suspend: `POST .../suspend`; Wait: `GET .../wait?state=started|stopped|suspended|destroyed&timeout=<s, default 60>` (`instance_id` required for `stopped`). Auth: `Authorization: Bearer ${FLY_API_TOKEN}`, `Content-Type: application/json`. The page uses `${FLY_API_HOSTNAME}`, does not name the host or token types, no rate limits listed. — [Machines resource API](https://docs.fly.io/machines/api/machines-resource)
- A search summary of Fly docs gives the host as `api.machines.dev` (docs.machines.dev is the OpenAPI reference); not seen verbatim on the fetched page, treat as likely but re-check. — [Fly Machines API skill / search result](https://fly.io/agent-skills/fly-machines-api/SKILL.md)
- A stopped Machine "is completely reset to its original state" on start (rootfs reset). Suspended Machine: next start tries to resume from snapshot, not guaranteed, otherwise cold boot (rootfs not reset); stopping a suspended Machine invalidates the snapshot. — [Machines resource API](https://docs.fly.io/machines/api/machines-resource)
- Suspend: resume "a few hundred ms" vs "about 2+ seconds" cold start for common apps; limit memory <= 2 GB; TCP connections "may survive", page says nothing about UDP sockets after resume; clock may lag until NTP syncs; snapshot can be lost on deploy/host migration/maintenance, design for both paths. — [Fly suspend/resume reference](https://docs.fly.io/reference/suspend-resume)
- Community: a user reported Machines with volumes restart instead of resume (4-5 s vs 250 ms); suspending can sit >15 s in "suspending" with lots of memory (later fixed so Machines can start from suspending). — [community thread](https://community.fly.io/t/new-feature-in-preview-suspend-resume-for-machines/20672), [fix announcement](https://community.fly.io/t/machines-can-now-start-from-the-suspending-state/26623) (second-hand via search summary, unverified in detail)
- `auto_stop_machines`: `"off"|"stop"|"suspend"`, Fly Proxy acts when the app is idle for several minutes (proxy stop loop runs every few minutes, at most one Machine per region per pass). `auto_start_machines`: starts Machines "based on requests and capacity"; enable both or neither. The page does not say which protocols trigger autostart and does not mention UDP; apps without services on the private network are not routed through Fly Proxy and cannot be auto-started/stopped. — [Autostop/autostart docs](https://docs.fly.io/launch/autostop-autostart)
- Billing: started Machines billed per second; stopped and suspended Machines billed only for rootfs at $0.15/GB/month, per second; no separate snapshot charge on this page. — [Fly billing](https://docs.fly.io/about/billing)
- Pattern "start from external trigger": call `/start`, then `/wait?state=started`, then forward to the app. `fly-replay: instance=<id>` after start is reported to fail silently in one thread (unresolved). I found no official Cloudflare-Worker example; the Worker-calls-API design is my inference. — [community thread](https://community.fly.io/t/fly-replay-to-specific-instance-failing-despite-machine-being-started-and-responsive/26928)
- Platform risk: Fly's VP Engineering (2026-08-03) stated "Machines are not getting wound down", no sunset timeline, after a blog post refocusing the company on Sprites triggered worries. — [community thread](https://community.fly.io/t/will-the-apps-and-machine-platform-start-winding-down/28380)

### Inferences
- Realistic "meeting start -> SFU ready" latency: API start + boot of a Rust binary is probably 1-5 s, dominated by app init and the client learning the machine is up; measure it, no official number for a custom image exists.
- Use stop, not suspend: UDP socket behaviour after resume is undocumented, snapshots are not guaranteed, and a sub-second saving does not matter for a meeting that starts in a scheduled way.
- Autostart cannot be relied on for UDP; the signalling service (HTTP/WebSocket via Worker) should call `/start` itself and tell clients the ICE address once `/wait` returns.
- Idle cost is about rootfs only (e.g. 1 GB image ~ $0.15/month) plus the $2/month dedicated IPv4 that earlier research covered.

### Gaps
- No official cold-start figure for a custom image; no source on UDP autostart; unconfirmed official API host/token type on the fetched page (`fly tokens create deploy` style tokens not verified); no verified rate limits.

## 2. Fly.io UDP specifics for WebRTC

### Takeaway
UDP needs a dedicated IPv4, binding to `fly-global-services`, and the same internal and external port; Fly only rewrites the IP. That fits str0m behind a single UDP port, advertised as a fixed public host candidate. No dated Fly report of Pion/mediasoup/str0m was found, but a July 2026 LiveKit-on-Fly report shows media works once the socket binds correctly.

### Cited Findings
- UDP needs a dedicated IPv4 (not shared v4, not IPv6); app must bind `fly-global-services` and reply from it; `0.0.0.0`/`*` can make Linux reply with the wrong source address; Fly rewrites only the IP, "Fly will not rewrite the port"; plan ~1300-byte MTU for custom UDP protocols. The page says nothing about port ranges or autostart for UDP. — [Fly UDP and TCP docs](https://docs.fly.io/networking/udp-and-tcp)
- LiveKit on Fly (post 2026-07-20, single user, no replies): wildcard bind breaks replies; fix was setting `rtc.ips.includes` to the `fly-global-services` IP /32 and `turn.bind_addresses`; IP resolved at boot from `/etc/hosts`; ICE connected and media published for livekit-server; livekit-sip failed. Single-user account. — [community post](https://community.fly.io/t/running-livekit-livekit-server-livekit-sip-on-fly-the-fly-global-services-udp-bind-requirement-and-a-specific-fix/28316)
- WebRTC port-range thread (Nov 2024): user tried UDP ranges 30000-31000 and a single port 4000, unresolved; replies say the internal port must match external and a range may not work; binding to `fly-global-services` is application dependent. No Fly staff reply. — [community thread](https://community.fly.io/t/opening-port-range-for-webrtc/22533)
- An older thread answer says you can listen on any UDP range and receive traffic (undated in my summary, possibly outdated); conflicts with the 2024 thread above. — [community thread](https://community.fly.io/t/define-port-range-for-service/1938)
- Elixir WebRTC guide for Fly: needs a STUN server, ICE IP filter, and a dedicated IPv4; closest precedent for a non-LiveKit stack. — [ex_webrtc Fly guide](https://ex-webrtc.hexdocs.pm/fly.html)
- str0m docs: chat example acts as an SFU and multiplexes everything over one UDP socket. — [str0m on docs.rs](https://docs.rs/str0m)

### Inferences
- Single UDP port (e.g. 10000) bound to the `fly-global-services` address, declared in `fly.toml` as a UDP service, with a fixed host candidate for the dedicated IPv4 added via str0m's `add_local_candidate`, avoids ranges entirely. How the app learns its IP: the dedicated IPv4 is known at allocation time (`fly ips list`), so pass it as an env var/secret; `fly-global-services` resolves inside the Machine (the LiveKit report read it from `/etc/hosts`). Unverified: whether the packet destination seen inside the Machine is the public IP or a private mapping; since only the IP is rewritten, advertising the public IPv4 as the candidate (no STUN) is the likely correct setup. Test early.
- Full ICE is simplest; ice-lite is optional.
- Single UDP port per IPv4 means one meeting-SFU Machine per dedicated IP, or one Machine multiplexing many meetings by ufrag.

### Gaps
- No dated report of Pion, mediasoup or str0m SFU on Fly 2024-2026 beyond LiveKit (July 2026) and Elixir WebRTC; no verification of port-range support for UDP; TURN/TCP fallback not covered.

## 3. Alternatives for on-demand VMs (EU, stop means no cost)

### Takeaway
Most VPS clouds keep billing stopped servers (Hetzner, DigitalOcean, Vultr), so the only "stop" semantic that truly cuts compute cost is Scaleway, GCE, Oracle (some shapes) and Exoscale; on Hetzner you must delete and recreate from a snapshot (hourly billing, slower than a Fly start). Fly's stopped Machine is the cheapest idle and fastest to start; GCE is the closest big-cloud analogue.

### Cited Findings
- Hetzner: billed "until you delete them, independent of their state", "regardless of whether it is turned on or not"; hourly rate, rounded up; capped monthly price; only outgoing traffic billed. — [Hetzner billing FAQ](https://docs.hetzner.com/cloud/billing/faq/)
- Hetzner snapshot-and-recreate: API `POST https://api.hetzner.cloud/v1/servers` with `image` = snapshot ID; no official create/boot timing found (a third-party tool doc claims about 10 s boot, unverified). Snapshot cost per GB/month (figure not verified). — [tutorial](https://community.hetzner.com/tutorials/create-server-via-batch-file)
- Scaleway: stopped Instance "compute billing stops, but you continue to pay for any attached storage" and Flexible IPs. Stop keeping local storage = "storage + IP (no compute)"; stop with archive = "snapshot storage + IP (no compute)", restored automatically on restart. A separate standby page/FAQ result suggested standby is billed as running (conflicting, not resolved). — [Scaleway instance states](https://www.scaleway.com/en/docs/instances/reference-content/instance-states.md)
- Vultr: stopped instances still billed; delete to stop. — [Vultr docs](https://docs.vultr.com/support/platform/billing/are-stopped-instances-still-billed-on-vultr)
- DigitalOcean: powered-off Droplets still billed (staff answers in community Q&A, not official docs). — [DO community](https://www.digitalocean.com/community/questions/why-i-get-billed-for-droplet-when-i-poweroff-from-terminal)
- Oracle OCI: stopping pauses compute billing for standard shapes (still counts toward limits); dense I/O, most GPU, HPC and optimized bare metal keep billing. — [OCI stopped-instance billing](https://docs.oracle.com/iaas/Content/Compute/Tasks/resource-billing-stopped-instances.htm) (via search summary, page not fetched directly)
- Exoscale: powered-off instances are charged only for resources still used (attached volume); trial accounts are charged compute even when stopped. — [Exoscale billing](https://community.exoscale.ch/documentation/platform/billing) (via search summary)
- UpCloud: no official statement found; third-party Spare Cores says billing depends on family. — [Spare Cores](https://sparecores.com/vendors/upcloud) (unverified)
- GCE: per-second billing with 1-minute minimum; stopped VM stops vCPU/memory charges but attached persistent disks and reserved static external IPs stay billed; Spot VMs can be preempted. — [GCE pricing](https://cloud.google.com/compute/all-pricing) (page truncated; confirmed by [stop/start docs](https://docs.cloud.google.com/compute/docs/instances/stop-start-instance): "you keep incurring charges for any resources that remain attached")

### Inferences
- Cheapest near-zero idle with API start: Fly (rootfs-only) first; then GCE (disk + optional ephemeral external IP; EU regions e.g. europe-west3 Frankfurt exist, not re-verified) or Scaleway (stopped, storage-only). Whether Scaleway/GCE reach "running with a bound port" in <30 s is unmeasured by me; GCE start of a stopped VM typically takes tens of seconds (unverified).
- Hetzner is cheapest while running but delete/recreate adds snapshot storage and creation latency; not scale-to-zero friendly.

### Gaps
- No measured start times for Hetzner/Scaleway/GCE/Exoscale; no Exoscale/UpCloud/OCI EU region-specific prices; Scaleway standby conflict.

## 4. Rust transcription on CPU

### Takeaway
whisper-rs is current (0.16.0, 2026-03-12) but its GitHub repo is archived and development moved to Codeberg; no authoritative whisper.cpp real-time-factor table for small/base/large-v3-turbo with German was found. The official sherpa-onnx crate (1.13.8) exposes true streaming models; for German the exact available ones are Kroko streaming zipformer (licence unclear) and, offline-only, NeMo German models and Parakeet v3.

### Cited Findings
- whisper-rs 0.16.0 (2026-03-12), 0.15.1 (2025-09-10). — [crates.io API](https://crates.io/api/v1/crates/whisper-rs)
- whisper-rs repo is archived/read-only, maintainer moved to Codeberg `codeberg.org/tazz4843/whisper-rs`; features (all off by default): `cuda`, `vulkan`, `openblas`, `metal`, `hipblas`, `raw-api`, `log_backend`, `tracing_backend`; Linux should build without extra setup; README examples take a fixed 16 kHz mono f32 buffer, no streaming example. — [whisper-rs GitHub](https://github.com/tazz4843/whisper-rs)
- Rough Whisper memory (third-party, GGML): base ~388 MB, small ~852 MB, turbo ~2.3 GB RAM; turbo = 809M params with 4 decoder layers, encoder identical to large-v3. — [OpenWhispr](https://openwhispr.com/blog/whisper-model-sizes-explained) (approximate, third-party)
- sherpa-onnx crates: `sherpa-onnx` 1.13.8 (2026-09-11), official repo k2-fsa/sherpa-onnx, "Safe Rust wrapper"; `sherpa-rs` 0.6.8 (2025-10-05, thewh1teagle, appears stale). — [sherpa-onnx crate](https://crates.io/api/v1/crates/sherpa-onnx), [sherpa-rs crate](https://crates.io/api/v1/crates/sherpa-rs)
- Exact model names in sherpa-onnx `asr-models` release assets that include German:
  - Streaming: `sherpa-onnx-streaming-zipformer-de-kroko-2025-08-06`
  - Offline: `sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` (25 European languages incl. German), `sherpa-onnx-nemo-canary-180m-flash-en-es-de-fr` (+`-int8`), `sherpa-onnx-nemo-fast-conformer-ctc-en-de-es-fr-14288` (+`-int8`), `sherpa-onnx-nemo-fast-conformer-ctc-be-de-en-es-fr-hr-it-pl-ru-uk-20k` (+`-int8`), `sherpa-onnx-nemo-stt_de_fastconformer_hybrid_large_pc` (+`-int8`). Assets list was truncated, so the list may be incomplete; Whisper (incl. `sherpa-onnx-whisper-turbo`) also exists as an offline family. — [sherpa-onnx asr-models release](https://github.com/k2-fsa/sherpa-onnx/releases/expanded_assets/asr-models)
- Parakeet TDT 0.6B v3: German among 25 languages; ~600M params; CC BY 4.0; German WER about 5.04% (Fleurs) / 4.84% (CoVoST); model card mentions chunked streaming via NeMo script. — [HF model card](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
- Kroko German streaming models added in sherpa-onnx 1.12.10; licence: one aggregator says non-commercial dual licence, unverified. — [sherpa-onnx changelog via search](https://classic.yarnpkg.com/en/package/sherpa-onnx), [aggregator](https://www.promptlayer.com/models/kroko-asr)
- CPU benchmark (English only): Nemotron speech streaming 0.6B int8 via sherpa-onnx 1.13.8 on a 6-core Ryzen 7500F: RTF 0.067 for 1 stream (6 threads), 0.327 per stream with 12 concurrent, keeps up through 24 callers; ~1.0 GB RSS +100 MB per 4 streams; author plans a Hetzner cx23 (2 shared vCPU) but did not benchmark it. A 40-language Nemotron 3.5 release exists, German not confirmed. — [Softcery](https://softcery.com/lab/hosting-nemotron-asr-streaming-on-a-cpu)
- whisper.cpp: no `stream`-example performance figures or German RTF found. A forum-style table for large-v3-turbo on Ryzen 7700X (faster-whisper, int8) of ~35x realtime is unreliable (conflicting sources). — [search summary](https://smartscope.blog/en/generative-ai/foundations/whisper-local-cpu-implementation/) (unverified)

### Inferences
- With 2-10 speakers, run one decoder per speaker track (or a mixed stream with diarization lost). The Softcery scaling (RTF 0.33 at 12 streams on 6 cores) suggests a streaming transducer model can handle 10 streams on 4 dedicated cores, but that is English Nemotron; the German Kroko zipformer is smaller and likely cheaper, though no German RTF exists. Benchmark on the target Fly CPU (shared-cpu is noisy).
- Whisper is not a streaming model; sliding-window re-decoding multiplies CPU cost. Per-utterance (VAD-segmented) offline decoding with Parakeet v3 or Whisper turbo is an alternative if a few seconds of delay is acceptable.
- Not recommended on shared-cpu-2x without measurement.

### Gaps
- No verified whisper.cpp RTF for base/small/large-v3-turbo on x86 with German; no candle whisper benchmark found (not searched beyond one pass); no German WER for Kroko; Kroko licence unverified; Parakeet v3 RTF on CPU in sherpa-onnx not found.

## 5. Scale-to-zero GPU for transcription

### Takeaway
Fly GPUs are gone (deprecated, unavailable after July 31/Aug 1, 2026). Modal has per-second billing and an `eu` region at a 1.15x multiplier, but no verified WebSocket doc; Beam has a documented realtime WebSocket decorator; Baseten has WebSocket Whisper streaming and an EU regional flag; RunPod serverless has no verified WebSocket support.

### Cited Findings
- Fly GPUs: a customer email "Fly.io GPUs will be fully deprecated as of July 31, 2026" (posted 2026-02-11); another user cites the earlier Fly blog "we're not getting rid of them"; search summary of Fly docs says GPUs unavailable after Aug 1. — [community thread](https://community.fly.io/t/gpu-migration-fly-io-gpus-will-be-deprecated-as-of-july-31-2026/27110), [Fly blog](https://fly.io/blog/wrong-about-gpu/)
- Modal pricing per second: T4 $0.000164, L4 $0.000222, A10 $0.000306, L40S $0.000542; CPU $0.0000131/core/s; $30/month free (Starter). Regions: `eu`, `eu-west`, `eu-north`, `eu-south` (no Germany-specific); broad region multiplier 1.15x, narrow 1.75x. Container boots in "about one second" but model load can take seconds to minutes; memory snapshots available. WebSocket support not confirmed on pages fetched (asgi_app supports ASGI; docs guide mentions websockets per a third-party index). — [Modal pricing](https://modal.com/pricing), [region selection](https://modal.com/docs/guide/region-selection), [cold start](https://modal.com/docs/guide/cold-start), [web functions](https://modal.com/docs/guide/webhooks)
- RunPod Serverless: billed per second, rounded up; worker start time billed (model load); pricing page shows hourly: 16 GB tier $0.58/h, 24 GB (L4, A5000, 3090) $0.69/h, 4090 $1.10/h (flex/active column not labelled). Load-balancing endpoints (beta) route directly to workers; WebSocket support not confirmed. EU: RunPod has EU-RO-1 (third-party listing); GDPR compliance page exists. — [RunPod pricing docs](https://docs.runpod.io/serverless/pricing), [pricing](https://www.runpod.io/pricing), [load balancing](https://docs.runpod.io/serverless/load-balancing/overview)
- Beam: per-second; docs list T4 $0.00017222/s, A10G $0.00046716/s (older beta9 docs) while the current page shows 4090 $0.000192/s and A10G $0.000292/s; `@realtime` decorator exposes a public `wss://` WebSocket endpoint (example is CPU-only); GPU support and connected-time billing not documented; default keep-warm 90 s; EU region not found. — [Beam pricing docs](https://docs.beta9.beam.cloud/account/pricing-and-billing), [beam.cloud pricing](https://beam.cloud/pricing), [realtime docs](https://docs.beam.cloud/v2/endpoint/realtime.md)
- Baseten: Whisper Large V3 streaming over WebSockets in its model library; per-minute dedicated GPU billing with scale-to-zero: T4 $0.01052/min, L4 $0.01414/min (third-party aggregators, July 2026, not verified on Baseten's page); regional deployments with `--region eu` (changelog 2026-09-10). — [Baseten WebSocket tutorial](https://baseten.co/blog/zero-to-real-time-transcription-the-complete-whisper-v3-websockets-tutorial), [regional changelog](https://www.baseten.co/resources/changelog/regional-deployments/), [pricing aggregator](https://www.morphllm.com/baseten-pricing)

### Inferences
- A GPU is likely overkill for transcribing a handful of tracks, and cold start (container + model load, seconds to minutes) is larger than a Fly CPU Machine start; the Rust SFU would hold a WebSocket client to the GPU service and keep it warm for the meeting duration, paying per second of the open connection.
- Ranking for this need (WebSocket + EU + per-second): Baseten (ready streaming Whisper, EU flag) and Modal (EU, cheap) look best; verify WebSocket on Modal before choosing.

### Gaps
- No verified cold-start numbers for any provider's Whisper container; Beam/RunPod EU availability and WebSocket+GPU support unverified; Baseten official price not fetched.

## 6. Opus decoding and resampling in Rust

### Takeaway
Use the `opus` crate 0.4.0 (libopus bindings; needs cmake and a C compiler by default) plus `rubato` 5.0.1; a pure-Rust `opus-decoder` 0.1.1 exists but is brand new and unvetted; Symphonia has no first-party stable Opus path (third-party libopus adapter).

### Cited Findings
- `opus` 0.4.0, updated 2026-08-23, "Safe Rust bindings for libopus", repo SpaceManiac/opus-rs; README: by default needs `cmake` and a C compiler (via `opusic-sys`); targets libopus 1.6 API. Whether libopus is bundled or system-linked is not stated. — [crates.io](https://crates.io/api/v1/crates/opus), [opus-rs README](https://github.com/SpaceManiac/opus-rs)
- `opus-decoder` 0.1.1 (2026-03-12): "Pure-Rust Opus decoder, RFC 8251 conformant, no unsafe, no FFI"; repo TadeuszWolfGang/Rusopus; earlier release listed a different repo URL; single-maintainer, not audited by me. — [crates.io](https://crates.io/api/v1/crates/opus-decoder)
- `symphonia` 0.6.1 (2026-08-13), "Pure Rust media container and audio decoding library"; its codec table lists Opus under `symphonia-codec-opus` with no maturity rating, and the wiki lists a third-party `symphonia-adapter-libopus` (needs cmake). — [crates.io](https://crates.io/api/v1/crates/symphonia), [Symphonia wiki](https://github.com/pdeljanov/Symphonia/wiki) (via search summary)
- `rubato` 5.0.1, updated 2026-10-01. — [crates.io](https://crates.io/api/v1/crates/rubato)
- str0m delivers depacketized RTP payloads (Opus frames) to the app, so the tap only needs an Opus frame decoder, not a container reader. — [str0m docs](https://docs.rs/str0m) (my understanding, not re-verified here)

### Inferences
- Opus RTP is 48 kHz; Whisper/sherpa need 16 kHz mono, so rubato (or a simple 3:1 decimator) follows decoding. Use `opus` for known-good behaviour; the cmake requirement is a build-time cost only (Docker builder stage).
- Packet loss concealment: libopus offers PLC/FEC; confirm the binding exposes it before relying on it.

### Gaps
- Not verified: whether `opus` 0.4.0 vendors libopus; Symphonia's current native Opus decoder status; `opus-decoder` quality/maintenance.
