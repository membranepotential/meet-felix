# STATUS

## Goal
Self-hosted, serverless Google-Meet replacement in Rust (functional core, imperative shell) on Cloudflare only: Realtime SFU + Rust Durable Objects, TS/Svelte shell over a Rust WASM core, three-word ephemeral links, announced per-speaker recording with a post-meeting Whisper transcript. Target < 10 €/month at real usage (Google baseline ≈16–17 €). See PLAN.md (v2).

## Now
- core — crates/meeting-core: RoomState, step(), Event/Effect, proptests — implementer running — next: integrate into worker
- worker — crates/worker + wrangler.toml: Room DO with hibernating WebSockets, /new, word links, spike 1 locally — implementer running — next: wire meeting-core
- web — web/: SvelteKit static scaffold, pre-join with consent + device preview, meeting grid shell — implementer running — next: signaling + SFU client

## Done
- 2026-10-09 Research (9 note files, synthesized report in reports/), PLAN.md v1, user feedback, PLAN.md v2 with cost analysis.

## Next
- Resolve PLAN.md §7 (invoice check, consent strength, word list language, retention, transcript sharing).
- Phase 0 spikes: Rust DO hibernation (and Free plan), adapter → Recorder DO → R2 at 10 tracks, Whisper vs Nova-3 German, iPhone/iPad, real egress.
- git init; Cargo workspace + pnpm web/ scaffold.

## Open questions
- Do the Realtime SFU and Workers AI speech models work on the Workers Free plan? (unverified)
- Max audio size per Workers AI speech request? (undocumented; Cloudflare's tutorial uses 1 MB chunks)
- What does an adapter do when its source track ends? (undocumented)
