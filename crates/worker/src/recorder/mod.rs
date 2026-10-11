//! `RecorderObject`: one Durable Object per recorded meeting (D16–D18 in `docs/recording`).
//!
//! The meeting's `RoomObject` creates it when recording starts and drives it with the internal
//! requests in [`crate::internal`]: `/start`, `/track` and `/untrack` per consenting speaker's
//! audio track, `/stop`, and `/end` when the meeting is over. For each track the recorder asks
//! the SFU for a WebSocket adapter that streams the audio to
//! `/rec/{recorder}/{secret}/{who}/{generation}`; the router forwards that upgrade here.
//!
//! Each adapter message goes through a [`capture::Capture`] (downmix, resample, silence gate,
//! Opus, chunk cuts) without awaiting anything. Encoded packets are spooled to SQLite at least
//! every 1.5 s, so an eviction or deploy loses at most that much; a finished chunk becomes one
//! Ogg/Opus object in R2, uploaded in the background with at most two uploads at a time. A chunk
//! left open by an eviction is closed from its spool and uploaded when the recorder next runs.
//!
//! After `/end`, the alarm runs the transcript job one small step per invocation (Workers Free
//! allows 10 ms of CPU per invocation): upload what is left, transcribe up to four chunks with
//! Whisper, then merge the segments into paragraphs a few hundred at a time, write
//! `transcript.json` and `transcript.md`, and delete the audio 48 hours after the meeting ended.
//! Every step records its progress in storage, so a repeated alarm repeats no finished work, and
//! no error leaves the alarm handler.
//!
//! The code is split by concern, each an `impl Inner` block: this file is the Durable Object
//! shell (routing, socket events, the alarm, storage); [`requests`] handles the room's
//! requests and looks after a running recording; [`adapters`] makes the SFU calls for the
//! adapters; [`streams`] takes the adapters' sockets and stores and uploads their chunks;
//! [`job`] runs the transcript job; [`serve`] answers the transcript page.
//!
//! Never log the adapter endpoint or the secret: the URL is the only credential of the stream.

mod adapters;
mod capture;
mod job;
mod opus;
mod packet;
mod requests;
mod resample;
mod serve;
mod state;
mod store;
mod streams;
mod whisper;

use std::cell::RefCell;
use std::rc::Rc;

use meeting_core::Millis;
use worker::{
    Bucket, DurableObject, Env, Method, Request, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, console_error, console_warn, durable_object,
};

pub(crate) use self::state::{Ordered, RequestSeq, Stop};
use self::state::{Recorder, RecorderPhase};
use self::store::Store;
use self::streams::Live;
use crate::internal::{
    RECORDER_END, RECORDER_START, RECORDER_STATUS, RECORDER_STOP, RECORDER_TRACK, RECORDER_UNTRACK,
    TRANSCRIPT_JSON, TRANSCRIPT_MD,
};
use crate::{is_websocket_upgrade, now};

/// R2 bucket of audio chunks and transcripts (EU jurisdiction).
const RECORDINGS: &str = "RECORDINGS";
/// Close code for streams the recorder ends.
const CLOSE_NORMAL: u16 = 1000;
/// How long the alarm waits after a step failed before it tries again.
const ALARM_RETRY_MS: u64 = 60_000;

#[durable_object]
pub struct RecorderObject {
    inner: Rc<Inner>,
}

struct Inner {
    state: State,
    env: Env,
    store: Store,
    /// Lost on eviction; the spool and the chunk states in `store` make up for it.
    live: RefCell<Live>,
}

