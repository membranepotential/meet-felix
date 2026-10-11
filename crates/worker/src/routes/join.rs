//! Joining a meeting: the pre-join lookup `GET /api/meeting/:words`, the ICE servers
//! `GET /api/ice`, the room's WebSocket `GET /ws/:words`, and the SFU adapters' sockets into the
//! recorder `GET /rec/...`. Anyone may call them; the per-client rate limit
//! ([`Limit::Public`]) bounds how fast one client guesses words or mints TURN credentials.

use meeting_core::MeetingId;
use meeting_core::http::{ApiErrorCode, MeetingLookup};
use worker::{KvStore, Request, Response, Result, RouteContext, Stub};

use crate::internal::{self, INFO, OWNER_HEADER, OWNER_YES, RecorderId, RoomId, RoomInfo};
use crate::limits::{self, Limit};
use crate::owner::Initiator;
use crate::routes::owner::{initiator_of, is_owner_session, owner_auth};
use crate::{Config, MEETINGS, RECORDERS, ROOMS, ice, is_websocket_upgrade, reply};

/// `GET /api/meeting/:words`: the room's [`MeetingLookup`].
pub async fn lookup(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !limits::allows(&ctx.env, Limit::Public, &req).await? {
        return limits::too_many_requests();
    }
    let Some(room) = room_stub(&ctx).await? else {
        return reply::error(ApiErrorCode::NotFound);
    };
    match room_info(&room).await? {
        Some(info) => Response::from_json(&MeetingLookup::new(info.phase, info.recording_policy)),
        // The room's storage is gone; the words still map to it until KV catches up.
        None => reply::error(ApiErrorCode::NotFound),
    }
}

/// The room's [`RoomInfo`], or `None` if it has no record: the router never initialised it, or
/// the ended room deleted its storage.
pub async fn room_info(room: &Stub) -> Result<Option<RoomInfo>> {
    let mut response = internal::get(room, INFO).await?;
    match response.status_code() {
        200 => Ok(Some(response.json().await?)),
        404 => Ok(None),
        status => Err(worker::Error::RustError(format!(
            "{INFO} answered {status}"
        ))),
    }
}

/// `GET /api/ice?meeting=:words`: the ICE servers, for a meeting that exists only.
pub async fn ice_servers(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !limits::allows(&ctx.env, Limit::Public, &req).await? {
        return limits::too_many_requests();
    }
    let url = req.url()?;
    let meeting = url
        .query_pairs()
        .find_map(|(key, value)| (key == "meeting").then_some(value));
    let known = match meeting {
        Some(words) => room_of(&ctx.kv(MEETINGS)?, &words).await?.is_some(),
        None => false,
    };
    if !known {
        return reply::error(ApiErrorCode::NotFound);
    }
    Response::from_json(&ice::ice_servers(&ctx.env).await)
}

/// `GET /ws/:words`: forwards the upgrade to the room, telling it in [`OWNER_HEADER`] whether
/// the socket is the owner's. Missing owner secrets make every socket a guest's, and so does a
/// browser socket from another origin, whose cookie is ignored: no other site joins as host, and
/// the socket still joins as anyone may. Refusing it instead would look like a lost connection
/// to the app, which retries without end, so an owner whose browser sends unexpected headers
/// would be locked out without a word.
pub async fn join(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !is_websocket_upgrade(&req)? {
        return reply::error(ApiErrorCode::UpgradeRequired);
    }
    if !limits::allows(&ctx.env, Limit::Public, &req).await? {
        return limits::too_many_requests();
    }
    let Some(room) = room_stub(&ctx).await? else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let owner = match owner_auth(&ctx) {
        Some(auth) => {
            is_owner_session(&req, &auth.session)? && initiator_of(&req)? != Initiator::Foreign
        }
        None => false,
    };
    let forwarded = req.clone_mut()?;
    forwarded.headers().delete(OWNER_HEADER)?;
    if owner {
        forwarded.headers().set(OWNER_HEADER, OWNER_YES)?;
    }
    room.fetch_with_request(forwarded).await
}

/// `GET /rec/:recorder/:secret/:who/:n`: forwards an SFU adapter's WebSocket upgrade to its
/// recorder. The recorder checks the secret in the path; an id that is not a recorder's fails
/// here.
pub async fn recorder_stream(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if !is_websocket_upgrade(&req)? {
        return reply::error(ApiErrorCode::UpgradeRequired);
    }
    let Some(id) = ctx.param("recorder") else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let Ok(recorder) = RecorderId::new(id.as_str()).stub(&ctx.durable_object(RECORDERS)?) else {
        return reply::error(ApiErrorCode::NotFound);
    };
    recorder.fetch_with_request(req).await
}

/// The room for the `:words` route parameter, matched case-insensitively; `None` for malformed
/// or unknown words.
async fn room_stub(ctx: &RouteContext<Config>) -> Result<Option<Stub>> {
    let Some(words) = ctx.param("words") else {
        return Ok(None);
    };
    let Some(room) = room_of(&ctx.kv(MEETINGS)?, words).await? else {
        return Ok(None);
    };
    room.stub(&ctx.durable_object(ROOMS)?).map(Some)
}

/// The room `words` map to, matched case-insensitively; `None` for malformed or unknown words.
async fn room_of(kv: &KvStore, words: &str) -> Result<Option<RoomId>> {
    let Ok(words) = MeetingId::parse(&words.to_ascii_lowercase()) else {
        return Ok(None);
    };
    Ok(kv.get(words.as_str()).text().await?.map(RoomId::new))
}
