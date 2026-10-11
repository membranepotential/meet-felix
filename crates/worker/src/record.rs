//! What a `RoomObject` keeps across hibernation: one storage record and one attachment per
//! socket; and the versioned envelope both Durable Objects keep their record in. Pure, so the
//! rules are tested natively.
//!
//! The record is stored with a version ([`RECORD_VERSION`], research 11 item 2), so a change of
//! its shape cannot make a room fail every request: a record of an older version loads as an
//! ended meeting ([`decode`]). New fields with an obvious default carry `#[serde(default)]`, so
//! adding one needs no new version. Records that deployed code wrote are frozen in
//! `crates/worker/fixtures/room/`, and the tests at the end of this file fail on any change of
//! the stored shape until it is handled (see `fixtures` there).

use std::fmt;

use meeting_core::{
    Effect, Event, MAX_PRESENT, Millis, ParticipantId, Phase, RecordingPolicy, RoomState, step,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::internal::FinishRecording;
use crate::recorder::RequestSeq;

/// The version of the stored record's shape. 1 was the unversioned record before 2026-10-09;
/// 2 added the consent record to the core's participants and the recorder id.
pub const RECORD_VERSION: u32 = 2;

/// A stored record with its version: the JSON a Durable Object keeps under its storage key.
#[derive(Serialize, Deserialize)]
pub struct Stored<R> {
    pub version: u32,
    pub record: R,
}

/// Why a stored record cannot be read.
#[derive(Debug)]
pub enum DecodeError {
    /// The JSON does not have the shape its version promises.
    Malformed(serde_json::Error),
    /// A newer version of the code wrote the record. An older Worker may run for minutes after
    /// a deploy, and must leave such a record alone.
    Newer { found: u32, supported: u32 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "the stored record is malformed: {e}"),
            Self::Newer { found, supported } => write!(
                f,
                "the stored record has version {found}, newer than this code's {supported}"
            ),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Malformed(e) => Some(e),
            Self::Newer { .. } => None,
        }
    }
}

impl From<serde_json::Error> for DecodeError {
    fn from(e: serde_json::Error) -> Self {
        Self::Malformed(e)
    }
}

impl From<DecodeError> for worker::Error {
    fn from(e: DecodeError) -> Self {
        Self::RustError(e.to_string())
    }
}

/// The version of a stored record: its `version` field, or 1 for a record stored before
/// records had versions. A version above `supported` is [`DecodeError::Newer`].
pub fn stored_version(json: &str, supported: u32) -> Result<u32, DecodeError> {
    #[derive(Deserialize)]
    struct Version {
        #[serde(default = "unversioned")]
        version: u32,
    }
    fn unversioned() -> u32 {
        1
    }
    let Version { version } = serde_json::from_str(json)?;
    if version > supported {
        return Err(DecodeError::Newer {
            found: version,
            supported,
        });
    }
    Ok(version)
}

/// The record inside a [`Stored`] envelope.
pub fn stored_record<R: DeserializeOwned>(json: &str) -> Result<R, DecodeError> {
    Ok(serde_json::from_str::<Stored<R>>(json)?.record)
}

/// What [`decode`] found in storage.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "made once per load and taken apart at once; a box would only add an allocation. \
              `allow`, not `expect`: the lint fires on 64-bit targets only"
)]
pub enum Decoded {
    /// A record of the current version.
    Current(Record),
    /// A record of an older version, which this code no longer reads. The room counts as
    /// ended; the meeting is lost, which beats a room that fails every request forever.
    Outdated { version: u32 },
}

/// The stored JSON of `record`.
pub fn encode(record: &Record) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        version: RECORD_VERSION,
        record,
    })
}

/// Reads a stored record. A record from a newer version is an error, not an outdated one: an
/// older Worker may run for minutes after a deploy, and must not end rooms a newer one stored.
pub fn decode(json: &str) -> Result<Decoded, DecodeError> {
    match stored_version(json, RECORD_VERSION)? {
        RECORD_VERSION => stored_record(json).map(Decoded::Current),
        older => Ok(Decoded::Outdated { version: older }),
    }
}

/// How long a socket may go without an auto-response ping before the room closes it, and how
/// often the room checks while sockets are open.
pub const KEEPALIVE_MS: u64 = 60 * 1000;

/// The room's single storage value, kept as JSON under [`Record::KEY`]. No record means the
/// router never initialised this object, or the ended room deleted its storage.
///
/// Externally tagged on purpose: serde buffers internally tagged enums, and the buffer cannot
/// read `RoomState`'s integer map keys (`{"0": ...}`) back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Record {
    /// The meeting runs.
    Open(OpenRoom),
    /// The meeting is over; the object refuses new sockets until its alarm deletes its storage
    /// ([`EndedRoom::alarm`]).
    Ended(EndedRoom),
}

/// What an ended room keeps until it deletes its storage. Holds no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndedRoom {
    pub recording_policy: RecordingPolicy,
    /// The recorder's `/end` that failed, which the room's alarm sends once more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_finish: Option<PendingFinish>,
}

/// A [`FinishRecording`] the recorder did not take, with the recorder's id (hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingFinish {
    pub recorder: String,
    pub finish: FinishRecording,
}

/// How long the room waits before it sends a failed `/end` to the recorder again.
pub const FINISH_RETRY_MS: u64 = 30 * 1000;

/// How long an ended room keeps its record, answering "ended" and refusing sockets, before it
/// deletes its storage (D8 in `docs/links`). It covers clients that still reach the room after
/// the words were released, through a KV read cached at the edge (60 s by default).
pub const ENDED_RETENTION_MS: u64 = 60 * 60 * 1000;

/// What the alarm of an ended room does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndedAlarm {
    /// Store `record`, which no longer holds the finish, wait [`ENDED_RETENTION_MS`] again, and
    /// send `pending` once more. It is sent at most once more: the recorder ends itself after
    /// hours without audio, with the names and consents its `/track`s carried.
    FinishAgain {
        record: EndedRoom,
        pending: PendingFinish,
    },
    /// Nothing pends and the retention is over: delete all storage.
    Delete,
}

