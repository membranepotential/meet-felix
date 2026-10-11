# STATUS

## Goal
Self-hosted, serverless Google-Meet replacement in Rust (functional core, imperative shell) on Cloudflare only: Realtime SFU + Rust Durable Objects, TS/Svelte shell over a Rust WASM core, three-word ephemeral links, announced per-speaker recording with a post-meeting Whisper transcript. Target < 10 €/month at real usage (Google baseline ≈16–17 €). Design: docs/README.md (decisions D1–D35), phases in docs/architecture/README.md.

## Now
- iphone-check — owner tries Mute feedback and Expand on the iPhone with the new version — waiting

## Done
- 2026-10-11 Deployed version 15b6a031: audit wave 3 (MediaLink split, mute bookkeeping, no unmute without consent, awaitingConsent flag, KV key module, e2e locators); the 2026-10-11 audit is resolved; worker 105, core 95, sfu-api 16, web 172, e2e 97 passed 3 skipped.
- 2026-10-11 Deployed version 4e1f4a76: audit wave 2 (protocol version and close codes D36, orphan sweep, recorder retries and 48 h job deadline, HTTP types from Rust, one error body, versioned meeting index); no rollback across it; worker 98, core 92, sfu-api 16, web 158, e2e 94 passed 3 skipped.
- 2026-10-11 Deployed version f8cb9611: audit wave 1 (all P1s and security findings; see DEVLOG); `SESSION_KEY` set; production checked (CSP, login, origin rule, rate limit through the custom domain); worker 83, core 77, sfu-api 16, web 152, e2e 88 passed 3 skipped.
- 2026-10-11 Published: this repo is public, with the history rewritten so no deployment details remain (the unrewritten history stays private). Deployed version e2bef2dd, commit 1dde930; production login and `/api/me` answer 200 with the owner's name and `no-store`.
- 2026-10-11 Cleanup before publishing: deployment values in git-ignored .env (wrangler.toml placeholders, rendered to wrangler.local.toml), owner name from `/api/me`, PLAN.md deleted, README rewritten, MIT LICENSE; codebase audit 4 P1 / 29 P2 / 45 P3 (reports/reviews/2026-10-11-1341, git-ignored).
- 2026-10-10 Deployed version b88fda55, commit 497ad78: polish batch (control bar stays on screen after a resize or rotation, lobby heading under the consent dialog, one capture per Start video, expected not-found pulls logged as info); web 144, e2e 66 passed 3 skipped.
- 2026-10-10 Deployed version 8c934a70, commit b3c5287: first pull retried after 500 ms (5×, then 3 s) when the SFU does not know the track yet; slowest first pull 4.2 s → 1.9 s against the real SFU; core 72, sfu-api 16.
- 2026-10-10 Deployed version 6fedc9d7, commit 397dab6: simulcast layer holds (`priorityOrdering: none`; the SFU climbed back to full after 12 s with `asciibetical`), real-SFU hold check and a five-person run (2 @real passed); sfu-api 16 tests.
- 2026-10-10 Deployed version db689400, commit 0df5c5b: recorder e2e as the Playwright project `recorder` (`pnpm test:e2e:recorder`, real Whisper, 1 passed), run.mjs deleted, data-testids in the fixtures; e2e 64 passed 3 skipped.
- 2026-10-10 Deployed version 4ed4cdfc, commit ec55203: an ended room deletes its storage an hour after End (retention alarm, pending recorder /end retried first); worker 56 tests.
- 2026-10-10 Deployed version aa8d8e50, commit dd1443c: partial publish carries the rejected tracks, NegotiationId on SFU effects and replies (late replies ignored); core 70, sfu-api 15, worker 55, web 143, e2e 64 passed 3 skipped; the @real three-party test passed afterwards.
- 2026-10-10 Deployed version b3ff7ba5, commit 8d5d8c6: D35 leave grace period (60 s; tracks and recording continue, "Reconnecting…" tile, pagehide leave, close codes logged); worker alarm ordering fix from review; core 64, worker 55, e2e 63 passed 3 skipped.
- 2026-10-10 Deployed version 644b05c1, commit 340fbee: 'Join as host' inside a live meeting; reconnect() keeps the old socket open until the resume (the owner came back as a new participant before); e2e 61 passed 3 skipped.
- 2026-10-10 Deployed version 5192341c, commit f007875: hosts see who has not accepted the recording (pill + "Not recorded" on tiles); consent.spec.ts; web 142 unit tests, e2e 59 passed 3 skipped.
- 2026-10-10 Deployed version f2bdd8b1, commit e32feb6: devices switchable inside a live meeting (Devices dialog in the control bar, track swap on the sender, last pick wins); review findings fixed; web 140 unit tests, e2e 57 passed 3 skipped.
- 2026-10-10 Deployed version 0aa254e5, commit b0150b4: mute state in core/protocol with tile icons, expanded shared screen keeping the control bar (replaces iPhone's native player), Whisper names prompt removed + avg_logprob/compression_ratio filters, four real-speech snippets in web/e2e/audio for the recorder e2e (22/22). Two code-reviewer-high reviews, findings fixed; core 57, worker 54, web 133, e2e 56.
- 2026-10-10 TURN live: the owner created a Realtime TURN key; secrets `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` uploaded; `/api/ice` on production returns STUN plus TURN credentials (port 53 filtered), and a relay-only gather in headless Chromium produced relay candidates over UDP, TCP and TLS.
- 2026-10-10 Phase 3 live: version 24f9bf24, commit 8c4a918. Production proof with headless Chromium through the real SFU: adapter connected, 3 Opus chunks (≈33 KB per 30 s) in the EU bucket, transcript ready 10–12 s after the end; with synthesized speech as the microphone the transcript has the German and English sentences correctly, labelled "Felix Peppert (host)" with wall-clock times, consent recorded. Playwright e2e: 56 local tests + @real call. R2 lifecycle rule audio/ 3 days added.
- 2026-10-09 Deployed version 7aafb17d, commit 64a56aa: D33 session replacement with epochs and timeouts, TURN via `/api/ice?meeting=`, simulcast fallback, fullscreen share, notification pills, device dedupe. Review by code-reviewer-high, all findings fixed; core 44, sfu-api 12, worker 27, web 113 tests, spike 73/73.
- 2026-10-09 Research 11 (docs/research/11-gotchas-on-cloudflare.md): 8 changes to make now (simulcast priorityOrdering, stored-record versioning, audio unlock after a session reset, iOS mic recovery control, consent record, retention, transcript-link hygiene, recorder checklist), 18 to watch; Workers Paid is required for the recorder.
- 2026-10-09 Second real test (Firefox + phone on mobile data): no server error; the phone's audio and video never reached the PC, log shows 41 × `HTTP 410 session_error` on subscribe retries against a dead session → session-recovery above. No recording existed yet (phase 3).
- 2026-10-09 First real test fixes, version b5cfcaa4: Workers Logs on; meeting list asks unsettled rooms for their phase and settles ended ones in the index (no more lost marks); owner's name prefilled; device lists deduplicated (Svelte `each_key_duplicate` seen in Firefox); start, meetings, lobby and end pages centred on desktop. Review by code-reviewer-high, findings fixed; worker 24 tests, spike 63/63, web 99 tests.
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
In this order, one at a time (owner, 2026-10-10: "step by step, at your discretion"). The 2026-10-10 backlog is done except the last core item.
1. Subscriber-side step-down on a weak link: since `priorityOrdering: none` the SFU no longer lowers the layer by itself; pick a lower layer when inbound loss is high or the bitrate low, if real use shows frozen video.
2. Start the first pull from the publisher's first packets rather than its connection: the SFU still refuses pulls for 0.5–1 s after `connected` (measured 2026-10-10).
3. Core, low value: a reply for an SFU offer on the publish session (sfu-api answers `MissingSdp` today); never observed in five real-SFU runs on 2026-10-10.
4. Known gaps documented in D35: a kick or a PublishAnswer during the grace is lost; the kicked client comes back as a new guest.
- Owner, manual: try on the iPhone the Devices dialog (front/back camera), "Reconnecting…" tiles after a drop, "Join as host" from inside a meeting; observe hibernation, egress and neurons on the dashboard; device checklist on real phones; per-speaker audio and a delete button for past meetings are unscheduled wishes from the DEVLOG.

## Open questions
- Cold starts used up to 32 ms CPU right after the deploys of 2026-10-11 (Free plan nominal 10 ms; all answered): watch Workers Logs for `exceededCpu`.
- Workers Free is the plan for now (owner, 2026-10-09): Workers AI gives 10,000 neurons/day ≈ 214 speaker-minutes of Whisper, calls beyond that fail; the transcript job retries after the daily reset. Realtime SFU/TURN bill separately (1,000 GB free). 10 ms CPU per invocation applies.
- Max audio size per Workers AI speech request? (undocumented; Cloudflare's tutorial uses 1 MB chunks)
- What does an adapter do when its source track ends? (undocumented)
- Does `SfuCloseTracks` need a renegotiation on the Cloudflare SFU? (core assumes not)
