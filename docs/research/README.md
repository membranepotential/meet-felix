# Research

The research the design rests on, all done on 2026-10-09. Cite it; don't
re-argue it without new evidence. The research came before the plan, and
some of its recommendations were changed by later decisions (for example live
captions and 128-bit meeting links); the decision pages say where. The notes
are kept as written; prices and versions are as of that date.

| Note | Covers |
|---|---|
| [00 — report](00-report-serverless-rust-video-meetings.md) | The synthesized report: Cloudflare SFU plus a Rust DO, the pure core, live STT options, TypeScript vs Rust UI, browser limits, cost per meeting-hour, open points for spikes. |
| [01 — hosting and media platforms](01-hosting-and-media-platforms.md) | Realtime SFU and TURN, Workers and DO pricing, scale-to-zero hosts for a self-run SFU, managed WebRTC platforms, bandwidth per meeting-hour. |
| [02 — Cloudflare deep dive](02-cloudflare-deep-dive.md) | The workers-rs Hibernation API, SFU features and limits, RealtimeKit and the WebSocket adapter, Workers AI streaming STT, data residency, DO limits. |
| [03 — self-hosted Fly.io path](03-selfhosted-fly-path.md) | The rejected fallback: str0m SFU on Fly.io Machines, UDP on Fly, other on-demand VMs, CPU and GPU transcription, Opus and resampling in Rust. |
| [04 — Rust WebRTC stack](04-rust-webrtc-stack.md) | str0m, webrtc-rs, existing Rust SFUs, TURN, signaling design, audio tap for transcription, testing sans-IO WebRTC logic. |
| [05 — frontend and browser](05-frontend-and-browser.md) | Rust web frameworks, web-sys gaps, a TypeScript shell over a Rust WASM core, browser limits in 2026, PWA and push, codecs, grid layouts. |
| [06 — existing projects](06-existing-projects-lessons.md) | Jitsi, LiveKit and other open-source meeting apps: architecture, mesh vs SFU, client features, invite links and lobbies, running costs. |
| [07 — live transcription](07-live-transcription.md) | Streaming STT services, in-browser and self-hosted models, per-speaker transcription in an SFU, how products build live captions. Background only: live captions are a non-goal. |
| [08 — Google Workspace baseline](08-google-workspace-baseline.md) | Workspace prices in Germany, which plans include Meet transcripts, Google AI Pro "Take notes for me". |
| [09 — post-meeting transcription and recording](09-post-meeting-transcription-recording.md) | Workers AI batch speech-to-text (Whisper, Nova-3), R2 from a Rust DO, Workflows vs DO alarms, per-speaker capture through the adapter, audio sizes, pure-Rust encoders. |
| [10 — custom hostname on external DNS](10-custom-hostname-external-dns.md) | Serving the Worker as `meet.example.com` with DNS at an external provider: why a CNAME to workers.dev fails, Pages custom domain plus service binding, Cloudflare for SaaS, relay VMs, redirects, ranked options. |
| [11 — gotchas on Cloudflare, after the first real tests](11-gotchas-on-cloudflare.md) | Three researchers (browser WebRTC against the SFU, the Cloudflare platform and plans, the recording and transcription path incl. German law) and one report, 2026-10-09: 8 changes to make now, 18 things to watch, 42 sources. Raw notes in [11-notes/](11-notes/). One finding is overruled by the owner: the recording notice does not name subprocessors (see D15). |
