//! What a `RecorderObject` keeps in its key-value storage, the status it reports, and the pure
//! rules of its transcript job: retry times, error classes, R2 keys, gaps.

use meeting_core::recording::{ConsentRecord, RosterEntry};
use meeting_core::transcript::{Gap, GapReason};
use meeting_core::{MeetingId, Millis, ParticipantId, SessionId, TrackName};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sfu_api::wire::AdapterId;

/// The recorder's storage key.
pub const KEY: &str = "recorder";

/// How long the audio stays after the meeting ended (D18).
pub const AUDIO_RETENTION_MS: u64 = 48 * 60 * 60 * 1000;
/// Job steps (merging, writing the transcript) fail this often in a row before the transcript
/// counts as failed. Quota errors from Workers AI never count.
pub const MAX_STEP_FAILURES: u32 = 6;
/// A chunk the speech model fails on this often is left out, marked as a gap.
pub const MAX_CHUNK_ATTEMPTS: u32 = 3;

/// Everything the recorder remembers besides the media in SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorder {
    pub meeting: MeetingId,
    /// When recording first started.
    pub started_at: Millis,
    /// The token in the adapters' endpoint URLs (128 bits, hex). Never logged.
    pub secret: String,
    /// The latest name and role of everyone whose audio was recorded, and at the end everyone
    /// on the roster.
    pub speakers: Vec<RosterEntry>,
    /// The consent record: the speakers' consents as `/track` and `/untrack` carried them,
    /// replaced at the end by the room's whole record.
    #[serde(default)]
    pub consents: Vec<ConsentRecord>,
    pub adapters: Vec<Adapter>,
    pub phase: Phase,
}

/// One WebSocket adapter: one speaker's audio track, streamed to
/// `/rec/{recorder}/{secret}/{who}/{generation}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub who: ParticipantId,
    /// Counts the speaker's adapters from 0; a republished or replaced track gets the next one.
    pub generation: u32,
    pub session: SessionId,
    pub track_name: TrackName,
    /// The SFU's id, stored as soon as the SFU returns it; `None` while it is being created, or
    /// if creating it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<AdapterId>,
    /// Whether the recorder still accepts this adapter's stream.
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// The meeting runs; adapters come and go.
    Recording,
    /// The meeting ended at `at`; the transcript job runs or ran.
    #[serde(rename_all = "camelCase")]
    Ended { at: Millis, job: Job },
}

/// Where the transcript job stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Job {
    /// Uploading and transcribing chunks. With `retry_at`, Workers AI refused for its quota or
    /// rate limit and the job waits until then; `waiting_since` is when it first refused since
    /// the last chunk went through.
    #[serde(rename_all = "camelCase")]
    Transcribing {
        retry_at: Option<Millis>,
        #[serde(default)]
        waiting_since: Option<Millis>,
    },
    /// Merging segments into paragraphs, a batch per run, after `after`.
    #[serde(rename_all = "camelCase")]
    Merging {
        after: Option<SegmentCursor>,
        failures: u32,
    },
    /// The transcript is in R2. The audio is deleted at `audio_until`.
    #[serde(rename_all = "camelCase")]
    Ready {
        audio_until: Millis,
        audio_deleted_at: Option<Millis>,
    },
    /// The job gave up.
    #[serde(rename_all = "camelCase")]
    Failed { at: Millis },
}

/// The last merged segment, in merge order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentCursor {
    pub start_ms: u64,
    pub id: i64,
}

