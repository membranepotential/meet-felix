//! The recorder's media bookkeeping in the Durable Object's SQLite database: chunks and their
//! state, the spool of encoded packets not yet in R2, speech segments, and paragraphs. SQL
//! calls are synchronous, so the WebSocket handler stores packets without awaiting anything,
//! and the statements of one handler between two awaits commit together.
//!
//! Rows written count against the Workers Free allowance of 100,000 a day (deletes included):
//! one spool row per speaker every 1.5 s, deleted again once its chunk is in R2, plus a few
//! writes per 30-second chunk, about 5,000 per speaker-hour.

use meeting_core::transcript::{Paragraph, Segment};
use meeting_core::{Millis, ParticipantId};
use serde::Deserialize;
use worker::{Result, SqlStorage, SqlStorageValue, console_error};

use meeting_core::transcript::GapReason;

use super::state::{
    ChunkSpan, MAX_CHUNK_ATTEMPTS, MAX_UPLOAD_ATTEMPTS, Progress, SegmentCursor, upload_backoff,
};

/// The schema version this code reads and writes. Durable Objects refuse `PRAGMA user_version`
/// (workerd allows only the pragmas in `ALLOWED_PRAGMAS`, checked 2026-10-11), so the version
/// lives in the table `schema_version`; a database without it predates the table and has
/// version 1.
const SCHEMA_VERSION: u32 = 1;

/// The statements that bring a database of version `n` to `n + 1`, at index `n - 1`. Its
/// length follows from [`SCHEMA_VERSION`], so raising the version without a migration fails to
/// compile. A migration must keep every existing row readable: a recorder's tables hold its
/// transcript job and the paragraphs until the transcript is written.
const MIGRATIONS: [&str; SCHEMA_VERSION as usize - 1] = [];

/// The tables as version 1 created them. Never change this: a new database is created at
/// version 1 and brought up to date by [`MIGRATIONS`], like every older one.
const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS chunks (
    who INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    silent INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    uploads INTEGER NOT NULL DEFAULT 0,
    upload_after INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (who, generation, idx)
);
CREATE TABLE IF NOT EXISTS spool (
    id INTEGER PRIMARY KEY,
    who INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    packets BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS segments (
    id INTEGER PRIMARY KEY,
    who INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS paragraphs (
    id INTEGER PRIMARY KEY,
    who INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);
";

/// The migrations a database of version `stored` still needs, or the newer version if newer
/// code wrote it.
fn pending_migrations(stored: u32) -> std::result::Result<&'static [&'static str], u32> {
    if stored > SCHEMA_VERSION {
        return Err(stored);
    }
    Ok(&MIGRATIONS[stored.saturating_sub(1) as usize..])
}

/// A chunk: speaker, adapter generation, and its number within that adapter's stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkId {
    pub who: ParticipantId,
    pub generation: u32,
    pub index: u32,
}

/// Where a chunk stands. Each state is set once, in this order, except that a chunk stays
/// `Closed` until its upload succeeds or fails [`MAX_UPLOAD_ATTEMPTS`] times, and `Stored` until
/// the speech model succeeds or fails [`MAX_CHUNK_ATTEMPTS`] times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChunkState {
    /// Being recorded; its packets go to the spool.
    Open,
    /// Complete in the spool, waiting for its R2 upload.
    Closed,
    /// In R2; the spool rows are gone.
    Stored,
    /// Transcribed; its segments are stored.
    Done,
    /// Silent, or without any audio; not sent to the speech model.
    Skipped,
    /// Its upload failed [`MAX_UPLOAD_ATTEMPTS`] times; the transcript shows it as missing.
    Lost,
    /// The speech model failed on it [`MAX_CHUNK_ATTEMPTS`] times.
    Failed,
}

