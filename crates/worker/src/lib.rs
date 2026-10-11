//! Cloudflare Worker for meet-felix: the HTTP router plus the per-meeting `RoomObject` and
//! `RecorderObject`.
//!
//! Routes ("owner only" means the owner's session cookie or
//! `Authorization: Bearer <CREATE_SECRET>`, else 401):
//! - `POST /api/login` with `{"password"}` sets the owner's session cookie and answers like
//!   `GET /api/me`, or 401.
//! - `POST /api/logout` clears it (204).
//! - `GET /api/me` with a valid session cookie returns `{"name"}` (the owner's display name from
//!   the var `OWNER_NAME`, or `null`) with `Cache-Control: no-store`; 401 without.
//! - `POST /api/new` (owner only, optional body `{"recording": "fromStart" | "hostDecides"}`)
//!   creates a scheduled meeting and returns `{"meeting"}` with status 201.
//! - `GET /api/meetings` (owner only) returns the owner's last meetings, newest first:
//!   `{"meetings": [{"meeting", "createdAt", "phase", "ended", "transcript"?}]}`.
//! - `GET /api/meeting/:words` returns `{"recordingPolicy", "phase", "ended"}`, or 404.
//! - `GET /api/ice?meeting=:words` returns `{"iceServers"}` for `RTCPeerConnection` in a known
//!   meeting, or 404: STUN, plus TURN with fresh credentials if the secrets `TURN_KEY_ID` and
//!   `TURN_KEY_API_TOKEN` are set. TURN traffic is billed, so strangers get no credentials.
//! - `GET /ws/:words` upgrades to the room's signalling WebSocket (410 once the meeting ended).
//!   A socket opened with the owner's session cookie joins as host (D32 in `docs/architecture`).
//! - `GET /rec/:recorder/:secret/:who/:n` upgrades an SFU adapter's WebSocket into the meeting's
//!   `RecorderObject`, which checks the secret.
//! - `GET /api/transcript/:token` returns the transcript's status and, once ready, the transcript
//!   (`{"status", "meeting", "startedAt", "endedAt"?, "progress": {"done", "total"}` in
//!   milliseconds of audio, `"retryAt"?, "audioUntil"?, "transcript"?}`), or 404;
//!   `GET /api/transcript/:token/transcript.md` and `.../transcript.json` download the files.
//!   These responses carry `Referrer-Policy: no-referrer`, `X-Robots-Tag: noindex` and
//!   `Cache-Control: no-store`, and the token is never logged (research 11, item 7).

mod ice;
mod internal;
mod meetings;
mod owner;
mod random;
mod record;
mod recorder;
mod room;
mod sfu;

use futures_util::future::join_all;
use meeting_core::{
    DisplayName, MeetingId, Millis, Phase, RecordingPolicy, SCHEDULED_TTL_MS, TranscriptRef,
};
use serde::{Deserialize, Serialize};
use worker::{
    Context, Date, Env, KvStore, Method, ObjectNamespace, Request, RequestInit, Response, Result,
    RouteContext, Router, console_error, event,
};

use crate::internal::{
    INFO, INIT, InitRoom, ORIGIN, OWNER_HEADER, OWNER_YES, RECORDER_STATUS, RoomInfo,
    TRANSCRIPT_JSON, TRANSCRIPT_MD,
};
use crate::owner::{CreateSecret, OwnerSession};

/// KV namespace mapping meeting words to the Room Durable Object id (hex); also holds the
/// owner's meeting index under [`meetings::KEY`].
const MEETINGS: &str = "MEETINGS";
/// Durable Object namespace of `RoomObject`.
const ROOMS: &str = "ROOMS";
/// Durable Object namespace of `RecorderObject`.
const RECORDERS: &str = "RECORDERS";
/// Var naming the jurisdiction new rooms are pinned to; see [`Jurisdiction`].
const ROOM_JURISDICTION: &str = "ROOM_JURISDICTION";
/// The owner's password, which `POST /api/login` checks and owner-only routes also accept as a
/// bearer token (D7 in `docs/links`).
const CREATE_SECRET: &str = "CREATE_SECRET";
/// Var with the owner's display name, which the app prefills when the owner joins (D32 in
/// `docs/architecture`). An unrendered config leaves the placeholder `${OWNER_NAME}`, which is a
/// valid name and shows as such.
const OWNER_NAME: &str = "OWNER_NAME";
/// Upper bound for a meeting link's life (90 days); the room releases it when it ends. Longer
/// than the core's `SCHEDULED_TTL_MS` (60 days), so a scheduled link works until the room
/// expires (D6 in `docs/links`).
const MEETING_TTL_SECS: u64 = 90 * 24 * 60 * 60;
const _: () = assert!(MEETING_TTL_SECS * 1000 > SCHEDULED_TTL_MS);
/// Draws of fresh words before giving up when all drawn combinations are taken.
const WORD_DRAWS: usize = 8;

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

