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
//! Never log the adapter endpoint or the secret: the URL is the only credential of the stream.

mod capture;
mod opus;
mod packet;
mod resample;
mod state;
mod store;
mod whisper;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use futures_util::future::join_all;
use meeting_core::recording::{RosterEntry, label_speakers};
use meeting_core::transcript::{Segment, Transcript, merge_segments};
use meeting_core::{Millis, ParticipantId, SessionId, TrackName};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sfu_api::wire::{AdapterId, AdapterTrack, MAX_ADAPTERS_PER_REQUEST, OutputCodec};
use worker::{
    Bucket, Data, DurableObject, Env, HttpMetadata, Method, Request, Response, Result, State,
    WebSocket, WebSocketIncomingMessage, WebSocketPair, console_error, console_log, console_warn,
    durable_object,
};

use self::capture::{Capture, Note};
use self::opus::{ogg_opus, spooled_packets};
use self::packet::Packet;
use self::state::{
    AUDIO_RETENTION_MS, Adapter, Job, MAX_QUOTA_WAIT_MS, MAX_STEP_FAILURES, Phase, Recorder,
    SegmentCursor, Status, audio_prefix, backoff, chunk_key, gaps, quota_retry, secret_matches,
    transcript_key,
};
use self::store::{ChunkId, ChunkRow, ChunkState, Store};
use self::whisper::{TranscribeError, Transcriber, WhisperTurbo};
use crate::internal::{
    FinishRecording, RECORDER_END, RECORDER_START, RECORDER_STATUS, RECORDER_STOP, RECORDER_TRACK,
    RECORDER_UNTRACK, StartRecording, TRANSCRIPT_JSON, TRANSCRIPT_MD, TrackRecording,
    UntrackRecording,
};
use crate::{is_websocket_upgrade, now, random, sfu};

/// R2 bucket of audio chunks and transcripts (EU jurisdiction).
const RECORDINGS: &str = "RECORDINGS";
/// Workers AI.
const AI: &str = "AI";
/// Var: the origin the SFU's adapters connect to, e.g. the `workers.dev` host, which skips the
/// Pages front door (research 11). `https://` becomes `wss://`.
const RECORDER_ORIGIN: &str = "RECORDER_ORIGIN";

/// Uploads to R2 at a time; Workers allow six open connections per invocation.
const MAX_UPLOADS: usize = 2;
/// How often the alarm looks after a recording meeting.
const HOUSEKEEPING_MS: u64 = 5 * 60 * 1000;
/// A stream without messages this long has its chunk closed; new messages start a new one.
const IDLE_STREAM_MS: u64 = 30 * 1000;
/// A recorder that heard nothing this long without an `/end` treats the meeting as over.
const ABANDONED_MS: u64 = 6 * 60 * 60 * 1000;
/// Chunks transcribed per alarm invocation.
const TRANSCRIBE_BATCH: u32 = 4;
/// Segments merged per alarm invocation.
const MERGE_BATCH: u32 = 300;
/// Close code for streams the recorder ends.
const CLOSE_NORMAL: u16 = 1000;

/// An adapter stream: speaker and adapter generation.
type StreamKey = (ParticipantId, u32);

/// The attachment of an adapter's socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct StreamTag {
    who: ParticipantId,
    generation: u32,
}

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
    /// Whether the meeting ended, so late adapter messages are refused. Set by `/end` and by the
    /// alarm, which also runs first after an eviction.
    ended: Cell<bool>,
}

#[derive(Default)]
struct Live {
    streams: HashMap<StreamKey, Stream>,
    uploads: Uploads,
}

struct Stream {
    capture: Capture,
    last_message: Millis,
}