/// A state as the `state` column holds it. Every statement binds states through this, so the
/// column holds only the names [`ChunkState`] reads back.
impl From<ChunkState> for SqlStorageValue {
    fn from(state: ChunkState) -> Self {
        let name = match state {
            ChunkState::Open => "open",
            ChunkState::Closed => "closed",
            ChunkState::Stored => "stored",
            ChunkState::Done => "done",
            ChunkState::Skipped => "skipped",
            ChunkState::Lost => "lost",
            ChunkState::Failed => "failed",
        };
        name.into()
    }
}

#[derive(Deserialize)]
struct ChunkUploads {
    uploads: u32,
}

/// A row of `chunks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct ChunkRow {
    who: u32,
    generation: u32,
    idx: u32,
    pub start_ms: u64,
    pub duration_ms: u64,
    silent: u8,
    pub state: ChunkState,
    /// Before this time, the next upload attempt waits.
    pub upload_after: u64,
}

impl ChunkRow {
    pub fn id(&self) -> ChunkId {
        ChunkId {
            who: ParticipantId(self.who),
            generation: self.generation,
            index: self.idx,
        }
    }

    pub fn silent(&self) -> bool {
        self.silent != 0
    }
}

pub struct Store(SqlStorage);

fn int(value: impl Into<i64>) -> SqlStorageValue {
    SqlStorageValue::Integer(value.into())
}

/// Milliseconds as an SQL integer; times since 1970 fit easily.
fn ms(value: u64) -> SqlStorageValue {
    int(i64::try_from(value).unwrap_or(i64::MAX))
}

fn key(id: ChunkId) -> Vec<SqlStorageValue> {
    vec![int(id.who.0), int(id.generation), int(id.index)]
}

const WHERE_CHUNK: &str = "who = ? AND generation = ? AND idx = ?";

impl Store {
    /// Opens the database: creates the version 1 tables on first use and runs the migrations
    /// the database still needs. A failure is logged; every later query then fails and is
    /// logged too.
    pub fn open(sql: SqlStorage) -> Self {
        let store = Self(sql);
        if let Err(e) = store.migrate() {
            console_error!("recorder: preparing the tables failed: {e}");
        }
        store
    }

    /// Brings the tables to [`SCHEMA_VERSION`]. The migrations and the new version go in one
    /// `exec`, which the SQL API docs say is wrapped in an implicit transaction, so they commit
    /// together.
    fn migrate(&self) -> Result<()> {
        #[derive(Deserialize)]
        struct Row {
            version: u32,
        }
        self.0.exec(SCHEMA_V1, None)?;
        let rows: Vec<Row> = self
            .0
            .exec("SELECT version FROM schema_version", None)?
            .to_array()?;
        let stored = rows.first().map(|row| row.version);
        let pending = pending_migrations(stored.unwrap_or(1)).map_err(|newer| {
            worker::Error::RustError(format!(
                "the tables have version {newer}, newer than this code's {SCHEMA_VERSION}"
            ))
        })?;
        if stored == Some(SCHEMA_VERSION) {
            return Ok(());
        }
        let statements = pending.concat();
        self.0.exec(
            &format!(
                "{statements}
                 DELETE FROM schema_version;
                 INSERT INTO schema_version (version) VALUES ({SCHEMA_VERSION});"
            ),
            None,
        )?;
        Ok(())
    }

    fn exec(&self, query: &str, bindings: Vec<SqlStorageValue>) -> Result<()> {
        self.0.exec(query, bindings).map(drop)
    }

    pub fn chunk_started(&self, id: ChunkId, start: Millis) -> Result<()> {
        let mut values = key(id);
        values.extend([ms(start.0), ChunkState::Open.into()]);
        self.exec(
            "INSERT OR REPLACE INTO chunks (who, generation, idx, start_ms, state)
             VALUES (?, ?, ?, ?, ?)",
            values,
        )
    }

    pub fn spool(&self, id: ChunkId, packets: Vec<u8>) -> Result<()> {
        let mut values = key(id);
        values.push(SqlStorageValue::Blob(packets));
        self.exec(
            "INSERT INTO spool (who, generation, idx, packets) VALUES (?, ?, ?, ?)",
            values,
        )
    }

