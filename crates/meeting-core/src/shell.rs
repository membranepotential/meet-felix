//! The interface to the imperative shell: events it feeds in, effects it must carry out.

use serde::{Deserialize, Serialize};

use crate::ids::{MeetingId, Mid, Millis, ParticipantId, ResumeToken, Sdp, SessionId, TrackName};
use crate::protocol::{ClientMsg, LocalTrack, Rid, ServerMsg, TrackKind};

/// Something that happened to the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A client sent a message on its socket.
    Client {
        /// The socket's participant id.
        from: ParticipantId,
        /// The parsed message.
        msg: ClientMsg,
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

/// Which of a participant's two SFU sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKind {
    /// Carries the participant's own tracks; the client sends offers.
    Publish,
    /// Carries other participants' tracks; the SFU sends offers.
    Subscribe,
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

/// The SFU call that failed, named after the effect that requested it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SfuCall {
    /// [`Effect::SfuNewSession`].
    NewSession(SessionKind),
    /// [`Effect::SfuPublish`].
    Publish,
    /// [`Effect::SfuSubscribe`].
    Subscribe,
    /// [`Effect::SfuRenegotiate`].
    Renegotiate,
}

/// The outcome of an SFU call. Replies about participants who left meanwhile are ignored.
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
    /// Reply to [`Effect::SfuPublish`]: all offered tracks are published.
    Published {
        /// The publisher.
        who: ParticipantId,
        /// The SFU's answer for the client.
        sdp_answer: Sdp,
    },
    /// Reply to [`Effect::SfuSubscribe`]: the SFU needs the client to answer this offer.
    SubscribeOffer {
        /// The subscriber.
        who: ParticipantId,
        /// The SFU's offer for the client.
        sdp_offer: Sdp,
        /// The tracks the offer adds.
        tracks: Vec<OfferedTrack>,
    },
    /// Reply to [`Effect::SfuRenegotiate`]: the subscribe session carries the offered tracks.
    Renegotiated {
        /// The subscriber.
        who: ParticipantId,
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
        /// The tracks to receive.
        tracks: Vec<RemoteTrack>,
    },
    /// Hand the client's answer to the SFU; reply with [`SfuReply::Renegotiated`].
    SfuRenegotiate {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
        session: SessionId,
        /// The client's answer.
        sdp_answer: Sdp,
    },
    /// Stop sending these tracks on a subscribe session, without renegotiation. No reply.
    SfuCloseTracks {
        /// The subscriber.
        who: ParticipantId,
        /// Their subscribe session.
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
    /// alarm, at the earliest time it must act: when an empty room ends, when a subscribe
    /// negotiation times out, or when a failed subscription is retried. It emits this whenever
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
    /// Stop recording this audio track: its speaker withdrew consent.
    StopRecordingTrack {
        /// The speaker.
        who: ParticipantId,
        /// Their audio track.
        track: RemoteTrack,
    },
    /// Stop recording all tracks.
    StopRecording,
    /// Make the meeting's words available for new meetings.
    ReleaseMeetingId {
        /// The words to release.
        meeting: MeetingId,
    },
    /// The room is over: start the transcript job and accept no more sockets. Always the last
    /// effect the room produces.
    EndMeeting,
}
