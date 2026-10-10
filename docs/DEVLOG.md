# Development log

Newest entries first. Each entry: what happened, what was decided, what's
next.

## 2026-10-10 — A dropped socket gets a minute before it counts as leaving

STATUS Next item 1, core first (D20), recorded as D35 in
docs/core/README.md. Until now `Event::SocketClosed` was a `Leave`: a phone
whose socket dropped for a moment lost its tracks for everyone, the
recorder closed its adapter, and the client came back as a new participant
with new SFU sessions, unless its new `Join` reached the room before the
close did.

Core: `Connection::Joined` has `socket_lost: Option<Millis>` (serde default,
so stored rooms load; no `RECORD_VERSION` bump). A `SocketClosed` for a
joined participant only sets it; the participant keeps place, sessions,
tracks and recording, and `ParticipantView.reconnecting` shows it. A second
report keeps the first time. A participant not yet joined leaves at once:
their client has no resume token before its `Welcome`, so a grace could
never end in a resume (the plan also graced `Connecting`; the code says
otherwise). The new `AlarmReason::Grace` makes the participant leave after
`LEAVE_GRACE_MS` (60 s, exported); the empty-room time starts then. A resume
clears the mark and, if a subscribe offer went to the lost socket, fails it
at once so the tracks are offered again after 3 s instead of 15. `greet`
now sends `ConsentRequired` after the `Welcome` of a joined participant the
room records without consent, so a recording started during the grace is
announced on the resume. The `State` broadcast skips participants without a
socket. The mark lives in `Connection::Joined`, not on `Participant` as the
plan had it, so "marked only while joined" holds by construction and every
way of leaving clears it.

Worker: no logic change; the DO alarm follows the core's next alarm.
`websocket_close` logs the close code and `was_clean`, `websocket_error`
the error, to learn which codes phones send. Web: `TileView.reconnecting`
(caption "Reconnecting…", picture or initials dimmed and grey), mapped from
the room view for other people's camera tiles; `Session.start()` sends
`Leave` on `pagehide` unless the page goes into the back/forward cache, so
a closed tab does not linger for a minute.

Found on the way: the browser test that drops the guest's socket closed the
server side with code 1001 from inside the page, where `WebSocket.close`
accepts only 1000 and 3000–4999, so the room never saw that close and the
test passed only because the old socket stayed open. It now uses 4000, and
the Worker logged `code 4000, clean true`. Local `wrangler dev` reported
otherwise 1000 and 1001, both clean.

Tests: core rules for the drop, the second report, the resume, the subscribe
offer failed on resume, the grace alarm, drops before joining, kick and end
during the grace, a recording announced on resume, and storage; `flow.rs`
now expects the empty timer after the grace; the property simulator models
detached sockets with five new invariants and reaches drops, resumes and
expiries in the grace; a worker record test for the grace alarm; protocol
e2e for drop and resume and for a knocking guest's drop; the browser drop
test checks the host's tile.

Next: code review, then deploy when no meeting is live; then the cleanup of
ended-room storage.

## 2026-10-10 — "Join as host" inside a live meeting

STATUS Next item 1. The owner who joined as a guest, for example from a
phone browser without the session cookie, could become host only from the
pre-join or the lobby, not inside the meeting. Now the control bar shows
non-hosts a quiet "Join as host" control. Without a session it opens a
dialog with the existing `LoginForm` (D24: the form exists only while the
dialog is open); after the login, or at once when the browser already holds
the session (`hostOffer` says `host`), it calls `Session.reconnect()`. The
room makes the same participant a host, and the bar switches to the host
controls with the next view. `MeetingFlow` passes its `OwnerSession` through
`Meeting` to `ControlBar`, as it does to the lobby. The participant keeps
the name they joined with; the core has no rename message, and none was
added. If the owner's other browser is in the call as host, the room has
two hosts; the core allows any number of host participants.

The e2e test found a bug in `SignalingClient.reconnect()`: it closed the old
socket before opening the new one. The Worker reports a close as
`SocketClosed` at once, so the room let the participant leave before the
new `Join` arrived, and the resume token found nobody: the owner came back
as a new participant with a new id and new peer connections, and the
others saw them leave and join. The lobby's "Join as host" had the same
race, unnoticed because a waiting participant has nothing to lose. Now the
old socket stays open until the room answers on the new one: the resuming
`Join` makes the room close it, and the client closes it on the first
answer as well. A second `reconnect()` while one is under way does nothing.
Recorded under D28 in docs/ui/README.md and in the Reconnecting section of
docs/core/README.md.

