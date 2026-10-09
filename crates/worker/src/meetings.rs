//! The owner's list of past meetings behind `GET /api/meetings`: one JSON array of [`Entry`]
//! in the `MEETINGS` KV namespace under [`KEY`], oldest first. `POST /api/new` appends to it and
//! the `RoomObject` marks its entry ended.
//!
//! Every change is a read-modify-write of the whole array ([`update`]). KV has no
//! compare-and-swap and is eventually consistent (a read may miss a write from another location
//! for up to 60 seconds), so two changes close together can lose one of them: a meeting created
//! right after another one ended may drop that end, and a room that ends within a minute of its
//! creation may not find its entry. Accepted, because only the holder of `CREATE_SECRET` creates
//! meetings, and rarely; a lost end leaves an entry that reads as not ended.

use meeting_core::{MeetingId, Millis, TranscriptRef};
use serde::{Deserialize, Serialize};
use worker::{KvStore, Result};

/// The KV key of the index. Meeting words always contain `-`, so they never collide with it.
pub const KEY: &str = "index";
/// How many meetings the index keeps; older ones drop out when new ones are added.
pub const MAX_ENTRIES: usize = 200;

/// One meeting in the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub meeting: MeetingId,
    /// The `RoomObject`'s id (hex). It identifies the entry, because words are recycled.
    pub room: String,
    pub created_at: Millis,
    pub transcript: TranscriptRef,
    /// When the room ended; `None` while it runs, or if its end was lost (see the module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Millis>,
}

/// Body of `GET /api/meetings`.
#[derive(Debug, Serialize)]
pub struct Listing {
    /// Newest first.
    pub meetings: Vec<PastMeeting>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PastMeeting {
    pub meeting: MeetingId,
    pub created_at: Millis,
    pub ended: bool,
    /// The transcript link token, once the meeting ended; the transcript job starts then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<TranscriptRef>,
}

/// The index with `entry` appended, keeping the newest [`MAX_ENTRIES`].
pub fn added(mut index: Vec<Entry>, entry: Entry) -> Vec<Entry> {
    index.push(entry);
    let excess = index.len().saturating_sub(MAX_ENTRIES);
    index.split_off(excess)
}

/// The index with `room`'s entry marked ended at `at`; `None` if it has no entry for `room` or
/// the entry is already marked.
pub fn ended(mut index: Vec<Entry>, room: &str, at: Millis) -> Option<Vec<Entry>> {
    let entry = index
        .iter_mut()
        .find(|entry| entry.room == room && entry.ended_at.is_none())?;
    entry.ended_at = Some(at);
    Some(index)
}

/// The response for `index`: newest first, at most [`MAX_ENTRIES`].
pub fn listing(mut index: Vec<Entry>) -> Listing {
    index.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
    Listing {
        meetings: index
            .into_iter()
            .take(MAX_ENTRIES)
            .map(|entry| PastMeeting {
                meeting: entry.meeting,
                created_at: entry.created_at,
                ended: entry.ended_at.is_some(),
                transcript: entry.ended_at.map(|_| entry.transcript),
            })
            .collect(),
    }
}

/// The stored index; empty if none was stored yet.
pub async fn load(kv: &KvStore) -> Result<Vec<Entry>> {
    Ok(kv.get(KEY).json().await?.unwrap_or_default())
}

/// Reads the index, applies `change` and stores the result, unless `change` returns `None`.
/// Returns whether it stored. Not atomic; see the module docs.
pub async fn update(
    kv: &KvStore,
    change: impl FnOnce(Vec<Entry>) -> Option<Vec<Entry>>,
) -> Result<bool> {
    let Some(index) = change(load(kv).await?) else {
        return Ok(false);
    };
    kv.put(KEY, serde_json::to_string(&index)?)?
        .execute()
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(n: u64) -> Entry {
        Entry {
            meeting: MeetingId::from_random([0, 1, 2]),
            room: format!("room{n}"),
            created_at: Millis(n),
            transcript: TranscriptRef::new(format!("t{n}")),
            ended_at: None,
        }
    }

    #[test]
    fn entry_is_camel_case_and_survives_json() {
        let running = entry(7);
        let json = serde_json::to_value(&running).unwrap();
        assert_eq!(json["createdAt"], json!(7));
        assert!(json.get("endedAt").is_none(), "{json}");
        let stopped = Entry {
            ended_at: Some(Millis(9)),
            ..running.clone()
        };
        for e in [running, stopped] {
            let back: Entry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
            assert_eq!(back, e);
        }
    }

    #[test]
    fn listing_shows_the_transcript_only_once_ended() {
        let index = ended(vec![entry(1), entry(2)], "room1", Millis(5)).unwrap();
        let json = serde_json::to_value(listing(index)).unwrap();
        let meeting = MeetingId::from_random([0, 1, 2]);
        assert_eq!(
            json,
            json!({ "meetings": [
                { "meeting": meeting, "createdAt": 2, "ended": false },
                { "meeting": meeting, "createdAt": 1, "ended": true, "transcript": "t1" },
            ]})
        );
    }

    #[test]
    fn index_keeps_the_newest_entries_and_lists_them_newest_first() {
        let index = (0..MAX_ENTRIES as u64 + 5).fold(Vec::new(), |index, n| added(index, entry(n)));
        assert_eq!(index.len(), MAX_ENTRIES);
        assert_eq!(index[0].created_at, Millis(5), "the oldest dropped out");
        let listed = listing(index).meetings;
        assert_eq!(listed.len(), MAX_ENTRIES);
        assert_eq!(listed[0].created_at, Millis(MAX_ENTRIES as u64 + 4));
        assert!(listed.windows(2).all(|w| w[0].created_at > w[1].created_at));
    }

    #[test]
    fn a_room_ends_its_own_entry_once() {
        let index = ended(vec![entry(1), entry(2)], "room2", Millis(9)).unwrap();
        assert_eq!(index[1].ended_at, Some(Millis(9)));
        assert_eq!(index[0].ended_at, None);
        assert_eq!(ended(index.clone(), "room2", Millis(10)), None);
        assert_eq!(ended(index, "room3", Millis(10)), None);
    }
}
