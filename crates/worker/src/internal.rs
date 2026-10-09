//! The requests the router sends to a `RoomObject`, besides forwarded WebSocket upgrades.

use meeting_core::{HostToken, MeetingId, RecordingPolicy, TranscriptRef};
use serde::{Deserialize, Serialize};

/// Origin of the router's requests; the object reads only the path.
pub const ORIGIN: &str = "https://room";
/// `POST`: create the room's state. Sent once, right after the router created the object.
pub const INIT: &str = "/init";
/// `GET`: [`RoomInfo`].
pub const INFO: &str = "/info";

/// Body of [`INIT`]. The host token reaches only the room's storage and the creator; the
/// transcript token also reaches the owner's meeting index (`crate::meetings`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitRoom {
    pub meeting: MeetingId,
    pub host_token: HostToken,
    pub transcript: TranscriptRef,
    pub recording_policy: RecordingPolicy,
}

/// Body of [`INFO`]'s response, which the router passes on as `GET /api/meeting/:words`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo {
    pub recording_policy: RecordingPolicy,
    pub ended: bool,
}
