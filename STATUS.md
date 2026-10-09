# STATUS

## Goal
Self-hosted, serverless Google-Meet replacement in Rust (functional core, imperative shell) on Cloudflare only: Realtime SFU + Rust Durable Objects, TS/Svelte shell over a Rust WASM core, three-word ephemeral links, announced per-speaker recording with a post-meeting Whisper transcript. Target < 10 €/month at real usage (Google baseline ≈16–17 €). Design: docs/README.md (decisions D1–D21), phases in docs/architecture/README.md.

## Now
- deployed-check — user tests https://meet.example.com from phone + laptop (two networks, 10 min) — waiting

## Done
- 2026-10-09 Quiet subscribe retries and Scheduled/Live/Ended labels; deployed version 4e79e91a.
- 2026-10-09 Redeployed (version 7f62b3e0); production probe of the owner flow through meet.example.com green; call test run 5 (owner session, scheduled meeting, join as host) 70/70 locally.
- 2026-10-09 D32 one owner; owner session cookie (worker 23 tests, spike 62/62); scheduled lifecycle in the core (37 tests); login-first web with join-as-host (93 tests); UI decisions D22–D31 implemented. Commits 626bf6a…b4c8b15.
- 2026-10-09 https://meet.example.com live: Pages front door `meet-felix-edge` (edge/dist/_worker.js, service binding) + external CNAME; HTTPS, API and a real WebSocket join verified (Welcome in 626 ms). Research in docs/research/10.
- 2026-10-09 UI decisions D22–D31 recorded (docs/ui). User added: login-first index page with a session cookie, scheduled meetings (link valid until used), join-as-host fallback.
- 2026-10-09 First deploy: https://meet-felix.example.workers.dev (version b9384325), secrets CREATE_SECRET/CALLS_APP_ID/CALLS_APP_SECRET set, HTTP smoke test green (200/404/401/201/426). Script renamed to `pnpm run release`.
- 2026-10-09 Phase 1 accepted locally: run 4 of the real-SFU call test 43/44 (docs/spikes/02-first-call.md), host receives media, 3-party call held 3 min. Commit 15abbe6.
- 2026-10-09 First real call through the SFU (docs/spikes/02-first-call.md): 24/31 checks; host-side subscribe race diagnosed and fixed in sfu-api (commit 94f1e7a). Worker review fixes (alarm loop, effect order, stale sockets). Core follow-ups (Unpublish, publish timeout, wasm parse/encode, rids q/h/f). Web adapted (54 tests).
- 2026-10-09 Worker runs meeting_core::step; sfu-api client (OpenAPI types); spike 34/34 with mock SFU (verified). Commit 62ef0fe.
- 2026-10-09 Web on the real protocol (generated types, reducer, both negotiations, keepalive, 25 vitest tests); assets + build/deploy scripts. Commit e372f86.
- 2026-10-09 Research (10 files, now docs/research/), plan v1 → user feedback → plan v2 → docs/ in the jumanji pattern (D1–D21, DEVLOG, TESTING).
- 2026-10-09 Repo: git, private GitHub repo membranepotential/meet-felix, Cargo workspace, pnpm workspace with wrangler 4.147, worker-build, wrangler login done.
- 2026-10-09 crates/meeting-core: ids, protocol, RoomState, step(), 1,848-word list, 31 tests incl. 512-case proptest; review fixes (consent gate, retry, empty timer, resume); clippy + wasm check clean (verified at root). Committed d1b28f9.
- 2026-10-09 web/: SvelteKit 3 static SPA, pre-join with consent, media layer, signaling/SFU placeholders, responsive grid; check/lint/build pass (verified).
- 2026-10-09 crates/sfu-api: placeholder manifest so the workspace builds.

## Next
- 'Join as host' inside a live meeting; shorten the first-pull retry (3 s) or pull after the publisher connects.
- Core later: reply for an SFU offer on the publish session; replace a subscribe session after 410; partial publish carries accepted tracks; grace period before a dropped socket counts as Leave; negotiation ids on SFU replies; cleanup of ended-room storage.
- Web later: active speaker, remote mute state, peer-connection failure recovery, waiting-for-consent list for hosts.
- Observe hibernation and egress on the dashboard after real use; decide Free vs Paid plan.
- Phase 2 remaining: TURN credentials, screen share end-to-end test, >3 participants, 'Join as host' inside a live meeting, device checklist on real phones.
- Phase 3: RecorderObject, adapters, R2 chunks, Whisper job, transcript page.

## Open questions
- Do the Realtime SFU and Workers AI speech models work on the Workers Free plan? (unverified)
- Max audio size per Workers AI speech request? (undocumented; Cloudflare's tutorial uses 1 MB chunks)
- What does an adapter do when its source track ends? (undocumented)
- Does `SfuCloseTracks` need a renegotiation on the Cloudflare SFU? (core assumes not)