/// Body of `GET /api/me` and of a successful `POST /api/login`.
#[derive(Serialize)]
struct Me {
    /// [`OWNER_NAME`]; `None` if it is unset or not a valid display name, so the app prefills
    /// nothing.
    name: Option<DisplayName>,
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
                me(&ctx)
            } else {
                Response::error("unauthorized", 401)
            }
        })
        .post_async("/api/new", new_meeting)
        .get_async("/api/meetings", past_meetings)
        .get_async("/api/meeting/:words", meeting_info)
        .get_async("/api/ice", ice_servers)
        .get_async("/ws/:words", join_meeting)
        .get_async("/rec/:recorder/:secret/:who/:n", recorder_stream)
        .get_async("/api/transcript/:token", |_, ctx| async move {
            transcript(&ctx, RECORDER_STATUS).await
        })
        .get_async("/api/transcript/:token/:file", |_, ctx| async move {
            match ctx.param("file").map(String::as_str) {
                Some(file @ (TRANSCRIPT_MD | TRANSCRIPT_JSON)) => {
                    transcript(&ctx, &format!("/{file}")).await
                }
                _ => private(Response::error("not found", 404)),
            }
        })
        .run(req, env)
        .await
}

/// The KV key that maps a transcript token to its `RecorderObject`'s id. Meeting words never
/// contain `:`, so it cannot collide with them.
fn transcript_key(token: &TranscriptRef) -> String {
    format!("transcript:{}", token.as_str())
}

/// Passes `GET /api/transcript/:token...` to the transcript's recorder at `path`. The token is
/// 32 hex digits; anything else is unknown without a KV lookup.
async fn transcript(ctx: &RouteContext<Config>, path: &str) -> Result<Response> {
    let token = ctx.param("token").filter(|t| {
        t.len() == 32
            && t.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    });
    let Some(token) = token else {
        return private(Response::error("unknown transcript", 404));
    };
    let key = transcript_key(&TranscriptRef::new(token.as_str()));
    let Some(recorder) = ctx.kv(MEETINGS)?.get(&key).text().await? else {
        return private(Response::error("unknown transcript", 404));
    };
    let response = ctx
        .durable_object(RECORDERS)?
        .id_from_string(&recorder)?
        .get_stub()?
        .fetch_with_str(&format!("{ORIGIN}{path}"))
        .await;
    private(response)
}

/// Adds the headers that keep a transcript out of referrers, search engines and caches.
fn private(response: Result<Response>) -> Result<Response> {
    let response = response?;
    // A copy: the headers of a response fetched from a Durable Object are immutable.
    let headers = response.headers().clone();
    headers.set("Referrer-Policy", "no-referrer")?;
    headers.set("X-Robots-Tag", "noindex")?;
    headers.set("Cache-Control", "no-store")?;
    Ok(response.with_headers(headers))
}

/// Forwards an SFU adapter's WebSocket upgrade to its recorder. The recorder checks the secret
/// in the path; an id that is not a recorder's fails here.
async fn recorder_stream(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !is_websocket_upgrade(&req)? {
        return Response::error("expected a WebSocket upgrade", 426);
    }
    let Some(id) = ctx.param("recorder") else {
        return Response::error("not found", 404);
    };
    let recorders = ctx.durable_object(RECORDERS)?;
    let Ok(id) = recorders.id_from_string(id) else {
        return Response::error("not found", 404);
    };
    id.get_stub()?.fetch_with_request(req).await
}

/// `POST /api/login`: sets the owner's session cookie if the password is `CREATE_SECRET`, and
/// answers like `GET /api/me`, so the app learns the owner's name without asking again.
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
    with_cookie(me(&ctx)?, &OwnerSession::new(&secret).issue(now()))
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
        ended_at: None,
    };
    if let Err(e) = meetings::update(&kv, |index| meetings::added(index, entry)).await {
        console_error!("adding {meeting:?} to the meeting index failed: {e}");
    }
    Ok(Response::from_json(&NewMeeting { meeting })?.with_status(201))
}