impl Recorder {
    /// The speaker's open adapter, if any.
    pub fn open_adapter(&self, who: ParticipantId) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.who == who && a.open)
    }

    /// The generation `who`'s next adapter gets.
    pub fn next_generation(&self, who: ParticipantId) -> u32 {
        self.adapters
            .iter()
            .filter(|a| a.who == who)
            .map(|a| a.generation + 1)
            .max()
            .unwrap_or(0)
    }

    /// Records `consent` as the speaker's current consent.
    pub fn record_consent(&mut self, consent: ConsentRecord) {
        match self.consents.iter_mut().find(|c| c.who == consent.who) {
            Some(known) => *known = consent,
            None => self.consents.push(consent),
        }
    }

    /// Records `entry`'s current name and role.
    pub fn name_speaker(&mut self, entry: RosterEntry) {
        match self.speakers.iter_mut().find(|s| s.id == entry.id) {
            Some(known) => *known = entry,
            None => self.speakers.push(entry),
        }
    }

    /// The prompt for the speech model: the participants' names, so they are spelled the same
    /// in every chunk (research 11).
    pub fn prompt(&self) -> String {
        let mut names: Vec<&str> = self.speakers.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        format!("Participants: {}.", names.join(", "))
    }
}

/// The status behind `GET /api/transcript/:token`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub status: StatusKind,
    pub meeting: MeetingId,
    /// When recording started.
    pub started_at: Millis,
    /// When the meeting ended, once it has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Millis>,
    pub progress: Progress,
    /// When the job continues after Workers AI refused for its quota.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<Millis>,
    /// Until when the audio is kept, once the transcript is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_until: Option<Millis>,
    /// `transcript.json`, once ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<Box<RawValue>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusKind {
    Recording,
    Processing,
    Ready,
    Failed,
}

/// Milliseconds of audio the job has finished (transcribed, skipped as silent, or given up
/// on), of all audio recorded so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

impl Phase {
    pub fn status(&self) -> (StatusKind, Option<Millis>, Option<Millis>) {
        match self {
            Self::Recording => (StatusKind::Recording, None, None),
            Self::Ended { job, .. } => match job {
                Job::Transcribing { retry_at, .. } => (StatusKind::Processing, *retry_at, None),
                Job::Merging { .. } => (StatusKind::Processing, None, None),
                Job::Ready { audio_until, .. } => (StatusKind::Ready, None, Some(*audio_until)),
                Job::Failed { .. } => (StatusKind::Failed, None, None),
            },
        }
    }
}

/// Why a speech request failed, as far as the job cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFailure {
    /// The daily allowance is used up (on Workers Free, 10,000 neurons a day, reset at 00:00
    /// UTC): wait for the reset.
    Quota,
    /// Too many requests (HTTP 429): wait an hour.
    RateLimited,
    /// Anything else: retry the chunk a few times.
    Other,
}

/// Classifies a Workers AI error by its text, since the binding throws plain errors whose
/// shape is undocumented. The caller logs the text, so the classes can be refined.
pub fn classify_ai_error(text: &str) -> AiFailure {
    let text = text.to_ascii_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if says(&[
        "neuron",
        "quota",
        "daily",
        "allocation",
        "free tier",
        "exceeded your",
    ]) {
        AiFailure::Quota
    } else if says(&["429", "rate limit", "too many requests"]) {
        AiFailure::RateLimited
    } else {
        AiFailure::Other
    }
}

const HOUR_MS: u64 = 60 * 60 * 1000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// When to try again after `failure` at `now`: five minutes after the next daily reset at
/// 00:00 UTC for the quota, an hour later for a rate limit.
pub fn quota_retry(failure: AiFailure, now: Millis) -> Option<Millis> {
    match failure {
        AiFailure::Quota => Some(Millis((now.0 / DAY_MS + 1) * DAY_MS + 5 * 60 * 1000)),
        AiFailure::RateLimited => Some(now.after(HOUR_MS)),
        AiFailure::Other => None,
    }
}

/// How long the job waits on Workers AI's quota without any chunk going through before it gives
/// up the waiting chunks like other failures.
pub const MAX_QUOTA_WAIT_MS: u64 = 7 * DAY_MS;

/// Uploads of a chunk fail this often before it is given up as missing audio.
pub const MAX_UPLOAD_ATTEMPTS: u32 = 5;

