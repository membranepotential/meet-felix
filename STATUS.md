# STATUS

## Goal
Self-hosted, serverless Google-Meet replacement in Rust (functional core, imperative shell) on Cloudflare only: Realtime SFU + Rust Durable Objects, TS/Svelte shell over a Rust WASM core, three-word ephemeral links, announced per-speaker recording with a post-meeting Whisper transcript. Target < 10 €/month at real usage (Google baseline ≈16–17 €). Design: docs/README.md (decisions D1–D21), phases in docs/architecture/README.md.

## Now
- worker-review — code-reviewer-high on crates/worker + crates/sfu-api (commit 62ef0fe) — running — next: fix must-fix items before first deploy
- core-followups — meeting-core: Unpublish, publish timeout, parseServerMsg/encodeClientMsg wasm exports, rid names f/h/q — implementer running — next: adapt web literals, commit
- e2e-local — two headless browsers through wrangler dev with the real SFU credentials (worktree at HEAD) — agent running — next: record result in docs/spikes/02-first-call.md

## Done
- 2026-10-09 Worker runs meeting_core::step; sfu-api client (OpenAPI types); spike 34/34 with mock SFU (verified). Commit 62ef0fe.
- 2026-10-09 Web on the real protocol (generated types, reducer, both negotiations, keepalive, 25 vitest tests); assets + build/deploy scripts. Commit e372f86.
- 2026-10-09 Research (10 files, now docs/research/), plan v1 → user feedback → plan v2 → docs/ in the jumanji pattern (D1–D21, DEVLOG, TESTING).
- 2026-10-09 Repo: git, private GitHub repo membranepotential/meet-felix, Cargo workspace, pnpm workspace with wrangler 4.147, worker-build, wrangler login done.
- 2026-10-09 crates/meeting-core: ids, protocol, RoomState, step(), 1,848-word list, 31 tests incl. 512-case proptest; review fixes (consent gate, retry, empty timer, resume); clippy + wasm check clean (verified at root). Committed d1b28f9.
- 2026-10-09 web/: SvelteKit 3 static SPA, pre-join with consent, media layer, signaling/SFU placeholders, responsive grid; check/lint/build pass (verified).
- 2026-10-09 crates/sfu-api: placeholder manifest so the workspace builds.

## Next
- Core later: grace period before a dropped socket counts as Leave; negotiation ids on SFU replies; cleanup of ended-room storage.
- Web later: active speaker, remote mute state, peer-connection failure recovery, waiting-for-consent list for hosts.
- First deploy: `wrangler secret put` CREATE_SECRET/CALLS_APP_ID/CALLS_APP_SECRET, `pnpm deploy`, observe hibernation on the dashboard (Free plan test). KV MEETINGS and the SFU app exist (.dev.vars).
- Phase 2: SFU sessions in the worker (sfu-api crate), publish/subscribe wiring in web, simulcast, screen share, lock/knock, reconnect.
- Phase 3: RecorderObject, adapters, R2 chunks, Whisper job, transcript page.

## Open questions
- Do the Realtime SFU and Workers AI speech models work on the Workers Free plan? (unverified)
- Max audio size per Workers AI speech request? (undocumented; Cloudflare's tutorial uses 1 MB chunks)
- What does an adapter do when its source track ends? (undocumented)
- Does `SfuCloseTracks` need a renegotiation on the Cloudflare SFU? (core assumes not)
