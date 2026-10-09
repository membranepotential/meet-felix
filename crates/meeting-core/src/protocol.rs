//! Messages between browser clients and the room, and the public room snapshot.

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use tsify::Tsify;

use crate::ids::{
    DisplayName, HostToken, MeetingId, Mid, Millis, ParticipantId, ResumeToken, Sdp, TrackName,
    TranscriptRef,
};

/// A participant's authority in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// Joined with the host token; may lock, admit, kick, mute, record and end.
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

/// Where a participant stands in the room.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum Presence {
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
    /// Knocking, awaiting consent, connecting or joined.
    pub presence: Presence,
    /// Kinds of the tracks they publish, sorted.
    pub tracks: Vec<TrackKind>,
}

/// The public snapshot of a room: no secrets, no SDP, no SFU sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct RoomView {
    /// The meeting's link words.
    pub meeting: MeetingId,
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

/// A message from a client to the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMsg {
    /// Enter the room. Must be the socket's first message; repeating it changes nothing and
    /// gets the same reply again.
    #[serde(rename_all = "camelCase")]
    Join {
        /// Display name.
        name: DisplayName,
        /// Whether the participant accepts recording and transcription.
        consent: bool,
        /// The host token, if this participant created the meeting.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        host_token: Option<HostToken>,
        /// The token from an earlier [`ServerMsg::Welcome`], after the socket dropped. If its
        /// participant is still in the room, this socket takes their place with their role and
        /// tracks, and the other fields are ignored; otherwise this is a fresh join.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        resume: Option<ResumeToken>,
    },
    /// Give or withdraw consent to recording. Withdrawing it while the room records stops the
    /// recording of your audio and mutes you.
    SetConsent {
        /// Whether you accept recording and transcription.
        consent: bool,
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
    /// Host: ask a participant to mute their microphone.
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
    },
    /// The room changed.
    State {
        /// The room as it is now.
        state: RoomView,
    },
    /// The room is locked; wait until a host admits you.
    Waiting,
    /// The SFU's answer to your publish offer.
    PublishAnswer {
        /// The answer.
        sdp: Sdp,
    },
    /// New remote tracks for your subscribe session; reply with [`ClientMsg::SubscribeAnswer`].
    SubscribeOffer {
        /// The SFU's offer.
        sdp: Sdp,
        /// Which transceiver carries which participant's track.
        tracks: Vec<IncomingTrack>,
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
    /// Mute your microphone: a host asked, or the room records without your consent.
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
    /// The host token is wrong.
    InvalidHostToken,
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
    /// The SFU could not create your sessions; reconnect later.
    SfuUnavailable,
    /// The SFU rejected your publish offer, or did not answer it in time.
    PublishFailed,
    /// The SFU could not add remote tracks to your subscribe session.
    SubscribeFailed,
}
