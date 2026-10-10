//! What a `RoomObject` keeps across hibernation: one storage record and one attachment per
//! socket. Pure, so the rules are tested natively.
//!
//! The record is stored with a version ([`RECORD_VERSION`], research 11 item 2), so a change of
//! its shape cannot make a room fail every request: a record of an older version loads as an
//! ended meeting ([`decode`]). New fields with an obvious default carry `#[serde(default)]`, so
//! adding one needs no new version.

use meeting_core::{Effect, Event, Millis, ParticipantId, Phase, RecordingPolicy, RoomState, step};
use serde::{Deserialize, Serialize};

use crate::internal::FinishRecording;

/// The version of the stored record's shape. 1 was the unversioned record before 2026-10-09;
/// 2 added the consent record to the core's participants and the recorder id.
pub const RECORD_VERSION: u32 = 2;

/// A stored record with its version.
#[derive(Serialize, Deserialize)]
struct Stored<R> {
    version: u32,
    record: R,
}

/// What [`decode`] found in storage.
#[derive(Debug, Clone, PartialEq, Eq)]
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
pub fn decode(json: &str) -> Result<Decoded, String> {
    #[derive(Deserialize)]
    struct Version {
        #[serde(default = "unversioned")]
        version: u32,
    }
    fn unversioned() -> u32 {
        1
    }
    let Version { version } = serde_json::from_str(json).map_err(|e| e.to_string())?;
    match version {
        RECORD_VERSION => serde_json::from_str::<Stored<Record>>(json)
            .map(|stored| Decoded::Current(stored.record))
            .map_err(|e| e.to_string()),
        older if older < RECORD_VERSION => Ok(Decoded::Outdated { version: older }),
        newer => Err(format!(
            "the record has version {newer}, newer than this code's {RECORD_VERSION}"
        )),
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
}

impl OpenRoom {
    /// Runs one event through the core; returns the record to store and the effects to carry
    /// out.
    pub fn step(self, now: Millis, event: Event) -> (Record, Vec<Effect>) {
        // A fired alarm is used up. The core sets it again after every `AlarmFired` if anything
        // is still due; keeping the old time would re-arm a time in the past, and the alarm
        // would fire again at once, forever.
        let alarm = match event {
            Event::AlarmFired => None,
            _ => self.alarm,
        };
        let (room, effects) = step(self.room, now, event);
        (
            Self {
                room,
                alarm,
                ..self
            }
            .settle(&effects),
            effects,
        )
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

#[cfg(test)]
mod tests {
    use meeting_core::{
        ClientMsg, DisplayName, LEAVE_GRACE_MS, MeetingId, ResumeToken, SCHEDULED_TTL_MS,
        SessionId, SessionKind, SfuReply, TranscriptRef,
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
        assert_eq!(decode(&json), Ok(Decoded::Current(waiting)));
        // The shape before `EndedRoom` was a struct of its own, and before `pendingFinish`.
        let without = r#"{"version":2,"record":{"ended":{"recordingPolicy":"fromStart"}}}"#;
        assert_eq!(decode(without), Ok(Decoded::Current(ended)));
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
        assert_eq!(decode(&json), Ok(Decoded::Current(record)));
    }

    #[test]
    fn an_unversioned_record_loads_as_outdated_and_a_newer_one_fails() {
        // The shape stored before versioning: the record itself, and participants without the
        // consent record.
        let old = r#"{"open":{"room":{"meeting":"abide-acid-acorn","participants":{"0":
            {"name":"Ada","consent_recording":true}}},"alarm":null,"nextParticipant":1}}"#;
        assert_eq!(decode(old), Ok(Decoded::Outdated { version: 1 }));
        assert_eq!(
            decode(r#"{"ended":{"recordingPolicy":"fromStart"}}"#),
            Ok(Decoded::Outdated { version: 1 })
        );
        assert!(decode(r#"{"version":3,"record":{}}"#).is_err());
        assert!(decode("not json").is_err());
    }

    #[test]
    fn attachment_survives_json_with_the_owner_flag() {
        let open = Attachment::Open(OpenSocket {
            participant: ParticipantId(3),
            since: T0,
            owner: true,
        });
        let json = serde_json::to_value(open).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "socket": "open", "participant": 3, "since": T0.0, "owner": true })
        );
        assert_eq!(serde_json::from_value::<Attachment>(json).unwrap(), open);
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
