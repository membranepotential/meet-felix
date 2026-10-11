//! What a `RecorderObject` keeps in its key-value storage, the status it reports, and the pure
//! rules of its transcript job: retry times, error classes, R2 keys, gaps.

use meeting_core::recording::{ConsentRecord, RosterEntry};
use meeting_core::transcript::{Gap, GapReason};
use meeting_core::{MeetingId, Millis, ParticipantId, SessionId, TrackName};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sfu_api::wire::AdapterId;

use crate::internal::{TrackRecording, UntrackRecording};
use crate::record::{DecodeError, Stored, stored_record, stored_version};

/// The recorder's storage key.
pub const KEY: &str = "recorder";

/// The version of the stored recorder's shape. 1 is the bare [`Recorder`] stored before
/// 2026-10-11; 2 wraps it in the [`Stored`] envelope the room uses.
pub const RECORDER_VERSION: u32 = 2;

/// The stored JSON of `recorder`.
pub fn encode(recorder: &Recorder) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        version: RECORDER_VERSION,
        record: recorder,
    })
}

/// Reads a stored recorder. Unlike a room, a recorder lives as long as its transcript (D18),
/// and its status page needs the record, so every version ever stored keeps a reader here: a
/// change of shape adds a version and an arm that migrates the old one. Frozen records in
/// `crates/worker/fixtures/recorder/` pin every version (see `crate::record` tests).
pub fn decode(json: &str) -> Result<Recorder, DecodeError> {
    match stored_version(json, RECORDER_VERSION)? {
        // Unversioned: the record itself, of today's shape.
        1 => Ok(serde_json::from_str(json)?),
        _ => stored_record(json),
    }
}

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
    /// The newest of the room's requests the recorder applied, so it can drop one that arrives
    /// after a newer one ([`RequestSeq`]). Recorders stored before it existed load as having
    /// applied none.
    #[serde(default)]
    pub applied: Applied,
}

/// The place of a request from the room among all the requests the room sends its recorder,
/// counted from [`RequestSeq::FIRST`] in the order the room's steps produced them.
///
/// The room carries out a step's effects after storing the step, and each request to the
/// recorder awaits a fetch with the input gate open, so a later step's `/stop` or `/untrack`
/// can reach the recorder before an earlier step's `/track`. Applied as it arrives, that
/// `/track` would record audio that `RoomState` says is not recorded (D15). The recorder
/// instead drops a request older than one it applied for the same speaker, and a `/track` older
/// than the last `/stop` ([`Recorder::begin_track`], [`Recorder::untrack`], [`Recorder::stop`]),
/// so it ends up as if every request had arrived in order. A request without a number (from a
/// room that ran older code) counts as [`RequestSeq::UNORDERED`], older than every numbered one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestSeq(pub u64);

impl RequestSeq {
    /// What a request from a room that numbered none carries.
    pub const UNORDERED: Self = Self(0);
    /// The number of a room's first request.
    pub const FIRST: Self = Self(1);

    /// The number `n` requests after this one.
    #[must_use]
    pub fn after(self, n: usize) -> Self {
        Self(self.0 + n as u64)
    }
}

impl Default for RequestSeq {
    fn default() -> Self {
        Self::UNORDERED
    }
}

/// A request from the room with its [`RequestSeq`], as the room posts it: the request's own
/// fields plus `seq`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Ordered<R> {
    #[serde(default)]
    pub seq: RequestSeq,
    #[serde(flatten)]
    pub request: R,
}

/// The body of `/stop`, which has no fields besides its [`RequestSeq`].
#[derive(Debug, Serialize, Deserialize)]
pub struct Stop {}

/// The newest requests the recorder applied: per speaker the last `/track` or `/untrack`, and
/// the last `/stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub stop: RequestSeq,
    pub speakers: Vec<(ParticipantId, RequestSeq)>,
}

impl Applied {
    /// The last `/track` or `/untrack` applied for `who`.
    fn speaker(&self, who: ParticipantId) -> RequestSeq {
        self.speakers
            .iter()
            .find(|(id, _)| *id == who)
            .map_or(RequestSeq::UNORDERED, |&(_, seq)| seq)
    }

