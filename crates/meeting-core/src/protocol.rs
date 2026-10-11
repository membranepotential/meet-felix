//! Messages between browser clients and the room, and the public room snapshot.

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use tsify::Tsify;

use crate::ids::{
    DisplayName, MeetingId, Mid, Millis, ParticipantId, ResumeToken, Sdp, SessionEpoch, TrackName,
    TranscriptRef,
};

/// A participant's authority in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Joined from a socket of the authenticated owner; may lock, admit, kick, mute, record and
    /// end.
    Host,
    /// Everyone else.
    Guest,
}

/// The kind of media a published track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    /// Microphone.
    Audio,
    /// Camera.
    Video,
    /// Screen share.
    Screen,
}

/// Which of a participant's two SFU sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    /// Carries the participant's own tracks; the client sends offers.
    Publish,
    /// Carries other participants' tracks; the SFU sends offers.
    Subscribe,
}

/// The epochs of a participant's current SFU sessions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct SessionEpochs {
    /// The publish session's.
    pub publish: SessionEpoch,
    /// The subscribe session's.
    pub subscribe: SessionEpoch,
}

impl SessionEpochs {
    /// The epoch of the session of `kind`.
    pub fn of(self, kind: SessionKind) -> SessionEpoch {
        match kind {
            SessionKind::Publish => self.publish,
            SessionKind::Subscribe => self.subscribe,
        }
    }

    pub(crate) fn of_mut(&mut self, kind: SessionKind) -> &mut SessionEpoch {
        match kind {
            SessionKind::Publish => &mut self.publish,
            SessionKind::Subscribe => &mut self.subscribe,
        }
    }
}

/// A simulcast layer. The publisher must use the serialized names (`f`, `h`, `q`) as its
/// encoding `rid`s.
///
/// The names sort the layers from the best to the worst asciibetically (`f` < `h` < `q`). The
/// SFU uses that order when the preferred layer is unavailable (`ridNotAvailable:
/// "asciibetical"`): it switches to the next name, so a missing full layer falls back to half,
/// not to quarter, as it would with `high` < `low` < `mid`. Cloudflare's simulcast docs use the
/// same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub enum Rid {
    /// Quarter resolution (`q`): the lowest bitrate.
    #[serde(rename = "q")]
    Low,
    /// Half resolution (`h`).
    #[serde(rename = "h")]
    Mid,
    /// Full resolution (`f`); the default for new subscriptions.
    #[serde(rename = "f")]
    High,
}

/// A published track, named by its publisher and kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct TrackRef {
    /// The publishing participant.
    pub owner: ParticipantId,
    /// Which of the owner's tracks.
    pub kind: TrackKind,
}

/// A track the client adds to its publish session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct LocalTrack {
    /// The transceiver's media id in the offer.
    pub mid: Mid,
    /// What the track carries.
    pub kind: TrackKind,
    /// The track's name in the SFU, unique within the publish session.
    pub name: TrackName,
}

/// A remote track in a subscribe offer, mapped to its transceiver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct IncomingTrack {
    /// The transceiver's media id in the offer.
    pub mid: Mid,
    /// Whose track it is.
    pub track: TrackRef,
}

/// An active recording. Part of every snapshot, so every client shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct Recording {
    /// When recording started.
    pub since: Millis,
    /// The host who started it.
    pub by: ParticipantId,
}

/// Whether a meeting records, chosen by the owner when creating it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum RecordingPolicy {
    /// Recording starts when the first host joins; the pre-join screen asks for consent.
    FromStart,
    /// Recording starts only when a host sends [`ClientMsg::SetRecording`].
    HostDecides,
}

/// Where the meeting stands in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Created ahead of time; no host has joined yet. Guests wait for the first host, and the
    /// room expires if no host joins within [`crate::SCHEDULED_TTL_MS`].
    Scheduled,
    /// A host has joined at least once. The room ends when a host ends it or after
    /// [`crate::EMPTY_ROOM_TIMEOUT_MS`] without admitted participants.
    Live,
    /// Over; the room accepts nobody.
    Ended,
}

