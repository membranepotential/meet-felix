//! Cloudflare Worker for meet-felix: the HTTP router plus the per-meeting `RoomObject`.
//!
//! Routes ("owner only" means the owner's session cookie or
//! `Authorization: Bearer <CREATE_SECRET>`, else 401):
//! - `POST /api/login` with `{"password"}` sets the owner's session cookie (204), or 401.
//! - `POST /api/logout` clears it (204).
//! - `GET /api/me` answers 204 with a valid session cookie, 401 without.
//! - `POST /api/new` (owner only, optional body `{"recording": "fromStart" | "hostDecides"}`)
//!   creates a scheduled meeting and returns `{"meeting"}` with status 201.
//! - `GET /api/meetings` (owner only) returns the owner's last meetings, newest first:
//!   `{"meetings": [{"meeting", "createdAt", "ended", "transcript"?}]}`.
//! - `GET /api/meeting/:words` returns `{"recordingPolicy", "phase", "ended"}`, or 404.
//! - `GET /api/ice` returns the ICE servers for `RTCPeerConnection`.
//! - `GET /ws/:words` upgrades to the room's signalling WebSocket (410 once the meeting ended).
//!   A socket opened with the owner's session cookie joins as host (D32 in `docs/architecture`).

mod internal;
mod meetings;
mod owner;
mod random;
mod record;
mod room;
mod sfu;

use meeting_core::{MeetingId, Millis, RecordingPolicy, SCHEDULED_TTL_MS};
use serde::{Deserialize, Serialize};
use worker::{
    Context, Date, Env, KvStore, Method, Request, RequestInit, Response, Result, RouteContext,
    Router, console_error, event,
};

use crate::internal::{INFO, INIT, InitRoom, ORIGIN, OWNER_HEADER, OWNER_YES};
use crate::owner::{CreateSecret, OwnerSession};

