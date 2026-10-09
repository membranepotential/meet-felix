//! The requests the router sends to a `RoomObject`, besides forwarded WebSocket upgrades.

use meeting_core::{MeetingId, Phase, RecordingPolicy, TranscriptRef};
use serde::{Deserialize, Serialize};

/// Origin of the router's requests; the object reads only the path.
pub const ORIGIN: &str = "https://room";
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
/// (`crate::meetings`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitRoom {
    pub meeting: MeetingId,
    pub transcript: TranscriptRef,
    pub recording_policy: RecordingPolicy,
}

/// Body of [`INFO`]'s response, which the router passes on as `GET /api/meeting/:words`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomInfo {
    pub recording_policy: RecordingPolicy,
    pub phase: Phase,
    /// Whether `phase` is [`Phase::Ended`]; kept for clients that read only this.
    pub ended: bool,
}

impl RoomInfo {
    pub fn new(recording_policy: RecordingPolicy, phase: Phase) -> Self {
        Self {
            recording_policy,
            phase,
            ended: phase == Phase::Ended,
        }
    }
}
