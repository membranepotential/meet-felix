//! What the transcript page reads, through the router: the job's status with the transcript
//! once it is ready, and the transcript files as downloads.

use serde_json::value::RawValue;
use worker::{Response, Result};

use super::state::{RecorderPhase, Status, StatusKind, transcript_key};
use super::{Inner, not_found};
use crate::internal::{TRANSCRIPT_JSON, TRANSCRIPT_MD};

impl Inner {
    pub(super) async fn status(&self) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return not_found();
        };
        let (status, retry_at, audio_until) = recorder.phase.status();
        let transcript = match status {
            StatusKind::Ready => match self.read(TRANSCRIPT_JSON).await? {
                Some(text) => Some(RawValue::from_string(text)?),
                None => None,
            },
            _ => None,
        };
        let ended_at = match recorder.phase {
            RecorderPhase::Ended { at, .. } => Some(at),
            RecorderPhase::Recording => None,
        };
        Response::from_json(&Status {
            status,
            meeting: recorder.meeting,
            started_at: recorder.started_at,
            ended_at,
            progress: self.store.progress()?,
            retry_at,
            audio_until,
            transcript,
        })
    }

    /// A transcript file as a download named after the meeting's words.
    pub(super) async fn file(&self, file: &str) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return not_found();
        };
        let Some(text) = self.read(file).await? else {
            return Response::error("the transcript is not ready", 404);
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
