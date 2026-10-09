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
  (name, role, recording consent, resume token, and connection state:
  waiting for the host, knocking, awaiting consent, connecting, joined with
  their SFU sessions and tracks, or left), the lock, the recording status,
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
  (`Event::SocketClosed`), or the alarm the core asked for
  (`Event::AlarmFired`).
- **`Effect`**: something the shell must do, in list order. Send to one
  socket or several (`Send`, `Broadcast`), close a socket (`CloseSocket`),
  move a resumed client's new socket to its participant (`Rebind`), call the
  SFU (`SfuNewSession`, `SfuPublish`, `SfuSubscribe`, `SfuRenegotiate`,
  `SfuCloseTracks`, `SfuUpdateLayer`), set the alarm (`SetAlarm`), drive the
  recorder (`StartRecording`, `RecordTrack`, `StopRecordingTrack`,
  `StopRecording`), free the meeting words (`ReleaseMeetingId`), and end the
  meeting (`EndMeeting`, always the last effect a room produces).
- **Protocol**: `ClientMsg` and `ServerMsg`, serialized with serde and
  exported to TypeScript with `tsify`. Client messages: `Join` (name,
  consent and an optional resume token; no host token since 2026-10-09, see
  [Phases](#phases)),
  `SetConsent`, `PublishOffer`, `Unpublish` (a track kind), `SubscribeAnswer`,
  `SetLayer`, `Lock`,
  `Admit`, `Kick`, `Mute`, `SetRecording`, `End` and `Leave`. Server
  messages: `Welcome` (with the resume token), `State`, `Waiting`,
  `PublishAnswer`, `SubscribeOffer`, `Knock`, `ConsentRequired`, `Muted`,
  `Kicked`, `Ended` and `Error`. `Waiting` means "wait outside": the room is
  locked, or no host has arrived yet. The plan's sketch also had an `Ice` client
  message; the code has none so far.

The names above are those in the code on 2026-10-09; the code is the source
of truth. Still to come in the core (from the plan): the simulcast layer
policy, the state machine that cuts recording chunks and gates silence, and
the transcript merge.

## How the shells run effects

**`RoomObject`** (server): on each WebSocket message, socket close, alarm or
SFU reply, it loads `RoomState` from its SQLite storage, calls `step`, stores
the new state, and carries out the effects in order:

- `Send` and `Broadcast` write to the hibernating WebSockets.
- The `Sfu*` effects become HTTPS calls to the SFU through `worker::Fetch`;
  each reply goes back into `step` as `Event::Sfu`.
- `CloseSocket` closes the socket and forgets it: the shell reports no
  `SocketClosed` for a socket the core closed.
- `Rebind { socket, participant }` re-attaches the new socket to the
  participant, so its messages and its close arrive as `participant`'s from
  then on. It always follows the `CloseSocket` for the participant's old
  socket.
- `SetAlarm` sets the DO's single alarm; a later one replaces an earlier one,
  and a late or spurious alarm is harmless (see [The alarm](#the-alarm)).
- `StartRecording`, `RecordTrack`, `StopRecordingTrack` and `StopRecording`
  go to the meeting's `RecorderObject`.
- `ReleaseMeetingId` deletes the words from KV; `EndMeeting` stops accepting
  sockets and starts the transcript job.

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
- Every change to the public `RoomView` reaches every joined client, so all
  of them see a recording start. Only consenting participants' audio is
  recorded.
- Each participant has a publish session (the client offers) and a subscribe
  session (the SFU offers, one negotiation at a time). New tracks reach every
  other joined participant. When a track stops being published (`Unpublish`,
  a replacement of the same kind, the SFU closing it, or its owner leaving or
  being kicked), it is closed on every subscriber that received it and, if it
  was recorded, `StopRecordingTrack` stops recording it; `Unpublish` also
  closes it on the publisher's publish session. A track stays in the
  subscriber's wanted set until a negotiation delivers it. A subscribe
  negotiation that fails, or whose step (SFU
  offer, client answer, SFU confirmation) takes longer than
  `NEGOTIATION_TIMEOUT_MS` (15 seconds), ends with `SfuCloseTracks` for the
  offered transceivers and `Error { SubscribeFailed }` to the client; its
  tracks are requested again after `SUBSCRIBE_RETRY_MS` (3 seconds).
- One publish offer runs at a time; a second gets `NegotiationInProgress`.
  If the SFU does not answer within `NEGOTIATION_TIMEOUT_MS`, the room drops
  the offer and sends `Error { PublishFailed }`, so the client may offer again
  (2026-10-09).
- A live room ends when a host ends it, or after `EMPTY_ROOM_TIMEOUT_MS` (two
  minutes) without admitted (connecting or joined) participants. Knocking
  guests and guests awaiting consent do not count, so their coming and going
  does not move the end. A scheduled room has no empty timer; it ends only
  after `SCHEDULED_TTL_MS` (60 days) without a host
  ([D8](../links/README.md#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09)).

## The alarm

A Durable Object has one alarm, so the core asks for one time: the earliest
time it must act. `RoomState` derives it from its own state, with a reason
for each candidate time:

- `Expire`: the room is still scheduled 60 days after its creation
  (`SCHEDULED_TTL_MS`). Only a scheduled room has this reason.
- `EmptyRoom`: the live room has been empty for two minutes. Only a live
  room has this reason.
- `Publish(participant)`: that participant's publish offer times out.
- `Subscribe(participant)`: that participant's subscribe negotiation times
  out, or, while no negotiation runs, their earliest wanted track is due for
  a retry.

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
as it would answer a repeated `Join` (`Welcome` when joined). Consent and
tracks stay as they were, and so does the role, except that a guest whose
place the owner's socket takes becomes a host (2026-10-09, see
[Phases](#phases)). Otherwise, for example after the shell reported
`SocketClosed` for the old socket, the `Join` is a fresh join.

A `Join` from a participant still in the room changes nothing and gets the
same answer again (`Welcome`, `Waiting`, or `ConsentRequired`), unless it
comes from the owner's socket and the participant is a guest: then they
become a host. A `Join` from a participant id that already left is a fresh
join.

## Invariants to test

Property tests (`proptest`) generate random sequences of join (from the
owner's socket or not, into a scheduled or live room), resume (also from the
owner's socket), leave,
kick, lock, knock, consent, publish, unpublish, renegotiation, SFU failure,
lost SFU replies, alarms (at random times, at negotiation deadlines and at
the scheduled expiry) and
recording events under both recording policies, run them through `step`, and check
after every step that:

- no SFU track is orphaned: every requested and received track belongs to a
  participant still in the room, and whenever a track stops being published
  under its name (unpublished, replaced, closed by the SFU, or its owner left
  or was kicked), every subscriber that received it gets `SfuCloseTracks`;
- each SFU session has exactly one offerer: every `SfuPublish` names the
  participant's publish session, and every `SfuSubscribe`, `SfuRenegotiate`,
  `SfuCloseTracks` and `SfuUpdateLayer` names their subscribe session, except
  that `Unpublish` closes the sender's own track on their publish session;
- no negotiation older than `NEGOTIATION_TIMEOUT_MS`, publish or subscribe,
  survives an alarm at or after its deadline;
- no subscription is lost: every track of every other joined participant is
  received under its current name or still wanted;
- recording is never on without a `State` broadcast or `Welcome` that tells
  every joined client first;
- no guest is admitted without consent while the room records; whoever is in
  the room without consent while it records got `ConsentRequired` and
  `Muted`; and exactly the consenting joined speakers' current audio is
  recorded, with no track left over from a speaker who left or a track that
  is gone;
- guests never succeed with `Lock`, `Admit`, `Kick`, `Mute`, `SetRecording`
  or `End`: the state stays the same and they get an `Error`;
- a host is never locked out of their own room (only guests wait for the
  host, knock or await consent); only the owner's socket turns a guest into a
  host, and nothing turns a host into a guest;
- no room is live without a host having joined; a scheduled room has nobody
  joined, never comes back after going live, and ends only at its expiry;
- a scheduled room has no empty-room alarm: its only alarm is the expiry;
  only a scheduled room has guests waiting for the host;
- guests waiting for the host are let in exactly in the step the first host
  joins, except that a guest without consent while the room records is asked
  for it and awaits it; before that they keep waiting until they leave, the
  room expires, or the owner's socket makes them a host;
- in a live room, the empty timer runs exactly while nobody is admitted, and
  the shell's alarm is always set to the room's next due time.

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
