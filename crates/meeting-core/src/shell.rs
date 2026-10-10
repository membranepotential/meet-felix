//! The interface to the imperative shell: events it feeds in, effects it must carry out.

use serde::{Deserialize, Serialize};

use crate::ids::{
    MeetingId, Mid, Millis, NegotiationId, ParticipantId, ResumeToken, Sdp, SessionId, TrackName,
};
use crate::protocol::{ClientMsg, LocalTrack, Rid, ServerMsg, SessionKind, TrackKind};
use crate::recording::{ConsentRecord, RosterEntry};

/// Something that happened to the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A client sent a message on its socket.
    Client {
        /// The socket's participant id.
        from: ParticipantId,
        /// The parsed message.
        msg: ClientMsg,
        /// Whether the socket belongs to the authenticated owner, which the shell decides (the
        /// Worker from the owner's session cookie). A [`ClientMsg::Join`] from such a socket
        /// makes a host, or turns the guest whose place it takes into one.
        owner: bool,
        /// A new token from the shell's random bytes. The room keeps it as the sender's resume
        /// token if `msg` is a [`ClientMsg::Join`] that adds a participant, and ignores it
        /// otherwise.
        fresh_token: ResumeToken,
    },
    /// The SFU answered a call the shell made for an [`Effect`].
    Sfu(SfuReply),
    /// A socket closed without [`ClientMsg::Leave`], and not because of [`Effect::CloseSocket`].
    SocketClosed {
        /// The socket's participant id.
        who: ParticipantId,
    },
    /// The alarm requested by [`Effect::SetAlarm`] fired. The room handles everything due by
    /// now and asks for the next alarm. Spurious or late alarms are harmless.
    AlarmFired,
}

/// A track in an SFU session, as the SFU API names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTrack {
    /// The publisher's publish session.
    pub session: SessionId,
    /// The track's name in that session.
    pub name: TrackName,
}

/// A remote track the SFU added to a subscribe offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfferedTrack {
    /// The transceiver's media id in the offer.
    pub mid: Mid,
    /// The track it carries.
    pub track: RemoteTrack,
}

/// The SFU call that failed, named after the effect that requested it, with the session it was
/// made on and, for a negotiation step, the negotiation it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SfuCall {
    /// [`Effect::SfuNewSession`].
    NewSession(SessionKind),
    /// [`Effect::SfuPublish`].
    Publish {
        /// The publish session.
        session: SessionId,
        /// The publish offer.
        negotiation: NegotiationId,
    },
    /// [`Effect::SfuSubscribe`].
    Subscribe {
        /// The subscribe session.
        session: SessionId,
        /// The subscribe negotiation.
        negotiation: NegotiationId,
    },
    /// [`Effect::SfuRenegotiate`].
    Renegotiate {
        /// The subscribe session.
        session: SessionId,
        /// The subscribe negotiation.
        negotiation: NegotiationId,
    },
}

/// Why an SFU call failed, as far as the room cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCause {
    /// The SFU no longer serves the session the call was made on (it answered HTTP 410, or
    /// with a `session_error`). Every later call on that session fails too, so the room
    /// replaces the session instead of retrying.
    SessionGone,
    /// Anything else; the room retries later.
    Transient,
}

/// The outcome of an SFU call. Replies about participants who left meanwhile, about a session
/// the room has replaced since, or about a negotiation that is no longer the session's running
/// one (it timed out, and perhaps a newer one started) are ignored; only a failure that reports
/// the session gone counts whatever negotiation it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SfuReply {
    /// Reply to [`Effect::SfuNewSession`].
    SessionCreated {
        /// The participant the session belongs to.
        who: ParticipantId,
        /// Which session.
        kind: SessionKind,
        /// The new session's id.
        session: SessionId,
    },
    /// Reply to [`Effect::SfuPublish`]: the SFU published the offered tracks except those of the
    /// `rejected` kinds, for which it returned a per-track error. If it rejected every track,
    /// the call failed instead ([`SfuReply::Failed`]); a reply that rejects every offered kind
    /// anyway counts as such a failure.
    Published {
        /// The publisher.
        who: ParticipantId,
        /// Their publish session.
        session: SessionId,
        /// The publish offer.
        negotiation: NegotiationId,
        /// The SFU's answer for the client; it covers the whole offer.
        sdp_answer: Sdp,
        /// The kinds of the offered tracks the SFU did not publish; empty if it published all.
        rejected: Vec<TrackKind>,
    },
    /// Reply to [`Effect::SfuSubscribe`]: the SFU needs the client to answer this offer.
    SubscribeOffer {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// The subscribe negotiation.
        negotiation: NegotiationId,
        /// The SFU's offer for the client.
        sdp_offer: Sdp,
        /// The tracks the offer adds.
        tracks: Vec<OfferedTrack>,
    },
    /// Reply to [`Effect::SfuRenegotiate`]: the subscribe session carries the offered tracks.
    Renegotiated {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// The subscribe negotiation.
        negotiation: NegotiationId,
    },
    /// The SFU no longer serves one of a participant's published tracks.
    TrackClosed {
        /// The publisher.
        who: ParticipantId,
        /// Which track.
        kind: TrackKind,
    },
    /// An SFU call failed.
    Failed {
        /// The participant the call was made for.
        who: ParticipantId,
        /// Which call.
        call: SfuCall,
        /// Why it failed.
        cause: FailureCause,
    },
}