/// `GET /api/meetings`: the owner's last [`meetings::MAX_ENTRIES`] meetings, newest first, from
/// the index in KV. Entries not yet settled as ended get the phase their room reports right now;
/// rooms that report ended are settled in the index, so the next listing does not ask them again.
/// A room that fails to answer is logged and left out, so one broken room does not hide the
/// others. The index is eventually consistent (see [`meetings`]), so a meeting created in the
/// last minute may be missing.
async fn past_meetings(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if let Some(refusal) = refuse_non_owner(&req, &ctx)? {
        return Ok(refusal);
    }
    let kv = ctx.kv(MEETINGS)?;
    let entries = meetings::newest(meetings::load(&kv).await?);
    let rooms = ctx.durable_object(ROOMS)?;
    let phases = join_all(entries.iter().map(|entry| async {
        match entry.ended_at {
            Some(_) => Ok(Phase::Ended),
            None => room_phase(&rooms, &entry.room).await,
        }
    }))
    .await;
    let mut listed = Vec::with_capacity(entries.len());
    let mut just_ended = Vec::new();
    for (entry, phase) in entries.into_iter().zip(phases) {
        match phase {
            Ok(phase) => {
                if phase == Phase::Ended && entry.ended_at.is_none() {
                    just_ended.push(entry.room.clone());
                }
                listed.push((entry, phase));
            }
            Err(e) => {
                console_error!(
                    "room {} of {:?} did not answer: {e}",
                    entry.room,
                    entry.meeting
                )
            }
        }
    }
    if !just_ended.is_empty()
        && let Err(e) =
            meetings::update(&kv, |index| meetings::settled(index, &just_ended, now())).await
    {
        // Logged, not returned: the next listing settles them again.
        console_error!("settling ended meetings in the index failed: {e}");
    }
    Response::from_json(&meetings::listing(listed))
}

/// Asks the room with id `room` where its meeting stands. A room without a record counts as
/// ended: the index entry is written only after the room was initialised, so no record means
/// its storage was removed.
async fn room_phase(rooms: &ObjectNamespace, room: &str) -> Result<Phase> {
    let mut response = rooms
        .id_from_string(room)?
        .get_stub()?
        .fetch_with_str(&format!("{ORIGIN}{INFO}"))
        .await?;
    match response.status_code() {
        404 => Ok(Phase::Ended),
        200 => Ok(response.json::<RoomInfo>().await?.phase),
        status => Err(worker::Error::RustError(format!("info answered {status}"))),
    }
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

/// The owner's [`Me`], never cached: the answer depends on the session cookie.
fn me(ctx: &RouteContext<Config>) -> Result<Response> {
    let response = Response::from_json(&Me {
        name: owner_name(ctx),
    })?;
    response.headers().set("Cache-Control", "no-store")?;
    Ok(response)
}

/// [`OWNER_NAME`], or `None` (logged) if it is unset or not a valid display name.
fn owner_name(ctx: &RouteContext<Config>) -> Option<DisplayName> {
    let Ok(value) = ctx.var(OWNER_NAME) else {
        console_error!("{OWNER_NAME} is not set; the app prefills no name for the owner");
        return None;
    };
    let value = value.to_string();
    DisplayName::parse(&value)
        .inspect_err(|e| {
            console_error!(
                "{OWNER_NAME} {value:?} is not a display name ({e}); the app prefills none"
            )
        })
        .ok()
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

/// `GET /api/ice?meeting=:words`: the ICE servers, for a meeting that exists only.
async fn ice_servers(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let url = req.url()?;
    let meeting = url
        .query_pairs()
        .find_map(|(key, value)| (key == "meeting").then_some(value));
    let known = match meeting {
        Some(words) => room_of(&ctx.kv(MEETINGS)?, &words).await?.is_some(),
        None => false,
    };
    if !known {
        return Response::error("unknown meeting", 404);
    }
    Response::from_json(&ice::ice_servers(&ctx.env).await)
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
    let Some(words) = ctx.param("words") else {
        return Ok(None);
    };
    let Some(room) = room_of(&ctx.kv(MEETINGS)?, words).await? else {
        return Ok(None);
    };
    Ok(Some(
        ctx.durable_object(ROOMS)?
            .id_from_string(&room)?
            .get_stub()?,
    ))
}

/// The id (hex) of the room for `words`, matched case-insensitively; `None` for malformed or
/// unknown words.
async fn room_of(kv: &KvStore, words: &str) -> Result<Option<String>> {
    let Ok(words) = MeetingId::parse(&words.to_ascii_lowercase()) else {
        return Ok(None);
    };
    Ok(kv.get(words.as_str()).text().await?)
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
