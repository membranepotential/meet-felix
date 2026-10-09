//! The room's state. Only [`crate::step`] changes it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{
    DisplayName, MeetingId, Mid, Millis, ParticipantId, ResumeToken, SessionId, TrackName,
    TranscriptRef,
};
use crate::protocol::{
    LocalTrack, ParticipantView, Phase, Presence, Recording, RecordingPolicy, Rid, Role, RoomView,
    TrackKind, TrackRef,
};
use crate::shell::{Effect, RemoteTrack};

/// How long a live room may have no admitted participant before it ends.
pub const EMPTY_ROOM_TIMEOUT_MS: u64 = 2 * 60 * 1000;

/// How long a scheduled room waits for its first host, counted from its creation, before it
/// ends unused: 60 days.
pub const SCHEDULED_TTL_MS: u64 = 60 * 24 * 60 * 60 * 1000;

/// How long one step of a negotiation may take before the room treats it as failed: the SFU's
/// answer to a publish offer, or one step of a subscribe negotiation (the SFU's offer, the
/// client's answer, the SFU's confirmation).
pub const NEGOTIATION_TIMEOUT_MS: u64 = 15 * 1000;

/// How long the room waits before it requests tracks again after a subscription failed.
pub const SUBSCRIBE_RETRY_MS: u64 = 3 * 1000;

/// Everything the room remembers. Serializable so the shell can persist it between events;
/// change it only through [`crate::step`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomState {
    pub(crate) meeting: MeetingId,
    /// Token of the transcript page, announced at the end if the meeting was recorded.
    pub(crate) transcript: TranscriptRef,
    pub(crate) recording_policy: RecordingPolicy,
    pub(crate) locked: bool,
    pub(crate) recording: RecordingStatus,
    pub(crate) participants: BTreeMap<ParticipantId, Participant>,
    pub(crate) lifecycle: Lifecycle,
}