/// Something the shell must do, in list order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Send a message to one socket.
    Send {
        /// The recipient.
        to: ParticipantId,
        /// The message.
        msg: ServerMsg,
    },
    /// Send the same message to several sockets (never an empty list).
    Broadcast {
        /// The recipients.
        to: Vec<ParticipantId>,
        /// The message.
        msg: ServerMsg,
    },
    /// Close a socket. The shell delivers no further events from it, not even
    /// [`Event::SocketClosed`].
    CloseSocket {
        /// Whose socket.
        who: ParticipantId,
    },
    /// A client resumed on a new socket: bind that socket to its participant. From now on the
    /// shell delivers the socket's messages and its close as coming from `participant`, and
    /// sends `participant`'s messages to it. Always follows the [`Effect::CloseSocket`] for the
    /// participant's previous socket.
    Rebind {
        /// The new socket's id, as the shell assigned it.
        socket: ParticipantId,
        /// The participant it now belongs to.
        participant: ParticipantId,
    },
    /// Create an SFU session; reply with [`SfuReply::SessionCreated`].
    SfuNewSession {
        /// The participant it is for.
        who: ParticipantId,
        /// Which of their sessions.
        kind: SessionKind,
    },
    /// Add the client's local tracks to its publish session; reply with
    /// [`SfuReply::Published`].
    SfuPublish {
        /// The publisher.
        who: ParticipantId,
        /// Their publish session.
        session: SessionId,
        /// Names this offer; the reply carries it back.
        negotiation: NegotiationId,
        /// The client's offer.
        sdp_offer: Sdp,
        /// The tracks to publish.
        tracks: Vec<LocalTrack>,
    },
    /// Add remote tracks to a subscribe session; reply with [`SfuReply::SubscribeOffer`].
    SfuSubscribe {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// Names the negotiation this request starts; the reply carries it back.
        negotiation: NegotiationId,
        /// The tracks to receive.
        tracks: Vec<RemoteTrack>,
    },
    /// Hand the client's answer to the SFU; reply with [`SfuReply::Renegotiated`].
    SfuRenegotiate {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// The negotiation the answer belongs to; the reply carries it back.
        negotiation: NegotiationId,
        /// The client's answer.
        sdp_answer: Sdp,
    },
    /// Close these transceivers without renegotiation. No reply. On a subscribe session the SFU
    /// stops sending the remote tracks; on the publish session (after
    /// [`ClientMsg::Unpublish`]) it drops the participant's own track.
    SfuCloseTracks {
        /// The session's owner.
        who: ParticipantId,
        /// One of their sessions.
        session: SessionId,
        /// The transceivers to close.
        mids: Vec<Mid>,
    },
    /// Change the simulcast layer a subscriber receives. No reply.
    SfuUpdateLayer {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// The transceiver in that session.
        mid: Mid,
        /// The track it carries.
        track: RemoteTrack,
        /// The preferred layer.
        rid: Rid,
    },
    /// Fire [`Event::AlarmFired`] at this time, replacing any earlier alarm. The room has one
    /// alarm, at the earliest time it must act: when an unused scheduled room expires, when an
    /// empty live room ends, when a publish offer or a subscribe negotiation times out, when a
    /// failed subscription is retried, or when a failed request for a replacement session is
    /// sent again. It emits this whenever
    /// that time changes and after every alarm; it never clears an alarm, because a spurious
    /// one is harmless.
    SetAlarm {
        /// When.
        at: Millis,
    },
    /// Start the recorder. [`Effect::RecordTrack`] effects name the tracks to record.
    StartRecording {
        /// The recording's start time.
        since: Millis,
    },
    /// Record this audio track of a participant who consented.
    RecordTrack {
        /// The speaker.
        who: ParticipantId,
        /// Their audio track.
        track: RemoteTrack,
    },
    /// Stop recording this audio track: its speaker withdrew consent, unpublished or replaced
    /// it, or left.
    StopRecordingTrack {
        /// The speaker.
        who: ParticipantId,
        /// Their audio track.
        track: RemoteTrack,
    },
    /// Stop recording all tracks.
    StopRecording,
    /// The meeting ended after it was recorded: the recorder finishes its chunks and starts the
    /// transcript job. Follows [`Effect::StopRecording`], if recording was on, and precedes
    /// [`Effect::EndMeeting`].
    FinishRecording {
        /// Everyone who ever joined, for naming the speakers.
        roster: Vec<RosterEntry>,
        /// The consent record for the transcript.
        consents: Vec<ConsentRecord>,
    },
    /// Make the meeting's words available for new meetings.
    ReleaseMeetingId {
        /// The words to release.
        meeting: MeetingId,
    },
    /// The room is over: start the transcript job and accept no more sockets. Always the last
    /// effect the room produces.
    EndMeeting,
}
