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
  the meeting id and host token, the participants (name, role, recording
  consent, and connection state: knocking, connecting or joined with their
  SFU sessions and tracks), the lock, the recording status, the transcript
  token, and when the room ended. The public part every client sees is the
  `RoomView`.
- **`Event`**: something that happened. A client message
  (`Event::Client`), an SFU reply to an earlier effect (`Event::Sfu`), a
  socket that closed without a `Leave` (`Event::SocketClosed`), or the alarm
  the core asked for (`Event::AlarmFired`).
- **`Effect`**: something the shell must do, in list order. Send to one
  socket or several (`Send`, `Broadcast`), close a socket (`CloseSocket`),
  call the SFU (`SfuNewSession`, `SfuPublish`, `SfuSubscribe`,
  `SfuRenegotiate`, `SfuCloseTracks`, `SfuUpdateLayer`), set the alarm
  (`SetAlarm`), drive the recorder (`StartRecording`, `RecordTrack`,
  `StopRecording`), free the meeting words (`ReleaseMeetingId`), and end the
  meeting (`EndMeeting`, always the last effect a room produces).
- **Protocol**: `ClientMsg` and `ServerMsg`, serialized with serde and
  exported to TypeScript with `tsify`. Client messages include `Join` (with
  name, consent and an optional host token), `PublishOffer`,
  `SubscribeAnswer`, `SetLayer`, `Lock`, `Admit`, `Kick`, `Mute`,
  `SetRecording`, `End` and `Leave`. Server messages include `Welcome`,
  `State`, `Waiting`, `PublishAnswer`, `SubscribeOffer`, `Knock`, `Muted`,
  `Kicked`, `Ended` and `Error`. The plan's sketch also had an `Ice` client
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
- `SetAlarm` sets the DO's single alarm; a later alarm replaces an earlier
  one, and a late or spurious alarm is harmless.
- `StartRecording`, `RecordTrack` and `StopRecording` go to the meeting's
  `RecorderObject`.
- `ReleaseMeetingId` deletes the words from KV; `EndMeeting` stops accepting
  sockets and starts the transcript job.

**Browser**: the Svelte app loads the core as WASM. It uses the generated
types for every message on the signaling socket, and core functions such as
`validateMeetingId` and `generateMeetingId` instead of restating the rules in
TypeScript. The browser performs the I/O itself: DOM, `RTCPeerConnection`,
devices ([D3](../architecture/README.md#d3-the-browser-runs-a-typescript-and-svelte-shell-over-the-core-as-wasm-2026-10-09)).

On 2026-10-09 the `RoomObject` from spike 1 does not yet call the core;
wiring it in is the next step of the worker workstream (see
[STATUS.md](../../STATUS.md)).

## Rules the core enforces

From the module comment in [`lib.rs`](../../crates/meeting-core/src/lib.rs):

- A joiner with the host token becomes a host. Guests who join a locked room
  knock until a host admits them. A guest must consent to join while the room
  records.
- Only hosts lock, admit, kick, mute, record and end.
- Every change to the public `RoomView` reaches every joined client, so all
  of them see a recording start. Only consenting participants' audio is
  recorded.
- Each participant has a publish session (the client offers) and a subscribe
  session (the SFU offers, one negotiation at a time). New tracks reach every
  other joined participant; a leaving participant's tracks are closed on all
  subscribers.
- The room ends when a host ends it, or after `EMPTY_ROOM_TIMEOUT_MS` (two
  minutes) without admitted participants.

## Invariants to test

Property tests (`proptest`) generate random sequences of join, leave, lock,
knock, renegotiation and recording events, run them through `step`, and check
after every step that:

- no SFU track is orphaned: every published track belongs to a participant
  still in the room, and a leaving participant's tracks are closed on every
  subscriber;
- each SFU session has exactly one offerer: the client on publish sessions,
  the SFU on subscribe sessions;
- recording is never on without a broadcast that tells every joined client;
- a host is never locked out of their own room.

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
