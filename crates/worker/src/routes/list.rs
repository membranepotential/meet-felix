//! `GET /api/meetings`: the owner's last [`meetings::MAX_ENTRIES`] meetings, newest first
//! (D26 in `docs/ui`).

use futures_util::future::join_all;
use meeting_core::Phase;
use worker::{KvStore, ObjectNamespace, Request, Response, Result, RouteContext, console_error};

use crate::meetings::{self, Entry, SeenEnded};
use crate::routes::join::room_info;
use crate::routes::owner::owner_proof;
use crate::{Config, MEETINGS, ROOMS, now, transcript_key};

/// Lists the meetings in the owner's list (see [`crate::meetings`]). Entries not yet marked
/// ended get the phase their room reports right now. For a meeting found ended, the listing
/// also looks up whether its transcript link exists, until that is known, and marks both in
/// the list ([`meetings::settled`]), so the next listing asks neither again. A room that fails
/// to answer is logged and left out, so one broken room does not hide the others. The list is
/// eventually consistent, so a meeting created in the last minute may be missing.
pub async fn list(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    if let Err(refusal) = owner_proof(&req, &ctx).await? {
        return Ok(refusal);
    }
    let kv = ctx.kv(MEETINGS)?;
    let rooms = ctx.durable_object(ROOMS)?;
    let entries = meetings::newest(meetings::load(&kv).await?);
    let answers = join_all(entries.iter().map(|entry| look_at(&kv, &rooms, entry))).await;
    let mut listed = Vec::with_capacity(entries.len());
    let mut seen = Vec::new();
    for (entry, answer) in entries.into_iter().zip(answers) {
        match answer {
            Ok((phase, linked)) => {
                if let Some(linked) = linked {
                    seen.push(SeenEnded {
                        room: entry.room.clone(),
                        linked,
                    });
                }
                listed.push((entry, phase));
            }
            Err(e) => console_error!(
                "room {} of {:?} did not answer: {e}",
                entry.room.as_str(),
                entry.meeting
            ),
        }
    }
    let now = now();
    if !seen.is_empty()
        && let Err(e) = meetings::update(&kv, |index| meetings::settled(index, &seen, now)).await
    {
        // Logged, not returned: the next listing learns the same again.
        console_error!("marking ended meetings in the meeting list failed: {e}");
    }
    let (entries, phases): (Vec<Entry>, Vec<Phase>) = listed.into_iter().unzip();
    let entries = meetings::settled(entries, &seen, now);
    Response::from_json(&meetings::listing(entries.into_iter().zip(phases)))
}

/// Where `entry`'s meeting stands, and, if [`meetings::wants_link_check`], whether its
/// transcript link exists. A room without a record counts as ended: the entry is written only
/// after the room was initialised, so no record means its storage was removed.
async fn look_at(
    kv: &KvStore,
    rooms: &ObjectNamespace,
    entry: &Entry,
) -> Result<(Phase, Option<bool>)> {
    let phase = match entry.ended {
        Some(_) => Phase::Ended,
        None => room_info(&entry.room.stub(rooms)?)
            .await?
            .map_or(Phase::Ended, |info| info.phase),
    };
    let linked = if meetings::wants_link_check(entry, phase) {
        let key = transcript_key(&entry.transcript);
        Some(kv.get(&key).text().await?.is_some())
    } else {
        None
    };
    Ok((phase, linked))
}