impl DurableObject for RecorderObject {
    fn new(state: State, env: Env) -> Self {
        let store = Store::open(state.storage().sql());
        Self {
            inner: Rc::new(Inner {
                state,
                env,
                store,
                live: RefCell::default(),
            }),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        let inner = &self.inner;
        let path = req.path();
        match (req.method(), path.as_str()) {
            (Method::Post, RECORDER_START) => inner.start(req.json().await?).await,
            (Method::Post, RECORDER_TRACK) => inner.track(req.json().await?).await,
            (Method::Post, RECORDER_UNTRACK) => inner.untrack(req.json().await?).await,
            // A room that ran older code posts `null`.
            (Method::Post, RECORDER_STOP) => {
                let stop: Option<Ordered<Stop>> = req.json().await?;
                inner
                    .stop(stop.map_or(RequestSeq::UNORDERED, |stop| stop.seq))
                    .await
            }
            (Method::Post, RECORDER_END) => inner.end(req.json().await?).await,
            (Method::Get, RECORDER_STATUS) => inner.status().await,
            (Method::Get, file) => match file.strip_prefix('/') {
                Some(file @ (TRANSCRIPT_MD | TRANSCRIPT_JSON)) => inner.file(file).await,
                _ if is_websocket_upgrade(&req)? => inner.accept(&path).await,
                _ => not_found(),
            },
            _ => not_found(),
        }
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        if let WebSocketIncomingMessage::Binary(bytes) = message {
            self.inner.on_message(&ws, &bytes);
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        self.inner.socket_gone(&ws);
        // Completes the close handshake on local workerd (see `RoomObject::websocket_close`).
        if let Err(e) = ws.close(Some(CLOSE_NORMAL), Some("bye")) {
            console_warn!("recorder: close failed: {e}");
        }
        Ok(())
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.inner.socket_gone(&ws);
        Ok(())
    }

    /// Runs one step of whatever is due and arms the next alarm. Never fails: an error is logged
    /// and the step retried [`ALARM_RETRY_MS`] later, because the runtime retries a failing
    /// alarm only six times. A request that armed a sooner alarm while the step ran (a failed
    /// adapter call) keeps it; every step checks what is due, so an early alarm is harmless.
    async fn alarm(&self) -> Result<Response> {
        let now = now();
        let next = match self.inner.work(now).await {
            Ok(next) => next,
            Err(e) => {
                console_error!("recorder: alarm step failed: {e}");
                Some(now.after(ALARM_RETRY_MS))
            }
        };
        if let Some(at) = next
            && let Err(e) = self.inner.arm_by(at).await
        {
            console_error!("recorder: setting the alarm failed: {e}");
        }
        Response::empty()
    }
}

impl Inner {
    /// The stored record, of whichever version stored it ([`state::decode`]).
    async fn load(&self) -> Result<Option<Recorder>> {
        let json: Option<String> = self.state.storage().get(state::KEY).await?;
        Ok(json.map(|json| state::decode(&json)).transpose()?)
    }

    async fn save(&self, recorder: &Recorder) -> Result<()> {
        self.state
            .storage()
            .put(state::KEY, state::encode(recorder)?)
            .await
    }

    async fn arm(&self, at: Millis) -> Result<()> {
        let offset = i64::try_from(at.0.saturating_sub(now().0)).unwrap_or(i64::MAX);
        self.state.storage().set_alarm(offset).await
    }

    /// Arms the alarm for `at`, unless it is set for an earlier time already.
    async fn arm_by(&self, at: Millis) -> Result<()> {
        let set = self.state.storage().get_alarm().await?;
        if set.is_some_and(|set| u64::try_from(set).is_ok_and(|set| set <= at.0)) {
            return Ok(());
        }
        self.arm(at).await
    }

    fn id(&self) -> String {
        self.state.id().to_string()
    }

    fn bucket(&self) -> Result<Bucket> {
        self.env.bucket(RECORDINGS)
    }

    /// One step of what is due; returns when to run again.
    async fn work(self: &Rc<Self>, now: Millis) -> Result<Option<Millis>> {
        let Some(mut recorder) = self.load().await? else {
            return Ok(None);
        };
        match recorder.phase.clone() {
            RecorderPhase::Recording => self.look_after(&mut recorder, now).await,
            RecorderPhase::Ended { at, job } => self.run_job(&recorder, at, job, now).await,
        }
    }
}

/// The answer to a request the recorder carried out and has nothing to say about.
fn no_content() -> Result<Response> {
    Response::empty().map(|r| r.with_status(204))
}

/// The answer to a request for a path, recorder or stream that does not exist. A recorder that
/// never started has no record, and its transcript link is unknown like any other.
fn not_found() -> Result<Response> {
    Response::error("not found", 404)
}
