# Development log

Newest entries first. Each entry: what happened, what was decided, what's
next.

## 2026-10-09 — research 11, simulcast fallback, retention, three tracks started

The user asked whether other pitfalls like the missing TURN had been missed.
Three researchers and a report writer answered
([research 11](research/11-gotchas-on-cloudflare.md)); the user approved
all eight "change now" items.

- **Done at once.** `priorityOrdering` on subscriptions is `asciibetical`
  like `ridNotAvailable`; with `none` the SFU never dropped to a smaller
  layer on a weak link ([D11](media/README.md#d11-cameras-send-simulcast-and-each-subscriber-picks-a-layer-2026-10-09)
  still describes the layer names; the fallback note is in the code).
  [D18](recording/README.md#d18-keep-the-audio-48-hours-keep-transcripts-until-the-owner-deletes-them-2026-10-09)
  now keeps audio for 48 hours after the meeting and transcripts until the
  owner deletes them. The recording notice stays short by the owner's
  decision (see D15). Workers Paid is required; the Free plan's daily caps
  fail hard.
- **No accent bars ([D34](ui/README.md#d34-cards-have-no-coloured-bar-on-their-left-edge-2026-10-09)).**
  The owner called the thick left border on notice and error cards an
  "annoying AI-slop pattern". Errors are now one shared `.error` card in
  `app.css` with the `alert` icon at the start of the text; the recording
  notice keeps its red tint without the bar; the transcript page follows
  when it lands.
- **Started in parallel (STATUS "Now").** A Playwright Test system
  (protocol layer with the mock SFU, browser layer with fake media) replaces
  the spike script; the recorder (phase 3) with record versioning, consent
  record, link headers and the 48-hour deletion; the browser fixes for audio
  playback across session resets and the iPhone microphone after a phone
  call.

## 2026-10-09 — fullscreen screen share, notification pills, Opus storage

From the second real test: the shared screen was hard to read on a phone,
and the user disliked the notification strip above the stage.

- **Fullscreen.** Every screen-share tile has a round button that puts the
  tile fullscreen (`requestFullscreen`; on iPhone Safari the video's own
  `webkitEnterFullscreen`, since the element API is missing there). The
  featured tile is rendered once for both orientations, so rotating a phone
  no longer recreates it and drops out of fullscreen. In portrait the shared
  screen sits at the top, the face chips right below it.
- **Notifications.** Toasts are compact pills with an icon per tone, three
  at most, bottom-centre above the control bar on phones and in the
  stage's bottom-left corner on wide screens, where shared content matters
  least ([D28](ui/README.md#d28-the-meeting-screen-keeps-a-labelled-control-bar-the-recording-badge-top-left-and-toasts-off-the-tiles-2026-10-09)
  updated). Knock cards and the connection note live in the same stack
  (`Toasts.svelte`). Known cost: the wide-screen column covers the
  bottom-left tile's name label.
- **Compressed recordings.** The user asked for compressed storage; spike 3
  ([spikes/03](spikes/03-whisper-input-and-opus.md)) showed Whisper on
  Workers AI reads Ogg/Opus directly and the pure-Rust `opus-rs` encodes at
  37× realtime inside wasm. D16 and D17 now say Ogg/Opus chunks from the
  start; the post-processing copy is gone. The R2 bucket
  `meet-felix-recordings` exists in the EU jurisdiction.

Next: the recorder itself (phase 3).

## 2026-10-09 — a dead SFU session is replaced, TURN from `/api/ice`

The second real test (Firefox on a PC, a phone on mobile data) failed
quietly: the phone's media never reached the PC, and the Worker log showed
41 × `HTTP 410: session_error: Session appears to be disconnected` on
subscribe retries. The room retried forever against a session the SFU had
given up, and nothing told the browser.

- **Decision [D33](media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09):**
  a dead session is replaced, not retried. `sfu-api` reports HTTP 410 or a
  `session_error` as `FailureCause::SessionGone` (`SfuError::cause`); the
  browser reports a peer connection whose `connectionState` became `failed`
  with the new `ClientMsg::ResetSession { kind }`.
- **Core.** `Connection::Joined` now holds a `SessionSlot` per kind: `Ready`
  or `Replacing(Replacement)` (`Requested`, `Failed { retry_at }`,
  `Retrying`), so everything that works on a session skips one being
  replaced. A lost publish session takes its tracks with it (closed on every
  subscriber, no longer recorded). The new session brings
  `ServerMsg::SubscribeReset` or `PublishReset`; a failed request is sent
  again through the alarm (`AlarmReason::Replace`), with one
  `Error { SfuUnavailable }`. `Published`, `SubscribeOffer`, `Renegotiated`
  and the session calls in `SfuCall` now carry their session id, so a late
  reply about the old session is ignored (the "negotiation ids on SFU
  replies" item, for sessions). Four rule tests; the property test also sends
  `ResetSession` and `SessionGone` failures and checks that a reply about a
  session no longer in use changes nothing.
- **Worker.** A failed SFU call is logged with its call, participant and
  session (`SFU Subscribe for ParticipantId(1) on session 444e… failed: HTTP
  410 …`). `GET /api/ice` adds Cloudflare TURN servers with 24-hour
  credentials when `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` are set, drops
  port-53 URLs, and falls back to STUN only otherwise.
- **Web.** The reducer turns the resets into `publish-reset` and
  `subscribe-reset`; the session replaces only the affected peer connection
  (the publisher republishes its current tracks). `Publisher` and
  `Subscriber` report a failed connection once; `SessionResets` sends
  `resetSession` at most once per kind until the reset arrives, again after
  15 s, and the user sees a quiet "Reconnecting…" notice.
- Checks after the review fixes: core 44 tests (46 with `wasm`), sfu-api 12,
  worker 27, web 113, spike 73/73 (new: 410 → new session →
  `SubscribeReset` with epoch 1 → track delivered; a stale `ResetSession`
  gets no answer; `ResetSession` for both kinds; `/api/ice` 404 without a
  known meeting).

- **Review fixes.** A request for a replacement session times out after
  `NEGOTIATION_TIMEOUT_MS` and counts as failed, so a lost reply no longer
  leaves the participant replacing forever (`Replacement` carries `since`;
  new property: no replacement outlives its deadline past an alarm).
  Sessions have epochs (`SessionEpoch`): `Welcome` carries them, each reset
  carries the new one, and `ResetSession` names the failed session's epoch,
  so a late failure of a replaced connection cannot replace the new session;
  a reconnecting client replaces connections whose epoch is behind.
  `PublishOffer`, `Unpublish`, `SubscribeAnswer` and `SetLayer` about a
  session being replaced are dropped silently instead of `NotJoined`.
  `/api/ice` needs `?meeting=` of a known meeting (404 otherwise, as TURN is
  billed); the Worker gives Cloudflare 3 s and the browser the route 4 s
  before falling back to STUN. Toasts are announced through two permanent
  live regions (polite, assertive). Broken D16 anchors fixed.

Next: create the TURN key and upload its secrets, deploy, repeat the phone
test. Rooms stored before this change do not deserialize (the `Joined` shape
changed); a deploy during a live meeting breaks that meeting.

## 2026-10-09 — first real test: logs, truthful meeting list, centred pages

The user's first test from a phone on mobile data and a laptop found three
things: the host's join showed an error toast, a page answered 500, and the
meeting list showed a meeting as scheduled although a host had joined it.

- **Cause of the list bug.** The room marked its index entry live and ended
  through KV read-modify-write; the live mark of a meeting that ended nine
  seconds after creation was lost. Fix: the index keeps only what the router
  knows at creation, and `GET /api/meetings` asks each listed room for its
  phase (`INFO`, fanned out with `join_all`; a room without a record counts
  as ended, a room that fails is logged and left out). An end is final, so
  the listing writes it into the entry (`settled`) and never asks that room
  again; this keeps the fan-out small, well under the 50 subrequests the
  Free plan allows per request. `mark_live`, `mark_ended`, `went_live` and
  the phase derivation from marks are gone.
- **The 500 is unexplained.** `wrangler tail` was not running during the
  test, and the Worker had no persisted logs. `[observability] enabled = true`
  is now set, so Workers Logs keep every invocation, console output and
  exception for 7 days (Workers & Pages → meet-felix → Observability).
- **The error toast** was most likely `malformedMessage` after a join with an
  empty name; the pre-join cannot send one, and the owner's name is now
  prefilled with `OWNER_NAME` (D32).
- **Layout.** The start, meetings, lobby and message pages are centred on
  tablets and desktops; the start page no longer stretches to the bottom of
  a tall screen.
- **`each_key_duplicate` in the browser.** The pre-join device lists are
  keyed by device id, and Chromium on Linux listed one device twice with the
  same id, which makes Svelte 5 throw and the page die. `listDevices` now
  keeps the first device per id (`uniqueById`, tested).

Next: the user repeats the test; with logs, a 500 becomes diagnosable.

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
- Worker: `GET /api/meetings` with a KV index that records when a room
  went live and ended, so the list shows Scheduled, Live or Ended; spike
  63 checks. The web app shows the subscribe-retry toast only if a retry
  has not succeeded within 10 s.
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
