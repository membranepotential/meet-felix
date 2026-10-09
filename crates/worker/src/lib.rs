//! Cloudflare Worker for meet-felix: the HTTP router plus the per-meeting `RoomObject`.
//!
//! Routes:
//! - `POST /api/new` (`Authorization: Bearer <CREATE_SECRET>`, optional body
//!   `{"recording": "fromStart" | "hostDecides"}`) creates a meeting and returns
//!   `{"meeting", "hostToken"}` with status 201.
//! - `GET /api/meeting/:words` returns `{"recordingPolicy", "ended"}`, or 404.
//! - `GET /api/ice` returns the ICE servers for `RTCPeerConnection`.
//! - `GET /ws/:words` upgrades to the room's signalling WebSocket (410 once the meeting ended).

mod internal;
mod random;
mod record;
mod room;
mod sfu;

use meeting_core::{HostToken, MeetingId, RecordingPolicy};
use serde::{Deserialize, Serialize};
use worker::{
    Context, Env, KvStore, Method, Request, RequestInit, Response, Result, RouteContext, Router,
    console_error, event,
};

use crate::internal::{INFO, INIT, InitRoom, ORIGIN};

/// KV namespace mapping meeting words to the Room Durable Object id (hex).
const MEETINGS: &str = "MEETINGS";
/// Durable Object namespace of `RoomObject`.
const ROOMS: &str = "ROOMS";
/// Var naming the jurisdiction new rooms are pinned to; see [`Jurisdiction`].
const ROOM_JURISDICTION: &str = "ROOM_JURISDICTION";
/// Secret that `POST /api/new` requires as a bearer token (D7 in `docs/links`).
const CREATE_SECRET: &str = "CREATE_SECRET";
/// Upper bound for a meeting link's life; the room usually releases it much earlier.
const MEETING_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// Draws of fresh words before giving up when all drawn combinations are taken.
const WORD_DRAWS: usize = 8;
/// Cloudflare's free STUN server. TURN credentials follow in phase 2.
const STUN_URL: &str = "stun:stun.cloudflare.com:3478";

/// Where new rooms run and store data
/// (<https://developers.cloudflare.com/durable-objects/reference/data-location/>).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Jurisdiction {
    Eu,
    Us,
    Fedramp,
}