#[derive(Default)]
struct Uploads {
    running: HashSet<ChunkId>,
    waiting: VecDeque<ChunkId>,
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
                ended: Cell::new(false),
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
            (Method::Post, RECORDER_STOP) => inner.stop().await,
            (Method::Post, RECORDER_END) => inner.end(req.json().await?).await,
            (Method::Get, RECORDER_STATUS) => inner.status().await,
            (Method::Get, file) => match file.strip_prefix('/') {
                Some(file @ (TRANSCRIPT_MD | TRANSCRIPT_JSON)) => inner.file(file).await,
                _ if is_websocket_upgrade(&req)? => inner.accept(&path).await,
                _ => Response::error("not found", 404),
            },
            _ => Response::error("not found", 404),
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
    /// and the step retried a minute later, because the runtime retries a failing alarm only
    /// six times.
    async fn alarm(&self) -> Result<Response> {
        let now = now();
        let next = match self.inner.work(now).await {
            Ok(next) => next,
            Err(e) => {
                console_error!("recorder: alarm step failed: {e}");
                Some(now.after(60_000))
            }
        };
        if let Some(at) = next
            && let Err(e) = self.inner.arm(at).await
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

    fn id(&self) -> String {
        self.state.id().to_string()
    }

    fn bucket(&self) -> Result<Bucket> {
        self.env.bucket(RECORDINGS)
    }

    // --- Requests from the room -------------------------------------------------------------

    async fn start(&self, start: StartRecording) -> Result<Response> {
        if self.load().await?.is_none() {
            let recorder = Recorder {
                meeting: start.meeting,
                started_at: start.since,
                secret: random::secret()?,
                speakers: Vec::new(),
                consents: Vec::new(),
                adapters: Vec::new(),
                phase: Phase::Recording,
            };
            self.save(&recorder).await?;
            self.arm(now().after(HOUSEKEEPING_MS)).await?;
            console_log!("recorder {}: recording started", self.id());
        }
        Response::empty().map(|r| r.with_status(204))
    }

    /// Records a speaker's audio track: creates an adapter for it, after closing the speaker's
    /// previous one if the track changed (a republish or a replaced session).
    async fn track(self: &Rc<Self>, track: TrackRecording) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return Response::error("unknown recorder", 404);
        };
        if recorder.phase != Phase::Recording {
            return Response::error("the meeting ended", 409);
        }
        let who = track.who;
        recorder.name_speaker(RosterEntry {
            id: who,
            name: track.name,
            role: track.role,
        });
        recorder.record_consent(track.consent);
        if recorder
            .open_adapter(who)
            .is_some_and(|a| a.session == track.session && a.track_name == track.track_name)
        {
            self.save(&recorder).await?;
            return Response::empty().map(|r| r.with_status(204));
        }
        let replaced = close_adapters(&mut recorder, |a| a.who == who);
        let generation = recorder.next_generation(who);
        recorder.adapters.push(Adapter {
            who,
            generation,
            session: track.session.clone(),
            track_name: track.track_name.clone(),
            id: None,
            open: true,
        });
        // Stored before the SFU call, so the adapter's socket is accepted the moment it opens.
        self.save(&recorder).await?;
        self.end_streams(&replaced);
        self.close_sfu_adapters(&replaced).await;