/// The room's [`Phase`] with the time each phase's timer counts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Lifecycle {
    /// No host has joined yet. The room was created at `since` and expires
    /// [`SCHEDULED_TTL_MS`] later; it has no empty timer.
    Scheduled { since: Millis },
    /// A host has joined. `empty_since` is since when no participant is admitted (connecting or
    /// joined), `None` while one is. Guests waiting outside do not count, so their coming and
    /// going does not move it.
    Live { empty_since: Option<Millis> },
    /// The room ended at `at`.
    Ended { at: Millis },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RecordingStatus {
    NeverStarted,
    On(Recording),
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Participant {
    pub(crate) name: DisplayName,
    pub(crate) role: Role,
    pub(crate) consent_recording: bool,
    /// Lets the participant's client take this place again from a new socket.
    pub(crate) resume: ResumeToken,
    pub(crate) connection: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Connection {
    /// A guest in a scheduled room, waiting for the first host to join.
    WaitingForHost,
    /// A guest in a locked live room, waiting for a host to admit them.
    Knocking,
    /// A guest let in while the room records, waiting for their own consent.
    AwaitingConsent,
    /// Admitted; waiting for the SFU to create both sessions.
    Connecting {
        publish: Option<SessionId>,
        subscribe: Option<SessionId>,
    },
    Joined {
        publish: PublishSession,
        subscribe: SubscribeSession,
    },
    Left,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PublishSession {
    pub(crate) id: SessionId,
    pub(crate) published: BTreeMap<TrackKind, PublishedTrack>,
    pub(crate) negotiation: PublishNegotiation,
}

/// A track the SFU accepted on a publish session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PublishedTrack {
    pub(crate) name: TrackName,
    /// The transceiver in the publish session, for closing the track there.
    pub(crate) mid: Mid,
}

/// One publish offer at a time. A pending offer fails after [`NEGOTIATION_TIMEOUT_MS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum PublishNegotiation {
    Idle,
    /// The client's offer is at the SFU since `since`.
    OfferPending {
        tracks: Vec<LocalTrack>,
        since: Millis,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubscribeSession {
    pub(crate) id: SessionId,
    #[serde(with = "pairs")]
    pub(crate) receiving: BTreeMap<TrackRef, Receiving>,
    /// Tracks to receive, each with the earliest time to request it. A track stays here until
    /// a negotiation delivers it, so a failed negotiation loses nothing.
    #[serde(with = "pairs")]
    pub(crate) wanted: BTreeMap<TrackRef, Millis>,
    pub(crate) negotiation: SubscribeNegotiation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Receiving {
    pub(crate) remote: RemoteTrack,
    pub(crate) mid: Mid,
    pub(crate) rid: Rid,
}

/// One room-initiated negotiation at a time, so offers never collide. Each step started at
/// `since` and fails after [`NEGOTIATION_TIMEOUT_MS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SubscribeNegotiation {
    Idle,
    /// `SfuSubscribe` sent; waiting for the SFU's offer.
    AwaitingOffer {
        requested: Vec<(TrackRef, RemoteTrack)>,
        since: Millis,
    },
    /// Offer forwarded to the client; waiting for its answer.
    AwaitingAnswer {
        offered: Vec<Offered>,
        since: Millis,
    },
    /// Answer forwarded to the SFU; waiting for confirmation.
    Renegotiating {
        offered: Vec<Offered>,
        since: Millis,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Offered {
    pub(crate) track: TrackRef,
    pub(crate) remote: RemoteTrack,
    pub(crate) mid: Mid,
}

/// Why the room needs its alarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlarmReason {
    /// The scheduled room has had no host for [`SCHEDULED_TTL_MS`] since its creation.
    Expire,
    /// The live room has been empty for [`EMPTY_ROOM_TIMEOUT_MS`].
    EmptyRoom,
    /// A participant's publish offer timed out.
    Publish(ParticipantId),
    /// A participant's subscribe negotiation timed out, or a wanted track is due for a retry.
    Subscribe(ParticipantId),
}

impl RoomState {
    /// A new, empty, scheduled room. The effects arm the alarm that ends it if no host joins
    /// within [`SCHEDULED_TTL_MS`].
    pub fn new(
        meeting: MeetingId,
        transcript: TranscriptRef,
        recording_policy: RecordingPolicy,
        now: Millis,
    ) -> (Self, Vec<Effect>) {
        let state = Self {
            meeting,
            transcript,
            recording_policy,
            locked: false,
            recording: RecordingStatus::NeverStarted,
            participants: BTreeMap::new(),
            lifecycle: Lifecycle::Scheduled { since: now },
        };
        let effects = state
            .next_alarm()
            .map(|at| Effect::SetAlarm { at })
            .into_iter()
            .collect();
        (state, effects)
    }

    /// The meeting's link words.
    pub fn meeting(&self) -> &MeetingId {
        &self.meeting
    }

    /// When the meeting ended, if it has.
    pub fn ended(&self) -> Option<Millis> {
        match self.lifecycle {
            Lifecycle::Ended { at } => Some(at),
            Lifecycle::Scheduled { .. } | Lifecycle::Live { .. } => None,
        }
    }

    /// The public snapshot every client sees.
    pub fn view(&self) -> RoomView {
        let participants = self
            .participants
            .iter()
            .filter_map(|(&id, p)| {
                let presence = p.connection.presence()?;
                let tracks = match &p.connection {
                    Connection::Joined { publish, .. } => publish.kinds(),
                    _ => Vec::new(),
                };
                Some(ParticipantView {
                    id,
                    name: p.name.clone(),
                    role: p.role,
                    consent_recording: p.consent_recording,
                    presence,
                    tracks,
                })
            })
            .collect();
        RoomView {
            meeting: self.meeting.clone(),
            phase: self.lifecycle.phase(),
            locked: self.locked,
            recording_policy: self.recording_policy,
            recording: self.recording.current(),
            participants,
        }
    }

    /// Joined participants with their sessions.
    pub(crate) fn joined(
        &self,
    ) -> impl Iterator<Item = (ParticipantId, &PublishSession, &SubscribeSession)> {
        self.participants
            .iter()
            .filter_map(|(&id, p)| match &p.connection {
                Connection::Joined { publish, subscribe } => Some((id, publish, subscribe)),
                _ => None,
            })
    }

    /// Participants who have not left.
    pub(crate) fn present(&self) -> impl Iterator<Item = ParticipantId> {
        self.participants
            .iter()
            .filter(|(_, p)| p.connection.presence().is_some())
            .map(|(&id, _)| id)
    }

    pub(crate) fn subscribe_session_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut SubscribeSession> {
        match &mut self.participants.get_mut(&who)?.connection {
            Connection::Joined { subscribe, .. } => Some(subscribe),
            _ => None,
        }
    }

    pub(crate) fn publish_session_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut PublishSession> {
        match &mut self.participants.get_mut(&who)?.connection {
            Connection::Joined { publish, .. } => Some(publish),
            _ => None,
        }
    }

    /// The SFU name of a published track, if its owner is joined and publishes it.
    pub(crate) fn resolve(&self, track: TrackRef) -> Option<RemoteTrack> {
        match &self.participants.get(&track.owner)?.connection {
            Connection::Joined { publish, .. } => Some(RemoteTrack {
                session: publish.id.clone(),
                name: publish.published.get(&track.kind)?.name.clone(),
            }),
            _ => None,
        }
    }

    /// Whether the room records and `who` has not consented.
    pub(crate) fn lacks_consent(&self, who: ParticipantId) -> bool {
        self.recording.current().is_some()
            && self
                .participants
                .get(&who)
                .is_some_and(|p| !p.consent_recording)
    }

    /// When the room must act next, and why. Times may lie in the past.
    pub(crate) fn alarms(&self) -> impl Iterator<Item = (Millis, AlarmReason)> {
        let end = match self.lifecycle {
            Lifecycle::Scheduled { since } => {
                Some((since.after(SCHEDULED_TTL_MS), AlarmReason::Expire))
            }
            Lifecycle::Live {
                empty_since: Some(since),
            } => Some((since.after(EMPTY_ROOM_TIMEOUT_MS), AlarmReason::EmptyRoom)),
            Lifecycle::Live { empty_since: None } | Lifecycle::Ended { .. } => None,
        };
        let negotiations = self.joined().flat_map(|(who, publish, subscribe)| {
            let publish = publish.negotiation.since().map(|since| {
                (
                    since.after(NEGOTIATION_TIMEOUT_MS),
                    AlarmReason::Publish(who),
                )
            });
            let subscribe = subscribe.due().map(|at| (at, AlarmReason::Subscribe(who)));
            publish.into_iter().chain(subscribe)
        });
        end.into_iter().chain(negotiations)
    }

    /// The time for [`Effect::SetAlarm`]: the earliest of [`RoomState::alarms`].
    pub(crate) fn next_alarm(&self) -> Option<Millis> {
        if self.ended().is_some() {
            return None;
        }
        self.alarms().map(|(at, _)| at).min()
    }
}

impl Lifecycle {
    pub(crate) fn phase(&self) -> Phase {
        match self {
            Self::Scheduled { .. } => Phase::Scheduled,
            Self::Live { .. } => Phase::Live,
            Self::Ended { .. } => Phase::Ended,
        }
    }
}

impl PublishSession {
    /// The kinds of the published tracks, sorted.
    pub(crate) fn kinds(&self) -> Vec<TrackKind> {
        self.published.keys().copied().collect()
    }
}

impl PublishNegotiation {
    /// When the pending offer reached the SFU; `None` while idle.
    pub(crate) fn since(&self) -> Option<Millis> {
        match self {
            Self::Idle => None,
            Self::OfferPending { since, .. } => Some(*since),
        }
    }
}

impl SubscribeSession {
    /// When the running negotiation times out or, while idle, when the earliest wanted track
    /// may be requested.
    fn due(&self) -> Option<Millis> {
        match self.negotiation.since() {
            Some(since) => Some(since.after(NEGOTIATION_TIMEOUT_MS)),
            None => self.wanted.values().min().copied(),
        }
    }
}

impl SubscribeNegotiation {
    /// When the current step started; `None` while idle.
    pub(crate) fn since(&self) -> Option<Millis> {
        match self {
            Self::Idle => None,
            Self::AwaitingOffer { since, .. }
            | Self::AwaitingAnswer { since, .. }
            | Self::Renegotiating { since, .. } => Some(*since),
        }
    }

    /// The tracks this negotiation is about.
    pub(crate) fn tracks(&self) -> Vec<TrackRef> {
        match self {
            Self::Idle => Vec::new(),
            Self::AwaitingOffer { requested, .. } => requested.iter().map(|(t, _)| *t).collect(),
            Self::AwaitingAnswer { offered, .. } | Self::Renegotiating { offered, .. } => {
                offered.iter().map(|o| o.track).collect()
            }
        }
    }
}

impl RecordingStatus {
    pub(crate) fn current(&self) -> Option<Recording> {
        match self {
            Self::On(recording) => Some(*recording),
            Self::NeverStarted | Self::Stopped => None,
        }
    }
}

impl Connection {
    /// `None` once the participant left.
    pub(crate) fn presence(&self) -> Option<Presence> {
        match self {
            Self::WaitingForHost => Some(Presence::WaitingForHost),
            Self::Knocking => Some(Presence::Knocking),
            Self::AwaitingConsent => Some(Presence::AwaitingConsent),
            Self::Connecting { .. } => Some(Presence::Connecting),
            Self::Joined { .. } => Some(Presence::Joined),
            Self::Left => None,
        }
    }

    /// Whether the participant was let in (by a host, an unlocked room or the first host's
    /// arrival) and has not left.
    pub(crate) fn is_admitted(&self) -> bool {
        matches!(self, Self::Connecting { .. } | Self::Joined { .. })
    }
}

/// Serializes a map as a list of pairs, for keys JSON cannot represent as strings.
mod pairs {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer, K: Serialize, V: Serialize>(
        map: &BTreeMap<K, V>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(map)
    }

    pub(crate) fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        Vec::<(K, V)>::deserialize(deserializer).map(|pairs| pairs.into_iter().collect())
    }
}