impl EndedRoom {
    /// What this room's alarm does: send a pending finish once more, or delete the storage.
    pub fn alarm(self) -> EndedAlarm {
        match self.pending_finish {
            Some(pending) => EndedAlarm::FinishAgain {
                record: Self {
                    pending_finish: None,
                    ..self
                },
                pending,
            },
            None => EndedAlarm::Delete,
        }
    }
}

impl Record {
    /// The storage key. On a SQLite-backed object, key plus value may take up to 2 MB, far more
    /// than a room of a few dozen participants needs.
    pub const KEY: &str = "room";

    /// The record of a room the core just created with `effects`.
    pub fn created(room: RoomState, effects: &[Effect]) -> Self {
        OpenRoom {
            room,
            alarm: None,
            next_participant: 0,
            recorder: None,
            next_recorder_request: None,
            write_budgets: Vec::new(),
        }
        .settle(effects)
    }

    /// Where the meeting stands.
    pub fn phase(&self) -> Phase {
        match self {
            Self::Open(open) => open.room.view().phase,
            Self::Ended(_) => Phase::Ended,
        }
    }

    pub fn recording_policy(&self) -> RecordingPolicy {
        match self {
            Self::Open(open) => open.room.view().recording_policy,
            Self::Ended(ended) => ended.recording_policy,
        }
    }

    /// An ended record that keeps `pending` to send again; any other record unchanged.
    #[must_use]
    pub fn with_pending_finish(self, pending: PendingFinish) -> Self {
        match self {
            Self::Ended(ended) => Self::Ended(EndedRoom {
                pending_finish: Some(pending),
                ..ended
            }),
            open @ Self::Open(_) => open,
        }
    }
}

/// A running meeting: the core's state plus what the shell must remember about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRoom {
    pub room: RoomState,
    /// The time of the core's last [`Effect::SetAlarm`]. The object's alarm also serves the
    /// keepalive check, so the core's time must be remembered separately.
    pub alarm: Option<Millis>,
    /// The id the next socket gets; ids are never reused within a room.
    pub next_participant: u32,
    /// The id (hex) of the meeting's `RecorderObject`, once recording started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorder: Option<String>,
    /// The number the next request to the recorder gets. A step's recorder requests are
    /// numbered from here in effect order, so the recorder can drop one that arrives after a
    /// newer one (`RequestSeq` in `crate::recorder`). `None` until the first request, and in a
    /// record that older code stored, which may have dropped the number: the next step then
    /// starts counting from its clock ([`OpenRoom::first_recorder_request`]), above every number
    /// handed out before.
    #[serde(default)]
    pub next_recorder_request: Option<RequestSeq>,
    /// Each participant's budget of writes their messages cause ([`OpenRoom::charge_write`]).
    /// Unlike a socket's message budget it survives a resume on a new socket and hibernation.
    /// Records stored before it existed load with every budget full.
    #[serde(default)]
    pub write_budgets: Vec<(ParticipantId, MessageBudget)>,
}

/// Whether `effect` is a request to the recorder.
pub fn is_recording(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::StartRecording { .. }
            | Effect::RecordTrack { .. }
            | Effect::StopRecordingTrack { .. }
            | Effect::StopRecording
            | Effect::FinishRecording { .. }
    )
}

impl OpenRoom {
    /// Runs one event through the core; returns the record to store and the effects to carry
    /// out. The recorder requests among the effects take the numbers from
    /// [`OpenRoom::first_recorder_request`] on, in order; an open record counts on after them.
    pub fn step(self, now: Millis, event: Event) -> (Record, Vec<Effect>) {
        // A fired alarm is used up. The core sets it again after every `AlarmFired` if anything
        // is still due; keeping the old time would re-arm a time in the past, and the alarm
        // would fire again at once, forever.
        let alarm = match event {
            Event::AlarmFired => None,
            _ => self.alarm,
        };
        let first = self.first_recorder_request(now);
        let (room, effects) = step(self.room, now, event);
        let requests = effects.iter().filter(|e| is_recording(e)).count();
        let next_recorder_request = match requests {
            0 => self.next_recorder_request,
            n => Some(first.after(n)),
        };
        (
            Self {
                room,
                alarm,
                next_recorder_request,
                ..self
            }
            .settle(&effects),
            effects,
        )
    }

    /// The number a step at `now` gives its first recorder request.
    pub fn first_recorder_request(&self, now: Millis) -> RequestSeq {
        self.next_recorder_request
            .unwrap_or_else(|| RequestSeq::starting_at(now))
    }

    /// The participants in the room, waiting ones included.
    pub fn present(&self) -> Vec<ParticipantId> {
        self.room.view().participants.iter().map(|p| p.id).collect()
    }

    /// Charges participant `who` at `now` for one write of the record that their message
    /// caused; `false` if they used up their budget, and the room must drop the step. Only
    /// participants on the roster are charged, so the budgets stay as bounded as the roster
    /// (a resume's `Join` comes from a socket id that is no participant).
    ///
    /// A socket's [`MessageBudget`] alone does not protect the daily write allowance: a client
    /// that overspends resumes on a new socket with a full one. This budget belongs to the
    /// participant and is stored with the record, in the write it charges for.
    pub fn charge_write(&mut self, who: ParticipantId, now: Millis) -> bool {
        if !self.room.roster().iter().any(|entry| entry.id == who) {
            return true;
        }
        let entry = match self.write_budgets.iter().position(|(id, _)| *id == who) {
            Some(i) => &mut self.write_budgets[i].1,
            None => {
                self.write_budgets.push((who, MessageBudget::default()));
                &mut self.write_budgets.last_mut().expect("just pushed").1
            }
        };
        match entry.spend(now) {
            Some(spent) => {
                *entry = spent;
                true
            }
            None => false,
        }
    }