Tests: a browser test in `owner.spec.ts`: the owner hosts in one browser
and joins as a guest from another without the cookie, logs in through the
dialog, and gets the host controls with the same participant id, the same
two peer connections, the typed name, and no view on the host's side
without them.

Next: STATUS Next item 2, the grace period before a dropped socket counts
as Leave.

Deployed as version 644b05c1 (commit 340fbee) with no meeting live.

## 2026-10-10 — Hosts see who has not accepted the recording

STATUS Next item 1. When a host starts recording, everyone without consent
is asked for it, but the host could not see who had not accepted. The core
already had what the web needs. A participant waits in `AwaitingConsent`
only if they were let in while the room records: they join, are admitted,
are let in by an unlock, or wait for the host of a `FromStart` meeting,
without consent; they stay outside without SFU sessions until they send
`SetConsent` or recording stops. A guest already in the call when recording
starts stays `Joined`, muted and not recorded, with `consentRecording`
false. So the web derives the list from both: `awaitingConsent(room, you)`
in `client-state.ts` returns the others with `consentRecording` false who
are joined, connecting or awaiting consent, and nothing while the room does
not record. Knocking guests are left to their knock request.

`Session.awaitingConsent` holds that list for hosts only, like `knocking`.
`Toasts.svelte` shows it as one info pill without a button, "Anna and Bo
have not accepted the recording yet" (names joined by `Intl.ListFormat`),
also in the polite live region; tiles of those in the room show "Not
recorded" after the name (`TileView.unrecorded`). Recorded under D15 in
docs/recording/README.md.

Tests: two unit tests for `awaitingConsent`; a new browser spec,
`consent.spec.ts`, for a guest in the call and a late guest waiting
outside, through both accepts.

Next: STATUS Next item 2, "Join as host" inside a live meeting.

Deployed as version 5192341c (commit f007875) with no meeting live.

## 2026-10-10 — Devices switchable inside a live meeting

In the test meeting of 2026-10-10 the owner looked for a way to change the
camera and microphone during the call; devices could only be chosen on the
pre-join page. Now the control bar has a "Devices" button that opens a
dialog, "Microphone and camera", with the same two selects as the pre-join
(`DevicePickers.svelte`, shared by both) and the capture error, if any.
`describeCaptureError` moved from `PreJoin.svelte` to `media/capture.ts`.
docs/ui/README.md records the decision under D28.

A switch reaches everyone without a new negotiation: the publisher swaps
the new track onto its RTP sender (`replaceTrack`). For the mic this
already worked. For the camera, `LocalMedia.selectCamera` used to empty the
video slot while it captured the new camera (iOS opens one camera at a time,
so the old one must stop first); the publisher saw the camera as off, sent
`unpublish`, and then published the new camera with a new offer. Now the
stopped old track stays in its slot during the capture, so the publisher
only replaces the track; the others see a short gap while the old camera is
released and the new one starts. With the camera off, a pick only records
the choice.

The last decision wins for both devices: a capture is stopped and dropped
when, by the time it finishes, another device was picked, the slot holds a
different track (the camera was turned off, the guard replaced the mic) or
`dispose()` ran. Only that last capture sets or clears `error`. If it
fails, the old mic stays, or the camera is turned off, and `choice` points
back to the device from before, so the select shows it again. The dialog
has its own short error texts (`describeSwitchError`) and clears an old
error when it opens. `local-media.test.ts` covers these races with a
stubbed `getUserMedia`.

Tests: Chromium's fake media flag is now
`--use-fake-device-for-media-stream=device-count=2`, which gives two cameras
and three microphones. A new test in `call.spec.ts` (Chromium only; Firefox
fakes one device per kind) switches both devices from the dialog and checks
the sender track labels, that no `unpublish` and no further `publishOffer`
was sent, and that the guest still sees the host's video. The pre-join test
checks that the pickers are there.

Deployed as version f2bdd8b1 (commit e32feb6) with no meeting live.

Next: the owner tries the switch on the iPhone (front and back camera).

## 2026-10-10 — first real test with a guest: mute state, expanded screen, Whisper filters

