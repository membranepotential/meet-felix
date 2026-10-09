//! What a `RoomObject` keeps across hibernation: one storage record and one attachment per
//! socket. Pure, so the rules are tested natively.
//!
//! Both are JSON without a version. Records and attachments stored before the core's phases and
//! the owner session (2026-10-09) do not deserialize; nothing with real users was deployed then.

use meeting_core::{Effect, Event, Millis, ParticipantId, RecordingPolicy, RoomState, step};
use serde::{Deserialize, Serialize};

/// How long a socket may go without an auto-response ping before the room closes it, and how
/// often the room checks while sockets are open.
pub const KEEPALIVE_MS: u64 = 60 * 1000;

/// The room's single storage value, kept as JSON under [`Record::KEY`]. No record means the
/// router never initialised this object.
///
/// Externally tagged on purpose: serde buffers internally tagged enums, and the buffer cannot
/// read `RoomState`'s integer map keys (`{"0": ...}`) back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Record {
    /// The meeting runs.
    Open(OpenRoom),
    /// The meeting is over; the object refuses new sockets for good. Holds no secrets.
    #[serde(rename_all = "camelCase")]
    Ended { recording_policy: RecordingPolicy },
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
        }
        .settle(effects)
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
            return Record::Ended {
                recording_policy: self.room.view().recording_policy,
            };
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
        ClientMsg, DisplayName, MeetingId, Phase, ResumeToken, SCHEDULED_TTL_MS, SessionId,
        SessionKind, SfuReply, TranscriptRef,
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
        assert_eq!(open_room().alarm, Some(T0.after(SCHEDULED_TTL_MS)));
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
        assert_eq!(open.room.view().phase, Phase::Live);
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
    fn ending_the_meeting_drops_the_state_and_keeps_the_policy() {
        let end = Event::Client {
            from: HOST,
            msg: ClientMsg::End,
            owner: true,
            fresh_token: ResumeToken::from_random([8; 16]),
        };
        let (record, effects) = room_with_host().step(T0, end);
        assert_eq!(effects.last(), Some(&Effect::EndMeeting));
        assert_eq!(
            record,
            Record::Ended {
                recording_policy: RecordingPolicy::HostDecides
            }
        );
    }

    #[test]
    fn record_with_participants_survives_json() {
        let record = Record::Open(room_with_host());
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
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
