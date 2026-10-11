//! The functional core of a meet-felix room.
//!
//! The shell (a Durable Object, or a test) owns a [`RoomState`], feeds every [`Event`] through
//! [`step`] together with the current time, and carries out the returned [`Effect`]s in order.
//! The core does no I/O and reads no clock or randomness, so it runs unchanged on the native
//! target and on `wasm32-unknown-unknown`.
//!
//! Rules the core enforces:
//! - A room is [`Phase::Scheduled`] until the first host joins, then [`Phase::Live`], then
//!   [`Phase::Ended`]. A joiner whose socket the shell reports as the authenticated owner's
//!   becomes a host; everyone else is a guest. Guests joining a scheduled room wait for the
//!   first host and are let in when it joins; guests joining a locked live room knock until a
//!   host admits them. A guest whose place an owner's socket takes becomes a host.
//! - While the room records, no guest is admitted without consent: they wait until they send
//!   `SetConsent` or recording stops. When recording starts, or a participant withdraws consent,
//!   everyone in the room without consent is asked for it and muted, and is not recorded.
//! - A [`RecordingPolicy::FromStart`] meeting starts recording when the first host joins.
//! - Only hosts lock, admit, kick, mute, record and end.
//! - Every change to the public [`RoomView`] reaches every joined client, so all of them see a
//!   recording start; only consenting participants' audio is recorded.
//! - Each participant has a publish session (the client offers) and a subscribe session (the SFU
//!   offers, one negotiation at a time); new tracks reach every other joined participant, and an
//!   unpublished track or a leaving participant's tracks are closed on all subscribers and no
//!   longer recorded. A track stays wanted until a negotiation delivers it; a failed or
//!   timed-out subscribe negotiation is retried after [`SUBSCRIBE_RETRY_MS`], and a track the
//!   SFU did not find yet (its publisher's connection is not up) after [`FIRST_PULL_RETRY_MS`],
//!   the first [`FIRST_PULL_RETRIES`] times. A publish offer
//!   the SFU does not answer within [`NEGOTIATION_TIMEOUT_MS`] fails, so the client may offer
//!   again. Only the tracks the SFU accepted are published; the client hears which kinds it
//!   rejected, and an answer that rejects all of them fails the offer.
//! - SFU effects and replies name their session and, for a negotiation step, the
//!   [`NegotiationId`]; a reply about a replaced session, or about a negotiation that is no
//!   longer running on its session, changes nothing, except a failure that reports the session
//!   gone.
//! - A subscribe or publish session the SFU declares gone ([`FailureCause::SessionGone`]), or
//!   whose peer connection the client reports failed ([`ClientMsg::ResetSession`]), is replaced
//!   by a new one, not retried. A lost publish session takes its tracks with it, for everyone.
//!   The client hears [`ServerMsg::SubscribeReset`] or [`ServerMsg::PublishReset`] once the new
//!   session exists; a failed request for it is sent again after [`SUBSCRIBE_RETRY_MS`].
//! - A client whose socket dropped takes its place again with its [`ResumeToken`]. A joined
//!   participant whose socket closes without a `Leave` keeps their place, sessions and tracks
//!   (and their recording) for [`LEAVE_GRACE_MS`], shown as reconnecting; only if no client takes
//!   the place within that time do they leave. Anyone not yet joined leaves at once.
//! - A live room ends when a host ends it or after [`EMPTY_ROOM_TIMEOUT_MS`] without admitted
//!   participants; knocking guests do not count, and a participant waiting to resume does. A
//!   scheduled room has no empty timer: it ends only if no host joins within
//!   [`SCHEDULED_TTL_MS`] of its creation.
//! - When a recorded meeting ends, only participants who were let in (connecting or joined,
//!   also while waiting to resume) get the transcript link in [`ServerMsg::Ended`].
//!
//! The [`recording`] module holds the recorder's rules (speaker labels, silence gate, chunk
//! cuts, holes in the adapter stream) and [`transcript`] the merge of speech segments into the
//! transcript; the `RecorderObject` carries them out.
//!
//! With the `wasm` feature, the message and id types derive `tsify` TypeScript declarations,
//! and `validateMeetingId`, `generateMeetingId`, `parseServerMsg` and `encodeClientMsg` are
//! exported through `wasm-bindgen`.

mod ids;
mod protocol;
pub mod recording;
mod shell;
mod state;
mod step;
pub mod transcript;
#[cfg(feature = "wasm")]
mod wasm;
pub mod words;

#[cfg(test)]
mod tests;

pub use ids::{
    DisplayName, InvalidDisplayName, InvalidMeetingId, MeetingId, Mid, Millis, NegotiationId,
    ParticipantId, ResumeToken, Sdp, SessionEpoch, SessionId, TrackName, TranscriptRef,
};
pub use protocol::{
    ClientMsg, ErrorCode, IncomingTrack, LocalTrack, ParticipantView, Phase, Presence, Recording,
    RecordingPolicy, Rid, Role, RoomView, ServerMsg, SessionEpochs, SessionKind, TrackKind,
    TrackRef,
};
pub use shell::{Effect, Event, FailureCause, OfferedTrack, RemoteTrack, SfuCall, SfuReply};
pub use state::{
    EMPTY_ROOM_TIMEOUT_MS, FIRST_PULL_RETRIES, FIRST_PULL_RETRY_MS, LEAVE_GRACE_MS,
    NEGOTIATION_TIMEOUT_MS, NOTICE_VERSION, RoomState, SCHEDULED_TTL_MS, SUBSCRIBE_RETRY_MS,
};
pub use step::step;
#[cfg(feature = "wasm")]
pub use wasm::{encode_client_msg, generate_meeting_id, parse_server_msg, validate_meeting_id};
