# The functional core

`meeting-core` holds all meeting logic as pure functions over one value, the
room state. It does no I/O, reads no clock and no randomness, and compiles
both natively and for `wasm32-unknown-unknown`. The `RoomObject` and the
browser are shells: they feed events in and carry out the effects that come
back. This page describes the state, the events and effects, how each shell
runs them, the invariants the tests must check, and the rule that new
behaviour starts here. The decision to build it this way is
[D2](../architecture/README.md#d2-a-pure-rust-core-with-imperative-shells-2026-10-09).

Decisions on this page:

- [D20: Core first (2026-10-09)](#d20-core-first-2026-10-09)
- [D35: A dropped socket gets a minute before it counts as leaving (2026-10-10)](#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)
- [D36: The socket protocol has a version, and every close code says what comes next (2026-10-11)](#d36-the-socket-protocol-has-a-version-and-every-close-code-says-what-comes-next-2026-10-11)

Code: [`crates/meeting-core/src/lib.rs`](../../crates/meeting-core/src/lib.rs)
(the rules, in its module comment), [`step.rs`](../../crates/meeting-core/src/step.rs),
[`state.rs`](../../crates/meeting-core/src/state.rs),
[`shell.rs`](../../crates/meeting-core/src/shell.rs) (`Event`, `Effect`),
[`protocol.rs`](../../crates/meeting-core/src/protocol.rs) (`ClientMsg`, `ServerMsg`),
[`ids.rs`](../../crates/meeting-core/src/ids.rs),
[`words.rs`](../../crates/meeting-core/src/words.rs),
[`wasm.rs`](../../crates/meeting-core/src/wasm.rs),
[`tests/`](../../crates/meeting-core/src/tests/).
Research: [00 — report](../research/00-report-serverless-rust-video-meetings.md)
(a pure Rust room core), [04 — Rust WebRTC stack](../research/04-rust-webrtc-stack.md)
(signaling design, testing sans-IO logic).

## State, events and effects

The core's centre is one function: the shell passes the current state, the
current time and an event, and gets back the new state and a list of effects.
The plan wrote it as `step(RoomState, Event) -> (RoomState, Vec<Effect>)`; the
time is an extra argument because the core reads no clock.

- **`RoomState`**: one serializable value holding everything about a room:
  the meeting id, the recording policy, the participants
  (name, role, recording consent, microphone switch, resume token, and
  connection state:
  waiting for the host, knocking, awaiting consent, connecting since when,
  joined with
  their SFU sessions and tracks and, while their socket is gone, since when
  (see [Reconnecting](#reconnecting)), or left), the lock, the recording status,
  the transcript token, and the phase with its timer (see
  [Phases](#phases)): scheduled since its creation, live with since when the
  room is empty, or ended at a time. The public part every client sees is the
  `RoomView`; it includes the `phase` and the `recordingPolicy`, so the
  pre-join screen can show whether the host is there yet and the recording
  notice.
- **`Event`**: something that happened. A client message
  (`Event::Client`, which also carries `owner`, whether the socket belongs to
  the authenticated owner, and a fresh `ResumeToken` made by the shell from
  random bytes, see [Reconnecting](#reconnecting)), an SFU reply
  to an earlier effect (`Event::Sfu`), a socket that closed without a `Leave`
  (`Event::SocketClosed`; for a joined participant it starts the grace period
  of [D35](#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)),
  or the alarm the core asked for
  (`Event::AlarmFired`).
- **`Effect`**: something the shell must do, in list order. Send to one
  socket or several (`Send`, `Broadcast`), close a socket with a
  `CloseCode` (`CloseSocket`, see
  [D36](#d36-the-socket-protocol-has-a-version-and-every-close-code-says-what-comes-next-2026-10-11)),
  move a resumed client's new socket to its participant (`Rebind`), call the
  SFU (`SfuNewSession`, `SfuPublish`, `SfuSubscribe`, `SfuRenegotiate`,
  `SfuCloseTracks`, `SfuUpdateLayer`), set the alarm (`SetAlarm`), drive the
  recorder (`StartRecording` with the meeting's words and transcript token,
  `RecordTrack` with the speaker's name, role and consent,
  `StopRecordingTrack` with their consent as it stands, `StopRecording`, and
  `FinishRecording` with the roster and the consent
  record when a recorded meeting ends), free the meeting words (`ReleaseMeetingId`), and end the
  meeting (`EndMeeting`, always the last effect a room produces).
  `Effect::target()` says who carries an effect out: the room itself, the
  SFU or the recorder (2026-10-11). The recorder's effects carry everything
  the recorder needs, so the shell looks nothing up in the room state.
- **Protocol**: `ClientMsg` and `ServerMsg`, serialized with serde and
  exported to TypeScript with `tsify`. Client messages: `Join` (name,
  consent, an optional resume token and the client's protocol version; no
  host token since 2026-10-09, see
  [Phases](#phases)),
  `SetConsent`, `SetMuted` (the sender's microphone switch), `PublishOffer`,
  `Unpublish` (a track kind), `SubscribeAnswer`,
  `SetLayer`, `Lock`,
  `Admit`, `Kick`, `Mute`, `SetRecording`, `End` and `Leave`. Server
  messages: `Welcome` (with the resume token), `State`, `Waiting`,
  `PublishAnswer`, `SubscribeOffer`, `Knock`, `ConsentRequired`, `Muted`,
  `Kicked`, `Ended` and `Error`. `Waiting` means "wait outside": the room is
  locked, or no host has arrived yet. The plan's sketch also had an `Ice` client
  message; the code has none so far.

The names above are those in the code on 2026-10-09; the code is the source
of truth. The recorder's rules (silence gate, chunk cuts, holes in the adapter
stream, speaker labels) are in `recording.rs` and the transcript merge in
`transcript.rs` (2026-10-09). Still to come in the core (from the plan): the
simulcast layer policy.

## How the shells run effects

**`RoomObject`** (server): on each WebSocket message, socket close, alarm or
SFU reply, it loads `RoomState` from its SQLite storage, calls `step`, stores
the new state, and carries out the effects in order:

- `Send` and `Broadcast` write to the hibernating WebSockets. A message to a
  participant without an open socket goes nowhere.
- The `Sfu*` effects become HTTPS calls to the SFU through `worker::Fetch`;
  each reply goes back into `step` as `Event::Sfu`. A reply names the
  session it is about and, for `SfuPublish`, `SfuSubscribe` and
  `SfuRenegotiate`, the effect's `NegotiationId`; `sfu-api` builds the
  replies, so the shell only passes them on. `SfuReply::Published` lists the
  offered kinds the SFU rejected with a per-track error (2026-10-10).
- `CloseSocket` closes the socket with the code and reason of its
  `CloseCode` and forgets it: the shell reports no `SocketClosed` for a
  socket the core closed. The shell's own closes (a socket that stopped
  pinging, overspent its budget, did not join in time, or was crowded out)
  use the core's `CloseCode` too.
- `Rebind { socket, participant }` re-attaches the new socket to the
  participant, so its messages and its close arrive as `participant`'s from
  then on. It always follows the `CloseSocket` for the participant's old
  socket.
- `SetAlarm` sets the DO's single alarm; a late or spurious alarm is
  harmless (see [The alarm](#the-alarm)). The DO's alarm also serves its
  sockets, so the shell sets it to the earliest of the core's time, the time
  the first open socket goes stale, and the time the first socket bound to
  nobody must have joined (`OpenRoom::alarm_time` in `record.rs`). That time
  covers every reason, so setting it never pushes a needed check later.
  Before 2026-10-11 each upgrade and each `SetAlarm` replaced the alarm with
  its own time, so upgrades every 9 seconds postponed the join and
  keepalive checks for good.
- `StartRecording`, `RecordTrack`, `StopRecordingTrack`, `StopRecording` and
  `FinishRecording` go to the meeting's `RecorderObject`, which the shell
  creates at the first `StartRecording`; it adds only the recorder's id and
  each request's number. `FinishRecording` starts the transcript job.
- `ReleaseMeetingId` deletes the words from KV; `EndMeeting` stops accepting
  sockets.

The `RoomObject` stores the record only when a step changed it (2026-10-11):
a repeated `SetMuted`, a message the core refused or an ignored late SFU reply
writes no row. Each socket also has a message budget, kept in its attachment
so it survives hibernation: a burst of 120 messages, then one per second
(`MESSAGE_BURST`, `MESSAGE_INTERVAL_MS` in `record.rs`). A socket that sends
more is closed with code 1008 and reported to the core as `SocketClosed`, like
a socket that stopped pinging; the client resumes on a new socket. Because a
resumed socket starts with a full budget, each participant also has a budget
of the same size for the messages that change the record, stored in the room
record itself (`write_budgets`, charged in the write it pays for, so it costs
no extra row). A message beyond it is dropped unstored and the participant's
sockets are closed the same way. Why: the Free plan allows 100,000 rows
written per day for all rooms and recorders of the account together
([cost/](../cost/README.md)), and before, any link holder could use it up
with a loop of messages. What remains: every WebSocket upgrade writes the
record once (the next participant id), so a client that reconnects in a loop
still costs a row per connection; limiting upgrades is the router's job.

**Browser**: the Svelte app loads the core as WASM. It uses the generated
types for every message on the signaling socket, and core functions such as
`validateMeetingId` and `generateMeetingId` instead of restating the rules in
TypeScript. `parseServerMsg(json)` parses and validates a whole message from
the socket with serde, and `encodeClientMsg(msg)` validates and serializes one
for it; both throw on an invalid message (2026-10-09). The browser performs the I/O itself: DOM, `RTCPeerConnection`,
devices ([D3](../architecture/README.md#d3-the-browser-runs-a-typescript-and-svelte-shell-over-the-core-as-wasm-2026-10-09)).

On 2026-10-09 the `RoomObject` from spike 1 does not yet call the core;
wiring it in is the next step of the worker workstream (see
[STATUS.md](../../STATUS.md)).

## Phases

Changed on 2026-10-09 (decided by the user): the owner creates a meeting
ahead of time and the link stays valid until it is used, because the user
wants to create a link today and meet next Tuesday. A room therefore has a
`Phase`, part of `RoomView`:

1. **`Scheduled`**: created, and no host has joined yet. Guests who join wait
   (`Presence::WaitingForHost`, message `Waiting`); they may leave. The room
   has no empty timer: it ends only if no host joins within
   `SCHEDULED_TTL_MS` (60 days) of its creation, like a normal end (`Ended`
   to the waiting guests, `ReleaseMeetingId`, `EndMeeting`) and without a
   transcript, because nothing was recorded. Nobody can lock a scheduled
   room, since only a joined host locks; so a lock never holds back the
   guests waiting for the first host.
2. **`Live`**: a host has joined at least once, meaning the host's SFU
   sessions exist, not just its `Join`. In that step the room starts
   recording if the policy is `FromStart`, then lets in every guest waiting
   for the host. A waiting guest without consent to that recording is asked
   for it (`ConsentRequired`) and waits in `AwaitingConsent` instead
   ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
   From then on the room follows the live rules: lock, knock and admit, and
   the end after two empty minutes. The room never goes back to `Scheduled`,
   even if every host leaves.
3. **`Ended`**: over; the core ignores every further event.

**Host = authenticated owner.** The browser no longer holds a host token.
The shell tells the core in `Event::Client { owner }` whether the socket
belongs to the authenticated owner (the Worker decides it from the owner's
session cookie). A `Join` from such a socket makes a host, any other `Join` a
guest. If the owner's socket takes a guest's place, by a resume token or a
repeated `Join`, that guest becomes a host; a host who was waiting outside is
let in, as hosts never wait. This is the "join as host" fallback: the owner
logs in and reconnects. `HostToken` and `ErrorCode::InvalidHostToken` are
gone.

## Rules the core enforces

From the module comment in [`lib.rs`](../../crates/meeting-core/src/lib.rs):

- A room is scheduled until the first host joins, then live, then ended
  ([Phases](#phases)). A joiner from the owner's socket becomes a host,
  everyone else a guest. Guests who join a scheduled room wait for the first
  host and are let in with it; guests who join a locked live room knock
  until a host admits them.
- While the room records, no guest is admitted without consent. Such a guest
  waits in the `AwaitingConsent` state until they send `SetConsent` or
  recording stops ([D15](../recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
  When recording starts, or a participant withdraws consent, everyone in the
  room without consent gets `ConsentRequired` and `Muted`, and their audio is
  not recorded.
- A meeting with `RecordingPolicy::FromStart` starts recording when the first
  host joins; with `HostDecides`, only `SetRecording` starts it.
- Only hosts lock, admit, kick, mute, record and end.
- A room holds at most `MAX_PRESENT` (20) participants at once, those
  waiting outside included, and `MAX_ROSTER` (200) in all, those who left
  after joining included (2026-10-11). Beyond either, a new participant's
  `Join` gets `Error { RoomFull }`, changes nothing, and the socket closes
  (code 4003). A join from the owner's socket and
  a resume are never refused, so a host is never locked out. A participant
  who leaves before they joined (while waiting, knocking, awaiting consent or
  connecting) is forgotten: they spoke in no recording, and before
  2026-10-11 a client whose join the SFU refused added one entry per attempt.
  Why: the room
  record keeps everyone who ever joined for the transcript's roster, and every
  step reads and writes it whole, so without a cap one link holder could join
  and leave in a loop until the record outgrows the storage limit or the CPU
  limit per event, and stuff a room with knocking strangers. The numbers are
  a judgement for a personal meeting app, not measured limits. The
  `RoomObject` never refuses a WebSocket upgrade, because any new socket may
  be a client resuming its place (D35). It bounds sockets instead: a socket
  bound to no participant in the room (it has not joined, or its `Join` was
  refused) is closed after `JOIN_TIMEOUT_MS` (10 s; the room arms its alarm
  for that), and when more than `2 × MAX_PRESENT` such sockets are open, a new
  one closes the oldest (code 1008). The owner's sockets never count. Changed
  on 2026-10-11 from refusing upgrades above `2 × MAX_PRESENT` open sockets
  (HTTP 429), which let 40 silent sockets that only pinged lock out every
  resume and every new guest (review of the audit fixes).
- Every participant's microphone switch is part of the room state
  (`ParticipantView::muted`), so every client shows who is muted
  (2026-10-10). A client reports its own switch with `SetMuted`. A host's
  `Mute` and the consent rule above set the flag at once and send `Muted`, so
  the host sees the result without waiting for the muted client; only the
  participant unmutes themselves, and not while the room records without
  their consent: that `SetMuted { muted: false }` changes nothing and is
  answered like the mute that asked for consent, with `ConsentRequired` and
  `Muted` (a client that unmuted while offline, or before the recording
  started, reports its stale switch after reconnecting). A participant joins unmuted, unless the
  room records without their consent. A muted microphone may still publish
  its silent audio track, so a client shows a microphone as on only if the
  participant publishes audio and is not muted.
- Every change to the public `RoomView` reaches every joined client, so all
  of them see a recording start. Only consenting participants' audio is
  recorded.
- Each participant has a publish session (the client offers) and a subscribe
  session (the SFU offers, one negotiation at a time). New tracks reach every
  other joined participant. When a track stops being published (`Unpublish`,
  a replacement of the same kind, the loss of its publish session, or its
  owner leaving or being kicked), it is closed on every subscriber that
  received it and, if it
  was recorded, `StopRecordingTrack` stops recording it; `Unpublish` also
  closes it on the publisher's publish session. A track stays in the
  subscriber's wanted set until a negotiation delivers it. A subscribe
  negotiation that fails, or whose step (SFU
  offer, client answer, SFU confirmation) takes longer than
  `NEGOTIATION_TIMEOUT_MS` (15 seconds), ends with `SfuCloseTracks` for the
  offered transceivers and `Error { SubscribeFailed }` to the client; its
  tracks are requested again after `SUBSCRIBE_RETRY_MS` (3 seconds).
- A track the SFU did not find is pulled again sooner (2026-10-10). The
  first pull of a new track usually comes before its publisher's connection
  is up, and the SFU then answers `not_found_track_error`: it leaves the
  track out of its offer, or, with no offer at all, fails the call
  (`FailureCause::TrackNotFound`, which still sends `Error { SubscribeFailed }`).
  Each wanted track counts these refusals (`Wanted::not_found`); the first
  `FIRST_PULL_RETRIES` (5) are retried after `FIRST_PULL_RETRY_MS` (500 ms),
  later ones after `SUBSCRIBE_RETRY_MS`, so a track that never appears costs
  one SFU call per 3 seconds, not two per second. A new track of the same
  kind starts at zero. Against the real SFU the publisher's connection came
  up 200–400 ms after its `PublishAnswer`, and a refused first pull reached
  the subscriber 3.8–4.2 s after the publish offer before this rule and
  1.0–1.9 s after it (DEVLOG 2026-10-10).
- An admitted participant waits at most `NEGOTIATION_TIMEOUT_MS` for both
  SFU sessions (2026-10-11). If the SFU refuses one, or does not answer in
  time (a reply can get lost when the object restarts in the middle of a
  call), they get `Error { SfuUnavailable }`, are forgotten, and their socket
  closes with code 1013, so the client tries again with backoff. Before, a
  lost reply left them connecting forever, and the room never emptied.
  The SFU's API pushes no notice of a closed track (its HTTPS API has only
  request and answer, checked 2026-10-11), so the core no longer has the
  `SfuReply::TrackClosed` it never received.
- A session the SFU declares gone (`SfuReply::Failed` with
  `FailureCause::SessionGone`), or whose peer connection the client reports
  failed (`ClientMsg::ResetSession`), is replaced, not retried
  ([D33](../media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09)). A lost publish session takes its tracks
  with it, for everyone. The client gets `SubscribeReset` or `PublishReset`
  with the new session's epoch once the new session exists. Replies name
  their session, and replies about a replaced session are ignored; a
  `ResetSession` with an old epoch is ignored too.
- Each session counts its negotiations, and SFU effects and replies name
  theirs (`NegotiationId`, 2026-10-10). A reply about a negotiation that is
  not the session's running one (it failed or timed out, and perhaps a
  newer one runs) changes nothing; only a failure that reports the session
  gone counts, whichever negotiation it names.
- One publish offer runs at a time; a second gets `NegotiationInProgress`.
  If the SFU does not answer within `NEGOTIATION_TIMEOUT_MS`, the room drops
  the offer and sends `Error { PublishFailed }`, so the client may offer again
  (2026-10-09). The SFU may reject some of the offered tracks (2026-10-10):
  the room publishes only the accepted ones and lists the rejected kinds in
  `PublishAnswer.rejected`; nobody pulls a rejected track. An answer that
  rejects every track fails the offer with `Error { PublishFailed }`.
- A joined participant whose socket closes without a `Leave` keeps their
  place, SFU sessions, tracks and recording for `LEAVE_GRACE_MS` (one
  minute), shown to the others as reconnecting; a client that resumes in
  that time carries on as if nothing happened, and otherwise the participant
  leaves when the minute is over. Anyone not yet joined leaves at once
  ([D35](#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)).
- A live room ends when a host ends it, or after `EMPTY_ROOM_TIMEOUT_MS` (two
  minutes) without admitted (connecting or joined) participants. Knocking
  guests and guests awaiting consent do not count, so their coming and going
  does not move the end; a participant in their grace period counts. A
  scheduled room has no empty timer; it ends only after `SCHEDULED_TTL_MS`
  (60 days) without a host
  ([D8](../links/README.md#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09)).
- When a recorded meeting ends, only participants who were let in
  (connecting or joined, also in the grace period) get the transcript link in
  `Ended`; whoever still waits outside gets `Ended` without it
  ([D19](../recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09),
  2026-10-11).

## The alarm

A Durable Object has one alarm, so the core asks for one time: the earliest
time it must act. `RoomState` derives it from its own state, with a reason
for each candidate time:

- `Expire`: the room is still scheduled 60 days after its creation
  (`SCHEDULED_TTL_MS`). Only a scheduled room has this reason.
- `EmptyRoom`: the live room has been empty for two minutes. Only a live
  room has this reason.
- `Connect(participant)`: that participant has been connecting for
  `NEGOTIATION_TIMEOUT_MS` without both SFU sessions; they are let go
  (2026-10-11). Records stored before the time existed load as connecting
  since time 0, so such a participant is let go at the first alarm.
- `Publish(participant)`: that participant's publish offer times out.
- `Subscribe(participant)`: that participant's subscribe negotiation times
  out, or, while no negotiation runs, their earliest wanted track is due for
  a retry (`SUBSCRIBE_RETRY_MS` after a failure, `FIRST_PULL_RETRY_MS` after
  one of the first refusals of a track the SFU did not find).
- `Replace(participant, kind)`: the request for that participant's
  replacement session got no reply within `NEGOTIATION_TIMEOUT_MS` (it then
  counts as failed), or failed and is due again (`SUBSCRIBE_RETRY_MS` after
  the failure).
- `Grace(participant)`: that joined participant's socket has been gone for
  `LEAVE_GRACE_MS`; they leave
  ([D35](#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)).

`step` emits `SetAlarm` when this time changes, and after every
`AlarmFired`, because the shell's alarm is used up once it fires. It never
clears the alarm; when nothing is due, a stray alarm does nothing. On
`AlarmFired` the core handles every reason that is due by then.

## Reconnecting

Each participant has a `ResumeToken`, 16 random bytes in hex. The core has no
randomness, so the shell puts a fresh token into every `Event::Client`; the
core keeps it only when the message is a `Join` that adds a participant, and
sends it to the client in `Welcome`.

When a client's socket drops, it opens a new socket, which the shell gives a
new participant id, and sends `Join { resume: Some(token) }`. If the token's
participant has not left, the core emits `CloseSocket` for the old socket and
`Rebind { socket: new id, participant: old id }`, and answers the participant
as it would answer a repeated `Join` (`Welcome` when joined). Consent,
tracks and the mute flag stay as they were, and so does the role, except that a guest whose
place the owner's socket takes becomes a host (2026-10-09, see
[Phases](#phases)). Otherwise, for example after the participant's grace
period ended, the `Join` is a fresh join.

**The grace period** (2026-10-10,
[D35](#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)).
The Worker reports a closed socket as `SocketClosed` at once, and a socket
whose client stopped pinging for a minute from its alarm. For a joined
participant, `SocketClosed` does not mean leaving: the core notes since when
the socket is gone (`socket_lost` in `Connection::Joined`) and keeps the
participant with their SFU sessions, their published and received tracks,
and the recording of their audio, for `LEAVE_GRACE_MS` (one minute). The
room view shows them with `reconnecting: true`, and the others' tiles say
"Reconnecting…". The grace ends in one of three ways:

- The client resumes with its token: the core clears the mark, and the
  `Welcome` carries the unchanged epochs, so the client keeps both peer
  connections.
- `LEAVE_GRACE_MS` pass: the `Grace` alarm makes the participant leave as a
  `Leave` would; their tracks close on every subscriber, the recording of
  their audio stops, and if nobody admitted is left, the empty time starts
  now, not when the socket dropped. Their token then starts a fresh join.
- A host kicks them or ends the meeting, as for anyone in the room; the
  `Kicked` or `Ended` goes nowhere. A kicked client's token no longer
  resumes, so it comes back as a new guest; a client of an ended meeting
  gets HTTP 410 and the meeting's end page.

A second report for the same socket (the runtime may report an error and a
close) changes nothing. A participant who is not joined yet (waiting for the
host, knocking, awaiting consent, or connecting) leaves at once and is
forgotten: they publish nothing, and their client holds no resume token,
which only the `Welcome` carries.

**Sockets the runtime lost without a close** (2026-10-11). A deploy restarts
every Durable Object and disconnects its WebSockets ("Code updates disconnect
all WebSockets", [Cloudflare: WebSockets best
practices](https://developers.cloudflare.com/durable-objects/best-practices/websockets/),
checked 2026-10-11); the docs do not say that the restarted object gets a
`webSocketClose` for them. The Worker's alarm therefore also reports, as
`SocketClosed`, every participant in the room whom no open socket belongs to
and who is not already reconnecting (`OpenRoom::orphans` in `record.rs`):
a joined one gets the grace, anyone else leaves. Without it, a participant
whose client never came back kept the room from emptying and ending, and a
recorded meeting never sent `/end`.

During the grace, what the room sends to the participant goes nowhere: the
`State` broadcast skips them, and the shell delivers a `Send` to a
participant without a socket to nobody. The resume catches up: the
`Welcome` carries the current view, with a recording that started and the
mute flag a host set meanwhile; a joined participant the room records
without consent gets `ConsentRequired` again after it; and a subscribe offer
still waiting for its answer fails at once (`SfuCloseTracks` for its
transceivers and `Error { SubscribeFailed }`), so its tracks are offered
again after `SUBSCRIBE_RETRY_MS` instead of after the offer's timeout. A
`PublishAnswer` sent during the grace is not repeated: the browser's offer
times out after 20 seconds and asks the user to turn the device off and on.

Before the grace period existed, a `Join` with the token had to reach the
room before the room noticed the old socket was gone, so a client that
replaces its socket on purpose ("Join as host" after a login) must not close
the old one first. The web client's `SignalingClient.reconnect()` keeps the
old socket open, the resuming `Join` makes the room close it, and the client
closes it itself once the room's first answer arrives on the new socket
(2026-10-10). The grace period makes this a second safeguard; both stay.

After every `Welcome`, the web client makes its microphone switch and the
room's mute flag agree, and muting wins: if the room shows it muted (a host
muted it while the socket was down, or the room records without its
consent), it switches the microphone off; if its microphone is off and the
room shows it on (switched off before joining or while the socket was down),
it sends `SetMuted` (`syncMute` in `web/src/lib/client-state.ts`, 2026-10-10).
A mute switch made while the socket was down is reported instead (the client
remembers the flag that last reached the room).

The room view marks whom a running recording leaves out: `awaitingConsent` on
a participant (left out of the JSON while false) is true while a recording runs, they have not consented, and
they are admitted or waiting for their consent (a knocking guest or one
waiting for the host is not marked). The core computes it from its own
consent rule (`RoomState::lacks_consent`), so a client lists those people
without re-deriving the rule (2026-10-11).

A `Join` from a participant still in the room changes nothing and gets the
same answer again (`Welcome`, `Waiting`, or `ConsentRequired`), unless it
comes from the owner's socket and the participant is a guest: then they
become a host. A `Join` from a participant id that already left is a fresh
join.

## Invariants to test

Property tests (`proptest`) generate random sequences of join (from the
owner's socket or not, into a scheduled or live room), resume (also from the
owner's socket), leave,
kick, lock, knock, consent, mute, microphone switch, publish, unpublish, renegotiation, SFU failure,
lost SFU replies, alarms (at random times, at negotiation deadlines and at
the scheduled expiry) and
recording events under both recording policies, run them through `step`, and check
after every step that:

- no SFU track is orphaned: every requested and received track belongs to a
  participant still in the room, and whenever a track stops being published
  under its name (unpublished, replaced, lost with its session, or its owner left
  or was kicked), every subscriber that received it gets `SfuCloseTracks`;
- each SFU session has exactly one offerer: every `SfuPublish` names the
  participant's publish session, and every `SfuSubscribe`, `SfuRenegotiate`,
  `SfuCloseTracks` and `SfuUpdateLayer` names their subscribe session, except
  that `Unpublish` closes the sender's own track on their publish session;
- no negotiation older than `NEGOTIATION_TIMEOUT_MS`, publish or subscribe,
  survives an alarm at or after its deadline;
- a reply about a negotiation other than the one running on its session
  changes nothing, unless it reports the session gone; the simulated SFU
  holds calls past their deadline and answers them later, so such replies
  occur;
- every track the room publishes, and every track it asks a subscriber's
  session to pull, was accepted by the SFU; the simulated SFU rejects some
  offered tracks;
- no subscription is lost: every track of every other joined participant is
  received under its current name or still wanted;
- no wanted track waits longer than `SUBSCRIBE_RETRY_MS`; a track's
  not-found count grows only when the SFU left it out of an offer or failed
  its pull with `TrackNotFound`, by one, and the track is then due
  `FIRST_PULL_RETRY_MS` later while the count is at most
  `FIRST_PULL_RETRIES`, else `SUBSCRIBE_RETRY_MS` later; the simulated SFU
  leaves out some pulled tracks and fails some pulls this way;
- recording is never on without a `State` broadcast or `Welcome` that tells
  every joined client first;
- the transcript link in `Ended` reaches only participants who were let in
  (2026-10-11);
- a room never holds more than `MAX_PRESENT` participants at once or
  `MAX_ROSTER` in all, except for joins from the owner's socket; a refused
  join changes nothing, gets `RoomFull` and closes the socket, and the owner
  always gets in (a
  separate property test with crowds of up to 1,500 joins, leaves and drops
  that reach both caps, 2026-10-11);
- no guest is admitted without consent while the room records; whoever is in
  the room without consent while it records got `ConsentRequired` and
  `Muted`; and exactly the consenting joined speakers' current audio is
  recorded, with no track left over from a speaker who left or a track that
  is gone;
- a participant's mute flag follows their own `SetMuted` and otherwise only
  turns on, together with a `Muted` to them; whoever gets `Muted` is shown
  muted; nobody admitted without consent is unmuted while the room records
  (their `SetMuted { muted: false }` is answered with `ConsentRequired` and
  `Muted`, 2026-10-11);
- guests never succeed with `Lock`, `Admit`, `Kick`, `Mute`, `SetRecording`
  or `End`: the state stays the same and they get an `Error`;
- a host is never locked out of their own room (only guests wait for the
  host, knock or await consent); only the owner's socket turns a guest into a
  host, and nothing turns a host into a guest;
- no room is live without a host having joined; a scheduled room has nobody
  joined, never comes back after going live, and ends only at its expiry;
- a scheduled room has no empty-room alarm: its only alarm for the room is
  the expiry, besides a connecting host's deadline; only a scheduled room has
  guests waiting for the host;
- guests waiting for the host are let in exactly in the step the first host
  joins, except that a guest without consent while the room records is asked
  for it and awaits it; before that they keep waiting until they leave, the
  room expires, or the owner's socket makes them a host;
- in a live room, the empty timer runs exactly while nobody is admitted, and
  the shell's alarm is always set to the room's next due time;
- a joined participant whose socket closes keeps everything for the grace
  period: in that step the room only marks them (the state is otherwise
  unchanged, and only a `State` broadcast to the others and the alarm
  follow), and a second report changes nothing; a participant not yet
  joined leaves at once;
- the mark never moves later, and no grace outlives an alarm at or after its
  end;
- a resume leaves the state as it was, except that the grace ends, an
  unanswered subscribe offer fails, and the owner's socket makes a guest a
  host;
- the room shows exactly the participants whose socket the shell lost as
  reconnecting, and every other joined client's last view is the current
  one; recording goes on during the grace, and the empty timer waits for it
  to end.

CI runs these tests from phase 1 on ([TESTING.md](../TESTING.md)).

## D20: Core first (2026-10-09)

New behaviour goes into `meeting-core` first, as events and effects with
tests. The worker and web shells only carry out effects; they make no
decisions of their own about the meeting.

Why: logic in the core is tested without a network, and it is the same on the
server and in the browser. Shells stay thin, so they are cheap to change or
replace ([D2](../architecture/README.md#d2-a-pure-rust-core-with-imperative-shells-2026-10-09)).
A change that puts meeting logic into a shell must update this decision in the
same commit.

## D35: A dropped socket gets a minute before it counts as leaving (2026-10-10)

When a joined participant's signaling socket closes without a `Leave`, the
room keeps them for `LEAVE_GRACE_MS` (one minute): their place, role,
consent, SFU sessions, published and received tracks, and the recording of
their audio all stay, and the room view marks them `reconnecting`. A client
that resumes with its token within the minute carries on with the same
participant id and the same peer connections. Otherwise the participant
leaves when the minute is over, exactly as on a `Leave`. A participant not
yet joined leaves at once. The web client sends `Leave` when its page is
closed or reloaded (`pagehide` without `persisted`), so a deliberate exit
does not linger. The rules are under [Reconnecting](#reconnecting).

Why: in the second real test (2026-10-09) a phone on mobile data took part;
a phone's socket drops when the network changes or the browser goes to the
background. Before, the room treated every closed socket as leaving: the
participant's tracks closed for everyone, the recorder closed their adapter,
and the client came back as a new participant with a new id, new SFU
sessions and a new recording generation, unless its new `Join` happened to
reach the room before the close did. A socket drop is often not a media
drop: the peer connections run on their own, and the SFU lets a session
that lost connectivity be reused for 30 seconds
([research 02](../research/02-cloudflare-deep-dive.md));
[D33](../media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09)
replaces a session that died.

Why one minute: the client's reconnect backoff reaches its seventh attempt
after about 35 seconds at worst, which leaves one negotiation timeout of
slack; it equals the Worker's keepalive tolerance for a silent socket, so a
socket in doubt gets one minute under either rule; and a longer grace makes
closed tabs and abandoned laptops linger as "Reconnecting…" tiles. A socket
that dies silently is found by the keepalive after a minute, so it can show
for up to two. The Worker's alarm reports the silent sockets before it feeds
`AlarmFired`, and drops a report whose socket a resume has closed meanwhile:
`AlarmFired` can wait on SFU calls, during which a client may resume on a new
socket, and the late report must not mark the resumed participant as lost.

Why not leave at once on a "clean" close (code 1000 or 1001): which code a
phone's browser sends when the system drops a backgrounded socket is
undocumented; if it is 1001, the case the grace exists for would still
leave at once. The Worker logs the code and `was_clean` of every close
(`websocket_close`) to settle this later. In the e2e runs of 2026-10-10,
local `wrangler dev` reported 1000 and 1001, both clean, and the
application code 4000 that the browser test uses for a dropped socket.

Known gaps: a kick during the grace reaches nobody, and the kicked client
comes back as a new guest (the host can kick again or lock the room); a
`PublishAnswer` sent during the grace is lost, and the browser asks the user
to turn the device off and on after its 20-second timeout; subscribers see
a frozen last frame until the participant is back or gone, which the tile
explains.

## D36: The socket protocol has a version, and every close code says what comes next (2026-10-11)

`ClientMsg::Join` carries `protocol`, the `PROTOCOL_VERSION` of the core the
client was built with; a `Join` without it is version 1, the clients built
before the field. The room serves every version from `MIN_PROTOCOL_VERSION`
on and answers an older one with `Error { OutdatedClient }`, then closes the
socket with code 4004. Raise `PROTOCOL_VERSION` with a change that would make
an older client misbehave, and `MIN_PROTOCOL_VERSION` only if the room must
refuse such clients. A change older clients survive needs neither: a client
ignores a server message it cannot parse, and the room answers a client
message it cannot parse with `MalformedMessage` and logs why.

Every socket the room closes gets a `CloseCode` from the core, and
`after_close(code)` (exported to the browser as `afterClose`) tells the
client what to do:

| Reason | Code | Client |
|---|---|---|
| `Left`, `Replaced` (the client left, or resumed on another socket) | 1000 | reconnect (it is gone already) |
| `Stale` (no ping for a minute) | 1001 | reconnect and resume |
| `EndedUnseen` (the meeting ended while the socket was bound to no participant, so no `Ended` message reached it) | 1001 | reconnect; the lookup before the retry finds the meeting ended |
| `OverBudget`, `NoJoin`, `Crowded` | 1008 | reconnect and resume |
| `TryLater` (the SFU refused a join) | 1013 | reconnect with backoff |
| `Kicked`, `Ended`, `RoomFull` | 4001–4003 | stop |
| `Outdated` | 4004 | reload the page |
| anything else, such as 1006 for a lost connection | | reconnect |

When a meeting ends, the core closes the sockets of everyone present with
`Ended`. The shell then closes every socket still open, which is bound to
nobody (such as a client resuming its place in the grace period of D35 whose
`Join` is not processed yet) and never heard the `Ended` message, with
`EndedUnseen`. That code is not final: the client reconnects, its lookup before
the retry finds the meeting ended, and it shows the end screen instead of a
refusal. (Closing it with `Ended` made that client stop and show "refused".)

The final codes and `Outdated` lie in 4000–4099, which version-1 clients
already treated as final: such a client stops on `Outdated` and shows its
reason, "This page is out of date. Reload it to join the meeting." A current
client reloads the page itself, at most once in five minutes per tab, so a
cached old app cannot make it loop; otherwise the end screen asks the user.
The web client starts its reconnect backoff over only once the room placed
the participant (`Welcome` or `Waiting`), so a join the room keeps refusing
is retried less and less often, up to every 10 seconds; `ConsentRequired`
does not count, because a host gets it before the room asks the SFU for
sessions (2026-10-11).

Why: the client keyed "do not come back" on 4000–4099, but the room closed
every socket with 1000 or 1001, so the rule never applied, and a join the SFU
refused was retried twice a second for as long as the SFU was down. And
without a version, a web app cached from before a deploy that changes the
protocol cannot be told apart from a current one. Version 1 stays served:
every meeting that runs during a deploy reconnects with the old app.
