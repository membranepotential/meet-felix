//! The owner's list of meetings behind `GET /api/meetings`: the [`Entry`]s, oldest first, in the
//! `MEETINGS` KV namespace under [`KEY`]. `POST /api/new` appends to it. Where a meeting stands
//! (scheduled, live, ended) is asked of its `RoomObject` when the list is read, so the room stays
//! the source of that fact; an end is final, so the listing writes it into the entry
//! ([`settled`]) and never asks that room again.
//!
//! Every write is a read-modify-write of the whole list ([`update`]) by the router. KV has no
//! compare-and-swap and is eventually consistent (a read may miss a write from another location
//! for up to 60 seconds), so a creation and a settle within a minute of each other can lose one
//! of them: a lost end is written again by the next listing, a lost entry is gone. Accepted,
//! because only the holder of `CREATE_SECRET` creates meetings, and rarely.
//!
//! The list is stored in the versioned envelope of [`crate::record`] ([`INDEX_VERSION`]), so a
//! change of its shape cannot break the list for good: a list from a newer version is an error
//! that leaves it untouched, and every older version is read. `fixtures/index/` freezes the
//! stored shapes, as `fixtures/room/` does the room's.

use meeting_core::http::{Listing, PastMeeting};
use meeting_core::{MeetingId, Millis, Phase, TranscriptRef};
use serde::{Deserialize, Serialize};
use worker::{KvStore, Result};

use crate::internal::RoomId;
use crate::record::{DecodeError, Stored, stored_record, stored_version};

/// The KV key of the list. Meeting words always contain `-`, so they never collide with it.
pub const KEY: &str = "index";
/// How many meetings the list keeps; older ones drop out when new ones are added.
pub const MAX_ENTRIES: usize = 200;
/// The version of the stored list's shape. 1 was a bare JSON array (until 2026-10-11); 2 wraps
/// it in the envelope and records whether an ended meeting's transcript link exists.
pub const INDEX_VERSION: u32 = 2;
/// How long after a listing first saw a meeting ended a missing transcript link counts as never
/// made. The room links the token in KV when recording starts, which is before the end; a KV
/// write can take 60 seconds or more to reach other locations, and a lookup of a missing key is
/// cached as well (research 11, "KV's cache fights recycled words"), so a miss soon after the
/// end proves nothing yet.
pub const LINK_SETTLE_MS: u64 = 5 * 60 * 1000;

/// One meeting in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub meeting: MeetingId,
    /// It identifies the entry, because words are recycled.
    pub room: RoomId,
    pub created_at: Millis,
    pub transcript: TranscriptRef,
    /// Set once a listing saw the room ended; `None` while the room may still change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// What the list knows about an ended meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    /// When a listing first saw the room ended.
    pub at: Millis,
    pub recorded: Recorded,
}

/// Whether an ended meeting recorded, so that its transcript link leads somewhere: whether its
/// room linked the transcript token in KV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Recorded {
    /// No link was found yet, within [`LINK_SETTLE_MS`] of the end.
    Unknown,
    Yes,
    No,
}

/// The shapes earlier code stored, as it stored them. Never change them: production may hold
/// them.
mod v1 {
    use meeting_core::{MeetingId, Millis, TranscriptRef};
    use serde::Deserialize;

    /// An entry of the bare array. Entries from before the rooms answered also carry `liveAt`,
    /// which nothing reads.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Entry {
        pub meeting: MeetingId,
        pub room: String,
        pub created_at: Millis,
        pub transcript: TranscriptRef,
        #[serde(default)]
        pub ended_at: Option<Millis>,
    }
}

impl From<v1::Entry> for Entry {
    fn from(old: v1::Entry) -> Self {
        Self {
            meeting: old.meeting,
            room: RoomId::new(old.room),
            created_at: old.created_at,
            transcript: old.transcript,
            ended: old.ended_at.map(|at| Ended {
                at,
                recorded: Recorded::Unknown,
            }),
        }
    }
}

/// The stored JSON of `index`.
pub fn encode(index: &[Entry]) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        version: INDEX_VERSION,
        record: index,
    })
}

/// Reads a stored list of any version up to [`INDEX_VERSION`].
pub fn decode(json: &str) -> std::result::Result<Vec<Entry>, DecodeError> {
    // Version 1 is a bare array, which has no `version` field to read.
    if json.trim_start().starts_with('[') {
        let entries: Vec<v1::Entry> = serde_json::from_str(json)?;
        return Ok(entries.into_iter().map(Entry::from).collect());
    }
    match stored_version(json, INDEX_VERSION)? {
        INDEX_VERSION => stored_record(json),
        other => Err(DecodeError::Malformed(serde::de::Error::custom(format!(
            "a stored meeting list has no version {other}"
        )))),
    }
}

/// The list with `entry` appended, keeping the newest [`MAX_ENTRIES`].
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

/// What a listing learned about a meeting it found ended: whether its transcript link exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenEnded {
    pub room: RoomId,
    pub linked: bool,
}

