//! The HTTPS client: one method per SFU operation, over a pluggable [`SfuTransport`].

use std::fmt;

use meeting_core::{LocalTrack, Mid, OfferedTrack, RemoteTrack, Rid, Sdp, SessionId};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::wire::{
    CLOSE_TRACK_ERROR, CloseTrackObject, CloseTracksRequest, ErrorFields, LayerFallback,
    NewSessionResponse, RenegotiateRequest, SdpType, SessionDescription, Simulcast, TrackObject,
    TrackResult, TracksRequest, TracksResponse, UpdateTracksRequest,
};

/// The API's base URL.
pub const BASE_URL: &str = "https://rtc.live.cloudflare.com/v1";

/// An SFU application's id (Worker secret `CALLS_APP_ID`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppId(String);

impl AppId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

/// An SFU application's secret (Worker secret `CALLS_APP_SECRET`). Its `Debug` output hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct AppSecret(String);

impl AppSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for AppSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AppSecret(..)")
    }
}

/// The HTTP methods the API uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Post,
    Put,
}

/// A request for the transport to send. It has no `Debug`, because `authorization` holds the
/// app secret.
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    /// The full `Authorization` header value.
    pub authorization: String,
    /// A JSON body, sent with `Content-Type: application/json`.
    pub body: Option<String>,
}

/// What came back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Sends HTTP requests. The worker implements it with `worker::Fetch`; tests with canned
/// responses.
pub trait SfuTransport {
    /// Why a request got no response.
    type Error: fmt::Display;

    #[allow(
        async_fn_in_trait,
        reason = "Workers run single-threaded, so no caller needs a `Send` future"
    )]
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Self::Error>;
}

/// Why an SFU call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SfuError {
    /// The request got no response.
    #[error("transport: {0}")]
    Transport(String),
    /// The API rejected the request as a whole.
    #[error("HTTP {status}: {code}: {description}")]
    Api {
        status: u16,
        code: String,
        description: String,
    },
    /// A non-success status without an API error body.
    #[error("HTTP {status}")]
    Status { status: u16 },
    /// The body is not the documented JSON.
    #[error("unexpected response: {0}")]
    Decode(String),
    /// The API rejected one of the tracks.
    #[error("track: {code}: {description}")]
    Track { code: String, description: String },
    /// The response lacks the SDP the operation must return.
    #[error("the response has no SDP {0:?}")]
    MissingSdp(SdpType),
}

/// The SFU's offer for a subscribe session and the tracks it adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeOffer {
    pub sdp: Sdp,
    pub tracks: Vec<OfferedTrack>,
}

/// A client for one SFU application.
pub struct SfuClient<T> {
    transport: T,
    base_url: String,
    app_id: AppId,
    secret: AppSecret,
}

impl<T: SfuTransport> SfuClient<T> {
    /// A client for Cloudflare's API at [`BASE_URL`].
    pub fn new(transport: T, app_id: AppId, secret: AppSecret) -> Self {
        Self {
            transport,
            base_url: BASE_URL.to_owned(),
            app_id,
            secret,
        }
    }

