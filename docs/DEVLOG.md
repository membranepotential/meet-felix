# Development log

Newest entries first. Each entry: what happened, what was decided, what's
next.

## 2026-10-09 — research, plan v2, repo, first workstreams, docs

Research: nine notes and a synthesized report on running small video
meetings serverless with Rust ([research/](research/README.md)). The report
recommended Cloudflare Realtime SFU for media, one Rust Durable Object per
meeting for signaling, and a pure Rust core shared with the browser as WASM.

Plan v1 followed the report, including live captions, a knock queue by
default, and the str0m-on-Fly.io path as a fallback.
After the owner's feedback, plan v2 changed course:

- no live captions; a transcript after the meeting, from per-speaker audio
  recorded on the server and transcribed by Whisper large-v3-turbo
  ([recording/](recording/README.md));
- three-word meeting links that live only while the meeting runs, with an
  optional room lock instead of a default knock queue ([links/](links/README.md));
- Cloudflare only, no fallback hosting ([architecture/](architecture/README.md));
- a cost model against Google Workspace Business Standard
  ([cost/](cost/README.md)).

Decided by the owner the same day: no further price analysis (D21); an
explicit "Join and accept recording" button (D15); short English words (D6);
recordings and transcripts kept indefinitely for now (D18); the transcript
link shown to everyone at the end, email later (D19).

Repo: git repository, Cargo workspace (`meeting-core`, `sfu-api`, `worker`),
pnpm workspace with wrangler, CLAUDE.md and STATUS.md.

Three implementers started in parallel: the core (`crates/meeting-core`:
`RoomState`, `step`, events, effects, property tests), the worker
(`crates/worker` and `wrangler.toml`: a Room DO with hibernating WebSockets,
`/new`, word links, spike 1 locally), and the web shell (`web/`: SvelteKit
static scaffold, pre-join with consent and device preview, meeting grid).

Docs: this folder, modelled on the jumanji docs. PLAN.md moved into it: an
index with decisions D1–D21, one folder per feature, the research moved into
[research/](research/README.md) with `git mv`, plus this log and
[TESTING.md](TESTING.md). PLAN.md is now a stub that points here.

Next: wire `meeting-core` into the `RoomObject`, connect the web shell to
signaling and the SFU, and write the spike 1 result in
[spikes/](spikes/01-do-hibernation.md).
