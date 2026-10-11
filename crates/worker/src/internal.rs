//! The requests the router sends to a `RoomObject` and the room sends to its `RecorderObject`,
//! besides forwarded WebSocket upgrades, and the ids the router addresses the objects by.

use meeting_core::recording::{ConsentRecord, RosterEntry};
use meeting_core::{
    DisplayName, MeetingId, Millis, ParticipantId, Phase, RecordingPolicy, Role, SessionId,
    TrackName, TranscriptRef,
};
use serde::{Deserialize, Serialize};
use worker::{Method, ObjectNamespace, Request, RequestInit, Response, Result, Stub};

/// Origin of internal requests to a Durable Object; the object reads only the path.
pub const ORIGIN: &str = "https://object";
/// `POST`: create the room's state. Sent once, right after the router created the object.
pub const INIT: &str = "/init";
/// `GET`: [`RoomInfo`].
pub const INFO: &str = "/info";
/// Header on a forwarded WebSocket upgrade: [`OWNER_YES`] if the router verified the owner's
/// session cookie. The router removes it from every client request first, so only the router
/// sets it.
pub const OWNER_HEADER: &str = "x-meet-owner";
pub const OWNER_YES: &str = "1";

/// Body of [`INIT`]. The transcript token also reaches the owner's meeting index
/// ([`crate::meetings`]).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitRoom {
    pub meeting: MeetingId,
    pub transcript: TranscriptRef,
    pub recording_policy: RecordingPolicy,
}

/// Body of [`INFO`]'s response. The router answers `GET /api/meeting/:words` with it as a
/// `meeting_core::http::MeetingLookup`, and the owner's list reads the phase.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo {
    pub recording_policy: RecordingPolicy,
    pub phase: Phase,
}

impl RoomInfo {
    pub fn new(recording_policy: RecordingPolicy, phase: Phase) -> Self {
        Self {
            recording_policy,
            phase,
        }
    }
}

/// `POST` to the recorder: recording started (again) at `since`. Creates the recorder on the
/// first call; later calls change nothing.
pub const RECORDER_START: &str = "/start";
/// `POST` [`TrackRecording`]: record this audio track.
pub const RECORDER_TRACK: &str = "/track";
/// `POST` [`UntrackRecording`]: stop recording this audio track.
pub const RECORDER_UNTRACK: &str = "/untrack";
/// `POST`: stop recording all tracks; the meeting goes on.
pub const RECORDER_STOP: &str = "/stop";
/// `POST` [`FinishRecording`]: the meeting ended; start the transcript job.
pub const RECORDER_END: &str = "/end";
/// `GET`: the transcript's status, which the router passes on as `GET /api/transcript/:token`.
pub const RECORDER_STATUS: &str = "/transcript";
/// `GET`: the transcript files, passed on as `GET /api/transcript/:token/<file>`.
pub const TRANSCRIPT_MD: &str = "transcript.md";
pub const TRANSCRIPT_JSON: &str = "transcript.json";

/// Body of [`RECORDER_START`].
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRecording {
    pub meeting: MeetingId,
    pub since: Millis,
}

/// Body of [`RECORDER_TRACK`]: a consenting speaker's audio track, with their name and consent,
/// so the recorder knows both even if the final [`FinishRecording`] never arrives.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackRecording {
    pub who: ParticipantId,
    pub name: DisplayName,
    pub role: Role,
    pub consent: ConsentRecord,
    pub session: SessionId,
    pub track_name: TrackName,
}

/// Body of [`RECORDER_UNTRACK`], with the speaker's consent record as it stands now, which
/// shows a withdrawal.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UntrackRecording {
    pub who: ParticipantId,
    pub session: SessionId,
    pub track_name: TrackName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<ConsentRecord>,
}

/// Body of [`RECORDER_END`]: everyone's names and the whole consent record, which replace what
/// the recorder collected from [`TrackRecording`]s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishRecording {
    pub roster: Vec<RosterEntry>,
    pub consents: Vec<ConsentRecord>,
}

/// A `RoomObject`'s id as text (hex), as KV maps meeting words to it and the meeting index keeps
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoomId(String);

/// A `RecorderObject`'s id as text (hex), as KV maps a transcript token to it and an SFU
/// adapter's URL names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderId(String);

macro_rules! object_id {
    ($id:ident) => {
        impl $id {
            /// Wraps an id the router stored or made; [`Self::stub`] checks it.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The object behind the id in `namespace`; an error if it is no id there.
            pub fn stub(&self, namespace: &ObjectNamespace) -> Result<Stub> {
                namespace.id_from_string(&self.0)?.get_stub()
            }
        }
    };
}

object_id!(RoomId);
object_id!(RecorderId);

impl RoomId {
    /// The id as KV stores it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `GET path` to the object behind `stub`; the caller reads the status.
pub async fn get(stub: &Stub, path: &str) -> Result<Response> {
    stub.fetch_with_str(&format!("{ORIGIN}{path}")).await
}

/// `POST path` with `body` as JSON to the object behind `stub`; an error unless it answers 2xx.
pub async fn post(stub: &Stub, path: &str, body: &impl Serialize) -> Result<Response> {
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(serde_json::to_string(body)?.into()));
    let response = stub
        .fetch_with_request(Request::new_with_init(&format!("{ORIGIN}{path}"), &init)?)
        .await?;
    match response.status_code() {
        200..300 => Ok(response),
        status => Err(worker::Error::RustError(format!(
            "{path} answered {status}"
        ))),
    }
}