/// The wait before the next upload after the `failures`-th failed one: 2 s, 10 s, then 60 s.
pub fn upload_backoff(failures: u32) -> u64 {
    match failures {
        0 | 1 => 2_000,
        2 => 10_000,
        _ => 60_000,
    }
}

/// The wait after the `failures`-th failure in a row: 30 s, doubling, at most 30 minutes.
pub fn backoff(failures: u32) -> u64 {
    (30_000u64 << failures.min(6)).min(30 * 60 * 1000)
}

/// Whether `given` is `secret`, comparing in constant time.
pub fn secret_matches(secret: &str, given: &str) -> bool {
    secret.len() == given.len()
        && secret
            .bytes()
            .zip(given.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// The R2 key of a chunk. Audio and transcripts live under separate prefixes, so an R2
/// lifecycle rule on `audio/` can delete old audio without touching transcripts (D18).
pub fn chunk_key(recorder: &str, who: ParticipantId, generation: u32, index: u32) -> String {
    format!(
        "{}{who}/{generation}-{index}.ogg",
        audio_prefix(recorder),
        who = who.0
    )
}

/// The prefix of all of a recorder's audio.
pub fn audio_prefix(recorder: &str) -> String {
    format!("audio/{recorder}/")
}

/// The R2 key of a transcript file ([`crate::internal::TRANSCRIPT_MD`] or `..._JSON`).
pub fn transcript_key(recorder: &str, file: &str) -> String {
    format!("transcripts/{recorder}/{file}")
}

/// A chunk as the gap rule needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSpan {
    pub who: ParticipantId,
    pub generation: u32,
    pub start: Millis,
    pub duration_ms: u64,
    /// Why the transcript lacks this chunk, if it does: its upload or its transcription
    /// failed for good.
    pub lacking: Option<GapReason>,
}

/// A hole between two chunks of one adapter stream shorter than this is not reported.
pub const MIN_GAP_MS: u64 = 1_000;

/// The audio the transcript lacks: holes of at least [`MIN_GAP_MS`] between consecutive chunks
/// of one adapter stream, and chunks that never reached R2 or the speech model failed on. Time between two adapters of a
/// speaker is not a gap: the speaker had stopped publishing.
pub fn gaps(mut chunks: Vec<ChunkSpan>) -> Vec<Gap> {
    chunks.sort_by_key(|c| (c.who, c.generation, c.start));
    let holes = chunks.windows(2).filter_map(|pair| {
        let [a, b] = pair else { return None };
        let end = a.start.after(a.duration_ms);
        (a.who == b.who && a.generation == b.generation && b.start.0 >= end.0 + MIN_GAP_MS)
            .then_some(Gap {
                who: a.who,
                start: end,
                end: b.start,
                reason: GapReason::Missing,
            })
    });
    let failed = chunks.iter().filter_map(|c| {
        Some(Gap {
            who: c.who,
            start: c.start,
            end: c.start.after(c.duration_ms),
            reason: c.lacking?,
        })
    });
    holes.chain(failed).collect()
}

#[cfg(test)]
mod tests {
    use meeting_core::{DisplayName, Role};

    use super::*;

    #[test]
    fn quota_errors_wait_for_the_daily_reset_and_rate_limits_an_hour() {
        // 2026-10-09 22:00 UTC.
        let now = Millis(1_791_583_200_000);
        let next_reset = Millis(1_791_590_400_000 + 5 * 60 * 1000);
        for text in [
            "AiError: 4006: you have used up your daily free allocation of 10,000 neurons",
            "Quota exceeded",
        ] {
            assert_eq!(classify_ai_error(text), AiFailure::Quota, "{text}");
        }
        assert_eq!(quota_retry(AiFailure::Quota, now), Some(next_reset));
        assert_eq!(
            classify_ai_error("HTTP 429 Too Many Requests"),
            AiFailure::RateLimited
        );
        assert_eq!(
            quota_retry(AiFailure::RateLimited, now),
            Some(now.after(HOUR_MS))
        );
        assert_eq!(
            classify_ai_error("AiError: invalid audio"),
            AiFailure::Other
        );
        // An input limit is no reason to wait: retrying cannot help.
        assert_eq!(
            classify_ai_error("AiError: audio exceeds the size limit"),
            AiFailure::Other
        );
        assert_eq!(
            classify_ai_error("Capacity temporarily exceeded"),
            AiFailure::Other
        );
        assert_eq!(quota_retry(AiFailure::Other, now), None);
    }

    #[test]
    fn uploads_back_off_from_2_s_to_a_minute() {
        let waits: Vec<u64> = (1..=MAX_UPLOAD_ATTEMPTS).map(upload_backoff).collect();
        assert_eq!(waits, [2_000, 10_000, 60_000, 60_000, 60_000]);
    }

    #[test]
    fn backoff_doubles_up_to_half_an_hour() {
        assert_eq!(backoff(0), 30_000);
        assert_eq!(backoff(1), 60_000);
        assert_eq!(backoff(20), 30 * 60 * 1000);
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(secret_matches("abc", "abc"));
        assert!(!secret_matches("abc", "abd"));
        assert!(!secret_matches("abc", "ab"));
        assert!(!secret_matches("abc", ""));
    }

    #[test]
    fn audio_and_transcripts_have_separate_prefixes() {
        let key = chunk_key("r1", ParticipantId(3), 1, 12);
        assert_eq!(key, "audio/r1/3/1-12.ogg");
        assert!(key.starts_with(&audio_prefix("r1")));
        assert_eq!(
            transcript_key("r1", "transcript.md"),
            "transcripts/r1/transcript.md"
        );
    }

    fn span(who: u32, generation: u32, start: u64, duration_ms: u64) -> ChunkSpan {
        ChunkSpan {
            who: ParticipantId(who),
            generation,
            start: Millis(start),
            duration_ms,
            lacking: None,
        }
    }

    #[test]
    fn gaps_are_holes_within_one_adapter_stream_and_failed_chunks() {
        let mut failed = span(2, 0, 0, 5_000);
        failed.lacking = Some(GapReason::Untranscribed);
        let found = gaps(vec![
            span(1, 0, 45_000, 10_000),
            span(1, 0, 0, 30_000),
            span(1, 0, 30_500, 9_000), // 500 ms hole: not reported
            span(1, 1, 90_000, 1_000), // a new adapter: no gap
            failed,
        ]);
        assert_eq!(
            found,
            [
                Gap {
                    who: ParticipantId(1),
                    start: Millis(39_500),
                    end: Millis(45_000),
                    reason: GapReason::Missing
                },
                Gap {
                    who: ParticipantId(2),
                    start: Millis(0),
                    end: Millis(5_000),
                    reason: GapReason::Untranscribed
                },
            ]
        );
    }

    #[test]
    fn adapters_count_generations_per_speaker_and_names_update() {
        let mut recorder = Recorder {
            meeting: MeetingId::from_random([0, 0, 0]),
            started_at: Millis(0),
            secret: "s".into(),
            speakers: Vec::new(),
            consents: Vec::new(),
            adapters: Vec::new(),
            phase: Phase::Recording,
        };
        let who = ParticipantId(4);
        assert_eq!(recorder.next_generation(who), 0);
        recorder.adapters.push(Adapter {
            who,
            generation: 0,
            session: SessionId::new("s"),
            track_name: TrackName::new("t"),
            id: None,
            open: false,
        });
        assert_eq!(recorder.next_generation(who), 1);
        assert_eq!(recorder.open_adapter(who), None);
        let entry = |name: &str| RosterEntry {
            id: who,
            name: DisplayName::parse(name).unwrap(),
            role: Role::Guest,
        };
        recorder.name_speaker(entry("Ana"));
        recorder.name_speaker(entry("Anna"));
        assert_eq!(recorder.speakers.len(), 1);
        assert_eq!(recorder.prompt(), "Participants: Anna.");
    }
}
