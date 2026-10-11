//! The transcript job after the meeting: transcribe the stored chunks, merge the segments into
//! paragraphs, write `transcript.json` and `transcript.md`, and delete the audio 48 hours after
//! the meeting ended (D17, D18). One small step per alarm invocation.

use std::collections::HashSet;
use std::rc::Rc;

use futures_util::future::join_all;
use meeting_core::recording::{RosterEntry, label_speakers};
use meeting_core::transcript::{Segment, Transcript, merge_segments};
use meeting_core::{Millis, ParticipantId};
use worker::{Bucket, Data, HttpMetadata, Result, console_error, console_log, console_warn};

use super::Inner;
use super::state::{
    self, AUDIO_RETENTION_MS, Job, JobStep, MAX_STEP_FAILURES, Recorder, RecorderPhase, Refusal,
    SegmentCursor, audio_prefix, backoff, chunk_key, gaps, on_refusal, transcript_file_key,
};
use super::store::{ChunkRow, ChunkState};
use super::whisper::{TranscribeError, Transcriber, WhisperTurbo};
use crate::internal::{TRANSCRIPT_JSON, TRANSCRIPT_MD};

/// Workers AI.
const AI: &str = "AI";
/// Chunks transcribed per alarm invocation.
const TRANSCRIBE_BATCH: u32 = 4;
/// Segments merged per alarm invocation.
const MERGE_BATCH: u32 = 300;

/// Why a chunk did not come back transcribed.
enum ChunkError {
    /// R2 no longer holds it; the lifecycle rule of D18 deleted it.
    Gone,
    /// Reading it or the speech model failed.
    Failed(TranscribeError),
}

impl Inner {
    /// The transcript job's next step ([`Job::step`]). `recorder` is the record as the alarm
    /// loaded it; a step that awaits anything stores its job on the record as it is then
    /// ([`Inner::store_job`]), so a `/end` that arrived meanwhile is kept.
    pub(super) async fn run_job(
        self: &Rc<Self>,
        recorder: &Recorder,
        ended_at: Millis,
        job: Job,
        now: Millis,
    ) -> Result<Option<Millis>> {
        match job.step(ended_at, now) {
            JobStep::Wait(at) => Ok(Some(at)),
            JobStep::Transcribe { audio_until } => self.transcribe(audio_until, now).await,
            JobStep::Merge { after, failures } => {
                self.merge(recorder, ended_at, after, failures, now).await
            }
            JobStep::DeleteAudio { audio_until } => self.delete_audio(audio_until, now).await,
            JobStep::Done => Ok(None),
        }
    }

    /// Stores `job` as the transcript job's step, on the record as it is now.
    async fn store_job(&self, job: Job) -> Result<()> {
        let Some(mut recorder) = self.load().await? else {
            return Ok(());
        };
        recorder.set_job(job);
        self.save(&recorder).await
    }

    /// Transcribes the next few stored chunks, once every chunk is in R2. The audio is kept
    /// until `audio_until` (D18), so a chunk not transcribed by then is given up.
    async fn transcribe(
        self: &Rc<Self>,
        audio_until: Millis,
        now: Millis,
    ) -> Result<Option<Millis>> {
        // `finish` let go of every stream already; this repeats it in case the object went
        // between storing the end and doing so.
        self.end_all_streams();
        self.recover(now);
        let closed = self.store.chunks_in(ChunkState::Closed, u32::MAX)?;
        if !closed.is_empty() {
            // Uploads run in the background; look again when the next one is due.
            let due = closed.iter().map(|c| c.upload_after).min().unwrap_or(0);
            return Ok(Some(Millis(due).max(now.after(2_000))));
        }
        self.store.skip_silent()?;
        if now >= audio_until {
            self.store.give_up_stored()?;
        }
        let batch = self.store.chunks_in(ChunkState::Stored, TRANSCRIBE_BATCH)?;
        if batch.is_empty() {
            self.store_job(Job::Merging {
                after: None,
                failures: 0,
            })
            .await?;
            return Ok(Some(now));
        }
        let transcriber = WhisperTurbo(self.env.ai(AI)?);
        let bucket = self.bucket()?;
        let results = join_all(
            batch
                .iter()
                .map(|row| self.transcribe_chunk(&transcriber, &bucket, row)),
        )
        .await;
        let mut retry_at: Option<Millis> = None;
        let mut failed = false;
        let mut give_up = false;
        for (row, result) in batch.iter().zip(results) {
            let id = row.id();
            match result {
                Ok(segments) => self.store.transcribed(id, &segments)?,
                Err(ChunkError::Gone) => {
                    console_warn!("recorder: chunk {id:?} is no longer in R2");
                    self.store.chunk_gone(id)?;
                }
                Err(ChunkError::Failed(e)) => {
                    console_error!(
                        "recorder: transcribing chunk {id:?} failed ({:?}): {}",
                        e.class,
                        e.message
                    );
                    match on_refusal(e.class, now, audio_until) {
                        Refusal::Attempt => {
                            self.store.attempt_failed(id)?;
                            failed = true;
                        }
                        Refusal::WaitUntil(at) => retry_at = retry_at.max(Some(at)),
                        Refusal::GiveUp => give_up = true,
                    }
                }
            }
        }
        if give_up {
            console_warn!("recorder: Workers AI refuses until after the audio goes; giving up");
            self.store.give_up_stored()?;
            retry_at = None;
        }
        self.store_job(Job::Transcribing { retry_at }).await?;
        Ok(Some(match retry_at {
            Some(at) => at,
            None if failed => now.after(backoff(0)),
            None => now,
        }))
    }