    pub fn chunk_closed(&self, id: ChunkId, duration_ms: u64, silent: bool) -> Result<()> {
        let mut values = vec![
            ms(duration_ms),
            int(i64::from(silent)),
            ChunkState::Closed.into(),
        ];
        values.extend(key(id));
        self.exec(
            &format!(
                "UPDATE chunks SET duration_ms = ?, silent = ?, state = ? WHERE {WHERE_CHUNK}"
            ),
            values,
        )
    }

    /// The number the next chunk of this adapter stream gets.
    pub fn next_index(&self, who: ParticipantId, generation: u32) -> Result<u32> {
        #[derive(Deserialize)]
        struct Next {
            next: u32,
        }
        let rows: Vec<Next> = self
            .0
            .exec(
                "SELECT COALESCE(MAX(idx) + 1, 0) AS next FROM chunks
                 WHERE who = ? AND generation = ?",
                vec![int(who.0), int(generation)],
            )?
            .to_array()?;
        Ok(rows.first().map_or(0, |row| row.next))
    }

    /// All chunks, ordered by start.
    pub fn chunks(&self) -> Result<Vec<ChunkRow>> {
        self.0
            .exec("SELECT * FROM chunks ORDER BY start_ms", None)?
            .to_array()
    }

    /// Up to `limit` chunks in `state`, ordered by start.
    pub fn chunks_in(&self, state: ChunkState, limit: u32) -> Result<Vec<ChunkRow>> {
        self.0
            .exec(
                "SELECT * FROM chunks WHERE state = ? ORDER BY start_ms LIMIT ?",
                vec![state.into(), int(limit)],
            )?
            .to_array()
    }

    /// The chunk's spooled packet batches, in order.
    pub fn spooled(&self, id: ChunkId) -> Result<Vec<Vec<u8>>> {
        self.0
            .exec(
                &format!("SELECT packets FROM spool WHERE {WHERE_CHUNK} ORDER BY id"),
                key(id),
            )?
            .raw()
            .map(|row| match row?.into_iter().next() {
                Some(SqlStorageValue::Blob(bytes)) => Ok(bytes),
                _ => Err(worker::Error::from("spool row without packets")),
            })
            .collect()
    }

    /// Closes a chunk whose recording was cut off by an eviction or a deploy: its length is
    /// what reached the spool. Its silence is unknown, so it counts as speech; a chunk with
    /// nothing in its spool is skipped, never uploaded or transcribed.
    pub fn close_orphan(&self, id: ChunkId, duration_ms: u64) -> Result<()> {
        if duration_ms == 0 {
            self.skip_empty(id)
        } else {
            self.chunk_closed(id, duration_ms, false)
        }
    }

    /// Marks a chunk without audio as silent and skipped.
    pub fn skip_empty(&self, id: ChunkId) -> Result<()> {
        let mut values = vec![ChunkState::Skipped.into()];
        values.extend(key(id));
        self.exec(
            &format!("UPDATE chunks SET silent = 1, state = ? WHERE {WHERE_CHUNK}"),
            values,
        )
    }

    /// Counts a failed upload: the next waits [`upload_backoff`], and after
    /// [`MAX_UPLOAD_ATTEMPTS`] the chunk is lost.
    pub fn upload_failed(&self, id: ChunkId, now: Millis) -> Result<()> {
        let rows: Vec<ChunkUploads> = self
            .0
            .exec(
                &format!("SELECT uploads FROM chunks WHERE {WHERE_CHUNK}"),
                key(id),
            )?
            .to_array()?;
        let failures = rows.first().map_or(0, |r| r.uploads) + 1;
        let state = if failures >= MAX_UPLOAD_ATTEMPTS {
            ChunkState::Lost
        } else {
            ChunkState::Closed
        };
        let mut values = vec![
            int(failures),
            ms(now.after(upload_backoff(failures)).0),
            state.into(),
        ];
        values.extend(key(id));
        self.exec(
            &format!(
                "UPDATE chunks SET uploads = ?, upload_after = ?, state = ? WHERE {WHERE_CHUNK}"
            ),
            values,
        )
    }