    /// Hands out the next participant id.
    pub fn connect(self) -> (Record, ParticipantId) {
        let id = ParticipantId(self.next_participant);
        let next = Self {
            next_participant: self.next_participant + 1,
            ..self
        };
        (Record::Open(next), id)
    }

    /// The record after the core produced `effects`: ended after [`Effect::EndMeeting`],
    /// otherwise open with the time of the last [`Effect::SetAlarm`], or the remembered one if
    /// there is none.
    fn settle(self, effects: &[Effect]) -> Record {
        if effects.contains(&Effect::EndMeeting) {
            return Record::Ended(EndedRoom {
                recording_policy: self.room.view().recording_policy,
                pending_finish: None,
            });
        }
        let alarm = effects
            .iter()
            .rev()
            .find_map(|effect| match effect {
                Effect::SetAlarm { at } => Some(*at),
                _ => None,
            })
            .or(self.alarm);
        Record::Open(Self { alarm, ..self })
    }
}

/// Per-socket data the runtime keeps across hibernation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "socket", rename_all = "camelCase")]
pub enum Attachment {
    /// The socket delivers events.
    Open(OpenSocket),
    /// The room closed this socket (`Effect::CloseSocket`, the keepalive check, the meeting's
    /// end) or saw it fail; it delivers no more events.
    Closed,
}

/// A socket the room has not closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSocket {
    /// Messages and the close of this socket are this participant's.
    pub participant: ParticipantId,
    /// When the socket connected, or `participant` resumed on it.
    pub since: Millis,
    /// Whether the router verified the owner's session cookie on the upgrade; the room passes
    /// it to the core with every message (`Event::Client.owner`).
    pub owner: bool,
    /// What the socket may still send. Attachments stored before the budget existed load
    /// with a full one.
    #[serde(default)]
    pub budget: MessageBudget,
}

/// The sustained rate of messages a socket may send: one per this many milliseconds.
pub const MESSAGE_INTERVAL_MS: u64 = 1_000;

/// How many messages a socket may send at once above the sustained rate. A client sends a few
/// dozen at most in a burst (a window resize changes the layer of every remote track, at most
/// once per track per 400 ms), and about one a minute otherwise.
pub const MESSAGE_BURST: u64 = 120;

/// A budget of messages, so that no client can make the room step, and store its record, as
/// often as it likes: the daily allowance of rows written is shared by every room and recorder
/// of the account. A token bucket kept as one time (the generic cell rate algorithm): the time
/// until which the messages sent so far used up the sustained rate. Each socket has one for
/// every message ([`OpenSocket::budget`]); each participant has one for the messages that
/// change the record ([`OpenRoom::charge_write`]). A socket that overspends either is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageBudget {
    spent_until: Millis,
}

impl Default for MessageBudget {
    /// A full budget.
    fn default() -> Self {
        Self {
            spent_until: Millis(0),
        }
    }
}

impl MessageBudget {
    /// The budget after one more message at `now`, or `None` if the socket has no budget left:
    /// it sent [`MESSAGE_BURST`] messages more than one per [`MESSAGE_INTERVAL_MS`].
    pub fn spend(self, now: Millis) -> Option<Self> {
        let spent_until = self.spent_until.max(now).after(MESSAGE_INTERVAL_MS);
        (spent_until.0 - now.0 <= MESSAGE_BURST * MESSAGE_INTERVAL_MS)
            .then_some(Self { spent_until })
    }
}

/// When the object's alarm must fire: the earlier of the core's alarm and, while sockets are
/// open, the next keepalive check.
pub fn alarm_time(core: Option<Millis>, open_sockets: bool, now: Millis) -> Option<Millis> {
    let keepalive = open_sockets.then(|| now.after(KEEPALIVE_MS));
    match (core, keepalive) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Whether a socket has been silent for longer than [`KEEPALIVE_MS`]: no auto-response ping
/// since it connected, or none in that time.
pub fn is_stale(now: Millis, since: Millis, last_ping: Option<Millis>) -> bool {
    let last = last_ping.map_or(since, |ping| ping.max(since));
    now.0.saturating_sub(last.0) > KEEPALIVE_MS
}

/// How long a socket may stay open without a participant in the room: time for its client's
/// `Join`, which the web client sends as soon as the socket opens. A socket that never joins,
/// or whose `Join` the room refused, is closed after it ([`join_overdue`]).
pub const JOIN_TIMEOUT_MS: u64 = 10 * 1000;

/// How many unbound sockets ([`is_unbound`]) the room keeps open. When one more connects, the
/// oldest are closed ([`crowded_out`]); the new socket, perhaps a client resuming its place, is
/// never refused.
pub const MAX_UNBOUND_SOCKETS: usize = 2 * MAX_PRESENT;

/// Whether `socket` is bound to no participant in the room: it has not joined (yet), or its
/// participant left or was refused. The owner's sockets never count, as only the owner opens
/// them.
pub fn is_unbound(socket: &OpenSocket, present: &[ParticipantId]) -> bool {
    !socket.owner && !present.contains(&socket.participant)
}

/// Whether an unbound socket that connected at `since` has had its time to join.
pub fn join_overdue(now: Millis, since: Millis) -> bool {
    now.0.saturating_sub(since.0) > JOIN_TIMEOUT_MS
}

/// Of the unbound sockets, the oldest ones to close so that one more fits within
/// [`MAX_UNBOUND_SOCKETS`].
pub fn crowded_out<T>(mut unbound: Vec<(T, OpenSocket)>) -> Vec<T> {
    unbound.sort_by_key(|(_, socket)| socket.since);
    let excess = (unbound.len() + 1).saturating_sub(MAX_UNBOUND_SOCKETS);
    unbound.into_iter().take(excess).map(|(t, _)| t).collect()
}

/// What the room does with the record a step produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// The step changed nothing: store nothing, so it costs no row.
    Skip,
    /// Store this JSON.
    Store(String),
    /// A message from this participant would change the record, but they used up their write
    /// budget ([`OpenRoom::charge_write`]): drop the step as if the message never came, and
    /// close their sockets.
    Refuse(ParticipantId),
}

