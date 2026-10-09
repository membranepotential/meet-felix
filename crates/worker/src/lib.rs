//! Cloudflare Worker for meet-felix: HTTP router plus the per-meeting `RoomObject`.

mod ids;
mod protocol;
mod room;

use serde::Serialize;
use worker::{Context, Env, KvStore, Request, Response, Result, RouteContext, Router, event};

use crate::ids::{HostToken, MeetingWords};

/// KV namespace mapping meeting words to the Room Durable Object id (hex).
const MEETINGS: &str = "MEETINGS";
/// Durable Object namespace of `RoomObject`.
const ROOMS: &str = "ROOMS";
/// Var naming the jurisdiction new rooms are pinned to (`"eu"`); empty for none, because local
/// workerd rejects every jurisdiction.
const ROOM_JURISDICTION: &str = "ROOM_JURISDICTION";
/// Upper bound for a meeting link's life; the room usually ends earlier.
const MEETING_TTL_SECS: u64 = 24 * 60 * 60;
/// Draws of fresh words before giving up when all drawn combinations are taken.
const WORD_DRAWS: usize = 8;

#[derive(Serialize)]
struct NewMeeting {
    meeting: MeetingWords,
    host_token: HostToken,
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .post_async("/api/new", new_meeting)
        .get_async("/api/meeting/:words", meeting_exists)
        .get_async("/ws/:words", join_meeting)
        .run(req, env)
        .await
}

async fn new_meeting(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let kv = ctx.kv(MEETINGS)?;
    let Some(meeting) = unused_words(&kv).await? else {
        return Response::error("no free meeting link, try again", 503);
    };
    let rooms = ctx.durable_object(ROOMS)?;
    let room = match ctx.var(ROOM_JURISDICTION)?.to_string().as_str() {
        "" => rooms.unique_id()?,
        jurisdiction => rooms.unique_id_with_jurisdiction(jurisdiction)?,
    }
    .to_string();
    kv.put(&meeting.to_string(), room)?
        .expiration_ttl(MEETING_TTL_SECS)
        .execute()
        .await?;
    let created = NewMeeting {
        meeting,
        host_token: HostToken::random()?,
    };
    Ok(Response::from_json(&created)?.with_status(201))
}

async fn meeting_exists(_req: Request, ctx: RouteContext<()>) -> Result<Response> {
    match room_id(&ctx).await? {
        Some(_) => Response::empty(),
        None => Response::error("unknown meeting", 404),
    }
}

async fn join_meeting(req: Request, ctx: RouteContext<()>) -> Result<Response> {
    let is_upgrade = req
        .headers()
        .get("Upgrade")?
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    if !is_upgrade {
        return Response::error("expected a WebSocket upgrade", 426);
    }
    let Some(room) = room_id(&ctx).await? else {
        return Response::error("unknown meeting", 404);
    };
    ctx.durable_object(ROOMS)?
        .id_from_string(&room)?
        .get_stub()?
        .fetch_with_request(req)
        .await
}

/// Draws words until a combination is not mapped in KV. KV is eventually consistent, so two
/// concurrent creations can still collide; acceptable while only the host creates meetings.
async fn unused_words(kv: &KvStore) -> Result<Option<MeetingWords>> {
    for _ in 0..WORD_DRAWS {
        let words = MeetingWords::random()?;
        if kv.get(&words.to_string()).text().await?.is_none() {
            return Ok(Some(words));
        }
    }
    Ok(None)
}

/// Looks up the `:words` route parameter; `None` for malformed or unknown words.
async fn room_id(ctx: &RouteContext<()>) -> Result<Option<String>> {
    let Some(words) = ctx.param("words").and_then(|w| MeetingWords::parse(w)) else {
        return Ok(None);
    };
    Ok(ctx.kv(MEETINGS)?.get(&words.to_string()).text().await?)
}
