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

/// `errorCode` of a pulled track the SFU does not find: "Track not found on remote peer. Make
/// sure the publisher peer is connected and sending packets for this track" (seen on every first
/// pull that comes before the publisher's connection is up, 2026-10-09 and 2026-10-10).
pub const NOT_FOUND_TRACK_ERROR: &str = "not_found_track_error";

/// `errorCode` of a request on a session the SFU no longer serves, e.g. "Session appears to be
/// disconnected" with HTTP 410 (seen in production on 2026-10-09).
pub const SESSION_ERROR: &str = "session_error";

/// The id of a WebSocket adapter, which closing it needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdapterId(String);

impl AdapterId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The codec an egress adapter delivers. `pcm` is 48 kHz stereo signed 16-bit little-endian,
/// left and right interleaved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputCodec {
    Pcm,
}

/// One entry of `adapters/websocket/new`
/// (<https://developers.cloudflare.com/realtime/sfu/media-transport-adapters/websocket-adapter/>,
/// checked 2026-10-09). Only media egress is modelled: the SFU streams a published track to
/// `endpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "location", rename_all = "lowercase")]
pub enum AdapterTrack {
    #[serde(rename_all = "camelCase")]
    Remote {
        /// The publisher's session.
        session_id: SessionId,
        /// The published track.
        track_name: TrackName,
        /// A public `wss://` URL; no custom headers, so any credential goes in the path.
        endpoint: String,
        output_codec: OutputCodec,
    },
}

/// Body of `adapters/websocket/new`: one to four tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewAdaptersRequest {
    pub tracks: Vec<AdapterTrack>,
}

/// Body of `adapters/websocket/close`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseAdaptersRequest {
    pub tracks: Vec<AdapterRef>,
}

/// An adapter to close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterRef {
    pub adapter_id: AdapterId,
}

/// Response of both adapter calls: one result per requested track, in request order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AdaptersResponse {
    #[serde(default)]
    pub tracks: Vec<AdapterResult>,
}

/// One entry of [`AdaptersResponse`]: an `adapterId` on success, `errorCode` and
/// `errorDescription` on failure.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterResult {
    pub adapter_id: Option<AdapterId>,
    pub track_name: Option<TrackName>,
    pub error_code: Option<String>,
    pub error_description: Option<String>,
}

/// The status of `adapters/websocket/new` when every entry failed; the body still lists the
/// per-entry errors.
pub const ALL_ADAPTERS_FAILED: u16 = 503;

/// The most entries one adapter request may carry.
pub const MAX_ADAPTERS_PER_REQUEST: usize = 4;