/// The write for a step that turned the record stored as `stored` into `record`. `writer` is
/// the participant whose message the step ran, if it ran one; they pay for the write.
pub fn write_for(
    stored: &str,
    record: &mut Record,
    writer: Option<ParticipantId>,
    now: Millis,
) -> serde_json::Result<Write> {
    if encode(record)? == stored {
        return Ok(Write::Skip);
    }
    if let (Some(who), Record::Open(open)) = (writer, &mut *record)
        && !open.charge_write(who, now)
    {
        return Ok(Write::Refuse(who));
    }
    Ok(Write::Store(encode(record)?))
}

#[cfg(test)]
pub(crate) mod tests {
    use meeting_core::{
        ClientMsg, DisplayName, LEAVE_GRACE_MS, LocalTrack, MeetingId, Mid, OfferedTrack,
        ResumeToken, SCHEDULED_TTL_MS, Sdp, SessionEpoch, SessionId, SessionKind, SfuReply,
        TrackKind, TrackName, TranscriptRef,
    };

    use super::*;

    const T0: Millis = Millis(1_000_000);

    fn open_room() -> OpenRoom {
        let (room, effects) = RoomState::new(
            MeetingId::from_random([1, 2, 3]),
            TranscriptRef::new("transcript"),
            RecordingPolicy::HostDecides,
            T0,
        );
        match Record::created(room, &effects) {
            Record::Open(open) => open,
            ended => panic!("a new room is open, got {ended:?}"),
        }
    }

    #[test]
    fn a_new_room_remembers_the_cores_alarm() {
        let open = open_room();
        assert_eq!(Record::Open(open.clone()).phase(), Phase::Scheduled);
        assert_eq!(open.alarm, Some(T0.after(SCHEDULED_TTL_MS)));
    }

    #[test]
    fn participant_ids_count_up() {
        let (record, first) = open_room().connect();
        let Record::Open(open) = record else {
            panic!("still open")
        };
        let (_, second) = open.connect();
        assert_eq!((first, second), (ParticipantId(0), ParticipantId(1)));
    }

    const HOST: ParticipantId = ParticipantId(0);

    /// A live room: its host joined from the owner's socket and got both SFU sessions.
    fn room_with_host() -> OpenRoom {
        let host = HOST;
        let join = Event::Client {
            from: host,
            msg: ClientMsg::Join {
                name: DisplayName::parse("Ada").unwrap(),
                consent: true,
                resume: None,
            },
            owner: true,
            fresh_token: ResumeToken::from_random([7; 16]),
        };
        let mut open = open_room();
        let created = [
            (SessionKind::Publish, "publish"),
            (SessionKind::Subscribe, "subscribe"),
        ]
        .map(|(kind, session)| {
            Event::Sfu(SfuReply::SessionCreated {
                who: host,
                kind,
                session: SessionId::new(session),
            })
        });
        for event in std::iter::once(join).chain(created) {
            let (Record::Open(next), _) = open.step(T0, event) else {
                panic!("joining keeps the room open")
            };
            open = next;
        }
        assert_eq!(Record::Open(open.clone()).phase(), Phase::Live);
        open
    }

    #[test]
    fn a_fired_alarm_is_forgotten_unless_the_core_sets_it_again() {
        let fired_at = T0.after(SCHEDULED_TTL_MS);
        let room = room_with_host();
        // Going live cancelled the scheduled room's expiry, but the core never clears an alarm.
        assert_eq!(room.alarm, Some(fired_at));
        let (Record::Open(open), effects) = room.step(fired_at, Event::AlarmFired) else {
            panic!("an alarm in an occupied room keeps it open")
        };
        let set = effects.iter().find_map(|effect| match effect {
            Effect::SetAlarm { at } => Some(*at),
            _ => None,
        });
        assert_eq!(open.alarm, set);
        assert_eq!(
            open.alarm, None,
            "nothing is due in a room with a joined host"
        );
    }

    #[test]
    fn each_step_numbers_its_recorder_requests_after_the_last_steps() {
        let record = |on| Event::Client {
            from: HOST,
            msg: ClientMsg::SetRecording { on },
            owner: true,
            fresh_token: ResumeToken::from_random([9; 16]),
        };
        let open = room_with_host();
        // A room counts from its clock once it sends its first request.
        assert_eq!(open.next_recorder_request, None);
        let first = RequestSeq::starting_at(T0);
        assert_eq!(open.first_recorder_request(T0), first);
        let (Record::Open(open), effects) = open.step(T0, record(true)) else {
            panic!("still open")
        };
        assert_eq!(effects.iter().filter(|e| is_recording(e)).count(), 1);
        assert_eq!(open.next_recorder_request, Some(first.after(1)));
        // A step without recorder requests keeps the number, even at a later time.
        let later = T0.after(60_000);
        let (Record::Open(open), _) = open.step(later, Event::AlarmFired) else {
            panic!("still open")
        };
        assert_eq!(open.next_recorder_request, Some(first.after(1)));
        let (Record::Open(open), _) = open.step(later, record(false)) else {
            panic!("still open")
        };
        assert_eq!(open.next_recorder_request, Some(first.after(2)));
    }

    #[test]
    fn a_room_that_lost_its_counter_numbers_above_everything_it_numbered_before() {
        // The rooms production holds were stored before the counter existed; older code also
        // drops it when it rewrites a record, as after a rollback.
        for (name, json) in fixtures::read("room", "older") {
            if let Ok(Decoded::Current(Record::Open(open))) = decode(&json) {
                assert_eq!(open.next_recorder_request, None, "{name}");
            }
        }
        // Before: 10,000 requests a millisecond after counting began at T0. After losing the
        // counter, the next step a millisecond later still numbers above them.
        let mut open = room_with_host();
        open.next_recorder_request = Some(RequestSeq::starting_at(T0).after(10_000));
        let before = open.first_recorder_request(T0);
        open.next_recorder_request = None;
        assert!(open.first_recorder_request(T0.after(11)) > before);
    }

