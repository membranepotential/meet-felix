//! What the transcript page reads, through the router: the job's status with the transcript
//! once it is ready, and the transcript files as downloads.

use meeting_core::transcript::Transcript;
use worker::{Response, Result};

use super::state::{Job, RecorderPhase, transcript_key};
use super::{Inner, not_found};
use crate::internal::{TRANSCRIPT_JSON, TRANSCRIPT_MD};

impl Inner {
    /// The [`meeting_core::http::TranscriptStatus`], with `transcript.json` once it is ready.
    pub(super) async fn status(&self) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return not_found();
        };
        let transcript = match recorder.phase {
            RecorderPhase::Ended {
                job: Job::Ready { .. },
                ..
            } => self
                .read(TRANSCRIPT_JSON)
                .await?
                .map(|json| serde_json::from_str::<Transcript>(&json))
                .transpose()?,
            _ => None,
        };
        match recorder.status(self.store.progress()?, transcript) {
            Some(status) => Response::from_json(&status),
            None => Err(worker::Error::RustError(format!(
                "recorder {}: the job is ready, but {TRANSCRIPT_JSON} is missing",
                self.id()
            ))),
        }
    }

    /// A transcript file as a download named after the meeting's words.
    pub(super) async fn file(&self, file: &str) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return not_found();
        };
        let Some(text) = self.read(file).await? else {
            return not_found();
        };
        let (content_type, extension) = if file == TRANSCRIPT_MD {
            ("text/markdown; charset=utf-8", "md")
        } else {
            ("application/json", "json")
        };
        let response = Response::ok(text)?;
        response.headers().set("Content-Type", content_type)?;
        response
            .headers()
            .set("X-Content-Type-Options", "nosniff")?;
        response.headers().set(
            "Content-Disposition",
            &format!(
                "attachment; filename=\"{}.{extension}\"",
                recorder.meeting.as_str()
            ),
        )?;
        Ok(response)
    }

    async fn read(&self, file: &str) -> Result<Option<String>> {
        let Some(object) = self
            .bucket()?
            .get(transcript_key(&self.id(), file))
            .execute()
            .await?
        else {
            return Ok(None);
        };
        match object.body() {
            Some(body) => Ok(Some(body.text().await?)),
            None => Ok(None),
        }
    }
}
