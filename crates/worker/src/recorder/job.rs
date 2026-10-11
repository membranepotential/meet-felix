//! The transcript job after the meeting: transcribe the stored chunks, merge the segments into
//! paragraphs, write `transcript.json` and `transcript.md`, and delete the audio 48 hours after
//! the meeting ended (D17, D18). One small step per alarm invocation.

use std::collections::HashSet;
use std::rc::Rc;

use futures_util::future::join_all;
use meeting_core::recording::{RosterEntry, label_speakers};
use meeting_core::transcript::{Segment, Transcript, merge_segments};
use meeting_core::{Millis, ParticipantId};
use worker::{Bucket, Data, HttpMetadata, Result, console_error, console_log};

use super::Inner;
use super::state::{
    self, AUDIO_RETENTION_MS, Job, MAX_QUOTA_WAIT_MS, MAX_STEP_FAILURES, Recorder, RecorderPhase,
    SegmentCursor, audio_prefix, backoff, chunk_key, gaps, quota_retry, transcript_key,
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

impl Inner {
    /// The transcript job's next step.
    pub(super) async fn run_job(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        at: Millis,
        job: Job,
        now: Millis,
    ) -> Result<Option<Millis>> {
        match job {
            Job::Transcribing {
                retry_at: Some(retry_at),
                ..
            } if retry_at > now => Ok(Some(retry_at)),
            Job::Transcribing { waiting_since, .. } => {
                self.transcribe(recorder, waiting_since, now).await
            }
            Job::Merging { after, failures } => {
                self.merge(recorder, at, after, failures, now).await
            }
            Job::Ready {
                audio_until,
                audio_deleted_at: None,
            } if audio_until <= now => self.delete_audio(recorder, now).await,
            Job::Ready {
                audio_until,
                audio_deleted_at: None,
            } => Ok(Some(audio_until)),
            Job::Ready { .. } | Job::Failed { .. } => Ok(None),
        }
    }

    /// Transcribes the next few stored chunks, once every chunk is in R2. `waiting_since` is
    /// when Workers AI first refused for its quota since a chunk last went through.
    async fn transcribe(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        waiting_since: Option<Millis>,
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
        let batch = self.store.chunks_in(ChunkState::Stored, TRANSCRIBE_BATCH)?;
        if batch.is_empty() {
            set_job(
                recorder,
                Job::Merging {
                    after: None,
                    failures: 0,
                },
            );
            self.save(recorder).await?;
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
        let mut went_through = false;
        let waited_too_long =
            waiting_since.is_some_and(|since| now.0.saturating_sub(since.0) > MAX_QUOTA_WAIT_MS);
        for (row, result) in batch.iter().zip(results) {
            let id = row.id();
            match result {
                Ok(segments) => {
                    self.store.transcribed(id, &segments)?;
                    went_through = true;
                }
                Err(e) => {
                    console_error!(
                        "recorder: transcribing chunk {id:?} failed ({:?}): {}",
                        e.class,
                        e.message
                    );
                    match quota_retry(e.class, now) {
                        // A week of waiting without progress: give the chunk up.
                        Some(_) if waited_too_long => self.store.give_up(id)?,
                        Some(at) => retry_at = Some(retry_at.map_or(at, |r| r.max(at))),
                        None => {
                            self.store.attempt_failed(id)?;
                            failed = true;
                        }
                    }
                }
            }
        }
        // Quota and rate limits wait without counting as failures, for up to a week.
        let waiting_since = match (retry_at, went_through) {
            (Some(_), false) => Some(waiting_since.unwrap_or(now)),
            _ => None,
        };
        set_job(
            recorder,
            Job::Transcribing {
                retry_at,
                waiting_since,
            },
        );
        self.save(recorder).await?;
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
    ) -> std::result::Result<Vec<Segment>, TranscribeError> {
        let id = row.id();
        let key = chunk_key(&self.id(), id.who, id.generation, id.index);
        let other = |message: String| TranscribeError {
            class: state::AiFailure::Other,
            message,
        };
        let object = bucket
            .get(key)
            .execute()
            .await
            .map_err(|e| other(e.to_string()))?
            .ok_or_else(|| other("the chunk is missing from R2".into()))?;
        let audio = object
            .body()
            .ok_or_else(|| other("the chunk has no body".into()))?
            .bytes()
            .await
            .map_err(|e| other(e.to_string()))?;
        let texts = transcriber.transcribe(&audio).await?;
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
        recorder: &mut Recorder,
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
            set_job(
                recorder,
                Job::Merging {
                    after: Some(cursor),
                    failures,
                },
            );
            self.save(recorder).await?;
            return Ok(Some(now));
        }
        match self.write_transcript(recorder, ended_at).await {
            Ok(()) => {
                let audio_until = ended_at.after(AUDIO_RETENTION_MS);
                set_job(
                    recorder,
                    Job::Ready {
                        audio_until,
                        audio_deleted_at: None,
                    },
                );
                self.save(recorder).await?;
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
                set_job(recorder, job);
                self.save(recorder).await?;
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
                .put(transcript_key(&id, file), Data::Text(body))
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
    async fn delete_audio(&self, recorder: &mut Recorder, now: Millis) -> Result<Option<Millis>> {
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
        if let RecorderPhase::Ended {
            job: Job::Ready {
                audio_deleted_at, ..
            },
            ..
        } = &mut recorder.phase
        {
            *audio_deleted_at = Some(now);
        }
        self.save(recorder).await?;
        console_log!("recorder {}: audio deleted", self.id());
        Ok(None)
    }
}

fn set_job(recorder: &mut Recorder, new: Job) {
    if let RecorderPhase::Ended { job, .. } = &mut recorder.phase {
        *job = new;
    }
}