/// The list with what a listing at `now` learned: the meetings in `seen` are ended (from `now`,
/// unless already marked), and recorded if their link exists, or not if it is still missing
/// [`LINK_SETTLE_MS`] after the end.
pub fn settled(mut index: Vec<Entry>, seen: &[SeenEnded], now: Millis) -> Vec<Entry> {
    for entry in &mut index {
        let Some(seen) = seen.iter().find(|seen| seen.room == entry.room) else {
            continue;
        };
        let ended = entry.ended.get_or_insert(Ended {
            at: now,
            recorded: Recorded::Unknown,
        });
        if ended.recorded == Recorded::Unknown {
            ended.recorded = if seen.linked {
                Recorded::Yes
            } else if now >= ended.at.after(LINK_SETTLE_MS) {
                Recorded::No
            } else {
                Recorded::Unknown
            };
        }
    }
    index
}

/// Whether a listing should look for the transcript link of `entry`, whose room is in `phase`.
pub fn wants_link_check(entry: &Entry, phase: Phase) -> bool {
    phase == Phase::Ended
        && entry
            .ended
            .is_none_or(|ended| ended.recorded == Recorded::Unknown)
}

/// The answer for entries in list order, each with the phase its room reported, or ended if the
/// entry is marked so. Only a meeting known to have recorded gets its transcript link.
pub fn listing(entries: impl IntoIterator<Item = (Entry, Phase)>) -> Listing {
    Listing {
        meetings: entries
            .into_iter()
            .map(|(entry, reported)| PastMeeting {
                phase: if entry.ended.is_some() {
                    Phase::Ended
                } else {
                    reported
                },
                transcript: entry
                    .ended
                    .is_some_and(|ended| ended.recorded == Recorded::Yes)
                    .then_some(entry.transcript),
                meeting: entry.meeting,
                created_at: entry.created_at,
            })
            .collect(),
    }
}

/// The stored list; empty if none was stored yet.
pub async fn load(kv: &KvStore) -> Result<Vec<Entry>> {
    match kv.get(KEY).text().await? {
        None => Ok(Vec::new()),
        Some(json) => Ok(decode(&json)?),
    }
}