The owner's test meeting (laptop plus an iPhone on mobile data, TURN live)
produced a correct two-speaker transcript. Three findings, all fixed locally:

- **No feedback after "Mute <guest>".** The protocol had no mute state: the
  host's `Mute` only sent `Muted` to the target, and a muted mic keeps its
  track published. Now `Participant.muted` is part of `RoomState` and
  `ParticipantView`; `ClientMsg::SetMuted` reports a participant's own
  switch; the host's `Mute` and the consent gate set the flag and still send
  `Muted`. The web shell sends `setMuted` when its mic switch changes and,
  after every Welcome, lets the room's view win ("muting wins"). The tile
  shows the muted icon for others, and the host's Mute button is disabled
  once the guest is muted. docs/core/README.md describes the rule.
- **No mute while a shared screen was fullscreen on the iPhone.** iPhone
  Safari's native player (`webkitEnterFullscreen`) hid the page, and on
  desktop the fullscreened tile hid the control bar too. A shared screen now
  expands inside the meeting view: the header (except the Recording badge)
  and the other tiles are hidden, the screen fills the stage on black, the
  control bar stays; where the Fullscreen API exists the whole meeting view
  goes fullscreen as well. docs/ui/README.md records the decision.
- **Hallucinated names on the phone's quiet chunks** ("Steven", "Anthony
  Chousou", and the prompt echoed as "Phone, Phone, Phone, …"). Cause: the
  Whisper prompt "Participants: Felix Peppert, Phone." Ran the same chunks
  through Workers AI without a prompt: a filler word or nothing. The prompt
  is gone, and segments with `avg_logprob` below −1.0 or
  `compression_ratio` above 2.4 are dropped (OpenAI's reference thresholds;
  real speech scored −0.26 to −0.44). D17's as-built note has the numbers.

Also: four snippets of real speech from that meeting (two per device) live in
`web/e2e/audio/` with a manifest; the recorder e2e streams them and checks
each snippet's keywords in the transcript instead of espeak sentences.

Deployed as version 0aa254e5 (commit b0150b4) after the owner's second
meeting ended. Next: the owner tries Mute and Expand on the iPhone; delete button for past meetings; per-speaker audio on the transcript
page.

## 2026-10-10 — TURN credentials live

The owner created a Realtime TURN key and put it in `.dev.vars`; the two
secrets `TURN_KEY_ID` and `TURN_KEY_API_TOKEN` are now on the worker. On
production, `GET /api/ice?meeting=…` returns the STUN entries plus a TURN
entry with 24-hour credentials over UDP 3478/443, TCP 3478/80 and TLS
5349/443 (port 53 dropped, as designed). A relay-only ICE gather in
headless Chromium (`iceTransportPolicy: "relay"`) produced relay candidates
over all three transports, so phones on mobile data have a relay path.
No code change.

Next: the owner records a real meeting with a guest on a phone and reads
the transcript page.

## 2026-10-10 — phase 3 live: recorder, transcript page, Playwright e2e

Deployed as version 24f9bf24 (commit 8c4a918) after a code review with
eight findings, all fixed (lost `/end` now retried and names/consents sent
early; restarted sequence counters count as a hole; transcript link without
expiry; narrower quota classification with a 7-day cap; the job no longer
polls dead streams; empty chunks skipped; orphaned adapters closed;
withdrawn consent kept with its time).

Production proof the same night, with headless Chromium through the real
SFU: the adapter reached `/rec/…`, three Ogg/Opus chunks of about 33 KB per
30 s landed in the EU bucket, and the transcript was ready 10–12 s after
the meeting ended. With spike 3's synthesized speech as the fake
microphone, the transcript holds the German and English sentences
correctly, labelled "Felix Peppert (host)" with Berlin wall-clock times,
and the consent record carries the join time and notice version 1.
Measured on the way: the real adapter's chunks compress to about 9 kbps
for a tone and the job needs one alarm step per four chunks.

The Playwright system (`pnpm test:e2e`: 38 protocol, 10 Chromium and
9 Firefox tests against the mock SFU; `test:e2e:real` for a 3-party call)
replaces the spike script. It found one product question: against the real
SFU the decoded layer went back to full about 12 s after `setLayer h`.

Next: the owner records a real meeting with a guest; TURN key; fold the
recorder e2e into the Playwright suite; per-speaker audio on the transcript
page within the 48 hours; delete button on past meetings.

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