    /// When the last recorded audio ends, if any was recorded.
    pub fn audio_end(&self) -> Result<Option<Millis>> {
        #[derive(Deserialize)]
        struct End {
            end: Option<u64>,
        }
        let rows: Vec<End> = self
            .0
            .exec(
                "SELECT MAX(start_ms + duration_ms) AS end FROM chunks",
                None,
            )?
            .to_array()?;
        Ok(rows.first().and_then(|r| r.end).map(Millis))
    }

    /// The chunk is in R2: its spool rows go.
    pub fn chunk_stored(&self, id: ChunkId) -> Result<()> {
        self.exec(&format!("DELETE FROM spool WHERE {WHERE_CHUNK}"), key(id))?;
        self.set_state(id, ChunkState::Stored)
    }

    fn set_state(&self, id: ChunkId, state: ChunkState) -> Result<()> {
        let mut values = vec![state.into()];
        values.extend(key(id));
        self.exec(
            &format!("UPDATE chunks SET state = ? WHERE {WHERE_CHUNK}"),
            values,
        )
    }

    /// Marks stored silent chunks as skipped.
    pub fn skip_silent(&self) -> Result<()> {
        self.exec(
            "UPDATE chunks SET state = ? WHERE state = ? AND silent = 1",
            vec![ChunkState::Skipped.into(), ChunkState::Stored.into()],
        )
    }

    /// Stores a chunk's segments and marks it done, in one commit.
    pub fn transcribed(&self, id: ChunkId, segments: &[Segment]) -> Result<()> {
        for segment in segments {
            self.exec(
                "INSERT INTO segments (who, start_ms, end_ms, text) VALUES (?, ?, ?, ?)",
                vec![
                    int(segment.who.0),
                    ms(segment.start.0),
                    ms(segment.end.0),
                    segment.text.as_str().into(),
                ],
            )?;
        }
        self.set_state(id, ChunkState::Done)
    }

    /// Counts a failed attempt; after [`MAX_CHUNK_ATTEMPTS`] the chunk is failed for good.
    pub fn attempt_failed(&self, id: ChunkId) -> Result<()> {
        let mut values = vec![int(MAX_CHUNK_ATTEMPTS), ChunkState::Failed.into()];
        values.extend(key(id));
        self.exec(
            &format!(
                "UPDATE chunks SET attempts = attempts + 1,
                 state = CASE WHEN attempts + 1 >= ? THEN ? ELSE state END
                 WHERE {WHERE_CHUNK}"
            ),
            values,
        )
    }

    /// Gives a chunk up: the transcript shows it as not transcribed.
    pub fn give_up(&self, id: ChunkId) -> Result<()> {
        self.set_state(id, ChunkState::Failed)
    }

    /// Milliseconds of recorded audio the job has finished, of all recorded so far.
    pub fn progress(&self) -> Result<Progress> {
        let rows: Vec<Progress> = self
            .0
            .exec(
                "SELECT COALESCE(SUM(duration_ms), 0) AS total,
                 COALESCE(SUM(CASE WHEN state IN (?, ?, ?, ?)
                     THEN duration_ms ELSE 0 END), 0) AS done
                 FROM chunks",
                [
                    ChunkState::Done,
                    ChunkState::Skipped,
                    ChunkState::Failed,
                    ChunkState::Lost,
                ]
                .map(SqlStorageValue::from)
                .to_vec(),
            )?
            .to_array()?;
        Ok(rows
            .first()
            .copied()
            .unwrap_or(Progress { done: 0, total: 0 }))
    }