/// KV namespace mapping meeting words to the Room Durable Object id (hex); also holds the
/// owner's meeting index under [`meetings::KEY`].
const MEETINGS: &str = "MEETINGS";
/// Durable Object namespace of `RoomObject`.
const ROOMS: &str = "ROOMS";
/// Var naming the jurisdiction new rooms are pinned to; see [`Jurisdiction`].
const ROOM_JURISDICTION: &str = "ROOM_JURISDICTION";
/// The owner's password, which `POST /api/login` checks and owner-only routes also accept as a
/// bearer token (D7 in `docs/links`).
const CREATE_SECRET: &str = "CREATE_SECRET";
/// Upper bound for a meeting link's life (90 days); the room releases it when it ends. Longer
/// than the core's `SCHEDULED_TTL_MS` (60 days), so a scheduled link works until the room
/// expires (D6 in `docs/links`).
const MEETING_TTL_SECS: u64 = 90 * 24 * 60 * 60;
const _: () = assert!(MEETING_TTL_SECS * 1000 > SCHEDULED_TTL_MS);
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
struct NewMeeting {
    meeting: MeetingId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    password: String,
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
        .post_async("/api/login", login)
        .post("/api/logout", |_, _| {
            with_cookie(Response::empty()?.with_status(204), &OwnerSession::clear())
        })
        .get("/api/me", |req, ctx| {
            let Some(secret) = create_secret(&ctx) else {
                return not_configured();
            };
            if is_owner_session(&req, &secret)? {
                Ok(Response::empty()?.with_status(204))
            } else {
                Response::error("unauthorized", 401)
            }
        })
        .post_async("/api/new", new_meeting)
        .get_async("/api/meetings", past_meetings)
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

/// `POST /api/login`: sets the owner's session cookie if the password is `CREATE_SECRET`.
async fn login(mut req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Some(secret) = create_secret(&ctx) else {
        return not_configured();
    };
    let request: LoginRequest = match serde_json::from_str(&req.text().await?) {
        Ok(request) => request,
        Err(e) => return Response::error(format!("bad request: {e}"), 400),
    };
    if !secret.matches(&request.password) {
        return Response::error("wrong password", 401);
    }
    with_cookie(
        Response::empty()?.with_status(204),
        &OwnerSession::new(&secret).issue(now()),
    )
}

async fn new_meeting(mut req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if let Some(refusal) = refuse_non_owner(&req, &ctx)? {
        return Ok(refusal);
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
    let transcript = random::transcript_ref()?;
    let init = InitRoom {
        meeting: meeting.clone(),
        transcript: transcript.clone(),
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
    // Logged, not returned: the meeting exists and works without its index entry.
    let entry = meetings::Entry {
        meeting: meeting.clone(),
        room: room.to_string(),
        created_at: now(),
        transcript,
        live_at: None,
        ended_at: None,
    };
    if let Err(e) = meetings::update(&kv, |index| Some(meetings::added(index, entry))).await {
        console_error!("adding {meeting:?} to the meeting index failed: {e}");
    }
    Ok(Response::from_json(&NewMeeting { meeting })?.with_status(201))
}

/// `GET /api/meetings`: the owner's last [`meetings::MAX_ENTRIES`] meetings, newest first, from
/// the index in KV. The index is eventually consistent and changed by read-modify-write (see
/// [`meetings`]), so a meeting created or ended in the last minute may be missing or not yet
/// show as ended.
async fn past_meetings(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if let Some(refusal) = refuse_non_owner(&req, &ctx)? {
        return Ok(refusal);
    }
    let index = meetings::load(&ctx.kv(MEETINGS)?).await?;
    Response::from_json(&meetings::listing(index))
}

/// The error response for a request that is not the owner's, or `None` for the owner: a valid
/// session cookie or the creation secret as bearer token (D7 in `docs/links`).
fn refuse_non_owner(req: &Request, ctx: &RouteContext<Config>) -> Result<Option<Response>> {
    let Some(secret) = create_secret(ctx) else {
        return not_configured().map(Some);
    };
    let bearer = is_bearer(req.headers().get("Authorization")?.as_deref(), &secret);
    if bearer || is_owner_session(req, &secret)? {
        Ok(None)
    } else {
        Response::error("unauthorized", 401).map(Some)
    }
}

/// [`CREATE_SECRET`], or `None` (logged) if it is unset or empty.
fn create_secret(ctx: &RouteContext<Config>) -> Option<CreateSecret> {
    let secret = ctx
        .secret(CREATE_SECRET)
        .ok()
        .and_then(|secret| CreateSecret::new(secret.to_string()));
    if secret.is_none() {
        console_error!("{CREATE_SECRET} is not set; nobody can log in, create or list meetings");
    }
    secret
}

fn not_configured() -> Result<Response> {
    Response::error("the owner's password is not configured", 500)
}

/// Whether `req` carries the owner's valid session cookie.
fn is_owner_session(req: &Request, secret: &CreateSecret) -> Result<bool> {
    let cookies = req.headers().get("Cookie")?;
    Ok(OwnerSession::new(secret).verify(cookies.as_deref(), now()))
}

fn with_cookie(response: Response, set_cookie: &str) -> Result<Response> {
    response.headers().set("Set-Cookie", set_cookie)?;
    Ok(response)
}

async fn meeting_info(_req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Some(room) = room_stub(&ctx).await? else {
        return Response::error("unknown meeting", 404);
    };
    room.fetch_with_str(&format!("{ORIGIN}{INFO}")).await
}

/// Forwards the upgrade to the room, telling it in [`OWNER_HEADER`] whether the socket is the
/// owner's. A missing `CREATE_SECRET` makes every socket a guest's.
async fn join_meeting(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !is_websocket_upgrade(&req)? {
        return Response::error("expected a WebSocket upgrade", 426);
    }
    let Some(room) = room_stub(&ctx).await? else {
        return Response::error("unknown meeting", 404);
    };
    let owner = match create_secret(&ctx) {
        Some(secret) => is_owner_session(&req, &secret)?,
        None => false,
    };
    let forwarded = req.clone_mut()?;
    forwarded.headers().delete(OWNER_HEADER)?;
    if owner {
        forwarded.headers().set(OWNER_HEADER, OWNER_YES)?;
    }
    room.fetch_with_request(forwarded).await
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

/// The Worker's clock.
fn now() -> Millis {
    Millis(Date::now().as_millis())
}

fn is_websocket_upgrade(req: &Request) -> Result<bool> {
    Ok(req
        .headers()
        .get("Upgrade")?
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket")))
}

/// Whether `header` is `Bearer <secret>`.
fn is_bearer(header: Option<&str>, secret: &CreateSecret) -> bool {
    header
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|token| secret.matches(token))
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
        let secret = CreateSecret::new("s3cret".into()).unwrap();
        assert!(is_bearer(Some("Bearer s3cret"), &secret));
        for header in [
            None,
            Some("s3cret"),
            Some("Bearer "),
            Some("Bearer s3cre"),
            Some("Bearer s3cret "),
        ] {
            assert!(!is_bearer(header, &secret), "{header:?}");
        }
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
    fn new_meeting_response_holds_only_the_meeting() {
        let meeting = MeetingId::from_random([0, 0, 0]);
        let json = serde_json::to_value(NewMeeting {
            meeting: meeting.clone(),
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({ "meeting": meeting }));
    }

    #[test]
    fn login_body_is_just_the_password() {
        let parse = |json| serde_json::from_str::<LoginRequest>(json).map(|r| r.password);
        assert_eq!(parse(r#"{"password":"pw"}"#).ok().as_deref(), Some("pw"));
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"password":"pw","user":"x"}"#).is_err());
    }
}