/// Reads the list, applies `change` and stores the result if it differs. Not atomic; see the
/// module docs.
pub async fn update(kv: &KvStore, change: impl FnOnce(Vec<Entry>) -> Vec<Entry>) -> Result<()> {
    let index = load(kv).await?;
    let changed = change(index.clone());
    if changed != index {
        kv.put(KEY, encode(&changed)?)?.execute().await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::record::tests::fixtures;

    const NOW: Millis = Millis(1_760_000_000_000);

    fn entry(n: u64) -> Entry {
        Entry {
            meeting: MeetingId::from_random([0, 1, 2]),
            room: RoomId::new(format!("room{n}")),
            created_at: Millis(n),
            transcript: TranscriptRef::from_random([n as u8; 16]),
            ended: None,
        }
    }

    fn ended(n: u64, at: Millis, recorded: Recorded) -> Entry {
        Entry {
            ended: Some(Ended { at, recorded }),
            ..entry(n)
        }
    }

    fn seen(n: u64, linked: bool) -> SeenEnded {
        SeenEnded {
            room: RoomId::new(format!("room{n}")),
            linked,
        }
    }

    /// The lists `fixtures/index/current/` freezes.
    fn fixture_lists() -> [(&'static str, Vec<Entry>); 1] {
        let room = |c: char| RoomId::new(c.to_string().repeat(64));
        let token = |c: char| TranscriptRef::parse(&c.to_string().repeat(32)).unwrap();
        let words = |w| MeetingId::parse(w).expect("listed words");
        [(
            "every-state.json",
            vec![
                Entry {
                    meeting: words("amber-otter-ocean"),
                    room: room('a'),
                    created_at: NOW,
                    transcript: token('1'),
                    ended: Some(Ended {
                        at: NOW.after(1),
                        recorded: Recorded::Yes,
                    }),
                },
                Entry {
                    meeting: words("abide-acid-acorn"),
                    room: room('b'),
                    created_at: NOW.after(2),
                    transcript: token('2'),
                    ended: Some(Ended {
                        at: NOW.after(3),
                        recorded: Recorded::No,
                    }),
                },
                Entry {
                    meeting: words("amber-otter-ocean"),
                    room: room('c'),
                    created_at: NOW.after(4),
                    transcript: token('3'),
                    ended: Some(Ended {
                        at: NOW.after(5),
                        recorded: Recorded::Unknown,
                    }),
                },
                Entry {
                    meeting: words("abide-acid-acorn"),
                    room: room('d'),
                    created_at: NOW.after(6),
                    transcript: token('4'),
                    ended: None,
                },
            ],
        )]
    }

    #[test]
    fn the_stored_list_keeps_its_shape() {
        for (name, index) in fixture_lists() {
            let stored = encode(&index).unwrap();
            let fixture = fixtures::read("index", "current")
                .into_iter()
                .find(|(file, _)| file == name)
                .unwrap_or_else(|| panic!("no fixture {name}"))
                .1;
            assert!(
                fixtures::same_json(&fixture, &stored),
                "{name} changed; see `crate::record` tests"
            );
            assert_eq!(decode(&fixture).unwrap(), index, "{name}");
        }
    }

    /// Production holds version 1 lists: a bare array whose ended entries carry `endedAt`, and
    /// whose entries from before the rooms answered also carry `liveAt`. Each loads with its
    /// ended mark and an unknown recording, which the next listing looks up.
    #[test]
    fn older_lists_still_load() {
        for (name, json) in fixtures::read("index", "older") {
            assert_eq!(fixtures::version_in_name(&name), 1, "{name}");
            let index = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(index.len(), 3, "{name}");
            let marks: Vec<_> = index.iter().map(|entry| entry.ended).collect();
            assert_eq!(
                marks,
                [
                    Some(Ended {
                        at: Millis(1_760_000_200_000),
                        recorded: Recorded::Unknown
                    }),
                    Some(Ended {
                        at: Millis(1_760_100_500_000),
                        recorded: Recorded::Unknown
                    }),
                    None,
                ],
                "{name}"
            );
            assert_eq!(index[0].room.as_str(), "a".repeat(64), "{name}");
        }
    }

    #[test]
    fn a_list_from_a_newer_version_is_an_error() {
        assert!(matches!(
            decode(r#"{"version":3,"record":[]}"#),
            Err(DecodeError::Newer {
                found: 3,
                supported: INDEX_VERSION
            })
        ));
        assert!(decode(r#"{"record":[]}"#).is_err(), "no version 1 object");
    }

    #[test]
    fn settling_marks_the_end_once_and_the_recording_when_known() {
        let index = vec![entry(1), entry(2), entry(3)];
        let index = settled(index, &[seen(2, true), seen(3, false)], NOW);
        assert_eq!(index[0].ended, None);
        assert_eq!(
            (index[1].ended, index[2].ended),
            (
                Some(Ended {
                    at: NOW,
                    recorded: Recorded::Yes
                }),
                Some(Ended {
                    at: NOW,
                    recorded: Recorded::Unknown
                })
            )
        );
        // The link is still missing: unknown until the settle time has passed, then no.
        let later = NOW.after(LINK_SETTLE_MS - 1);
        let index = settled(index, &[seen(2, false), seen(3, false)], later);
        assert_eq!(index[1].ended.map(|e| e.recorded), Some(Recorded::Yes));
        assert_eq!(index[2].ended.map(|e| e.recorded), Some(Recorded::Unknown));
        let index = settled(index, &[seen(3, false)], NOW.after(LINK_SETTLE_MS));
        assert_eq!(
            index[2].ended,
            Some(Ended {
                at: NOW,
                recorded: Recorded::No
            }),
            "the first end stands"
        );
        assert!(!wants_link_check(&index[2], Phase::Ended));
        assert!(wants_link_check(&entry(4), Phase::Ended));
        assert!(!wants_link_check(&entry(4), Phase::Live));
    }

    #[test]
    fn only_a_meeting_known_to_have_recorded_lists_its_transcript() {
        let json = serde_json::to_value(listing([
            (entry(5), Phase::Live),
            (entry(4), Phase::Scheduled),
            (ended(3, NOW, Recorded::Yes), Phase::Ended),
            (ended(2, NOW, Recorded::Unknown), Phase::Ended),
            (ended(1, NOW, Recorded::No), Phase::Live),
        ]))
        .unwrap();
        let meeting = MeetingId::from_random([0, 1, 2]);
        assert_eq!(
            json,
            json!({ "meetings": [
                { "meeting": meeting, "createdAt": 5, "phase": "live" },
                { "meeting": meeting, "createdAt": 4, "phase": "scheduled" },
                { "meeting": meeting, "createdAt": 3, "phase": "ended", "transcript": "03".repeat(16) },
                { "meeting": meeting, "createdAt": 2, "phase": "ended" },
                { "meeting": meeting, "createdAt": 1, "phase": "ended" },
            ]})
        );
    }

    #[test]
    fn the_list_keeps_the_newest_entries_and_lists_them_newest_first() {
        let index = (0..MAX_ENTRIES as u64 + 5).fold(Vec::new(), |index, n| added(index, entry(n)));
        assert_eq!(index.len(), MAX_ENTRIES);
        assert_eq!(index[0].created_at, Millis(5), "the oldest dropped out");
        let listed = newest(index);
        assert_eq!(listed.len(), MAX_ENTRIES);
        assert_eq!(listed[0].created_at, Millis(MAX_ENTRIES as u64 + 4));
        assert!(listed.windows(2).all(|w| w[0].created_at > w[1].created_at));
    }

    /// Writes the `current/` fixtures from [`fixture_lists`]; see `crate::record::tests::fixtures`
    /// for when.
    #[test]
    #[ignore = "writes fixtures; run on purpose"]
    fn capture() {
        for (name, index) in fixture_lists() {
            fixtures::capture("index", name, &encode(&index).unwrap());
        }
    }
}