    /// Up to `limit` segments after `cursor` in merge order (start, then id), with the cursor
    /// of the last one.
    pub fn segments_after(
        &self,
        cursor: Option<SegmentCursor>,
        limit: u32,
    ) -> Result<Vec<(SegmentCursor, Segment)>> {
        #[derive(Deserialize)]
        struct Row {
            id: i64,
            who: u32,
            start_ms: u64,
            end_ms: u64,
            text: String,
        }
        let cursor = cursor.unwrap_or(SegmentCursor {
            start_ms: 0,
            id: -1,
        });
        let rows: Vec<Row> = self
            .0
            .exec(
                "SELECT * FROM segments WHERE start_ms > ?1 OR (start_ms = ?1 AND id > ?2)
                 ORDER BY start_ms, id LIMIT ?3",
                vec![ms(cursor.start_ms), int(cursor.id), int(limit)],
            )?
            .to_array()?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (
                    SegmentCursor {
                        start_ms: row.start_ms,
                        id: row.id,
                    },
                    Segment {
                        who: ParticipantId(row.who),
                        start: Millis(row.start_ms),
                        end: Millis(row.end_ms),
                        text: row.text,
                    },
                )
            })
            .collect())
    }

    /// All paragraphs, in order.
    pub fn paragraphs(&self) -> Result<Vec<Paragraph>> {
        #[derive(Deserialize)]
        struct Row {
            who: u32,
            start_ms: u64,
            end_ms: u64,
            text: String,
        }
        let rows: Vec<Row> = self
            .0
            .exec("SELECT * FROM paragraphs ORDER BY id", None)?
            .to_array()?;
        Ok(rows
            .into_iter()
            .map(|row| Paragraph {
                who: ParticipantId(row.who),
                start: Millis(row.start_ms),
                end: Millis(row.end_ms),
                text: row.text,
            })
            .collect())
    }

    /// Removes and returns the last paragraph, which the next merge batch may extend.
    pub fn take_last_paragraph(&self) -> Result<Option<Paragraph>> {
        #[derive(Deserialize)]
        struct Row {
            id: i64,
            who: u32,
            start_ms: u64,
            end_ms: u64,
            text: String,
        }
        let rows: Vec<Row> = self
            .0
            .exec("SELECT * FROM paragraphs ORDER BY id DESC LIMIT 1", None)?
            .to_array()?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        self.exec("DELETE FROM paragraphs WHERE id = ?", vec![int(row.id)])?;
        Ok(Some(Paragraph {
            who: ParticipantId(row.who),
            start: Millis(row.start_ms),
            end: Millis(row.end_ms),
            text: row.text,
        }))
    }

    pub fn append_paragraphs(&self, paragraphs: &[Paragraph]) -> Result<()> {
        for p in paragraphs {
            self.exec(
                "INSERT INTO paragraphs (who, start_ms, end_ms, text) VALUES (?, ?, ?, ?)",
                vec![
                    int(p.who.0),
                    ms(p.start.0),
                    ms(p.end.0),
                    p.text.as_str().into(),
                ],
            )?;
        }
        Ok(())
    }

    /// Every chunk as the gap rule needs it.
    pub fn spans(&self) -> Result<Vec<ChunkSpan>> {
        Ok(self
            .chunks()?
            .into_iter()
            .map(|row| ChunkSpan {
                who: ParticipantId(row.who),
                generation: row.generation,
                start: Millis(row.start_ms),
                duration_ms: row.duration_ms,
                lacking: match row.state {
                    ChunkState::Failed => Some(GapReason::Untranscribed),
                    ChunkState::Lost => Some(GapReason::Missing),
                    _ => None,
                },
            })
            .collect())
    }

    /// Drops what is left of the spool once the audio is deleted.
    pub fn clear_spool(&self) -> Result<()> {
        self.exec("DELETE FROM spool", Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_gets_the_migrations_after_its_version_and_a_newer_one_none() {
        // Version 1 is every database created before the version table, and every new one.
        assert_eq!(pending_migrations(1), Ok(&MIGRATIONS[..]));
        assert_eq!(pending_migrations(SCHEMA_VERSION), Ok(&[][..]));
        assert_eq!(
            pending_migrations(SCHEMA_VERSION + 1),
            Err(SCHEMA_VERSION + 1)
        );
    }
}
