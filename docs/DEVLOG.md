# Development log

Newest entries first. Each entry: what happened, what was decided, what's
next.

## 2026-10-09 — meeting-core review fixes: consent gate, resume, subscription retry

A review of `crates/meeting-core` found seven problems; all are fixed in the
core, with tests.

- Consent ([D15](recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)):
  the check ran only at `Join`, so `Admit` and unlocking let in guests
  without consent. Now no guest is admitted without consent while the room
  records; they wait in the new `AwaitingConsent` presence. New messages
  `ClientMsg::SetConsent` and `ServerMsg::ConsentRequired` (which replaces
  `ErrorCode::ConsentRequired`), and the effect `StopRecordingTrack`. When
  recording starts, everyone without consent is asked and muted. A new
  `RecordingPolicy` (`FromStart`, `HostDecides`) is an argument of
  `RoomState::new` and part of `RoomView`.
- Subscriptions: a failed SFU call dropped the wanted tracks for good, and a
  client that never answered blocked its subscribe session. Tracks now stay
  wanted until delivered; a failed or timed-out negotiation (15 s per step)
  is retried after 3 s. The room derives one alarm from its state (empty
  room or subscribe retry) instead of setting it in two places.
- Empty room ([D8](links/README.md#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09)):
  a guest who knocked and left restarted the two-minute timer. `RoomState`
  now stores `empty_since`, which only admitted participants change.
- Word list ([D6](links/README.md#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)):
  152 words removed (offensive or sensitive words, past tenses,
  near-homophones); 1,848 remain, and a test keeps the removed ones out.
  `MeetingId` deserialization no longer checks the word list, so stored
  state survives a removal; `MeetingId::parse` still does for user input.
- Reconnect: `Welcome` carries a `ResumeToken`; `Join { resume }` moves the
  participant to the new socket (`Effect::Rebind`). A repeated `Join` is
  answered again instead of rejected (`ErrorCode::AlreadyJoined` is gone).
- Property tests now also check that leavers' tracks are closed on every
  subscriber, that SFU effects name the right session, that guests cannot
  run host commands, the consent rules, lost subscriptions and the alarm.

Protocol changes the worker and web shells must follow: `Event::Client` has
a `fresh_token` the shell fills with 16 random bytes; `CloseSocket` means the
shell reports no `SocketClosed` for that socket.

Next: wire the worker and web shells to these messages and effects.

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
