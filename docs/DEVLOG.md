# Development log

Newest entries first. Each entry: what happened, what was decided, what's
next.

## 2026-10-09 — one owner, login session, scheduled meetings, meet.example.com

The user set the product frame: a personal meeting app with one owner as the
only host ([D32](architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)).
Three changes followed on the same day.

- **Owner session.** The index page is a login. `POST /api/login` checks
  the password and sets a signed HttpOnly cookie (30 days); the owner routes
  accept it or the bearer; the cookie on the WebSocket upgrade makes the
  socket the host. Host tokens are gone from the core, the worker and the
  web app ([D7](links/README.md), [D24](ui/README.md)).
- **Scheduled meetings.** A room is `Scheduled` until the first host is
  joined, then `Live`, then `Ended`. Scheduled rooms expire after 60 unused
  days; the two-minute empty rule applies only to live rooms; guests who
  arrive early wait and are admitted when the host joins
  ([D8](links/README.md)). KV keeps the words for 90 days.
- **Custom domain.** `https://meet.example.com` serves the Worker through a
  Cloudflare Pages project (`edge/`) with a service binding, so DNS stays at the
  external provider ([research/10](research/10-custom-hostname-external-dns.md)). A
  real WebSocket join through it answered in 626 ms.
- **UI decisions D22–D31** from the mockup review are implemented: phone-first
  start and pre-join pages, past-meetings page, grid fix, phone chips for a
  shared screen, fog ring and level bars for the speaker, toasts off the
  tiles, transcript placeholder.
- Worker: `GET /api/meetings` with a KV index; spike 62 checks.
- First deploy to `meet-felix.example.workers.dev` and secrets set;
  the call test run 4 accepted phase 1 locally ([spikes/02](spikes/02-first-call.md)).

Open: a guest inside a live meeting has no "Join as host" control; a late
visitor of an ended meeting sees "does not exist" (the words are released on
end, by design); stored room records from before today do not deserialize
(nothing was in use).

## 2026-10-09 — UI decisions from the mockup review (D22–D31)

The user reviewed a mockup of every screen (screenshots of the app as it
runs, next to proposals) and answered its questions. The answers are now
decisions D22–D31 in the new [ui/](ui/README.md) page.

- Phones and tablets are fully supported; the start page and the guest
  pre-join are laid out for phones first (D22). Dark theme only (D23).
- Start page: a "Password" field with `autocomplete="current-password"`,
  filled by the password manager every time and never stored in the browser
  (D24); one "Record and transcribe" switch, on by default; the words on one
  line with hyphens; "Copy link", "Share…" and "Join now" (D25).
- A separate owner-only page behind the same password lists past meetings
  and transcripts (D26); D7, D9 and D19 now point to it.
- Pre-join: the guest's name is remembered, the button stays "Join and
  accept recording", and the page does not show who is in the room (D27).
- Meeting: labelled control bar, no "Share screen" on phones, recording
  badge top left, toasts never over tiles (D28); shared screen in a column
  layout on desktops and with round chips on phones (D29); the active
  speaker gets a fog ring and level bars, never the lamp colour (D30).
- Transcript page: speaker-labelled paragraphs with wall-clock times (D31).
  [D17](recording/README.md#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09)
  gains a post-processing step that makes a compressed copy of each
  speaker's audio; a raw WAV is about 115 MB per speaker-hour.

Open: whether the password field goes away once Cloudflare Access protects
`/new`, and why the phone grid leaves the bottom third of the screen empty
([ui/ open questions](ui/README.md#open-questions)). Next: change the web
app to match; the codec for the compressed copy is not chosen.

## 2026-10-09 — phase 1 wired end to end; first real call through the SFU

The three shells now run the real protocol, and two browsers held a call
through the Cloudflare Realtime SFU on the user's account.

- Worker: `RoomObject` loads the record, runs `meeting_core::step`, stores,
  then carries out effects (socket effects first, SFU calls after, so a
  kicked client cannot rejoin during an `await`). `sfu-api` holds the typed
  SFU client (serde types from Cloudflare's OpenAPI schema). `/api/new`
  needs the creation secret ([D7](links/README.md#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09)
  changed from Cloudflare Access to a secret until a domain is attached).
  The built web app is served as static assets. Spike 1 reruns with 43
  checks against a mock SFU ([spikes/01](spikes/01-do-hibernation.md)).
- Web: types come from the core through tsify; frames are decoded and
  encoded by the core in WASM; a pure reducer drives the UI; both
  negotiations, layer selection, keepalive pings, consent and host controls
  are wired; 54 vitest tests.
- Core follow-ups: `Unpublish`, a publish timeout, `parseServerMsg` and
  `encodeClientMsg`, and rids `q`/`h`/`f` so the SFU's asciibetical fallback
  degrades one step at a time ([D11](media/README.md)).
- First real call ([spikes/02](spikes/02-first-call.md)): guest and third
  participant received 640x360 H.264 at about 300 kbit/s with three
  simulcast layers published; lock, knock, admit, camera off/on and end all
  worked. The host received nothing: a pull before the publisher was
  connected returned an offer with a `not_found_track_error`, the client
  dropped the offer, and the session was stuck (406, then 410). Fixed in
  `sfu-api`: the offer is always handed to the client, missing tracks are
  retried. A second run is pending.
- Reviews fixed: alarm loop after the core's alarm time passed, stale
  sockets closed before the store, KV errors aborting the end of a meeting.

Open: the core has no reply for an SFU offer on the publish session, does
not replace a subscribe session after a 410, and treats a partial publish
as fully published. Recording effects are logged, not executed (phase 3).

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