    fn set_speaker(&mut self, who: ParticipantId, seq: RequestSeq) {
        match self.speakers.iter_mut().find(|(id, _)| *id == who) {
            Some((_, known)) => *known = seq,
            None => self.speakers.push((who, seq)),
        }
    }
}

/// What a `/track` asks of the recorder ([`Recorder::begin_track`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackPlan {
    /// The request is older than a `/stop`, `/track` or `/untrack` already applied: nothing
    /// changed.
    Stale,
    /// The speaker's open adapter carries the track already; only the name and consent changed.
    Unchanged,
    /// A new adapter of this generation is to be created, after closing `replaced`, the
    /// speaker's previous ones.
    New {
        generation: u32,
        replaced: Vec<Adapter>,
    },
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

    /// Marks the adapters that `which` selects closed; returns those that were open.
    pub fn close_adapters(&mut self, which: impl Fn(&Adapter) -> bool) -> Vec<Adapter> {
        let mut closed = Vec::new();
        for adapter in self.adapters.iter_mut().filter(|a| a.open && which(a)) {
            adapter.open = false;
            closed.push(adapter.clone());
        }
        closed
    }

    /// Applies the room's `/track` numbered `seq`, unless it is stale: records the speaker's
    /// name and consent, and opens an adapter for the track unless the speaker's open one
    /// carries it already. The caller creates a new adapter at the SFU.
    pub fn begin_track(&mut self, track: &TrackRecording, seq: RequestSeq) -> TrackPlan {
        let who = track.who;
        if seq < self.applied.stop.max(self.applied.speaker(who)) {
            return TrackPlan::Stale;
        }
        self.applied.set_speaker(who, seq);
        self.name_speaker(RosterEntry {
            id: who,
            name: track.name.clone(),
            role: track.role,
        });
        self.record_consent(track.consent.clone());
        if self
            .open_adapter(who)
            .is_some_and(|a| a.session == track.session && a.track_name == track.track_name)
        {
            return TrackPlan::Unchanged;
        }
        let replaced = self.close_adapters(|a| a.who == who);
        let generation = self.next_generation(who);
        self.adapters.push(Adapter {
            who,
            generation,
            session: track.session.clone(),
            track_name: track.track_name.clone(),
            id: None,
            open: true,
        });
        TrackPlan::New {
            generation,
            replaced,
        }
    }

    /// Applies the room's `/untrack` numbered `seq`, unless it is older than a `/track` or
    /// `/untrack` applied for the speaker (`None`): closes the speaker's adapters and records
    /// the consent it carries. Returns the adapters it closed.
    ///
    /// It closes all of them, not only the named track's: the room untracks only the track it
    /// records, so after the newest request for a speaker is an `/untrack`, nothing of theirs
    /// is recorded. An older `/track` for another of their tracks may have opened an adapter
    /// before this arrived.
    pub fn untrack(&mut self, untrack: &UntrackRecording, seq: RequestSeq) -> Option<Vec<Adapter>> {
        let who = untrack.who;
        if seq < self.applied.speaker(who) {
            return None;
        }
        self.applied.set_speaker(who, seq);
        let closed = self.close_adapters(|a| a.who == who);
        if let Some(consent) = &untrack.consent {
            self.record_consent(consent.clone());
        }
        Some(closed)
    }

    /// Applies the room's `/stop` numbered `seq`: closes the adapters of every speaker whose
    /// last applied request is not newer. Returns the adapters it closed.
    pub fn stop(&mut self, seq: RequestSeq) -> Vec<Adapter> {
        self.applied.stop = self.applied.stop.max(seq);
        let applied = &self.applied;
        let newer: Vec<ParticipantId> = applied
            .speakers
            .iter()
            .filter(|&&(_, last)| last > seq)
            .map(|&(who, _)| who)
            .collect();
        self.close_adapters(|a| !newer.contains(&a.who))
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
    use crate::record::tests::fixtures;

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
            applied: Applied::default(),
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
        assert_eq!(recorder.speakers[0].name.as_str(), "Anna");
    }

    /// A request from the room, as the recorder receives it.
    #[derive(Debug)]
    enum Request {
        Track(TrackRecording),
        Untrack(UntrackRecording),
        Stop,
    }

    fn consent(who: ParticipantId, withdrawn: bool) -> ConsentRecord {
        ConsentRecord {
            who,
            name: DisplayName::parse("Ada").unwrap(),
            at: Millis(0),
            notice_version: 1,
            withdrawn_at: withdrawn.then_some(Millis(1)),
        }
    }

    fn track(who: u32, session: &str, name: &str) -> Request {
        let who = ParticipantId(who);
        Request::Track(TrackRecording {
            who,
            name: DisplayName::parse("Ada").unwrap(),
            role: Role::Guest,
            consent: consent(who, false),
            session: SessionId::new(session),
            track_name: TrackName::new(name),
        })
    }

    fn untrack(who: u32, session: &str, name: &str) -> Request {
        let who = ParticipantId(who);
        Request::Untrack(UntrackRecording {
            who,
            session: SessionId::new(session),
            track_name: TrackName::new(name),
            consent: Some(consent(who, true)),
        })
    }

    fn empty_recorder() -> Recorder {
        Recorder {
            meeting: MeetingId::from_random([0, 0, 0]),
            started_at: Millis(0),
            secret: "s".into(),
            speakers: Vec::new(),
            consents: Vec::new(),
            adapters: Vec::new(),
            phase: Phase::Recording,
            applied: Applied::default(),
        }
    }

    /// Applies `(seq, request)` the way `RecorderObject` does, with adapters created at once.
    fn apply(recorder: &mut Recorder, (seq, request): &(RequestSeq, Request)) {
        match request {
            Request::Track(track) => {
                recorder.begin_track(track, *seq);
            }
            Request::Untrack(untrack) => {
                recorder.untrack(untrack, *seq);
            }
            Request::Stop => {
                recorder.stop(*seq);
            }
        }
    }

    /// The tracks the recorder records: its open adapters.
    fn recorded(recorder: &Recorder) -> Vec<(ParticipantId, String)> {
        let mut open: Vec<_> = recorder
            .adapters
            .iter()
            .filter(|a| a.open)
            .map(|a| (a.who, a.track_name.as_str().to_owned()))
            .collect();
        open.sort();
        open
    }

    /// Every order of `items`.
    fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        (0..items.len())
            .flat_map(|i| {
                let mut rest = items.to_vec();
                let first = rest.remove(i);
                permutations(&rest).into_iter().map(move |mut tail| {
                    tail.insert(0, first);
                    tail
                })
            })
            .collect()
    }

    #[test]
    fn a_track_older_than_a_stop_or_untrack_that_arrived_first_records_nothing() {
        // Recording starts with speaker 0's track (1) and stops (2); the stop arrives first.
        let mut recorder = empty_recorder();
        recorder.stop(RequestSeq(2));
        let Request::Track(late) = track(0, "p0", "a0") else {
            unreachable!()
        };
        assert_eq!(recorder.begin_track(&late, RequestSeq(1)), TrackPlan::Stale);
        assert_eq!(recorder, {
            let mut stopped = empty_recorder();
            stopped.applied.stop = RequestSeq(2);
            stopped
        });
        // Speaker 0 withdraws consent (4) after their track (3); the untrack arrives first.
        let mut recorder = empty_recorder();
        let Request::Untrack(withdrawn) = untrack(0, "p0", "a0") else {
            unreachable!()
        };
        assert_eq!(recorder.untrack(&withdrawn, RequestSeq(4)), Some(Vec::new()));
        assert_eq!(recorder.begin_track(&late, RequestSeq(3)), TrackPlan::Stale);
        assert!(recorded(&recorder).is_empty());
        assert_eq!(recorder.consents, [consent(ParticipantId(0), true)]);
    }

    #[test]
    fn the_recorder_ends_as_if_the_rooms_requests_had_arrived_in_order() {
        // What a room sends: speakers 0 and 1 are recorded; 0 withdraws consent; recording
        // stops and starts again with 0, who consented again, and 1, who republished; then 1
        // unpublishes.
        let history: Vec<(RequestSeq, Request)> = [
            track(0, "p0", "a0"),
            track(1, "p1", "a1"),
            untrack(0, "p0", "a0"),
            Request::Stop,
            track(0, "p0", "a0"),
            track(1, "p1b", "a1b"),
            untrack(1, "p1b", "a1b"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, request)| (RequestSeq::FIRST.after(i), request))
        .collect();
        let mut in_order = empty_recorder();
        for request in &history {
            apply(&mut in_order, request);
        }
        assert_eq!(recorded(&in_order), [(ParticipantId(0), "a0".to_owned())]);
        let indices: Vec<usize> = (0..history.len()).collect();
        for arrival in permutations(&indices) {
            let mut recorder = empty_recorder();
            for &i in &arrival {
                apply(&mut recorder, &history[i]);
            }
            assert_eq!(recorded(&recorder), recorded(&in_order), "arrival {arrival:?}");
        }
    }

    #[test]
    fn requests_without_a_number_count_as_older_than_numbered_ones() {
        let Request::Track(track) = track(0, "p0", "a0") else {
            unreachable!()
        };
        // A room that ran older code: its unnumbered requests apply as they arrive.
        let mut recorder = empty_recorder();
        assert!(matches!(
            recorder.begin_track(&track, RequestSeq::UNORDERED),
            TrackPlan::New { .. }
        ));
        assert_eq!(recorder.stop(RequestSeq::UNORDERED).len(), 1);
        // Once a numbered request was applied, an unnumbered one is stale.
        recorder.stop(RequestSeq::FIRST);
        assert_eq!(
            recorder.begin_track(&track, RequestSeq::UNORDERED),
            TrackPlan::Stale
        );
    }

    #[test]
    fn the_rooms_requests_carry_their_number_beside_their_fields() {
        let Request::Track(track) = track(0, "p0", "a0") else {
            unreachable!()
        };
        let json = serde_json::to_value(Ordered {
            seq: RequestSeq(5),
            request: track,
        })
        .unwrap();
        assert_eq!(json["seq"], 5);
        assert_eq!(json["trackName"], "a0");
        // A body without a number, as a room on older code posts it.
        let mut old = json;
        old.as_object_mut().unwrap().remove("seq");
        let parsed: Ordered<TrackRecording> = serde_json::from_value(old).unwrap();
        assert_eq!(parsed.seq, RequestSeq::UNORDERED);
        let stop = serde_json::to_string(&Ordered {
            seq: RequestSeq(6),
            request: Stop {},
        })
        .unwrap();
        assert_eq!(stop, r#"{"seq":6}"#);
        let old_stop: Option<Ordered<Stop>> = serde_json::from_str("null").unwrap();
        assert!(old_stop.is_none());
    }

    /// Recorders the fixtures freeze: one per phase and job step, with speakers, a withdrawn
    /// consent, and adapters open, closed and being created.
    fn fixture_recorders() -> Vec<(&'static str, Recorder)> {
        let (ada, bo) = (ParticipantId(0), ParticipantId(1));
        let recording = Recorder {
            meeting: MeetingId::from_random([1, 2, 3]),
            started_at: Millis(1_791_583_200_000),
            secret: "00112233445566778899aabbccddeeff".into(),
            speakers: vec![
                RosterEntry {
                    id: ada,
                    name: DisplayName::parse("Ada").unwrap(),
                    role: Role::Host,
                },
                RosterEntry {
                    id: bo,
                    name: DisplayName::parse("Bo").unwrap(),
                    role: Role::Guest,
                },
            ],
            consents: vec![
                ConsentRecord {
                    who: ada,
                    name: DisplayName::parse("Ada").unwrap(),
                    at: Millis(1_791_583_200_000),
                    notice_version: 1,
                    withdrawn_at: None,
                },
                ConsentRecord {
                    who: bo,
                    name: DisplayName::parse("Bo").unwrap(),
                    at: Millis(1_791_583_260_000),
                    notice_version: 1,
                    withdrawn_at: Some(Millis(1_791_583_500_000)),
                },
            ],
            adapters: vec![
                Adapter {
                    who: ada,
                    generation: 0,
                    session: SessionId::new("publish-0"),
                    track_name: TrackName::new("audio-0"),
                    id: Some(AdapterId::new("adapter-0")),
                    open: true,
                },
                Adapter {
                    who: bo,
                    generation: 0,
                    session: SessionId::new("publish-1"),
                    track_name: TrackName::new("audio-1"),
                    id: Some(AdapterId::new("adapter-1")),
                    open: false,
                },
                Adapter {
                    who: bo,
                    generation: 1,
                    session: SessionId::new("publish-1b"),
                    track_name: TrackName::new("audio-1b"),
                    id: None,
                    open: true,
                },
            ],
            phase: Phase::Recording,
            applied: Applied {
                stop: RequestSeq(4),
                speakers: vec![(ada, RequestSeq(3)), (bo, RequestSeq(7))],
            },
        };
        let at = Millis(1_791_586_800_000);
        let ended = |job| Recorder {
            phase: Phase::Ended { at, job },
            ..recording.clone()
        };
        vec![
            ("recording", recording.clone()),
            (
                "transcribing",
                ended(Job::Transcribing {
                    retry_at: Some(at.after(HOUR_MS)),
                    waiting_since: Some(at),
                }),
            ),
            (
                "merging",
                ended(Job::Merging {
                    after: Some(SegmentCursor {
                        start_ms: 1_791_583_300_000,
                        id: 42,
                    }),
                    failures: 1,
                }),
            ),
            (
                "ready",
                ended(Job::Ready {
                    audio_until: at.after(AUDIO_RETENTION_MS),
                    audio_deleted_at: Some(at.after(AUDIO_RETENTION_MS + 60_000)),
                }),
            ),
            ("failed", ended(Job::Failed { at: at.after(HOUR_MS) })),
        ]
    }

    /// Writes the `current/` fixtures from [`fixture_recorders`]; see
    /// `crate::record::tests::fixtures` for when.
    #[test]
    #[ignore = "writes the fixtures of the current shape; run it only after a shape change"]
    fn capture_recorder_fixtures() {
        for (name, recorder) in fixture_recorders() {
            fixtures::capture("recorder", &format!("{name}.json"), &encode(&recorder).unwrap());
        }
    }

    #[test]
    fn the_current_code_stores_the_frozen_recorders_unchanged() {
        for (name, json) in fixtures::read("recorder", "current") {
            assert_eq!(
                stored_version(&json, RECORDER_VERSION).unwrap(),
                RECORDER_VERSION,
                "{name}"
            );
            let recorder = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                fixtures::same_json(&json, &encode(&recorder).unwrap()),
                "{name} no longer stores as frozen: the stored shape changed"
            );
        }
    }

    #[test]
    fn every_recorder_deployed_code_stored_still_loads() {
        let built = fixture_recorders();
        for (name, json) in fixtures::read("recorder", "older") {
            let version = fixtures::version_in_name(&name);
            assert_eq!(stored_version(&json, RECORDER_VERSION).unwrap(), version);
            let recorder = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
            // The unversioned records were captured from these scenarios, before the recorder
            // kept the requests it applied.
            if let Some((_, expected)) = built
                .iter()
                .find(|(n, _)| format!("v1-{n}.json") == name)
            {
                let before_applied = Recorder {
                    applied: Applied::default(),
                    ..expected.clone()
                };
                assert_eq!(recorder, before_applied, "{name}");
            }
        }
    }

    #[test]
    fn a_recorder_from_newer_code_is_refused() {
        assert!(matches!(
            decode(r#"{"version":3,"record":{}}"#),
            Err(DecodeError::Newer {
                found: 3,
                supported: RECORDER_VERSION
            })
        ));
    }
}
