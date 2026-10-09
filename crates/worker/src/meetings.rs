//! The owner's list of meetings behind `GET /api/meetings`: one JSON array of [`Entry`] in the
//! `MEETINGS` KV namespace under [`KEY`], oldest first. `POST /api/new` appends to it. Where a
//! meeting stands (scheduled, live, ended) is asked of its `RoomObject` when the list is read, so
//! the room stays the source of that fact; an end is final, so the listing writes it into the
//! entry ([`settled`]) and never asks that room again.
//!
//! Every write is a read-modify-write of the whole array ([`update`]) by the router. KV has no
//! compare-and-swap and is eventually consistent (a read may miss a write from another location
//! for up to 60 seconds), so a creation and a settle within a minute of each other can lose one
//! of them: a lost end is written again by the next listing, a lost entry is gone. Accepted,
//! because only the holder of `CREATE_SECRET` creates meetings, and rarely.

use meeting_core::{MeetingId, Millis, Phase, TranscriptRef};
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
    /// When a listing first saw the room ended; `None` while the room may still change.
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
    /// As the room reported it when the list was read.
    pub phase: Phase,
    /// Whether `phase` is [`Phase::Ended`]; kept for clients that predate `phase`.
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

/// The entries to list: newest first, at most [`MAX_ENTRIES`].
pub fn newest(mut index: Vec<Entry>) -> Vec<Entry> {
    index.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
    index.truncate(MAX_ENTRIES);
    index
}

/// The index with the entries of `rooms` marked ended at `at`, unless already marked.
pub fn settled(mut index: Vec<Entry>, rooms: &[String], at: Millis) -> Vec<Entry> {
    for entry in &mut index {
        if entry.ended_at.is_none() && rooms.contains(&entry.room) {
            entry.ended_at = Some(at);
        }
    }
    index
}

/// The response for entries in list order, each with the phase its room reported, or ended if
/// the entry is settled.
pub fn listing(entries: impl IntoIterator<Item = (Entry, Phase)>) -> Listing {
    Listing {
        meetings: entries
            .into_iter()
            .map(|(entry, reported)| {
                let phase = if entry.ended_at.is_some() {
                    Phase::Ended
                } else {
                    reported
                };
                PastMeeting {
                    meeting: entry.meeting,
                    created_at: entry.created_at,
                    phase,
                    ended: phase == Phase::Ended,
                    transcript: (phase == Phase::Ended).then_some(entry.transcript),
                }
            })
            .collect(),
    }
}

/// The stored index; empty if none was stored yet.
pub async fn load(kv: &KvStore) -> Result<Vec<Entry>> {
    Ok(kv.get(KEY).json().await?.unwrap_or_default())
}

/// Reads the index, applies `change` and stores the result. Not atomic; see the module docs.
pub async fn update(kv: &KvStore, change: impl FnOnce(Vec<Entry>) -> Vec<Entry>) -> Result<()> {
    let index = change(load(kv).await?);
    kv.put(KEY, serde_json::to_string(&index)?)?
        .execute()
        .await?;
    Ok(())
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
        let e = entry(7);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["createdAt"], json!(7));
        let back: Entry = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn entries_from_before_the_rooms_answered_still_load() {
        let old = json!({ "meeting": "a-b-c", "room": "room1", "createdAt": 1, "transcript": "t1",
            "liveAt": 2, "endedAt": 3 });
        let back: Entry = serde_json::from_value(old).unwrap();
        assert_eq!(
            (back.room.as_str(), back.ended_at),
            ("room1", Some(Millis(3)))
        );
        let json = serde_json::to_value(entry(1)).unwrap();
        assert!(json.get("endedAt").is_none(), "{json}");
    }

    #[test]
    fn settling_marks_the_named_rooms_once() {
        let index = settled(vec![entry(1), entry(2)], &["room2".into()], Millis(9));
        assert_eq!(
            (index[0].ended_at, index[1].ended_at),
            (None, Some(Millis(9)))
        );
        let again = settled(index, &["room2".into(), "room3".into()], Millis(10));
        assert_eq!(again[1].ended_at, Some(Millis(9)), "the first end stands");
        let listed = listing([(again[1].clone(), Phase::Live)]).meetings;
        assert_eq!((listed[0].phase, listed[0].ended), (Phase::Ended, true));
    }

    #[test]
    fn listing_shows_the_transcript_only_once_ended() {
        let json = serde_json::to_value(listing([
            (entry(3), Phase::Live),
            (entry(2), Phase::Scheduled),
            (entry(1), Phase::Ended),
        ]))
        .unwrap();
        let meeting = MeetingId::from_random([0, 1, 2]);
        assert_eq!(
            json,
            json!({ "meetings": [
                { "meeting": meeting, "createdAt": 3, "phase": "live", "ended": false },
                { "meeting": meeting, "createdAt": 2, "phase": "scheduled", "ended": false },
                { "meeting": meeting, "createdAt": 1, "phase": "ended", "ended": true, "transcript": "t1" },
            ]})
        );
    }

    #[test]
    fn index_keeps_the_newest_entries_and_lists_them_newest_first() {
        let index = (0..MAX_ENTRIES as u64 + 5).fold(Vec::new(), |index, n| added(index, entry(n)));
        assert_eq!(index.len(), MAX_ENTRIES);
        assert_eq!(index[0].created_at, Millis(5), "the oldest dropped out");
        let listed = newest(index);
        assert_eq!(listed.len(), MAX_ENTRIES);
        assert_eq!(listed[0].created_at, Millis(MAX_ENTRIES as u64 + 4));
        assert!(listed.windows(2).all(|w| w[0].created_at > w[1].created_at));
    }
}
