//! `POST /api/new`: the owner creates a scheduled meeting (D6, D7 in `docs/links`).
//!
//! Creating takes three writes that no transaction joins: the room's record, the words in KV
//! and the entry in the owner's list. The room comes first, because nothing points at it until
//! KV does; the words next, so the answer names a meeting that works; the list entry last, and
//! a failure there is only logged, because the meeting works without it.

use meeting_core::http::{ApiErrorCode, NewMeeting, NewMeetingRequest};
use meeting_core::{MeetingId, SCHEDULED_TTL_MS};
use worker::{KvStore, Request, Response, Result, RouteContext, console_error};

use crate::internal::{self, INIT, InitRoom, RoomId};
use crate::owner::{self as credentials, Initiator};
use crate::routes::owner::{OwnerProof, initiator_of, owner_proof};
use crate::{Config, ROOMS, jurisdiction, kv, meetings, now, random, reply};

/// Upper bound for a meeting link's life (90 days); the room releases it when it ends. Longer
/// than the core's `SCHEDULED_TTL_MS` (60 days), so a scheduled link works until the room
/// expires (D6 in `docs/links`).
const MEETING_TTL_SECS: u64 = 90 * 24 * 60 * 60;
const _: () = assert!(MEETING_TTL_SECS * 1000 > SCHEDULED_TTL_MS);
/// Draws of fresh words before giving up when all drawn combinations are taken.
const WORD_DRAWS: usize = 8;

/// `POST /api/new` with an optional [`NewMeetingRequest`]; answers 201 with [`NewMeeting`].
/// With the session cookie, a browser request must come from this origin and carry JSON, so
/// neither another site nor a form injected into this one can create meetings.
pub async fn create(mut req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let proof = match owner_proof(&req, &ctx).await? {
        Ok(proof) => proof,
        Err(refusal) => return Ok(refusal),
    };
    if proof == OwnerProof::Cookie {
        match initiator_of(&req)? {
            Initiator::Foreign => return reply::error(ApiErrorCode::CrossOrigin),
            Initiator::SameOrigin
                if !credentials::is_json(req.headers().get("Content-Type")?.as_deref()) =>
            {
                return reply::error(ApiErrorCode::NotJson);
            }
            Initiator::SameOrigin | Initiator::Unknown => {}
        }
    }
    let Some(request) = parse_request(&req.text().await?) else {
        return reply::error(ApiErrorCode::BadRequest);
    };

    let store = kv::open(&ctx.env)?;
    let Some(meeting) = unused_words(&store).await? else {
        return reply::error(ApiErrorCode::NoFreeWords);
    };
    let rooms = ctx.durable_object(ROOMS)?;
    let room_id = jurisdiction::unique_id(&rooms, ctx.data.jurisdiction)?;
    let room = RoomId::new(room_id.to_string());
    let transcript = random::transcript_ref()?;
    let init = InitRoom {
        meeting: meeting.clone(),
        transcript: transcript.clone(),
        recording_policy: request.recording,
    };
    internal::post(&room_id.get_stub()?, INIT, &init).await?;
    // Check-then-write: KV has no atomic insert, so two creations that draw the same words at
    // the same moment could both pass `unused_words`. Accepted, because only the holder of
    // CREATE_SECRET creates meetings, and rarely.
    kv::map_words(&store, &meeting, &room, MEETING_TTL_SECS).await?;
    // Logged, not returned: the meeting exists and works without its list entry.
    let entry = meetings::Entry {
        meeting: meeting.clone(),
        room,
        created_at: now(),
        transcript,
        ended: None,
    };
    if let Err(e) = meetings::update(&store, |index| meetings::added(index, entry)).await {
        console_error!("adding {meeting:?} to the meeting list failed: {e}");
    }
    Ok(Response::from_json(&NewMeeting { meeting })?.with_status(201))
}

/// The request in `body`: an empty body asks for the defaults. `None` if it is not a
/// [`NewMeetingRequest`].
fn parse_request(body: &str) -> Option<NewMeetingRequest> {
    match body.trim() {
        "" => Some(NewMeetingRequest::default()),
        body => serde_json::from_str(body).ok(),
    }
}

/// Draws words until a combination is not mapped in KV.
async fn unused_words(store: &KvStore) -> Result<Option<MeetingId>> {
    for _ in 0..WORD_DRAWS {
        let words = random::meeting_id()?;
        if kv::room_of(store, &words).await?.is_none() {
            return Ok(Some(words));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use meeting_core::RecordingPolicy;

    use super::*;

    #[test]
    fn an_empty_body_asks_for_the_defaults() {
        let policy = |body| parse_request(body).map(|r| r.recording);
        assert_eq!(policy(""), Some(RecordingPolicy::FromStart));
        assert_eq!(policy(" \n"), Some(RecordingPolicy::FromStart));
        assert_eq!(
            policy(r#"{"recording":"hostDecides"}"#),
            Some(RecordingPolicy::HostDecides)
        );
        assert_eq!(policy("{"), None);
    }
}