    fn socket(participant: u32, since: Millis, owner: bool) -> OpenSocket {
        OpenSocket {
            participant: ParticipantId(participant),
            since,
            owner,
            budget: MessageBudget::default(),
        }
    }

    #[test]
    fn sockets_bound_to_nobody_are_closed_oldest_first_and_after_their_time_to_join() {
        let present = [ParticipantId(0), ParticipantId(1)];
        assert!(!is_unbound(&socket(1, T0, false), &present));
        assert!(is_unbound(&socket(7, T0, false), &present));
        assert!(
            !is_unbound(&socket(7, T0, true), &present),
            "the owner's sockets never count"
        );
        // Below the limit nothing is crowded out; at it, the oldest make room for one more.
        let unbound = |n: u32| -> Vec<(u32, OpenSocket)> {
            (0..n)
                .map(|i| (i, socket(100 + i, T0.after(u64::from(i)), false)))
                .rev()
                .collect()
        };
        let limit = u32::try_from(MAX_UNBOUND_SOCKETS).unwrap();
        assert!(crowded_out(unbound(limit - 1)).is_empty());
        assert_eq!(crowded_out(unbound(limit)), [0]);
        assert_eq!(crowded_out(unbound(limit + 2)), [0, 1, 2]);
        assert!(!join_overdue(T0.after(JOIN_TIMEOUT_MS), T0));
        assert!(join_overdue(T0.after(JOIN_TIMEOUT_MS + 1), T0));
    }

    #[test]
    fn a_step_that_changed_nothing_writes_nothing_and_a_writer_pays_for_a_write() {
        let open = room_with_host();
        let stored = encode(&Record::Open(open.clone())).unwrap();
        let muted = |muted| Event::Client {
            from: HOST,
            msg: ClientMsg::SetMuted { muted },
            owner: true,
            fresh_token: ResumeToken::from_random([9; 16]),
        };
        // Unmuted already: nothing changes, nothing is written or charged.
        let (mut record, _) = open.clone().step(T0, muted(false));
        assert_eq!(
            write_for(&stored, &mut record, Some(HOST), T0).unwrap(),
            Write::Skip
        );
        // A change is written, with the writer's budget charged in the same record.
        let (mut record, _) = open.step(T0, muted(true));
        let Write::Store(json) = write_for(&stored, &mut record, Some(HOST), T0).unwrap() else {
            panic!("a change is stored")
        };
        let Decoded::Current(Record::Open(written)) = decode(&json).unwrap() else {
            panic!("an open room")
        };
        assert_eq!(written.write_budgets.len(), 1);
        // A step no participant caused (an SFU reply, an alarm) charges nobody.
        let (mut record, _) = written.clone().step(T0, Event::AlarmFired);
        let stored = encode(&Record::Open(written.clone())).unwrap();
        if let Write::Store(json) = write_for(&stored, &mut record, None, T0).unwrap() {
            let Decoded::Current(Record::Open(after)) = decode(&json).unwrap() else {
                panic!("an open room")
            };
            assert_eq!(after.write_budgets, written.write_budgets);
        }
    }

    #[test]
    fn a_writer_without_budget_has_the_step_dropped() {
        let mut open = room_with_host();
        while open.charge_write(HOST, T0) {}
        let stored = encode(&Record::Open(open.clone())).unwrap();
        let mute = Event::Client {
            from: HOST,
            msg: ClientMsg::SetMuted { muted: true },
            owner: true,
            fresh_token: ResumeToken::from_random([9; 16]),
        };
        let (mut record, _) = open.clone().step(T0, mute.clone());
        assert_eq!(
            write_for(&stored, &mut record, Some(HOST), T0).unwrap(),
            Write::Refuse(HOST)
        );
        // A second later the budget allows one more write.
        let later = T0.after(MESSAGE_INTERVAL_MS);
        let (mut record, _) = open.step(later, mute);
        assert!(matches!(
            write_for(&stored, &mut record, Some(HOST), later).unwrap(),
            Write::Store(_)
        ));
    }

    #[test]
    fn a_dropped_socket_arms_the_grace_alarm_and_keeps_the_room_open() {
        let dropped = T0.after(1_000);
        let (record, _) = room_with_host().step(dropped, Event::SocketClosed { who: HOST });
        assert_eq!(record.phase(), Phase::Live);
        let Record::Open(open) = record else {
            panic!("a host in the grace period keeps the room open")
        };
        assert_eq!(open.alarm, Some(dropped.after(LEAVE_GRACE_MS)));
    }

    #[test]
    fn ending_the_meeting_drops_the_state_and_keeps_the_policy() {
        let end = Event::Client {
            from: HOST,
            msg: ClientMsg::End,
            owner: true,
            fresh_token: ResumeToken::from_random([8; 16]),
        };
        let (record, effects) = room_with_host().step(T0, end);
        assert_eq!(effects.last(), Some(&Effect::EndMeeting));
        assert_eq!(record.phase(), Phase::Ended);
        let ended = EndedRoom {
            recording_policy: RecordingPolicy::HostDecides,
            pending_finish: None,
        };
        assert_eq!(record, Record::Ended(ended.clone()));
        // Nothing pends, so the retention alarm deletes the storage.
        assert_eq!(ended.alarm(), EndedAlarm::Delete);
    }

    fn pending_finish() -> PendingFinish {
        PendingFinish {
            recorder: "abc".into(),
            finish: FinishRecording {
                roster: Vec::new(),
                consents: Vec::new(),
            },
        }
    }

    #[test]
    fn a_pending_finish_keeps_the_record_until_it_is_sent_again_then_the_storage_goes() {
        let ended = EndedRoom {
            recording_policy: RecordingPolicy::FromStart,
            pending_finish: None,
        };
        let Record::Ended(waiting) =
            Record::Ended(ended.clone()).with_pending_finish(pending_finish())
        else {
            panic!("an ended record stays ended")
        };
        let EndedAlarm::FinishAgain { record, pending } = waiting.alarm() else {
            panic!("a pending finish is sent again before the storage goes")
        };
        assert_eq!((record.clone(), pending), (ended, pending_finish()));
        assert_eq!(record.alarm(), EndedAlarm::Delete);
    }