    async fn transcribe_chunk(
        &self,
        transcriber: &impl Transcriber,
        bucket: &Bucket,
        row: &ChunkRow,
    ) -> std::result::Result<Vec<Segment>, ChunkError> {
        let id = row.id();
        let key = chunk_key(&self.id(), id.who, id.generation, id.index);
        let other = |message: String| {
            ChunkError::Failed(TranscribeError {
                class: state::AiFailure::Other,
                message,
            })
        };
        let object = bucket
            .get(key)
            .execute()
            .await
            .map_err(|e| other(e.to_string()))?
            .ok_or(ChunkError::Gone)?;
        let audio = object
            .body()
            .ok_or_else(|| other("the chunk has no body".into()))?
            .bytes()
            .await
            .map_err(|e| other(e.to_string()))?;
        let texts = transcriber
            .transcribe(&audio)
            .await
            .map_err(ChunkError::Failed)?;
        let at = |seconds: f64| {
            let offset = (seconds.max(0.0) * 1000.0).round() as u64;
            Millis(row.start_ms + offset.min(row.duration_ms))
        };
        Ok(texts
            .into_iter()
            .map(|t| Segment {
                who: id.who,
                start: at(t.start_s),
                end: at(t.end_s),
                text: t.text,
            })
            .collect())
    }

    /// Merges the next batch of segments into paragraphs; once all are merged, writes the
    /// transcript.
    async fn merge(
        &self,
        recorder: &Recorder,
        ended_at: Millis,
        after: Option<SegmentCursor>,
        failures: u32,
        now: Millis,
    ) -> Result<Option<Millis>> {
        let batch = self.store.segments_after(after, MERGE_BATCH)?;
        if let Some(&(cursor, _)) = batch.last() {
            let last = self.store.take_last_paragraph()?;
            let paragraphs = merge_segments(last, batch.into_iter().map(|(_, s)| s));
            self.store.append_paragraphs(&paragraphs)?;
            self.store_job(Job::Merging {
                after: Some(cursor),
                failures,
            })
            .await?;
            return Ok(Some(now));
        }
        match self.write_transcript(recorder, ended_at).await {
            Ok(()) => {
                let audio_until = ended_at.after(AUDIO_RETENTION_MS);
                self.store_job(Job::Ready {
                    audio_until,
                    audio_deleted_at: None,
                })
                .await?;
                console_log!("recorder {}: transcript ready", self.id());
                Ok(Some(audio_until.max(now)))
            }
            Err(e) => {
                console_error!("recorder: writing the transcript failed: {e}");
                let failures = failures + 1;
                let (job, next) = if failures >= MAX_STEP_FAILURES {
                    (Job::Failed { at: now }, None)
                } else {
                    (
                        Job::Merging { after, failures },
                        Some(now.after(backoff(failures))),
                    )
                };
                self.store_job(job).await?;
                Ok(next)
            }
        }
    }

    async fn write_transcript(&self, recorder: &Recorder, ended_at: Millis) -> Result<()> {
        if recorder.phase == RecorderPhase::Recording {
            return Err("the meeting has not ended".into());
        }
        let paragraphs = self.store.paragraphs()?;
        let gaps = gaps(self.store.spans()?, &recorder.unrecorded);
        // Everyone the transcript mentions gets their name, also a speaker of whom it has only
        // gaps.
        let mentioned: HashSet<ParticipantId> = paragraphs
            .iter()
            .map(|p| p.who)
            .chain(gaps.iter().map(|g| g.who))
            .collect();
        let speakers: Vec<RosterEntry> = recorder
            .speakers
            .iter()
            .filter(|s| mentioned.contains(&s.id))
            .cloned()
            .collect();
        let transcript = Transcript::new(
            recorder.meeting.clone(),
            recorder.started_at,
            ended_at,
            label_speakers(&speakers),
            recorder.consents.clone(),
            gaps,
            paragraphs,
        );
        let bucket = self.bucket()?;
        let id = self.id();
        for (file, body, content_type) in [
            (
                TRANSCRIPT_JSON,
                serde_json::to_string(&transcript)?,
                "application/json",
            ),
            (
                TRANSCRIPT_MD,
                transcript.markdown(),
                "text/markdown; charset=utf-8",
            ),
        ] {
            bucket
                .put(transcript_file_key(&id, file), Data::Text(body))
                .http_metadata(HttpMetadata {
                    content_type: Some(content_type.into()),
                    ..HttpMetadata::default()
                })
                .execute()
                .await?;
        }
        Ok(())
    }

    /// Deletes the meeting's audio from R2, a thousand objects per run (D18).
    async fn delete_audio(&self, audio_until: Millis, now: Millis) -> Result<Option<Millis>> {
        let bucket = self.bucket()?;
        let listed = bucket
            .list()
            .prefix(audio_prefix(&self.id()))
            .limit(1_000)
            .execute()
            .await?;
        let keys: Vec<String> = listed.objects().iter().map(worker::Object::key).collect();
        if !keys.is_empty() {
            bucket.delete_multiple(keys).await?;
        }
        if listed.truncated() {
            return Ok(Some(now));
        }
        self.store.clear_spool()?;
        self.store_job(Job::Ready {
            audio_until,
            audio_deleted_at: Some(now),
        })
        .await?;
        console_log!("recorder {}: audio deleted", self.id());
        Ok(None)
    }
}
