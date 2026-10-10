//! The HTTPS client: one method per SFU operation, over a pluggable [`SfuTransport`].

use std::fmt;

use meeting_core::{
    FailureCause, LocalTrack, Mid, OfferedTrack, RemoteTrack, Rid, Sdp, SessionId, TrackKind,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::wire::{
    ALL_ADAPTERS_FAILED, AdapterId, AdapterRef, AdapterResult, AdapterTrack, AdaptersResponse,
    CLOSE_TRACK_ERROR, CloseAdaptersRequest, CloseTrackObject, CloseTracksRequest, ErrorFields,
    LayerFallback, MAX_ADAPTERS_PER_REQUEST, NewAdaptersRequest, NewSessionResponse,
    RenegotiateRequest, SESSION_ERROR, SdpType, SessionDescription, Simulcast, TrackObject,
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

/// HTTP 410 Gone: the SFU no longer serves the session.
const GONE: u16 = 410;

impl SfuError {
    /// Whether the session the call was made on is gone: the SFU answered HTTP 410, or with
    /// [`SESSION_ERROR`] (also seen with 406 after an unanswered offer). Every later call on such
    /// a session fails as well, so the room replaces it rather than retrying.
    pub fn cause(&self) -> FailureCause {
        match self {
            Self::Api { status: GONE, .. } | Self::Status { status: GONE } => {
                FailureCause::SessionGone
            }
            Self::Api { code, .. } if code == SESSION_ERROR => FailureCause::SessionGone,
            Self::Api { .. }
            | Self::Status { .. }
            | Self::Transport(_)
            | Self::Decode(_)
            | Self::Track { .. }
            | Self::MissingSdp(_) => FailureCause::Transient,
        }
    }
}

/// The SFU's answer to a publish offer and the kinds of the offered tracks it rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishAnswer {
    pub sdp: Sdp,
    /// Empty if the SFU published every offered track.
    pub rejected: Vec<TrackKind>,
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
    /// SFU's answer and the kinds of the tracks it rejected with a per-track `errorCode`. Fails
    /// only if the SFU rejected every track: the API docs say to "preserve successful
    /// allocations from partially successful requests", and the answer covers the whole offer,
    /// so the client must apply it either way.
    ///
    /// Neither the docs nor the OpenAPI schema say whether the SFU echoes `trackName` or `mid`
    /// in a failed entry or keeps the request's order (checked 2026-10-10), so
    /// [`publish_errors`] matches results to tracks by name first and by position only where
    /// that is unambiguous.
    ///
    /// The SFU answers an endpoint's offer with an answer and
    /// `requiresImmediateRenegotiation: false` (OpenAPI example `local_tracks`); a response
    /// that instead carries an SFU offer fails with [`SfuError::MissingSdp`], as the core has
    /// no reply that would send that offer to the client.
    pub async fn publish(
        &self,
        session: &SessionId,
        offer: &Sdp,
        tracks: &[LocalTrack],
    ) -> Result<PublishAnswer, SfuError> {
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
        let errors = publish_errors(tracks, &response.tracks)?;
        if errors.len() == tracks.len()
            && let Some((_, error)) = errors.first()
        {
            return Err(error.clone());
        }
        Ok(PublishAnswer {
            sdp: sdp_of(response.session_description, SdpType::Answer)?,
            rejected: errors.into_iter().map(|(kind, _)| kind).collect(),
        })
    }

    /// `POST sessions/{id}/tracks/new` with remote tracks; returns the SFU's offer and the tracks
    /// it carries. Tracks the SFU rejected are left out, so the core requests them again.
    ///
    /// Whenever the response carries an offer, the client must answer it, or the session stays
    /// "expecting a remote answer" and the SFU refuses every later call on it (406, then 410).
    /// So an offer is returned even if every track failed and the list is empty: a publisher
    /// whose `PeerConnection` is not connected yet gets `not_found_track_error` together with an
    /// offer (seen against the live SFU, see `docs/spikes/02-first-call.md`), and the core retries
    /// such tracks after `SUBSCRIBE_RETRY_MS`. Fails only if there is no offer.
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
                    // No layer preference until `SetLayer`: `RemoteTrack` does not say whether a
                    // track is video, and audio has no layers. Neither the simulcast docs nor
                    // the OpenAPI schema (both checked 2026-10-09) say which layer the SFU sends
                    // without a `simulcast` object; the core assumes `f` (`Rid::High`).
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
        let sdp = sdp_of(response.session_description, SdpType::Offer)
            .map_err(|missing| first_error.unwrap_or(missing))?;
        Ok(SubscribeOffer {
            sdp,
            tracks: response.tracks.into_iter().filter_map(offered).collect(),
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

    /// `PUT sessions/{id}/tracks/update`: receive layer `rid` of the track on transceiver `mid`,
    /// and keep it while the publisher sends it.
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
                    // `none`: the SFU keeps sending `rid` even when bandwidth is short.
                    // With `asciibetical` it ranks the layers a–z, `f` first, and against
                    // the real SFU it went back to `f` about 12 s after a switch to `h`
                    // (2026-10-10, D11), so a small tile cost the full layer's egress.
                    priority_ordering: LayerFallback::None,
                    // A layer the publisher stops sending falls back to the next one in
                    // a–z order (f, h, q).
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

    /// `POST adapters/websocket/new`: one WebSocket adapter per entry, each streaming a
    /// published track to its endpoint. Returns one result per entry, in request order: an
    /// entry can fail while others succeed (HTTP 200), and HTTP 503 means all failed. Retrying
    /// a batch would duplicate its successful adapters, so the caller must store every id
    /// before retrying anything.
    ///
    /// # Panics
    /// If `tracks` is empty or longer than [`MAX_ADAPTERS_PER_REQUEST`].
    pub async fn create_adapters(
        &self,
        tracks: Vec<AdapterTrack>,
    ) -> Result<Vec<Result<AdapterId, SfuError>>, SfuError> {
        assert!(
            (1..=MAX_ADAPTERS_PER_REQUEST).contains(&tracks.len()),
            "1 to {MAX_ADAPTERS_PER_REQUEST} adapters per request"
        );
        let count = tracks.len();
        let response = self
            .send(
                Method::Post,
                "adapters/websocket/new",
                Some(&NewAdaptersRequest { tracks }),
            )
            .await?;
        let results: AdaptersResponse = match response.status {
            // The body of an all-failed batch carries the per-entry errors, which say why.
            ALL_ADAPTERS_FAILED => serde_json::from_str(&response.body)
                .ok()
                .filter(|r: &AdaptersResponse| !r.tracks.is_empty())
                .map_or_else(|| decode(&response), Ok)?,
            _ => decode(&response)?,
        };
        if results.tracks.len() != count {
            return Err(SfuError::Decode(format!(
                "{} adapter results for {count} tracks",
                results.tracks.len()
            )));
        }
        Ok(results.tracks.into_iter().map(adapter_result).collect())
    }

    /// `POST adapters/websocket/close` for up to [`MAX_ADAPTERS_PER_REQUEST`] adapters. Closing
    /// is idempotent, so an adapter that is already closed counts as closed; any other entry
    /// error fails the call.
    pub async fn close_adapters(&self, adapters: &[AdapterId]) -> Result<(), SfuError> {
        let request = CloseAdaptersRequest {
            tracks: adapters
                .iter()
                .map(|id| AdapterRef {
                    adapter_id: id.clone(),
                })
                .collect(),
        };
        let response: AdaptersResponse = self
            .call(Method::Post, "adapters/websocket/close", Some(&request))
            .await?;
        match response
            .tracks
            .into_iter()
            .find_map(|r| adapter_result(r).err())
        {
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
        decode(&self.send(method, path, body).await?)
    }

    /// Sends one request and returns the raw response.
    async fn send<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<HttpResponse, SfuError> {
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
        self.transport
            .send(request)
            .await
            .map_err(|e| SfuError::Transport(e.to_string()))
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

/// The offered tracks a publish response rejects, in offer order, each with its error.
///
/// An entry whose `trackName` or `mid` matches an offered track belongs to that track. The
/// remaining (unnamed) entries belong to the tracks no entry names, by position, but only if
/// there are as many of them as there are such tracks. A track without a result counts as
/// published, so unnamed entries without an error need no match. Unnamed errors that cannot be
/// matched make the response inconsistent: that fails with [`SfuError::Decode`] rather than
/// guess which track the SFU rejected.
fn publish_errors(
    tracks: &[LocalTrack],
    results: &[TrackResult],
) -> Result<Vec<(TrackKind, SfuError)>, SfuError> {
    let named_track = |result: &TrackResult| {
        tracks.iter().position(|track| {
            result.track_name.as_ref() == Some(&track.name)
                || result.mid.as_ref() == Some(&track.mid)
        })
    };
    let named: Vec<(usize, &TrackResult)> = results
        .iter()
        .filter_map(|result| Some((named_track(result)?, result)))
        .collect();
    let unnamed: Vec<&TrackResult> = results
        .iter()
        .filter(|result| named_track(result).is_none())
        .collect();
    let unmatched: Vec<usize> = (0..tracks.len())
        .filter(|i| named.iter().all(|(j, _)| j != i))
        .collect();
    let positional: Vec<(usize, &TrackResult)> = if unnamed.len() == unmatched.len() {
        unmatched.into_iter().zip(unnamed).collect()
    } else if unnamed.iter().all(|result| result.error_code.is_none()) {
        Vec::new()
    } else {
        return Err(SfuError::Decode(format!(
            "{} track results name no offered track, but {} offered tracks have no result",
            unnamed.len(),
            unmatched.len()
        )));
    };
    let attributed = [named, positional].concat();
    Ok(tracks
        .iter()
        .enumerate()
        .filter_map(|(i, track)| {
            let error = attributed
                .iter()
                .filter(|(j, _)| *j == i)
                .find_map(|(_, result)| track_error(result))?;
            Some((track.kind, error))
        })
        .collect())
}

fn track_error(result: &TrackResult) -> Option<SfuError> {
    result.error_code.as_ref().map(|code| SfuError::Track {
        code: code.clone(),
        description: result.error_description.clone().unwrap_or_default(),
    })
}

/// An adapter entry's id, or its error.
fn adapter_result(result: AdapterResult) -> Result<AdapterId, SfuError> {
    match result {
        AdapterResult {
            error_code: Some(code),
            error_description,
            ..
        } => Err(SfuError::Track {
            code,
            description: error_description.unwrap_or_default(),
        }),
        AdapterResult {
            adapter_id: Some(id),
            ..
        } => Ok(id),
        AdapterResult { .. } => Err(SfuError::Decode("adapter result without id".into())),
    }
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
