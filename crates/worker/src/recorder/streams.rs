//! The adapters' WebSockets and what comes of them: each message goes through its stream's
//! [`Capture`], the chunks go to SQLite, and closed chunks go to R2 in the background.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use meeting_core::recording::FRAME_MS;
use meeting_core::{Millis, ParticipantId};
use serde::{Deserialize, Serialize};
use worker::{
    Data, HttpMetadata, Response, Result, WebSocket, WebSocketPair, console_error, console_warn,
};

use super::capture::{Capture, Note};
use super::opus::{ogg_opus, spooled_packets};
use super::packet::Packet;
use super::state::{Adapter, chunk_key, secret_matches};
use super::store::{ChunkId, ChunkState};
use super::{CLOSE_NORMAL, Inner, not_found};
use crate::now;

/// Uploads to R2 at a time; Workers allow six open connections per invocation.
const MAX_UPLOADS: usize = 2;

/// An adapter stream: speaker and adapter generation.
pub(super) type StreamKey = (ParticipantId, u32);

/// The attachment of an adapter's socket: the stream it carries. A socket the recorder let go
/// ([`release`]) has none, so a message that still arrives on it is ignored, and its close
/// event does not end the stream's capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct StreamTag {
    who: ParticipantId,
    generation: u32,
}

/// The stream `ws` carries, unless the recorder let it go.
fn stream_of(ws: &WebSocket) -> Option<StreamKey> {
    match ws.deserialize_attachment::<StreamTag>() {
        Ok(tag) => tag.map(|StreamTag { who, generation }| (who, generation)),
        Err(e) => {
            console_warn!("recorder: unreadable socket attachment: {e}");
            None
        }
    }
}

/// Lets a socket go: removes its stream, so nothing it still sends is recorded, then closes it.
/// Removing the stream first matters, because messages can arrive until the adapter answers
/// the close, and the attachment survives hibernation and eviction.
fn release(ws: &WebSocket, reason: &str) {
    if let Err(e) = ws.serialize_attachment(Option::<StreamTag>::None) {
        console_warn!("recorder: detaching a socket failed: {e}");
    }
    if let Err(e) = ws.close(Some(CLOSE_NORMAL), Some(reason)) {
        console_warn!("recorder: close failed: {e}");
    }
}

/// What the recorder holds in memory; an eviction loses it, and the spool and the chunk states
/// in the store make up for that.
#[derive(Default)]
pub(super) struct Live {
    pub(super) streams: HashMap<StreamKey, Stream>,
    uploads: Uploads,
}

pub(super) struct Stream {
    capture: Capture,
    last_message: Millis,
}

#[derive(Default)]
struct Uploads {
    running: HashSet<ChunkId>,
    waiting: VecDeque<ChunkId>,
}

/// The secret and the stream in an adapter's path, `/rec/{recorder}/{secret}/{who}/{generation}`.
fn stream_path(path: &str) -> Option<(&str, StreamKey)> {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let ["rec", _, secret, who, generation] = parts.as_slice() else {
        return None;
    };
    Some((
        secret,
        (ParticipantId(who.parse().ok()?), generation.parse().ok()?),
    ))
}

impl Inner {
    /// Accepts an adapter's WebSocket for `/rec/{recorder}/{secret}/{who}/{generation}` if the
    /// secret matches and the adapter is open. The stream's earlier socket, if any, is let go:
    /// the adapter reconnects after a dropped connection, so the newer socket is the live one,
    /// and with one socket per stream the packets of two connections never interleave.
    pub(super) async fn accept(&self, path: &str) -> Result<Response> {
        let Some((secret, key @ (who, generation))) = stream_path(path) else {
            return not_found();
        };
        let Some(recorder) = self.load().await? else {
            return not_found();
        };
        if !secret_matches(&recorder.secret, secret) {
            return Response::error("forbidden", 403);
        }
        if !recorder
            .adapters
            .iter()
            .any(|a| a.who == who && a.generation == generation && a.open)
        {
            return Response::error("adapter closed", 410);
        }
        for ws in self.sockets_of(&[key]) {
            release(&ws, "replaced by a newer connection");
        }
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        self.state.accept_web_socket(&server);
        server.serialize_attachment(StreamTag { who, generation })?;
        Response::from_websocket(client)
    }

    /// One adapter message: runs it through the stream's capture and stores what comes out.
    /// Synchronous on purpose: no other event runs in between, and no I/O is awaited.
    pub(super) fn on_message(self: &Rc<Self>, ws: &WebSocket, bytes: &[u8]) {
        let Some(key @ (who, generation)) = stream_of(ws) else {
            return;
        };
        let Ok(packet) = Packet::decode(bytes) else {
            console_warn!("recorder: malformed packet from {who:?}");
            return;
        };
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

    pub(super) fn socket_gone(self: &Rc<Self>, ws: &WebSocket) {
        if let Some(key) = stream_of(ws) {
            self.finish_capture(key);
        }
    }

    /// The sockets that carry one of `streams`.
    fn sockets_of(&self, streams: &[StreamKey]) -> Vec<WebSocket> {
        self.state
            .get_websockets()
            .into_iter()
            .filter(|ws| stream_of(ws).is_some_and(|key| streams.contains(&key)))
            .collect()
    }

    /// Closes the chunks and lets go of the sockets of the adapters' streams. Every path that
    /// closes an adapter (an untrack, a stop, a newer track, the end) comes here, so a stream
    /// whose adapter is closed records nothing more.
    pub(super) fn end_streams(self: &Rc<Self>, adapters: &[Adapter]) {
        let keys: Vec<StreamKey> = adapters.iter().map(|a| (a.who, a.generation)).collect();
        for &key in &keys {
            self.finish_capture(key);
        }
        for ws in self.sockets_of(&keys) {
            release(&ws, "recording stopped");
        }
    }

    /// Closes every chunk and lets go of every socket: the meeting ended, and no audio after
    /// the end is recorded.
    pub(super) fn end_all_streams(self: &Rc<Self>) {
        self.finish_all_captures();
        for ws in self.state.get_websockets() {
            if stream_of(&ws).is_some() {
                release(&ws, "the meeting ended");
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

    /// Ends the captures of streams without messages for more than `idle_ms`, closing their
    /// open chunks; a new message starts a new chunk.
    pub(super) fn finish_idle_captures(self: &Rc<Self>, now: Millis, idle_ms: u64) {
        let idle: Vec<StreamKey> = self
            .live
            .borrow()
            .streams
            .iter()
            .filter(|(_, s)| now.0.saturating_sub(s.last_message.0) > idle_ms)
            .map(|(&key, _)| key)
            .collect();
        for key in idle {
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
    pub(super) fn recover(self: &Rc<Self>, now: Millis) {
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
                self.store.close_orphan(id, frames as u64 * FRAME_MS)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_adapter_path_names_the_secret_and_the_stream() {
        assert_eq!(
            stream_path("/rec/abc/s3cret/4/2"),
            Some(("s3cret", (ParticipantId(4), 2)))
        );
        for path in [
            "/rec/abc/s3cret/4",
            "/rec/abc/s3cret/4/2/extra",
            "/rec/abc/s3cret/x/2",
            "/rec/abc/s3cret/4/-1",
            "/transcript/abc/s3cret/4/2",
        ] {
            assert_eq!(stream_path(path), None, "{path}");
        }
    }
}
