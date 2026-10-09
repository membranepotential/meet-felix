//! Request and response bodies of the Realtime SFU HTTPS API, as in Cloudflare's OpenAPI schema
//! (`realtime-api-2024-05-21.yaml`, checked on 2026-10-09). Only the fields this app uses are
//! modelled; unknown response fields are ignored.

use meeting_core::{Mid, Rid, Sdp, SessionId, TrackName};
use serde::{Deserialize, Serialize};

/// Whether an SDP is an offer or an answer (`SessionDescription.type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SdpType {
    Offer,
    Answer,
}

/// `SessionDescription`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDescription {
    #[serde(rename = "type")]
    pub kind: SdpType,
    pub sdp: Sdp,
}

/// A `TrackObject` in a `tracks/new` or `tracks/update` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "location", rename_all = "lowercase")]
pub enum TrackObject {
    /// Publish a track from the client's offer.
    #[serde(rename_all = "camelCase")]
    Local { mid: Mid, track_name: TrackName },
    /// Receive another session's published track. `mid` names an existing receiving transceiver
    /// (only in `tracks/update`).
    #[serde(rename_all = "camelCase")]
    Remote {
        session_id: SessionId,
        track_name: TrackName,
        #[serde(skip_serializing_if = "Option::is_none")]
        mid: Option<Mid>,
        #[serde(skip_serializing_if = "Option::is_none")]
        simulcast: Option<Simulcast>,
    },
}

/// The `simulcast` object of a remote track: which layer a subscriber prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Simulcast {
    /// The publisher's encoding `rid`; the core's [`Rid`] names are the rids clients publish.
    pub preferred_rid: Rid,
    /// What the SFU sends when bandwidth does not suffice for the preferred layer.
    pub priority_ordering: LayerFallback,
    /// What the SFU sends when the publisher stops sending the preferred layer.
    pub rid_not_available: LayerFallback,
}

/// `priorityOrdering` / `ridNotAvailable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerFallback {
    /// Keep the selected layer.
    None,
    /// Fall back to the next rid in a–z order.
    Asciibetical,
}

/// `TracksRequest` (`tracks/new`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracksRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_description: Option<SessionDescription>,
    pub tracks: Vec<TrackObject>,
}

/// `UpdateTracksRequest` (`tracks/update`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateTracksRequest {
    pub tracks: Vec<TrackObject>,
}

/// `RenegotiateRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenegotiateRequest {
    pub session_description: SessionDescription,
}

/// `CloseTracksRequest`, always forced: the SFU stops forwarding without an SDP exchange, which
/// is what the core's `SfuCloseTracks` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseTracksRequest {
    pub tracks: Vec<CloseTrackObject>,
    pub force: bool,
}

/// `CloseTrackObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseTrackObject {
    pub mid: Mid,
}

/// The request-level error fields every response may carry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorFields {
    pub error_code: Option<String>,
    pub error_description: Option<String>,
}

/// `NewSessionResponse`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: SessionId,
}

/// `TracksResponse`, `UpdateTracksResponse` and `CloseTracksResponse`: per-track results and,
/// for `tracks/new`, the SDP the client must apply.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TracksResponse {
    #[serde(default)]
    pub requires_immediate_renegotiation: bool,
    pub session_description: Option<SessionDescription>,
    #[serde(default)]
    pub tracks: Vec<TrackResult>,
}

/// One entry of a response's `tracks`. The SFU echoes the fields it resolved; a failed entry
/// carries `errorCode` and `errorDescription`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackResult {
    pub mid: Option<Mid>,
    pub session_id: Option<SessionId>,
    pub track_name: Option<TrackName>,
    pub error_code: Option<String>,
    pub error_description: Option<String>,
}

/// `errorCode` of a close result whose track is absent or already closed, which satisfies the
/// close.
pub const CLOSE_TRACK_ERROR: &str = "close_track_error";