/// Where a participant stands in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum Presence {
    /// A guest in a scheduled room, waiting for the first host; let in automatically when the
    /// first host joins.
    WaitingForHost,
    /// Waiting for a host to admit them into the locked room.
    Knocking,
    /// Let in by a host or an unlocked room, but the room records and they have not consented;
    /// they enter once they send [`ClientMsg::SetConsent`] or recording stops.
    AwaitingConsent,
    /// Admitted; the SFU sessions are being created.
    Connecting,
    /// In the meeting.
    Joined,
}

/// A participant as every client sees them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct ParticipantView {
    /// Who.
    pub id: ParticipantId,
    /// Display name.
    pub name: DisplayName,
    /// Host or guest.
    pub role: Role,
    /// Whether they agreed to be recorded; only consenting participants are recorded.
    pub consent_recording: bool,
    /// Waiting for the host, knocking, awaiting consent, connecting or joined.
    pub presence: Presence,
    /// Kinds of the tracks they publish, sorted.
    pub tracks: Vec<TrackKind>,
    /// Whether their microphone is switched off: by themselves ([`ClientMsg::SetMuted`]), by a
    /// host ([`ClientMsg::Mute`]), or because the room records without their consent. A muted
    /// microphone may still publish its (silent) audio track.
    pub muted: bool,
    /// Whether their socket dropped and the room waits up to [`crate::LEAVE_GRACE_MS`] for their
    /// client to come back. They stay joined meanwhile, and their tracks stay listed, but what
    /// others receive from them may be frozen or silent.
    pub reconnecting: bool,
    /// Whether a running recording leaves them out because they have not consented (D15): they
    /// are admitted (muted and not recorded) or let in but waiting for their consent. A host's
    /// client lists them. Always false while nothing records, for a participant who consents,
    /// and for one still waiting for the host or knocking. Absent in views sent before the
    /// field existed, which read as false.
    #[serde(default)]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub awaiting_consent: bool,
}

/// The public snapshot of a room: no secrets, no SDP, no SFU sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct RoomView {
    /// The meeting's link words.
    pub meeting: MeetingId,
    /// Scheduled, live or ended.
    pub phase: Phase,
    /// Whether guests must knock.
    pub locked: bool,
    /// Whether the meeting records from the start; the pre-join screen shows it.
    pub recording_policy: RecordingPolicy,
    /// The running recording, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub recording: Option<Recording>,
    /// Everyone who has not left, ordered by id.
    pub participants: Vec<ParticipantView>,
}

/// The version of the socket protocol: [`ClientMsg`], [`ServerMsg`] and the close codes
/// ([`CloseCode`]). Raise it with a change that would make an older client misbehave, and
/// raise [`MIN_PROTOCOL_VERSION`] to it if the room must refuse such clients. A change older
/// clients survive needs neither: a client ignores a server message it cannot parse, and the
/// room answers a client message it cannot parse with [`ErrorCode::MalformedMessage`].
///
/// 1 is every client built before the version existed (2026-10-11); 2 sends it in `Join`.
pub const PROTOCOL_VERSION: u32 = 2;

/// The oldest client protocol the room serves. A web app cached from before a deploy reconnects
/// to the new Worker with its old protocol, in every meeting that runs during the deploy, so
/// this stays as low as the room can serve.
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// The protocol of a `Join` that names none: a client built before the version existed.
const fn unversioned() -> u32 {
    1
}