impl Jurisdiction {
    /// Reads [`ROOM_JURISDICTION`]: empty means none (local workerd implements none), any other
    /// value must name a jurisdiction.
    fn from_var(value: &str) -> std::result::Result<Option<Self>, String> {
        match value {
            "" => Ok(None),
            "eu" => Ok(Some(Self::Eu)),
            "us" => Ok(Some(Self::Us)),
            "fedramp" => Ok(Some(Self::Fedramp)),
            other => Err(format!(
                "{ROOM_JURISDICTION} must be empty, \"eu\", \"us\" or \"fedramp\", not {other:?}"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Eu => "eu",
            Self::Us => "us",
            Self::Fedramp => "fedramp",
        }
    }
}

/// Configuration checked before any route runs, so a wrong value fails every request.
struct Config {
    jurisdiction: Option<Jurisdiction>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewMeetingRequest {
    #[serde(default = "record_from_start")]
    recording: RecordingPolicy,
}

fn record_from_start() -> RecordingPolicy {
    RecordingPolicy::FromStart
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NewMeeting {
    meeting: MeetingId,
    host_token: HostToken,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IceServers {
    ice_servers: Vec<IceServer>,
}

/// An `RTCIceServer`.
#[derive(Serialize)]
struct IceServer {
    urls: Vec<&'static str>,
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let jurisdiction = match Jurisdiction::from_var(&env.var(ROOM_JURISDICTION)?.to_string()) {
        Ok(jurisdiction) => jurisdiction,
        Err(e) => {
            console_error!("{e}");
            return Response::error("server misconfigured", 500);
        }
    };
    Router::with_data(Config { jurisdiction })
        .post_async("/api/new", new_meeting)
        .get_async("/api/meeting/:words", meeting_info)
        .get("/api/ice", |_, _| {
            Response::from_json(&IceServers {
                ice_servers: vec![IceServer {
                    urls: vec![STUN_URL],
                }],
            })
        })
        .get_async("/ws/:words", join_meeting)
        .run(req, env)
        .await
}

async fn new_meeting(mut req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Ok(secret) = ctx.secret(CREATE_SECRET) else {
        console_error!("{CREATE_SECRET} is not set; nobody can create meetings");
        return Response::error("meeting creation is not configured", 500);
    };
    if !is_bearer(
        req.headers().get("Authorization")?.as_deref(),
        &secret.to_string(),
    ) {
        return Response::error("unauthorized", 401);
    }
    let body = req.text().await?;
    let request: NewMeetingRequest = match serde_json::from_str(match body.trim() {
        "" => "{}",
        body => body,
    }) {
        Ok(request) => request,
        Err(e) => return Response::error(format!("bad request: {e}"), 400),
    };

    let kv = ctx.kv(MEETINGS)?;
    let Some(meeting) = unused_words(&kv).await? else {
        return Response::error("no free meeting link, try again", 503);
    };
    let rooms = ctx.durable_object(ROOMS)?;
    let room = match ctx.data.jurisdiction {
        None => rooms.unique_id()?,
        Some(jurisdiction) => rooms.unique_id_with_jurisdiction(jurisdiction.as_str())?,
    };
    let host_token = random::host_token()?;
    let init = InitRoom {
        meeting: meeting.clone(),
        host_token: host_token.clone(),
        recording_policy: request.recording,
    };
    let mut init_request = RequestInit::new();
    init_request
        .with_method(Method::Post)
        .with_body(Some(serde_json::to_string(&init)?.into()));
    let created = room
        .get_stub()?
        .fetch_with_request(Request::new_with_init(
            &format!("{ORIGIN}{INIT}"),
            &init_request,
        )?)
        .await?;
    if !(200..300).contains(&created.status_code()) {
        return Response::error("could not create the room", 500);
    }
    // Check-then-write: KV has no atomic insert, so two creations that draw the same words at
    // the same moment could both pass `unused_words`. Accepted, because only the holder of
    // CREATE_SECRET creates meetings, and rarely.
    kv.put(meeting.as_str(), room.to_string())?
        .expiration_ttl(MEETING_TTL_SECS)
        .execute()
        .await?;
    Ok(Response::from_json(&NewMeeting {
        meeting,
        host_token,
    })?
    .with_status(201))
}

async fn meeting_info(_req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Some(room) = room_stub(&ctx).await? else {
        return Response::error("unknown meeting", 404);
    };
    room.fetch_with_str(&format!("{ORIGIN}{INFO}")).await
}

async fn join_meeting(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !is_websocket_upgrade(&req)? {
        return Response::error("expected a WebSocket upgrade", 426);
    }
    let Some(room) = room_stub(&ctx).await? else {
        return Response::error("unknown meeting", 404);
    };
    room.fetch_with_request(req).await
}

/// Draws words until a combination is not mapped in KV.
async fn unused_words(kv: &KvStore) -> Result<Option<MeetingId>> {
    for _ in 0..WORD_DRAWS {
        let words = random::meeting_id()?;
        if kv.get(words.as_str()).text().await?.is_none() {
            return Ok(Some(words));
        }
    }
    Ok(None)
}

/// The room for the `:words` route parameter, matched case-insensitively; `None` for malformed
/// or unknown words.
async fn room_stub(ctx: &RouteContext<Config>) -> Result<Option<worker::Stub>> {
    let Some(words) = ctx
        .param("words")
        .and_then(|words| MeetingId::parse(&words.to_ascii_lowercase()).ok())
    else {
        return Ok(None);
    };
    let Some(room) = ctx.kv(MEETINGS)?.get(words.as_str()).text().await? else {
        return Ok(None);
    };
    Ok(Some(
        ctx.durable_object(ROOMS)?
            .id_from_string(&room)?
            .get_stub()?,
    ))
}

fn is_websocket_upgrade(req: &Request) -> Result<bool> {
    Ok(req
        .headers()
        .get("Upgrade")?
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket")))
}

/// Whether `header` is `Bearer <secret>`. Compares in time independent of where the token
/// differs, so response times do not reveal the secret.
fn is_bearer(header: Option<&str>, secret: &str) -> bool {
    let Some(token) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    !secret.is_empty()
        && token.len() == secret.len()
        && token
            .bytes()
            .zip(secret.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jurisdiction_var_accepts_known_values_only() {
        assert_eq!(Jurisdiction::from_var(""), Ok(None));
        assert_eq!(Jurisdiction::from_var("eu"), Ok(Some(Jurisdiction::Eu)));
        for bad in ["EU", "de", " eu"] {
            assert!(Jurisdiction::from_var(bad).is_err(), "{bad:?}");
        }
        for j in [Jurisdiction::Eu, Jurisdiction::Us, Jurisdiction::Fedramp] {
            assert_eq!(Jurisdiction::from_var(j.as_str()), Ok(Some(j)));
        }
    }

    #[test]
    fn bearer_must_match_the_secret_exactly() {
        assert!(is_bearer(Some("Bearer s3cret"), "s3cret"));
        for header in [
            None,
            Some("s3cret"),
            Some("Bearer s3cre"),
            Some("Bearer s3cret "),
        ] {
            assert!(!is_bearer(header, "s3cret"), "{header:?}");
        }
        assert!(
            !is_bearer(Some("Bearer "), ""),
            "an empty secret opens nothing"
        );
    }

    #[test]
    fn new_meeting_body_defaults_to_recording_from_start() {
        let parse = |json| serde_json::from_str::<NewMeetingRequest>(json).map(|r| r.recording);
        assert_eq!(parse("{}").ok(), Some(RecordingPolicy::FromStart));
        assert_eq!(
            parse(r#"{"recording":"hostDecides"}"#).ok(),
            Some(RecordingPolicy::HostDecides)
        );
        assert!(parse(r#"{"recording":"never"}"#).is_err());
    }

    #[test]
    fn new_meeting_response_is_camel_case() {
        let json = serde_json::to_value(NewMeeting {
            meeting: MeetingId::from_random([0, 0, 0]),
            host_token: HostToken::new("t"),
        })
        .unwrap();
        assert!(json.get("hostToken").is_some());
    }
}