    /// The same client talking to another base URL, such as a local mock SFU.
    #[must_use]
    pub fn with_base_url(self, base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..self
        }
    }

    /// `POST sessions/new` without a body.
    pub async fn new_session(&self) -> Result<SessionId, SfuError> {
        let response: NewSessionResponse =
            self.call(Method::Post, "sessions/new", None::<&()>).await?;
        Ok(response.session_id)
    }

    /// `POST sessions/{id}/tracks/new` with the client's offer and local tracks; returns the
    /// SFU's answer. Fails unless every track is published.
    pub async fn publish(
        &self,
        session: &SessionId,
        offer: &Sdp,
        tracks: &[LocalTrack],
    ) -> Result<Sdp, SfuError> {
        let request = TracksRequest {
            session_description: Some(SessionDescription {
                kind: SdpType::Offer,
                sdp: offer.clone(),
            }),
            tracks: tracks
                .iter()
                .map(|track| TrackObject::Local {
                    mid: track.mid.clone(),
                    track_name: track.name.clone(),
                })
                .collect(),
        };
        let response: TracksResponse = self
            .call(
                Method::Post,
                &session_path(session, "tracks/new"),
                Some(&request),
            )
            .await?;
        if let Some(error) = response.tracks.iter().find_map(track_error) {
            return Err(error);
        }
        sdp_of(response.session_description, SdpType::Answer)
    }

    /// `POST sessions/{id}/tracks/new` with remote tracks; returns the SFU's offer and the tracks
    /// it carries. Tracks the SFU rejected are left out, so the core requests them again; fails
    /// if it rejected all of them.
    pub async fn subscribe(
        &self,
        session: &SessionId,
        tracks: &[RemoteTrack],
    ) -> Result<SubscribeOffer, SfuError> {
        let request = TracksRequest {
            session_description: None,
            tracks: tracks
                .iter()
                .map(|track| TrackObject::Remote {
                    session_id: track.session.clone(),
                    track_name: track.name.clone(),
                    mid: None,
                    // The SFU's default layer applies until `SetLayer`: `RemoteTrack` does not
                    // say whether a track is video, and audio has no layers.
                    simulcast: None,
                })
                .collect(),
        };
        let response: TracksResponse = self
            .call(
                Method::Post,
                &session_path(session, "tracks/new"),
                Some(&request),
            )
            .await?;
        let first_error = response.tracks.iter().find_map(track_error);
        let offered: Vec<OfferedTrack> = response.tracks.into_iter().filter_map(offered).collect();
        if offered.is_empty() {
            return Err(first_error.unwrap_or(SfuError::Decode(
                "no track in the subscribe response".to_owned(),
            )));
        }
        Ok(SubscribeOffer {
            sdp: sdp_of(response.session_description, SdpType::Offer)?,
            tracks: offered,
        })
    }

    /// `PUT sessions/{id}/renegotiate` with the client's answer to the SFU's last offer.
    pub async fn renegotiate(&self, session: &SessionId, answer: &Sdp) -> Result<(), SfuError> {
        let request = RenegotiateRequest {
            session_description: SessionDescription {
                kind: SdpType::Answer,
                sdp: answer.clone(),
            },
        };
        let _: ErrorFields = self
            .call(
                Method::Put,
                &session_path(session, "renegotiate"),
                Some(&request),
            )
            .await?;
        Ok(())
    }

    /// `PUT sessions/{id}/tracks/close` with `force: true`: the SFU stops forwarding without an
    /// SDP exchange and returns no SDP. A track that is already gone counts as closed.
    pub async fn close_tracks(&self, session: &SessionId, mids: &[Mid]) -> Result<(), SfuError> {
        let request = CloseTracksRequest {
            tracks: mids
                .iter()
                .map(|mid| CloseTrackObject { mid: mid.clone() })
                .collect(),
            force: true,
        };
        let response: TracksResponse = self
            .call(
                Method::Put,
                &session_path(session, "tracks/close"),
                Some(&request),
            )
            .await?;
        match response
            .tracks
            .iter()
            .filter(|result| result.error_code.as_deref() != Some(CLOSE_TRACK_ERROR))
            .find_map(track_error)
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// `PUT sessions/{id}/tracks/update`: receive layer `rid` of the track on transceiver `mid`.
    pub async fn update_layer(
        &self,
        session: &SessionId,
        mid: &Mid,
        track: &RemoteTrack,
        rid: Rid,
    ) -> Result<(), SfuError> {
        let request = UpdateTracksRequest {
            tracks: vec![TrackObject::Remote {
                session_id: track.session.clone(),
                track_name: track.name.clone(),
                mid: Some(mid.clone()),
                simulcast: Some(Simulcast {
                    preferred_rid: rid,
                    // Keep the chosen layer under bandwidth pressure, but never show nothing
                    // when the publisher drops it. The a–z order of the rids (high, low, mid)
                    // falls back from `high` to `low`, skipping `mid`.
                    priority_ordering: LayerFallback::None,
                    rid_not_available: LayerFallback::Asciibetical,
                }),
            }],
        };
        let response: TracksResponse = self
            .call(
                Method::Put,
                &session_path(session, "tracks/update"),
                Some(&request),
            )
            .await?;
        match response.tracks.iter().find_map(track_error) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Sends one request and decodes the response, turning API errors into [`SfuError`].
    async fn call<B: Serialize, R: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<R, SfuError> {
        let body = body
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| SfuError::Decode(e.to_string()))?;
        let request = HttpRequest {
            method,
            url: format!("{}/apps/{}/{path}", self.base_url, self.app_id.0),
            authorization: format!("Bearer {}", self.secret.0),
            body,
        };
        let response = self
            .transport
            .send(request)
            .await
            .map_err(|e| SfuError::Transport(e.to_string()))?;
        decode(&response)
    }
}

fn session_path(session: &SessionId, operation: &str) -> String {
    format!("sessions/{}/{operation}", session.as_str())
}

/// Checks the status and the request-level error fields, then decodes the body.
pub(crate) fn decode<R: DeserializeOwned>(response: &HttpResponse) -> Result<R, SfuError> {
    let status = response.status;
    // `renegotiate` documents `{}`; read an empty body the same way.
    let body = match response.body.trim() {
        "" => "{}",
        body => body,
    };
    if let Ok(ErrorFields {
        error_code: Some(code),
        error_description,
    }) = serde_json::from_str(body)
    {
        return Err(SfuError::Api {
            status,
            code,
            description: error_description.unwrap_or_default(),
        });
    }
    if !(200..300).contains(&status) {
        return Err(SfuError::Status { status });
    }
    serde_json::from_str(body).map_err(|e| SfuError::Decode(e.to_string()))
}

fn track_error(result: &TrackResult) -> Option<SfuError> {
    result.error_code.as_ref().map(|code| SfuError::Track {
        code: code.clone(),
        description: result.error_description.clone().unwrap_or_default(),
    })
}

/// A successful remote-track result as the core names it.
fn offered(result: TrackResult) -> Option<OfferedTrack> {
    match result {
        TrackResult {
            mid: Some(mid),
            session_id: Some(session),
            track_name: Some(name),
            error_code: None,
            ..
        } => Some(OfferedTrack {
            mid,
            track: RemoteTrack { session, name },
        }),
        _ => None,
    }
}

fn sdp_of(description: Option<SessionDescription>, kind: SdpType) -> Result<Sdp, SfuError> {
    match description {
        Some(description) if description.kind == kind => Ok(description.sdp),
        _ => Err(SfuError::MissingSdp(kind)),
    }
}
