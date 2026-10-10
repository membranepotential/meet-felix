//! The room's state. Only [`crate::step`] changes it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{
    DisplayName, MeetingId, Mid, Millis, ParticipantId, ResumeToken, SessionId, TrackName,
    TranscriptRef,
};
use crate::protocol::{
    LocalTrack, ParticipantView, Phase, Presence, Recording, RecordingPolicy, Rid, Role, RoomView,
    SessionEpochs, SessionKind, TrackKind, TrackRef,
};
use crate::recording::{ConsentRecord, RosterEntry};
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

/// How long the room waits before it requests tracks again after a subscription failed, and
/// before it asks again for a replacement session after the SFU failed to create one.
pub const SUBSCRIBE_RETRY_MS: u64 = 3 * 1000;

/// How long a joined participant whose socket closed without a `Leave` keeps their place, SFU
/// sessions and tracks before the room lets them leave, so their client can take the place again
/// from a new socket (D35 in `docs/core`). It equals the worker's keepalive tolerance, so a socket
/// in doubt gets one minute under either rule.
pub const LEAVE_GRACE_MS: u64 = 60 * 1000;

/// The version of the recording notice the pre-join screen shows; a consent records it. Raise it
/// whenever the notice's text changes in substance.
pub const NOTICE_VERSION: u32 = 1;

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
    /// The participant's consent to recording, once given; withdrawing it sets its
    /// `withdrawn_at` instead of forgetting it, so the consent record keeps both times.
    pub(crate) consent: Option<Consent>,
    /// Lets the participant's client take this place again from a new socket.
    pub(crate) resume: ResumeToken,
    pub(crate) connection: Connection,
    /// Whether their microphone is off ([`ParticipantView::muted`]). Records stored before the
    /// field existed load as unmuted.
    #[serde(default)]
    pub(crate) muted: bool,
}

/// A participant's consent to recording: when they first gave it, which notice they saw, and
/// when they withdrew it, if they did and have not given it again. It goes into the transcript as
/// the consent record (research 11, item 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Consent {
    pub(crate) at: Millis,
    pub(crate) notice_version: u32,
    pub(crate) withdrawn_at: Option<Millis>,
}

impl Participant {
    /// Whether the participant consents to recording right now.
    pub(crate) fn consents(&self) -> bool {
        self.consent.is_some_and(|c| c.withdrawn_at.is_none())
    }
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
        publish: SessionSlot<PublishSession>,
        subscribe: SessionSlot<SubscribeSession>,
        /// The epochs of the sessions in the slots, or of the lost ones while replacing.
        epochs: SessionEpochs,
        /// Since when the participant's socket is gone, while they wait to resume; `None` while
        /// connected. Records stored before the field existed load as connected.
        #[serde(default)]
        socket_lost: Option<Millis>,
    },
    Left,
}

/// One of a joined participant's SFU sessions: in use, or being replaced because the SFU
/// declared it gone or the client's peer connection for it failed (D33 in `docs/media`).
/// Everything that works on a session skips a slot that is being replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SessionSlot<S> {
    Ready(S),
    Replacing(Replacement),
}