/// Why the room closes a client's socket. Each reason has its WebSocket close code, which tells
/// the client what to do next ([`after_close`]), and a reason text for the close frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseCode {
    /// The client sent [`ClientMsg::Leave`].
    Left,
    /// The client resumed its place on a new socket; this is the old one.
    Replaced,
    /// The client stopped pinging (the room's keepalive check).
    Stale,
    /// The socket sent more messages, or caused more writes, than its budget allows.
    OverBudget,
    /// The socket did not join in time.
    NoJoin,
    /// The socket was the oldest of too many that have not joined.
    Crowded,
    /// The SFU could not create a session for a participant who was not in yet.
    TryLater,
    /// A host removed the participant ([`ServerMsg::Kicked`] came first).
    Kicked,
    /// The meeting ended ([`ServerMsg::Ended`] came first).
    Ended,
    /// The room took nobody new ([`ErrorCode::RoomFull`] came first).
    RoomFull,
    /// The client's protocol is older than [`MIN_PROTOCOL_VERSION`]
    /// ([`ErrorCode::OutdatedClient`] came first).
    Outdated,
    /// The meeting ended while the socket was bound to no participant, such as a client resuming
    /// its place whose `Join` was not processed yet, so no [`ServerMsg::Ended`] reached it. The
    /// client reconnects and its lookup finds the meeting ended; a final code would show it
    /// "refused".
    EndedUnseen,
}

/// What a client does after the room closed its socket with a code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum AfterClose {
    /// Open a new socket after a backoff, and resume with the token if it has one.
    Reconnect,
    /// The stay is over: reconnecting would not help.
    Stop,
    /// The page is out of date: load it again.
    Reload,
}

/// The close code of [`CloseCode::Outdated`].
const OUTDATED: u16 = 4004;

impl CloseCode {
    /// The WebSocket close code. The final ones lie in 4000–4099, which clients built before
    /// protocol 2 already treat as final, so such a client stops on [`CloseCode::Outdated`] too
    /// and shows its reason.
    pub const fn code(self) -> u16 {
        match self {
            Self::Left | Self::Replaced => 1000,
            // Going away.
            Self::Stale | Self::EndedUnseen => 1001,
            // Policy violation.
            Self::OverBudget | Self::NoJoin | Self::Crowded => 1008,
            // Try again later.
            Self::TryLater => 1013,
            Self::Kicked => 4001,
            Self::Ended => 4002,
            Self::RoomFull => 4003,
            Self::Outdated => OUTDATED,
        }
    }

    /// The close frame's reason (at most 123 bytes). A client shows it when its socket closes
    /// for good without a message that says why, so the final ones are sentences for the user.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Replaced => "resumed on another socket",
            Self::Stale => "no ping",
            Self::OverBudget => "too many messages",
            Self::NoJoin => "no join",
            Self::Crowded => "too many sockets",
            Self::TryLater => "the media server is not reachable",
            Self::Kicked => "A host removed you from the meeting.",
            Self::Ended => "The meeting has ended.",
            Self::RoomFull => "This meeting is full. Try again later.",
            Self::Outdated => "This page is out of date. Reload it to join the meeting.",
            Self::EndedUnseen => "The meeting has ended.",
        }
    }
}

/// What a client does after its socket closed with `code`: codes 4000–4099 are final, the code
/// of [`CloseCode::Outdated`] asks for a reload, and every other code means reconnect, also the
/// codes a browser reports for a lost connection, such as 1006.
pub fn after_close(code: u16) -> AfterClose {
    match code {
        OUTDATED => AfterClose::Reload,
        4000..=4099 => AfterClose::Stop,
        _ => AfterClose::Reconnect,
    }
}