        let created = match self.endpoint(&recorder.secret, who, generation) {
            Ok(endpoint) => {
                self.create_adapter(track.session, track.track_name, endpoint)
                    .await
            }
            Err(e) => Err(e),
        };
        // Reloaded: the record may have changed while the SFU call was out.
        let mut recorder = self.load().await?.unwrap_or(recorder);
        let adapter = recorder
            .adapters
            .iter_mut()
            .find(|a| a.who == who && a.generation == generation);
        let mut orphaned = None;
        let response = match (created, adapter) {
            (Ok(id), Some(adapter)) => {
                adapter.id = Some(id);
                if !adapter.open {
                    // Closed (an untrack, a stop, the end) while the SFU created it.
                    orphaned = Some(adapter.clone());
                }
                Response::empty().map(|r| r.with_status(204))
            }
            (Err(e), Some(adapter)) => {
                adapter.open = false;
                console_error!("recorder: no adapter for {who:?} generation {generation}: {e}");
                Response::error("could not create the adapter", 502)
            }
            (_, None) => Response::error("adapter vanished", 409),
        };
        self.save(&recorder).await?;
        if let Some(adapter) = orphaned {
            self.close_sfu_adapters(&[adapter]).await;
        }
        response
    }

    async fn create_adapter(
        &self,
        session_id: SessionId,
        track_name: TrackName,
        endpoint: String,
    ) -> Result<AdapterId> {
        let Some(client) = sfu::client(&self.env) else {
            return Err("CALLS_APP_ID or CALLS_APP_SECRET is not set".into());
        };
        let request = AdapterTrack::Remote {
            session_id,
            track_name,
            endpoint,
            output_codec: OutputCodec::Pcm,
        };
        let mut results = client
            .create_adapters(vec![request])
            .await
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
        results
            .pop()
            .ok_or_else(|| "no adapter result".into())
            .and_then(|r| r.map_err(|e| worker::Error::RustError(e.to_string())))
    }

    /// The adapter's endpoint: the stream's path under [`RECORDER_ORIGIN`].
    fn endpoint(&self, secret: &str, who: ParticipantId, generation: u32) -> Result<String> {
        let origin = self.env.var(RECORDER_ORIGIN)?.to_string();
        let origin = origin.trim_end_matches('/');
        let ws_origin = if let Some(host) = origin.strip_prefix("https://") {
            format!("wss://{host}")
        } else if let Some(host) = origin.strip_prefix("http://") {
            format!("ws://{host}")
        } else {
            return Err(format!("{RECORDER_ORIGIN} must start with https:// or http://").into());
        };
        Ok(format!(
            "{ws_origin}/rec/{}/{secret}/{}/{generation}",
            self.id(),
            who.0
        ))
    }

    async fn untrack(self: &Rc<Self>, untrack: UntrackRecording) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return Response::error("unknown recorder", 404);
        };
        let closed = close_adapters(&mut recorder, |a| {
            a.who == untrack.who
                && a.session == untrack.session
                && a.track_name == untrack.track_name
        });
        if let Some(consent) = untrack.consent {
            recorder.record_consent(consent);
        }
        self.save(&recorder).await?;
        self.end_streams(&closed);
        self.close_sfu_adapters(&closed).await;
        Response::empty().map(|r| r.with_status(204))
    }

    async fn stop(self: &Rc<Self>) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return Response::error("unknown recorder", 404);
        };
        let closed = close_adapters(&mut recorder, |_| true);
        self.save(&recorder).await?;
        self.end_streams(&closed);
        self.close_sfu_adapters(&closed).await;
        Response::empty().map(|r| r.with_status(204))
    }

    /// The meeting is over: closes every stream, keeps the roster's names and the consent
    /// record, and starts the transcript job.
    async fn end(self: &Rc<Self>, finish: FinishRecording) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return Response::error("unknown recorder", 404);
        };
        if recorder.phase == Phase::Recording {
            self.finish(&mut recorder, Some(finish), now()).await?;
        }
        Response::empty().map(|r| r.with_status(204))
    }

    /// Ends the recording. `finish` is the room's roster and consent record, which replace what
    /// the `/track`s carried; without it (the room's `/end` never came) those stay.
    async fn finish(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        finish: Option<FinishRecording>,
        now: Millis,
    ) -> Result<()> {
        let closed = close_adapters(recorder, |_| true);
        if let Some(finish) = finish {
            for entry in finish.roster {
                recorder.name_speaker(entry);
            }
            recorder.consents = finish.consents;
        }
        recorder.phase = Phase::Ended {
            at: now,
            job: Job::Transcribing {
                retry_at: None,
                waiting_since: None,
            },
        };
        self.save(recorder).await?;
        self.ended.set(true);
        self.end_streams(&closed);
        self.finish_all_captures();
        self.close_sfu_adapters(&closed).await;
        console_log!(
            "recorder {}: meeting ended, transcript job starts",
            self.id()
        );
        self.arm(now).await
    }

    /// Closes adapters at the SFU, four per request. Failures are logged: an adapter whose
    /// socket the recorder closed stops streaming anyway, and closing is idempotent.
    async fn close_sfu_adapters(&self, adapters: &[Adapter]) {
        let ids: Vec<AdapterId> = adapters.iter().filter_map(|a| a.id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let Some(client) = sfu::client(&self.env) else {
            return;
        };
        for batch in ids.chunks(MAX_ADAPTERS_PER_REQUEST) {
            if let Err(e) = client.close_adapters(batch).await {
                console_warn!("recorder: closing adapters failed: {e}");
            }
        }
    }

    // --- Adapter streams --------------------------------------------------------------------

    /// Accepts an adapter's WebSocket for `/rec/{recorder}/{secret}/{who}/{generation}` if the
    /// secret matches and the adapter is open.
    async fn accept(&self, path: &str) -> Result<Response> {
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let ["rec", _, secret, who, generation] = parts.as_slice() else {
            return Response::error("not found", 404);
        };
        let (Ok(who), Ok(generation)) = (who.parse::<u32>(), generation.parse::<u32>()) else {
            return Response::error("not found", 404);
        };
        let Some(recorder) = self.load().await? else {
            return Response::error("not found", 404);
        };
        if !secret_matches(&recorder.secret, secret) {
            return Response::error("forbidden", 403);
        }
        let who = ParticipantId(who);
        if !recorder
            .adapters
            .iter()
            .any(|a| a.who == who && a.generation == generation && a.open)
        {
            return Response::error("adapter closed", 410);
        }
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        self.state.accept_web_socket(&server);
        server.serialize_attachment(StreamTag { who, generation })?;
        Response::from_websocket(client)
    }

    /// One adapter message: runs it through the stream's capture and stores what comes out.
    /// Synchronous on purpose: no other event runs in between, and no I/O is awaited.
    fn on_message(self: &Rc<Self>, ws: &WebSocket, bytes: &[u8]) {
        let Ok(Some(StreamTag { who, generation })) = ws.deserialize_attachment::<StreamTag>()
        else {
            return;
        };
        if self.ended.get() {
            if let Err(e) = ws.close(Some(CLOSE_NORMAL), Some("the meeting ended")) {
                console_warn!("recorder: close failed: {e}");
            }
            return;
        }
        let Ok(packet) = Packet::decode(bytes) else {
            console_warn!("recorder: malformed packet from {who:?}");
            return;
        };
        let key = (who, generation);
        let now = now();
        if !self.live.borrow().streams.contains_key(&key) {
            // First message since the stream began or since an eviction: chunks the previous
            // incarnation left open are closed first, so numbering continues after them.
            self.recover(now);
            let next = match self.store.next_index(who, generation) {
                Ok(next) => next,
                Err(e) => {
                    console_error!("recorder: reading chunk numbers failed: {e}");
                    return;
                }
            };
            self.live.borrow_mut().streams.insert(
                key,
                Stream {
                    capture: Capture::new(next),
                    last_message: now,
                },
            );
        }
        let mut notes = Vec::new();
        let pushed = {
            let mut live = self.live.borrow_mut();
            let stream = live.streams.get_mut(&key).expect("inserted above");
            stream.last_message = now;
            stream.capture.push(packet, now, &mut notes)
        };
        self.apply(key, notes);
        if let Err(e) = pushed {
            console_error!("recorder: encoding {who:?} failed: {e}");
        }
    }

    fn socket_gone(self: &Rc<Self>, ws: &WebSocket) {
        if let Ok(Some(StreamTag { who, generation })) = ws.deserialize_attachment::<StreamTag>() {
            self.finish_capture((who, generation));
        }
    }

    /// Closes the chunks and sockets of the adapters' streams.
    fn end_streams(self: &Rc<Self>, adapters: &[Adapter]) {
        for adapter in adapters {
            let key = (adapter.who, adapter.generation);
            self.finish_capture(key);
            for ws in self.state.get_websockets() {
                if let Ok(Some(tag)) = ws.deserialize_attachment::<StreamTag>()
                    && (tag.who, tag.generation) == key
                    && let Err(e) = ws.close(Some(CLOSE_NORMAL), Some("recording stopped"))
                {
                    console_warn!("recorder: close failed: {e}");
                }
            }
        }
    }

    /// Ends every capture still in memory, closing their open chunks.
    fn finish_all_captures(self: &Rc<Self>) {
        let open: Vec<StreamKey> = self.live.borrow().streams.keys().copied().collect();
        for key in open {
            self.finish_capture(key);
        }
    }

    /// Ends a stream's capture, closing its open chunk.
    fn finish_capture(self: &Rc<Self>, key: StreamKey) {
        let Some(mut stream) = self.live.borrow_mut().streams.remove(&key) else {
            return;
        };
        let mut notes = Vec::new();
        stream.capture.finish(&mut notes);
        self.apply(key, notes);
    }

    /// Stores what a capture reported and uploads closed chunks.
    fn apply(self: &Rc<Self>, (who, generation): StreamKey, notes: Vec<Note>) {
        let id = |index| ChunkId {
            who,
            generation,
            index,
        };
        for note in notes {
            let stored = match note {
                Note::Started { index, start } => self.store.chunk_started(id(index), start),
                Note::Spool { index, packets } => self.store.spool(id(index), packets),
                Note::Closed {
                    index,
                    duration_ms,
                    silent,
                } => {
                    let closed = self.store.chunk_closed(id(index), duration_ms, silent);
                    if closed.is_ok() {
                        self.upload(id(index));
                    }
                    closed
                }
            };
            if let Err(e) = stored {
                console_error!("recorder: storing a chunk of {who:?} failed: {e}");
            }
        }
    }

    /// Closes chunks that an eviction left open (no capture holds them) from their spool, and
    /// uploads every closed chunk that is not uploading yet and whose backoff is over.
    fn recover(self: &Rc<Self>, now: Millis) {
        let recovered = (|| -> Result<()> {
            let live: HashSet<StreamKey> = self.live.borrow().streams.keys().copied().collect();
            for row in self.store.chunks_in(ChunkState::Open, u32::MAX)? {
                let id = row.id();
                if live.contains(&(id.who, id.generation)) {
                    continue;
                }
                let frames: usize = self
                    .store
                    .spooled(id)?
                    .iter()
                    .map(|batch| spooled_packets(batch).count())
                    .sum();
                self.store.close_orphan(id, frames as u64 * 20)?;
            }
            for row in self.store.chunks_in(ChunkState::Closed, u32::MAX)? {
                if row.upload_after <= now.0 {
                    self.upload(row.id());
                }
            }
            Ok(())
        })();
        if let Err(e) = recovered {
            console_error!("recorder: recovering chunks failed: {e}");
        }
    }

    // --- Uploads ----------------------------------------------------------------------------

    /// Uploads a closed chunk in the background, or queues it while [`MAX_UPLOADS`] run.
    fn upload(self: &Rc<Self>, id: ChunkId) {
        {
            let mut live = self.live.borrow_mut();
            let uploads = &mut live.uploads;
            if uploads.running.contains(&id) || uploads.waiting.contains(&id) {
                return;
            }
            if uploads.running.len() >= MAX_UPLOADS {
                uploads.waiting.push_back(id);
                return;
            }
            uploads.running.insert(id);
        }
        let this = Rc::clone(self);
        // The DO State docs (checked 2026-10-09) say a promise passed to `waitUntil` "prevents
        // eviction until it settles or for up to 15 minutes". If the object goes anyway, the
        // chunk stays `closed` with its spool, and `recover` uploads it later.
        self.state.wait_until(async move {
            if let Err(e) = this.put_chunk(id).await {
                console_error!("recorder: uploading chunk {id:?} failed: {e}");
                if let Err(e) = this.store.upload_failed(id, now()) {
                    console_error!("recorder: counting the failed upload failed: {e}");
                }
            }
            let next = {
                let mut live = this.live.borrow_mut();
                live.uploads.running.remove(&id);
                live.uploads.waiting.pop_front()
            };
            if let Some(next) = next {
                this.upload(next);
            }
        });
    }

    /// Writes a closed chunk from its spool to R2 as one Ogg/Opus object, then drops the spool.
    async fn put_chunk(&self, id: ChunkId) -> Result<()> {
        let Some(row) = self
            .store
            .chunks_in(ChunkState::Closed, u32::MAX)?
            .into_iter()
            .find(|row| row.id() == id)
        else {
            return Ok(());
        };
        let batches = self.store.spooled(id)?;
        if batches
            .iter()
            .all(|batch| spooled_packets(batch).next().is_none())
        {
            return self.store.skip_empty(id);
        }
        let file = ogg_opus(batches.iter().flat_map(|batch| spooled_packets(batch)));
        let metadata = HashMap::from([
            ("startMs".to_owned(), row.start_ms.to_string()),
            ("durationMs".to_owned(), row.duration_ms.to_string()),
            ("silent".to_owned(), row.silent().to_string()),
            ("who".to_owned(), id.who.0.to_string()),
            ("n".to_owned(), id.generation.to_string()),
        ]);
        self.bucket()?
            .put(
                chunk_key(&self.id(), id.who, id.generation, id.index),
                Data::Bytes(file),
            )
            .http_metadata(HttpMetadata {
                content_type: Some("audio/ogg".into()),
                ..HttpMetadata::default()
            })
            .custom_metadata(metadata)
            .execute()
            .await?;
        self.store.chunk_stored(id)
    }

    // --- The alarm ----------------------------------------------------------------------------

    /// One step of what is due; returns when to run again.
    async fn work(self: &Rc<Self>, now: Millis) -> Result<Option<Millis>> {
        let Some(mut recorder) = self.load().await? else {
            return Ok(None);
        };
        match recorder.phase.clone() {
            Phase::Recording => self.look_after(&mut recorder, now).await,
            Phase::Ended { at, job } => {
                self.ended.set(true);
                self.run_job(&mut recorder, at, job, now).await
            }
        }
    }

    /// The transcript job's next step.
    async fn run_job(
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

    /// While the meeting runs: closes the chunks of streams that went quiet, recovers chunks an
    /// eviction left behind, and ends a recording nobody ended.
    async fn look_after(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        now: Millis,
    ) -> Result<Option<Millis>> {
        let idle: Vec<StreamKey> = self
            .live
            .borrow()
            .streams
            .iter()
            .filter(|(_, s)| now.0.saturating_sub(s.last_message.0) > IDLE_STREAM_MS)
            .map(|(&key, _)| key)
            .collect();
        for key in idle {
            self.finish_capture(key);
        }
        self.recover(now);
        let heard = self.store.audio_end()?.unwrap_or(recorder.started_at);
        if self.live.borrow().streams.is_empty() && now.0.saturating_sub(heard.0) > ABANDONED_MS {
            console_warn!(
                "recorder {}: no audio and no end for hours; ending",
                self.id()
            );
            self.finish(recorder, None, now).await?;
            return Ok(Some(now));
        }
        Ok(Some(now.after(HOUSEKEEPING_MS)))
    }

    /// Transcribes the next few stored chunks, once every chunk is in R2. `waiting_since` is
    /// when Workers AI first refused for its quota since a chunk last went through.
    async fn transcribe(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        waiting_since: Option<Millis>,
        now: Millis,
    ) -> Result<Option<Millis>> {
        // Streams still in memory (a socket the SFU has not closed yet) end here, not when the
        // socket goes.
        self.finish_all_captures();
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
        if recorder.phase == Phase::Recording {
            return Err("the meeting has not ended".into());
        }
        let paragraphs = self.store.paragraphs()?;
        let spoke: HashSet<ParticipantId> = paragraphs.iter().map(|p| p.who).collect();
        let speakers: Vec<RosterEntry> = recorder
            .speakers
            .iter()
            .filter(|s| spoke.contains(&s.id))
            .cloned()
            .collect();
        let transcript = Transcript::new(
            recorder.meeting.clone(),
            recorder.started_at,
            ended_at,
            label_speakers(&speakers),
            recorder.consents.clone(),
            gaps(self.store.spans()?),
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
        if let Phase::Ended {
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

    // --- The transcript page ------------------------------------------------------------------

    async fn status(&self) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return Response::error("unknown transcript", 404);
        };
        let (status, retry_at, audio_until) = recorder.phase.status();
        let transcript = match status {
            state::StatusKind::Ready => match self.read(TRANSCRIPT_JSON).await? {
                Some(text) => Some(RawValue::from_string(text)?),
                None => None,
            },
            _ => None,
        };
        let ended_at = match recorder.phase {
            Phase::Ended { at, .. } => Some(at),
            Phase::Recording => None,
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
    async fn file(&self, file: &str) -> Result<Response> {
        let Some(recorder) = self.load().await? else {
            return Response::error("unknown transcript", 404);
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

/// Marks the adapters that `which` selects closed; returns those that were open.
fn close_adapters(recorder: &mut Recorder, which: impl Fn(&Adapter) -> bool) -> Vec<Adapter> {
    let mut closed = Vec::new();
    for adapter in recorder.adapters.iter_mut().filter(|a| a.open && which(a)) {
        adapter.open = false;
        closed.push(adapter.clone());
    }
    closed
}

fn set_job(recorder: &mut Recorder, new: Job) {
    if let Phase::Ended { job, .. } = &mut recorder.phase {
        *job = new;
    }
}