/// Where the request for a replacement session stands. A request that gets no reply within
/// [`NEGOTIATION_TIMEOUT_MS`] counts as failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Replacement {
    /// The first [`Effect::SfuNewSession`] is out since `since`.
    Requested { since: Millis },
    /// A request failed and the participant was told; the next one goes out at `retry_at`.
    Failed { retry_at: Millis },
    /// A later request is out since `since`; the participant was told about the first failure.
    Retrying { since: Millis },
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
    /// The request for a participant's replacement session of this kind timed out, or is due to
    /// be sent again.
    Replace(ParticipantId, SessionKind),
    /// A joined participant's socket has been gone for [`LEAVE_GRACE_MS`]: they leave.
    Grace(ParticipantId),
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

    /// The transcript link token.
    pub fn transcript(&self) -> &TranscriptRef {
        &self.transcript
    }

    /// Everyone who ever joined the room, including those who left, ordered by id. Names stay
    /// in the state after a participant leaves, so the transcript can name every speaker.
    pub fn roster(&self) -> Vec<RosterEntry> {
        self.participants
            .iter()
            .map(|(&id, p)| RosterEntry {
                id,
                name: p.name.clone(),
                role: p.role,
            })
            .collect()
    }

    /// The consent record for the transcript: everyone who ever consented to recording, with
    /// when they gave it, the notice they saw and when they withdrew it, ordered by id.
    pub fn consents(&self) -> Vec<ConsentRecord> {
        self.participants
            .iter()
            .filter_map(|(&who, p)| {
                let consent = p.consent?;
                Some(ConsentRecord {
                    who,
                    name: p.name.clone(),
                    at: consent.at,
                    notice_version: consent.notice_version,
                    withdrawn_at: consent.withdrawn_at,
                })
            })
            .collect()
    }

    /// The public snapshot every client sees.
    pub fn view(&self) -> RoomView {
        let participants = self
            .participants
            .iter()
            .filter_map(|(&id, p)| {
                let presence = p.connection.presence()?;
                let tracks = match &p.connection {
                    Connection::Joined {
                        publish: SessionSlot::Ready(publish),
                        ..
                    } => publish.kinds(),
                    _ => Vec::new(),
                };
                Some(ParticipantView {
                    id,
                    name: p.name.clone(),
                    role: p.role,
                    consent_recording: p.consents(),
                    presence,
                    tracks,
                    muted: p.muted,
                    reconnecting: p.connection.socket_lost().is_some(),
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
    ) -> impl Iterator<
        Item = (
            ParticipantId,
            &SessionSlot<PublishSession>,
            &SessionSlot<SubscribeSession>,
        ),
    > {
        self.participants
            .iter()
            .filter_map(|(&id, p)| match &p.connection {
                Connection::Joined {
                    publish, subscribe, ..
                } => Some((id, publish, subscribe)),
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

    /// Every track a joined participant publishes.
    pub(crate) fn published_tracks(&self) -> impl Iterator<Item = TrackRef> {
        self.joined().flat_map(|(owner, publish, _)| {
            publish.ready().into_iter().flat_map(move |publish| {
                publish
                    .published
                    .keys()
                    .map(move |&kind| TrackRef { owner, kind })
            })
        })
    }

    pub(crate) fn subscribe_slot_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut SessionSlot<SubscribeSession>> {
        match &mut self.participants.get_mut(&who)?.connection {
            Connection::Joined { subscribe, .. } => Some(subscribe),
            _ => None,
        }
    }

    pub(crate) fn publish_slot_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut SessionSlot<PublishSession>> {
        match &mut self.participants.get_mut(&who)?.connection {
            Connection::Joined { publish, .. } => Some(publish),
            _ => None,
        }
    }

    /// `who`'s subscribe session, if they are joined and it is not being replaced.
    pub(crate) fn subscribe_session_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut SubscribeSession> {
        self.subscribe_slot_mut(who)?.ready_mut()
    }

    /// `who`'s publish session, if they are joined and it is not being replaced.
    pub(crate) fn publish_session_mut(
        &mut self,
        who: ParticipantId,
    ) -> Option<&mut PublishSession> {
        self.publish_slot_mut(who)?.ready_mut()
    }

    /// The epochs of `who`'s sessions, if they are joined.
    pub(crate) fn epochs_mut(&mut self, who: ParticipantId) -> Option<&mut SessionEpochs> {
        match &mut self.participants.get_mut(&who)?.connection {
            Connection::Joined { epochs, .. } => Some(epochs),
            _ => None,
        }
    }

    /// Where the replacement of `who`'s session of `kind` stands, if it is being replaced.
    pub(crate) fn replacement_mut(
        &mut self,
        who: ParticipantId,
        kind: SessionKind,
    ) -> Option<&mut Replacement> {
        match kind {
            SessionKind::Publish => self.publish_slot_mut(who)?.replacement_mut(),
            SessionKind::Subscribe => self.subscribe_slot_mut(who)?.replacement_mut(),
        }
    }

    /// The SFU name of a published track, if its owner is joined and publishes it.
    pub(crate) fn resolve(&self, track: TrackRef) -> Option<RemoteTrack> {
        match &self.participants.get(&track.owner)?.connection {
            Connection::Joined {
                publish: SessionSlot::Ready(publish),
                ..
            } => Some(RemoteTrack {
                session: publish.id.clone(),
                name: publish.published.get(&track.kind)?.name.clone(),
            }),
            _ => None,
        }
    }

    /// Whether the room records and `who` has not consented.
    pub(crate) fn lacks_consent(&self, who: ParticipantId) -> bool {
        self.recording.current().is_some()
            && self.participants.get(&who).is_some_and(|p| !p.consents())
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
        let grace = self.participants.iter().filter_map(|(&who, p)| {
            let lost = p.connection.socket_lost()?;
            Some((lost.after(LEAVE_GRACE_MS), AlarmReason::Grace(who)))
        });
        let sessions = self.joined().flat_map(|(who, publish, subscribe)| {
            let replace = |kind, r: &Replacement| Some((r.due(), AlarmReason::Replace(who, kind)));

            let publish = match publish {
                SessionSlot::Ready(publish) => publish.negotiation.since().map(|since| {
                    (
                        since.after(NEGOTIATION_TIMEOUT_MS),
                        AlarmReason::Publish(who),
                    )
                }),
                SessionSlot::Replacing(r) => replace(SessionKind::Publish, r),
            };
            let subscribe = match subscribe {
                SessionSlot::Ready(subscribe) => {
                    subscribe.due().map(|at| (at, AlarmReason::Subscribe(who)))
                }
                SessionSlot::Replacing(r) => replace(SessionKind::Subscribe, r),
            };
            publish.into_iter().chain(subscribe)
        });
        end.into_iter().chain(grace).chain(sessions)
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

impl<S> SessionSlot<S> {
    /// The session, unless it is being replaced.
    pub(crate) fn ready(&self) -> Option<&S> {
        match self {
            Self::Ready(session) => Some(session),
            Self::Replacing(_) => None,
        }
    }

    pub(crate) fn ready_mut(&mut self) -> Option<&mut S> {
        match self {
            Self::Ready(session) => Some(session),
            Self::Replacing(_) => None,
        }
    }

    pub(crate) fn replacement_mut(&mut self) -> Option<&mut Replacement> {
        match self {
            Self::Ready(_) => None,
            Self::Replacing(replacement) => Some(replacement),
        }
    }
}

impl Replacement {
    /// When the room must act: when the request that is out times out, or when the next one
    /// goes out.
    pub(crate) fn due(self) -> Millis {
        match self {
            Self::Requested { since } | Self::Retrying { since } => {
                since.after(NEGOTIATION_TIMEOUT_MS)
            }
            Self::Failed { retry_at } => retry_at,
        }
    }
}

impl PublishSession {
    /// A new session with nothing published.
    pub(crate) fn new(id: SessionId) -> Self {
        Self {
            id,
            published: BTreeMap::new(),
            negotiation: PublishNegotiation::Idle,
        }
    }

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
    /// A new session that receives nothing yet and wants `wanted`.
    pub(crate) fn new(id: SessionId, wanted: BTreeMap<TrackRef, Millis>) -> Self {
        Self {
            id,
            receiving: BTreeMap::new(),
            wanted,
            negotiation: SubscribeNegotiation::Idle,
        }
    }

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

    /// Since when a joined participant's socket is gone; `None` while connected or not joined.
    pub(crate) fn socket_lost(&self) -> Option<Millis> {
        match self {
            Self::Joined { socket_lost, .. } => *socket_lost,
            _ => None,
        }
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