    #[test]
    fn ended_records_survive_json_with_and_without_a_pending_finish() {
        let ended = Record::Ended(EndedRoom {
            recording_policy: RecordingPolicy::FromStart,
            pending_finish: None,
        });
        let waiting = ended.clone().with_pending_finish(pending_finish());
        let json = encode(&waiting).unwrap();
        assert_eq!(decode(&json).unwrap(), Decoded::Current(waiting));
        // The shape before `pendingFinish` (frozen in `older/v2-ended-without-finish.json`).
        let without = r#"{"version":2,"record":{"ended":{"recordingPolicy":"fromStart"}}}"#;
        assert_eq!(decode(without).unwrap(), Decoded::Current(ended));
    }

    #[test]
    fn record_with_participants_survives_json() {
        let record = Record::Open(room_with_host());
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
    }

    #[test]
    fn records_are_stored_with_their_version() {
        let record = Record::Open(room_with_host());
        let json = encode(&record).unwrap();
        assert!(
            json.starts_with(r#"{"version":2,"record":{"open":"#),
            "{json}"
        );
        assert_eq!(decode(&json).unwrap(), Decoded::Current(record));
    }

    #[test]
    fn a_newer_record_and_a_malformed_one_are_told_apart() {
        assert!(matches!(
            decode(r#"{"version":3,"record":{}}"#),
            Err(DecodeError::Newer {
                found: 3,
                supported: RECORD_VERSION
            })
        ));
        assert!(matches!(decode("not json"), Err(DecodeError::Malformed(_))));
        assert!(matches!(
            decode(r#"{"version":2,"record":{"open":{}}}"#),
            Err(DecodeError::Malformed(_))
        ));
    }

    /// Frozen stored records, one file each, under `crates/worker/fixtures/<object>/`:
    ///
    /// - `current/` holds records written by the current code, from a few rich scenarios.
    ///   Each must load and store again as exactly the same JSON, so any change of the stored
    ///   shape (a field added, renamed or removed, a variant renamed) fails a test.
    /// - `older/` holds records that earlier deployed code wrote, named `v<version>-<what>.json`.
    ///   They must still load: a record of the current version as such, an older one as the
    ///   object's rule for that version says.
    ///
    /// When a `current/` test fails after a change of a stored type, decide: if every older
    /// record still loads (new fields have a default), move the `current/` files to `older/`
    /// under a name that says what they lack, and capture new ones with
    /// `cargo test -p meet-worker -- --ignored capture`. Otherwise raise the version and give
    /// the old version a rule (for the room: it loads as outdated) before capturing. Never edit
    /// a file in `older/`: production may hold that exact record.
    pub(crate) mod fixtures {
        use std::path::{Path, PathBuf};

        fn dir(object: &str, which: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(object)
                .join(which)
        }

        /// The fixtures in `fixtures/<object>/<which>/`, as (file name, JSON), sorted by name.
        pub(crate) fn read(object: &str, which: &str) -> Vec<(String, String)> {
            let mut files: Vec<(String, String)> = std::fs::read_dir(dir(object, which))
                .expect("the fixture directory exists")
                .map(|entry| {
                    let path = entry.expect("a readable entry").path();
                    let name = path.file_name().unwrap().to_string_lossy().into_owned();
                    (
                        name,
                        std::fs::read_to_string(&path).expect("a readable file"),
                    )
                })
                .collect();
            files.sort();
            assert!(!files.is_empty(), "no fixtures in {object}/{which}");
            files
        }

        /// The version a file in `older/` was stored with, from its name `v<version>-…`.
        pub(crate) fn version_in_name(name: &str) -> u32 {
            name.strip_prefix('v')
                .and_then(|rest| rest.split('-').next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} is not named v<version>-<what>.json"))
        }

        /// Writes `json` as `fixtures/<object>/current/<name>`, pretty-printed.
        pub(crate) fn capture(object: &str, name: &str, json: &str) {
            let value: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
            let pretty = serde_json::to_string_pretty(&value).expect("printable") + "\n";
            std::fs::write(dir(object, "current").join(name), pretty).expect("writable");
        }

        /// Whether `stored` is the same JSON as `fixture`, ignoring layout.
        pub(crate) fn same_json(fixture: &str, stored: &str) -> bool {
            let parse = |json| serde_json::from_str::<serde_json::Value>(json).expect("valid JSON");
            parse(fixture) == parse(stored)
        }
    }

    /// The rooms the `current/` fixtures freeze: a scheduled room with a guest waiting for the
    /// host, a live room that holds most states a participant and a session can be in, and an
    /// ended room with a pending finish.
    fn fixture_rooms() -> [(&'static str, Record); 3] {
        let live = live_fixture_room();
        let finish = FinishRecording {
            roster: live.room.roster(),
            consents: live.room.consents(),
        };
        let ended = Record::Ended(EndedRoom {
            recording_policy: RecordingPolicy::HostDecides,
            pending_finish: None,
        })
        .with_pending_finish(PendingFinish {
            recorder: RECORDER_ID.into(),
            finish,
        });
        [
            ("scheduled.json", Record::Open(scheduled_fixture_room())),
            ("live.json", Record::Open(live)),
            ("ended.json", ended),
        ]
    }

    const RECORDER_ID: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    fn client(from: u32, msg: ClientMsg, owner: bool) -> Event {
        Event::Client {
            from: ParticipantId(from),
            msg,
            owner,
            fresh_token: ResumeToken::from_random([from as u8 + 1; 16]),
        }
    }

    fn join(from: u32, owner: bool, consent: bool) -> Event {
        let name = DisplayName::parse(&format!("Person {from}")).unwrap();
        client(
            from,
            ClientMsg::Join {
                name,
                consent,
                resume: None,
            },
            owner,
        )
    }

    fn created(who: u32, kind: SessionKind) -> Event {
        Event::Sfu(SfuReply::SessionCreated {
            who: ParticipantId(who),
            kind,
            session: SessionId::new(format!("{kind:?}-{who}")),
        })
    }

    /// Steps `open` through `event`; returns the open room and the effects.
    fn drive(open: OpenRoom, now: Millis, event: Event) -> (OpenRoom, Vec<Effect>) {
        match open.step(now, event) {
            (Record::Open(open), effects) => (open, effects),
            (ended, _) => panic!("the fixture room ended: {ended:?}"),
        }
    }

    fn scheduled_fixture_room() -> OpenRoom {
        let (room, effects) = RoomState::new(
            MeetingId::from_random([4, 5, 6]),
            TranscriptRef::new("0123456789abcdef0123456789abcdef"),
            RecordingPolicy::FromStart,
            T0,
        );
        let Record::Open(open) = Record::created(room, &effects) else {
            panic!("a new room is open")
        };
        let (open, _) = open.connect();
        let Record::Open(open) = open else {
            panic!("still open")
        };
        drive(open, T0.after(1_000), join(0, false, true)).0
    }

    fn live_fixture_room() -> OpenRoom {
        let (room, effects) = RoomState::new(
            MeetingId::from_random([7, 8, 9]),
            TranscriptRef::new("fedcba9876543210fedcba9876543210"),
            RecordingPolicy::HostDecides,
            T0,
        );
        let Record::Open(mut open) = Record::created(room, &effects) else {
            panic!("a new room is open")
        };
        let mut now = T0;
        let mut step = |open: OpenRoom, event: Event| {
            now = now.after(1_000);
            drive(open, now, event)
        };
        let (host, guest, knocking, awaiting, connecting, left) = (0, 1, 2, 3, 4, 5);
        let track = |who: u32, kind: TrackKind, mid: &str| LocalTrack {
            mid: Mid::new(mid),
            kind,
            name: TrackName::new(format!("{kind:?}-{who}")),
        };
        // The host joins, goes live and records; a guest joins.
        for event in [
            join(host, true, true),
            created(host, SessionKind::Publish),
            created(host, SessionKind::Subscribe),
            client(host, ClientMsg::SetRecording { on: true }, true),
            join(guest, false, true),
            created(guest, SessionKind::Publish),
            created(guest, SessionKind::Subscribe),
        ] {
            open = step(open, event).0;
        }
        // The host publishes; the guest receives both tracks.
        let offer = ClientMsg::PublishOffer {
            sdp: Sdp::new("v=0 host offer"),
            tracks: vec![
                track(host, TrackKind::Audio, "0"),
                track(host, TrackKind::Video, "1"),
            ],
        };
        let (next, effects) = step(open, client(host, offer, true));
        let Some(Effect::SfuPublish { negotiation, .. }) = effects
            .iter()
            .find(|e| matches!(e, Effect::SfuPublish { .. }))
        else {
            panic!("the offer goes to the SFU")
        };
        let published = SfuReply::Published {
            who: ParticipantId(host),
            session: SessionId::new("Publish-0"),
            negotiation: *negotiation,
            sdp_answer: Sdp::new("v=0 host answer"),
            rejected: Vec::new(),
        };
        let (next, effects) = step(next, Event::Sfu(published));
        let Some(Effect::SfuSubscribe {
            negotiation,
            tracks,
            ..
        }) = effects
            .iter()
            .find(|e| matches!(e, Effect::SfuSubscribe { .. }))
        else {
            panic!("the guest pulls the host's tracks")
        };
        let offered = SfuReply::SubscribeOffer {
            who: ParticipantId(guest),
            session: SessionId::new("Subscribe-1"),
            negotiation: *negotiation,
            sdp_offer: Sdp::new("v=0 subscribe offer"),
            tracks: tracks
                .iter()
                .zip(["0", "1"])
                .map(|(track, mid)| OfferedTrack {
                    mid: Mid::new(mid),
                    track: track.clone(),
                })
                .collect(),
        };
        let negotiation = *negotiation;
        let (next, _) = step(next, Event::Sfu(offered));
        let answer = ClientMsg::SubscribeAnswer {
            sdp: Sdp::new("v=0 subscribe answer"),
        };
        let (next, _) = step(next, client(guest, answer, false));
        let renegotiated = SfuReply::Renegotiated {
            who: ParticipantId(guest),
            session: SessionId::new("Subscribe-1"),
            negotiation,
        };
        open = step(next, Event::Sfu(renegotiated)).0;
        // The guest offers audio the SFU has not answered yet, then withdraws consent (and is
        // muted); others wait outside, connect, or left; the host's subscribe session is being
        // replaced; the guest's socket dropped.
        let guest_offer = ClientMsg::PublishOffer {
            sdp: Sdp::new("v=0 guest offer"),
            tracks: vec![track(guest, TrackKind::Audio, "0")],
        };
        let reset = ClientMsg::ResetSession {
            kind: SessionKind::Subscribe,
            epoch: SessionEpoch(0),
        };
        for event in [
            client(guest, guest_offer, false),
            client(guest, ClientMsg::SetConsent { consent: false }, false),
            join(awaiting, false, false),
            join(connecting, false, true),
            created(connecting, SessionKind::Publish),
            join(left, false, true),
            client(left, ClientMsg::Leave, false),
            client(host, ClientMsg::Lock { on: true }, true),
            join(knocking, false, false),
            client(host, reset, true),
            Event::SocketClosed {
                who: ParticipantId(guest),
            },
        ] {
            open = step(open, event).0;
        }
        let mut open = OpenRoom {
            next_participant: 6,
            recorder: Some(RECORDER_ID.into()),
            ..open
        };
        assert!(open.charge_write(ParticipantId(host), now));
        open
    }

    /// Writes the `current/` fixtures from [`fixture_rooms`]; see [`fixtures`] for when.
    #[test]
    #[ignore = "writes the fixtures of the current shape; run it only after a shape change"]
    fn capture_room_fixtures() {
        for (name, record) in fixture_rooms() {
            fixtures::capture("room", name, &encode(&record).unwrap());
        }
    }

    #[test]
    fn the_current_code_stores_the_frozen_room_records_unchanged() {
        for (name, json) in fixtures::read("room", "current") {
            let Decoded::Current(record) = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"))
            else {
                panic!("{name} is a record of the current version")
            };
            assert!(
                fixtures::same_json(&json, &encode(&record).unwrap()),
                "{name} no longer stores as frozen: the stored shape changed (see `fixtures`)"
            );
        }
    }

    #[test]
    fn every_room_record_deployed_code_stored_still_loads() {
        for (name, json) in fixtures::read("room", "older") {
            let version = fixtures::version_in_name(&name);
            match decode(&json).unwrap_or_else(|e| panic!("{name}: {e}")) {
                Decoded::Current(_) => assert_eq!(version, RECORD_VERSION, "{name}"),
                Decoded::Outdated { version: found } => {
                    assert_eq!(found, version, "{name}");
                    assert!(found < RECORD_VERSION, "{name}");
                }
            }
        }
    }

    #[test]
    fn the_frozen_live_room_covers_the_participant_states() {
        let (_, json) = fixtures::read("room", "current")
            .into_iter()
            .find(|(name, _)| name == "live.json")
            .expect("live.json");
        for tag in [
            "\"Knocking\"",
            "\"AwaitingConsent\"",
            "\"Connecting\"",
            "\"Joined\"",
            "\"Left\"",
            "\"Replacing\"",
            "\"Requested\"",
            "\"OfferPending\"",
            "\"withdrawn_at\"",
            "\"socket_lost\"",
            "\"receiving\"",
            "\"On\"",
        ] {
            assert!(json.contains(tag), "live.json lacks {tag}");
        }
    }

    #[test]
    fn attachment_survives_json_with_the_owner_flag_and_budget() {
        let budget = MessageBudget::default().spend(T0).unwrap();
        let open = Attachment::Open(OpenSocket {
            participant: ParticipantId(3),
            since: T0,
            owner: true,
            budget,
        });
        let json = serde_json::to_value(open).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "socket": "open",
                "participant": 3,
                "since": T0.0,
                "owner": true,
                "budget": T0.0 + MESSAGE_INTERVAL_MS,
            })
        );
        assert_eq!(serde_json::from_value::<Attachment>(json).unwrap(), open);
        // An attachment from before the budget loads with a full one.
        let old = serde_json::json!({ "socket": "open", "participant": 3, "since": T0.0, "owner": false });
        let Attachment::Open(loaded) = serde_json::from_value(old).unwrap() else {
            panic!("an open socket")
        };
        assert_eq!(loaded.budget, MessageBudget::default());
    }

    #[test]
    fn a_participants_write_budget_holds_across_their_sockets() {
        let mut open = room_with_host();
        for _ in 0..MESSAGE_BURST {
            assert!(open.charge_write(HOST, T0));
        }
        assert!(!open.charge_write(HOST, T0), "the burst is used up");
        // A resume reaches the room as a `Join` from the new socket's id, which is no
        // participant: it is not charged and adds no budget.
        assert!(open.charge_write(ParticipantId(42), T0));
        assert_eq!(open.write_budgets.len(), 1);
        // The budget is stored with the record, so it holds after the resume.
        let json = encode(&Record::Open(open)).unwrap();
        let Decoded::Current(Record::Open(mut loaded)) = decode(&json).unwrap() else {
            panic!("an open room")
        };
        assert!(!loaded.charge_write(HOST, T0));
        assert!(loaded.charge_write(HOST, T0.after(MESSAGE_INTERVAL_MS)));
    }

    #[test]
    fn a_socket_may_send_a_burst_then_one_message_per_interval() {
        let mut budget = MessageBudget::default();
        for _ in 0..MESSAGE_BURST {
            budget = budget.spend(T0).expect("within the burst");
        }
        assert_eq!(budget.spend(T0), None, "the burst is used up");
        // One more message per interval, for as long as the socket keeps to the rate.
        let mut now = T0;
        for _ in 0..1_000 {
            now = now.after(MESSAGE_INTERVAL_MS);
            budget = budget.spend(now).expect("at the sustained rate");
            assert_eq!(budget.spend(now), None);
        }
        // A quiet socket earns its burst back, and no more.
        let rested = now.after(10 * MESSAGE_BURST * MESSAGE_INTERVAL_MS);
        for _ in 0..MESSAGE_BURST {
            budget = budget.spend(rested).expect("a full burst again");
        }
        assert_eq!(budget.spend(rested), None);
    }

    #[test]
    fn alarm_is_the_earlier_of_core_and_keepalive() {
        let soon = T0.after(1_000);
        let late = T0.after(10 * KEEPALIVE_MS);
        assert_eq!(alarm_time(Some(soon), true, T0), Some(soon));
        assert_eq!(
            alarm_time(Some(late), true, T0),
            Some(T0.after(KEEPALIVE_MS))
        );
        assert_eq!(alarm_time(Some(late), false, T0), Some(late));
        assert_eq!(alarm_time(None, true, T0), Some(T0.after(KEEPALIVE_MS)));
        assert_eq!(alarm_time(None, false, T0), None);
    }

    #[test]
    fn sockets_go_stale_a_minute_after_their_last_ping() {
        let since = T0;
        assert!(!is_stale(T0.after(KEEPALIVE_MS), since, None));
        assert!(is_stale(T0.after(KEEPALIVE_MS + 1), since, None));
        let pinged = Some(T0.after(30_000));
        assert!(!is_stale(T0.after(KEEPALIVE_MS + 1), since, pinged));
        // A ping older than the socket does not make it stale.
        assert!(!is_stale(T0.after(1), since, Some(Millis(0))));
    }
}
