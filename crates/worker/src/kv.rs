//! The `MEETINGS` KV namespace: the one module that names its keys and the shapes of their
//! values. Three kinds of key share the namespace, and their spellings cannot collide:
//!
//! | key                  | value                          | written by                       |
//! |----------------------|--------------------------------|----------------------------------|
//! | meeting words        | the room's [`RoomId`], expires | creation; the room deletes it    |
//! | `transcript:<token>` | the recorder's [`RecorderId`]  | the room, when recording starts  |
//! | `index`              | the owner's meeting list (JSON, versioned by [`crate::meetings`]) | creation, listing |
//!
//! Meeting words always contain `-`, a transcript key contains `:`, and `index` contains
//! neither. The stored spellings are what production holds; do not change them.
//!
//! KV is eventually consistent: a read may miss a write from another location for 60 seconds,
//! and a lookup of a missing key is cached as well.

use meeting_core::{MeetingId, TranscriptRef};
use worker::{Env, KvStore, Result};

use crate::internal::{RecorderId, RoomId};

/// The binding's name in `wrangler.toml`.
const NAMESPACE: &str = "MEETINGS";
/// The key of the owner's meeting list.
const INDEX: &str = "index";

/// The namespace of `env`.
pub fn open(env: &Env) -> Result<KvStore> {
    env.kv(NAMESPACE)
}

/// The key that maps a transcript token to its recorder.
fn transcript(token: &TranscriptRef) -> String {
    format!("transcript:{}", token.as_str())
}

/// The room `words` map to, if they are in use.
pub async fn room_of(kv: &KvStore, words: &MeetingId) -> Result<Option<RoomId>> {
    Ok(kv.get(words.as_str()).text().await?.map(RoomId::new))
}

/// Maps `words` to `room` for `ttl_secs`. Not atomic with a preceding [`room_of`].
pub async fn map_words(
    kv: &KvStore,
    words: &MeetingId,
    room: &RoomId,
    ttl_secs: u64,
) -> Result<()> {
    kv.put(words.as_str(), room.as_str())?
        .expiration_ttl(ttl_secs)
        .execute()
        .await?;
    Ok(())
}

/// Frees `words` if they still map to `room`; after the TTL they may belong to another meeting.
pub async fn release_words(kv: &KvStore, words: &MeetingId, room: &RoomId) -> Result<()> {
    if room_of(kv, words).await?.as_ref() == Some(room) {
        kv.delete(words.as_str()).await?;
    }
    Ok(())
}

/// The recorder `token` leads to, if its link exists.
pub async fn recorder_of(kv: &KvStore, token: &TranscriptRef) -> Result<Option<RecorderId>> {
    Ok(kv
        .get(&transcript(token))
        .text()
        .await?
        .map(RecorderId::new))
}

/// Links `token` to `recorder`. The link has no expiry: transcripts stay until the owner
/// deletes them (D18).
pub async fn link_transcript(
    kv: &KvStore,
    token: &TranscriptRef,
    recorder: &RecorderId,
) -> Result<()> {
    kv.put(&transcript(token), recorder.as_str())?
        .execute()
        .await?;
    Ok(())
}

/// The stored meeting list, as JSON.
pub async fn read_index(kv: &KvStore) -> Result<Option<String>> {
    Ok(kv.get(INDEX).text().await?)
}

/// Replaces the stored meeting list.
pub async fn write_index(kv: &KvStore, json: &str) -> Result<()> {
    kv.put(INDEX, json)?.execute().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token() -> TranscriptRef {
        TranscriptRef::parse(&"a".repeat(32)).expect("a token")
    }

    #[test]
    fn the_three_kinds_of_key_cannot_collide() {
        let words = MeetingId::parse("amber-otter-ocean").expect("listed words");
        assert!(words.as_str().contains('-') && !words.as_str().contains(':'));
        assert!(transcript(&token()).contains(':') && !transcript(&token()).contains('-'));
        assert!(!INDEX.contains(['-', ':']));
    }

    #[test]
    fn the_stored_spellings_stay_what_production_holds() {
        assert_eq!(
            transcript(&token()),
            format!("transcript:{}", "a".repeat(32))
        );
        assert_eq!(INDEX, "index");
        assert_eq!(NAMESPACE, "MEETINGS");
    }
}