/// A message from a client to the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMsg {
    /// Enter the room. Must be the socket's first message; repeating it changes nothing and
    /// gets the same reply again. The participant becomes a host if the shell reports the socket
    /// as the authenticated owner's ([`crate::Event::Client`]), and a guest otherwise.
    #[serde(rename_all = "camelCase")]
    Join {
        /// Display name.
        name: DisplayName,
        /// Whether the participant accepts recording and transcription.
        consent: bool,
        /// The token from an earlier [`ServerMsg::Welcome`], after the socket dropped. If its
        /// participant is still in the room, this socket takes their place with their role and
        /// tracks, and the other fields are ignored; otherwise this is a fresh join. A guest's
        /// place taken from the owner's socket becomes a host's.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        resume: Option<ResumeToken>,
        /// The protocol version the client speaks: the [`PROTOCOL_VERSION`] of the core it was
        /// built with. A client built before the field existed sends none and counts as
        /// version 1. The room refuses a version below [`MIN_PROTOCOL_VERSION`] with
        /// [`ErrorCode::OutdatedClient`] and [`CloseCode::Outdated`].
        #[serde(default = "unversioned")]
        protocol: u32,
    },
    /// Give or withdraw consent to recording. Withdrawing it while the room records stops the
    /// recording of your audio and mutes you.
    SetConsent {
        /// Whether you accept recording and transcription.
        consent: bool,
    },
    /// Report your microphone switch: send it whenever it changes, and after every
    /// [`ServerMsg::Welcome`] whose state shows you otherwise. Every client sees it as
    /// [`ParticipantView::muted`].
    SetMuted {
        /// Whether your microphone is off.
        muted: bool,
    },
    /// Offer new tracks on the publish session.
    PublishOffer {
        /// The client's offer.
        sdp: Sdp,
        /// The transceivers to publish, with distinct kinds, mids and names.
        tracks: Vec<LocalTrack>,
    },
    /// Stop publishing your track of this kind, e.g. because the camera was turned off. The
    /// room closes it on the publish session and on every subscriber. The client stops the
    /// transceiver itself; a later [`ClientMsg::PublishOffer`] may publish the kind again.
    Unpublish {
        /// Which of your tracks.
        kind: TrackKind,
    },
    /// Answer the room's last [`ServerMsg::SubscribeOffer`].
    SubscribeAnswer {
        /// The client's answer.
        sdp: Sdp,
    },
    /// Prefer a simulcast layer for a received track.
    SetLayer {
        /// The received track.
        track: TrackRef,
        /// The preferred layer.
        rid: Rid,
    },
    /// Your peer connection for this session failed (`connectionState` became `failed`): the
    /// room replaces the SFU session and answers with [`ServerMsg::PublishReset`] or
    /// [`ServerMsg::SubscribeReset`]. Ignored while the replacement runs, and if `epoch` is not
    /// the session's current one, i.e. the failed connection belonged to an earlier session.
    ResetSession {
        /// Which of your sessions.
        kind: SessionKind,
        /// The epoch of the session whose connection failed, from [`ServerMsg::Welcome`] or the
        /// last reset of that kind.
        epoch: SessionEpoch,
    },
    /// Host: lock or unlock the room. Unlocking admits everyone knocking.
    Lock {
        /// Whether guests must knock from now on.
        on: bool,
    },
    /// Host: let a knocking guest in.
    Admit {
        /// The knocking guest.
        who: ParticipantId,
    },
    /// Host: remove another participant.
    Kick {
        /// The participant to remove.
        who: ParticipantId,
    },
    /// Host: mute a participant's microphone. The room shows them muted at once and sends them
    /// [`ServerMsg::Muted`]; only they can unmute themselves.
    Mute {
        /// The participant to mute.
        who: ParticipantId,
    },
    /// Host: start or stop recording.
    SetRecording {
        /// Whether to record.
        on: bool,
    },
    /// Host: end the meeting for everyone.
    End,
    /// Leave the meeting.
    Leave,
}

