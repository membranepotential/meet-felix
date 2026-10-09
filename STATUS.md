# STATUS

## Goal
Self-hosted, serverless Google-Meet replacement in Rust (functional core, imperative shell) on Cloudflare only: Realtime SFU + Rust Durable Objects, TS/Svelte shell over a Rust WASM core, three-word ephemeral links, announced per-speaker recording with a post-meeting Whisper transcript. Target < 10 €/month at real usage (Google baseline ≈16–17 €). Design: docs/README.md (decisions D1–D21), phases in docs/architecture/README.md.

## Now
- worker — crates/worker + wrangler config: Room DO with hibernating WebSockets, /api/new, word links, spike 1 locally — implementer running — next: wire meeting-core
- core review — code-reviewer-high on crates/meeting-core — running — next: fix findings, then wire into worker

## Done
- 2026-10-09 Research (10 files, now docs/research/), plan v1 → user feedback → plan v2 → docs/ in the jumanji pattern (D1–D21, DEVLOG, TESTING).
- 2026-10-09 Repo: git, private GitHub repo membranepotential/meet-felix, Cargo workspace, pnpm workspace with wrangler 4.147, worker-build, wrangler login done.
- 2026-10-09 crates/meeting-core: ids, protocol, RoomState, step(), 2,000-word list, 20 tests incl. 512-case proptest; clippy + wasm check clean (verified at root).
- 2026-10-09 web/: SvelteKit 3 static SPA, pre-join with consent, media layer, signaling/SFU placeholders, responsive grid; check/lint/build pass (verified).
- 2026-10-09 crates/sfu-api: placeholder manifest so the workspace builds.

## Next
- Integrate: worker RoomObject runs meeting_core::step; generate web/src/lib/protocol.ts from tsify output; replace hand-written types.
- Commit + push phase-1 skeleton after review fixes.
- Cloudflare resources: Realtime SFU app (dashboard), KV namespace MEETINGS, R2 bucket; deploy spike 1 and observe hibernation on the dashboard (and test the Free plan).
- Phase 2: SFU sessions in the worker (sfu-api crate), publish/subscribe wiring in web, simulcast, screen share, lock/knock, reconnect.
- Phase 3: RecorderObject, adapters, R2 chunks, Whisper job, transcript page.

## Open questions
- Do the Realtime SFU and Workers AI speech models work on the Workers Free plan? (unverified)
- Max audio size per Workers AI speech request? (undocumented; Cloudflare's tutorial uses 1 MB chunks)
- What does an adapter do when its source track ends? (undocumented)
- Does `SfuCloseTracks` need a renegotiation on the Cloudflare SFU? (core assumes not)