/// A message from the room to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMsg {
    /// You are in; the client may now publish.
    Welcome {
        /// Your participant id.
        you: ParticipantId,
        /// The room as it is now.
        state: RoomView,
        /// Send this in [`ClientMsg::Join`] to take your place again if the socket drops.
        resume: ResumeToken,
        /// Your current SFU sessions. After a reconnect, a peer connection made for an older
        /// epoch belongs to a replaced session: replace it as on a reset.
        epochs: SessionEpochs,
    },
    /// The room changed.
    State {
        /// The room as it is now.
        state: RoomView,
    },
    /// Wait outside: the room is locked and a host must admit you, or the meeting is scheduled
    /// and you enter when the first host joins.
    Waiting,
    /// The SFU's answer to your publish offer. Apply it whole: it covers every offered track,
    /// the rejected ones too.
    PublishAnswer {
        /// The answer.
        sdp: Sdp,
        /// The kinds of the offered tracks the SFU did not publish: nobody receives them, and
        /// the room does not list them. Stop their transceivers; a later
        /// [`ClientMsg::PublishOffer`] may try the kind again. Absent if every track was
        /// published.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        rejected: Vec<TrackKind>,
    },
    /// New remote tracks for your subscribe session; reply with [`ClientMsg::SubscribeAnswer`].
    SubscribeOffer {
        /// The SFU's offer.
        sdp: Sdp,
        /// Which transceiver carries which participant's track.
        tracks: Vec<IncomingTrack>,
    },
    /// The room replaced your publish session: close the publisher's peer connection, open a
    /// new one and publish your current tracks again with a new [`ClientMsg::PublishOffer`].
    PublishReset {
        /// The new session's epoch.
        epoch: SessionEpoch,
    },
    /// The room replaced your subscribe session: close the subscriber's peer connection and
    /// open a new one. Every [`ServerMsg::SubscribeOffer`] after this applies to the new one.
    SubscribeReset {
        /// The new session's epoch.
        epoch: SessionEpoch,
    },
    /// Hosts only: someone is waiting to be admitted.
    Knock {
        /// The knocking guest.
        who: ParticipantId,
    },
    /// The room records and you have not consented. Show the consent dialog and reply with
    /// [`ClientMsg::SetConsent`]. Until you consent you stay outside the room or, if you are
    /// already in, muted and not recorded.
    ConsentRequired,
    /// Mute your microphone: a host asked, or the room records without your consent. The room
    /// already shows you muted; send [`ClientMsg::SetMuted`] when you unmute.
    Muted,
    /// A host removed you; the socket closes next.
    Kicked,
    /// The meeting is over; the socket closes next.
    Ended {
        /// The transcript link token, if the meeting was recorded.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        transcript: Option<TranscriptRef>,
    },
    /// Your last message was rejected.
    Error {
        /// Why.
        code: ErrorCode,
    },
}

/// Why the room rejected a client message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// The shell could not parse the message.
    MalformedMessage,
    /// The message needs a joined participant.
    NotJoined,
    /// Only hosts may do this.
    NotHost,
    /// The target participant does not exist or is in the wrong state.
    InvalidTarget,
    /// The publish offer lists no tracks, duplicates, or names already in use.
    InvalidOffer,
    /// The previous publish offer is still being processed.
    NegotiationInProgress,
    /// You do not publish a track of that kind.
    NotPublished,
    /// There is no subscribe offer waiting for an answer.
    UnexpectedAnswer,
    /// You do not receive that track.
    NotSubscribed,
    /// The SFU could not create a session for you. Before you joined, the room lets you go and
    /// closes the socket ([`CloseCode::TryLater`]): reconnect later. Once you are in, the room keeps trying to replace a lost session.
    SfuUnavailable,
    /// The SFU rejected your publish offer, or did not answer it in time.
    PublishFailed,
    /// The SFU could not add remote tracks to your subscribe session.
    SubscribeFailed,
    /// The room takes nobody new: it has as many participants as it allows at once, or in all
    /// ([`crate::MAX_PRESENT`], [`crate::MAX_ROSTER`]). Your `Join` changed nothing, and the
    /// socket closes next ([`CloseCode::RoomFull`]).
    RoomFull,
    /// Your `Join` names a protocol older than the room serves ([`MIN_PROTOCOL_VERSION`]):
    /// load the page again. The socket closes next ([`CloseCode::Outdated`]).
    OutdatedClient,
}
