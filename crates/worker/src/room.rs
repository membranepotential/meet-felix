//! `RoomObject`: one Durable Object per meeting, running `meeting_core::step` over hibernating
//! WebSockets. Everything it needs lives in its storage [`Record`] and in socket
//! [`Attachment`]s, never in fields, so it behaves the same after being evicted and woken.
//!
//! Every handler loads the record, steps the core, stores the record, then carries out the
//! effects. Loading and storing are storage calls, so the runtime's input gate delivers no other
//! event in between and each step is atomic. Events can interleave while an SFU call (or a KV
//! call) is in flight; the core is built for that (replies about participants who left are
//! ignored), and [`RoomObject::execute_step`] carries out the socket effects of a step before its
//! SFU calls, so no socket the step closes or rebinds can slip an event into that gap.

use std::collections::VecDeque;

use meeting_core::{
    ClientMsg, Effect, ErrorCode, Event, MeetingId, Millis, ParticipantId, RoomState, ServerMsg,
    SfuReply,
};
use sfu_api::{Failure, failure_reply};
use worker::js_sys::{Date as JsDate, Function, Reflect};
use worker::wasm_bindgen::JsCast;
use worker::{
    DurableObject, Env, Method, Request, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, WebSocketRequestResponsePair, console_error,
    console_log, console_warn, durable_object,
};

use crate::internal::{INFO, INIT, InitRoom, OWNER_HEADER, OWNER_YES, RoomInfo};
use crate::record::{Attachment, OpenSocket, Record, alarm_time, is_stale};
use crate::{MEETINGS, is_websocket_upgrade, now, random, sfu};

/// The exact frame the runtime answers with [`PONG`] without waking the room
/// (`setWebSocketAutoResponse` matches the raw string). Clients send it at least every
/// [`crate::record::KEEPALIVE_MS`], or the keepalive check closes their socket.
const PING: &str = r#"{"type":"ping"}"#;
const PONG: &str = r#"{"type":"pong"}"#;

/// Upper bound on SFU effects one handler carries out, including those that follow from SFU
/// replies. The core never chains more than a few; this only stops a runaway loop.
const MAX_SFU_CALLS: usize = 64;

/// Close code for sockets the room closes: normal closure.
const CLOSE_NORMAL: u16 = 1000;
/// Close code for sockets whose client stopped pinging: going away.
const CLOSE_STALE: u16 = 1001;

#[durable_object]
pub struct RoomObject {
    state: State,
    env: Env,
}

impl DurableObject for RoomObject {
    fn new(state: State, env: Env) -> Self {
        let ping_pong =
            WebSocketRequestResponsePair::new(PING, PONG).expect("PING and PONG are plain strings");
        state.set_websocket_auto_response(&ping_pong);
        Self { state, env }
    }

    /// The router's [`INIT`] and [`INFO`] requests, and forwarded WebSocket upgrades. Only the
    /// router reaches the object, so [`OWNER_HEADER`] is the router's verdict.
    async fn fetch(&self, mut req: Request) -> Result<Response> {
        match (req.method(), req.path().as_str()) {
            (Method::Post, INIT) => self.init(req.json().await?).await,
            (Method::Get, INFO) => self.info().await,
            _ if is_websocket_upgrade(&req)? => {
                let owner = req.headers().get(OWNER_HEADER)?.as_deref() == Some(OWNER_YES);
                self.accept(owner).await
            }
            _ => Response::error("not found", 404),
        }
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let Some(Attachment::Open(socket)) = attachment(&ws)? else {
            return Ok(());
        };
        let msg = match message {
            WebSocketIncomingMessage::String(text) => serde_json::from_str::<ClientMsg>(&text).ok(),
            WebSocketIncomingMessage::Binary(_) => None,
        };
        let Some(msg) = msg else {
            return send(
                &ws,
                &ServerMsg::Error {
                    code: ErrorCode::MalformedMessage,
                },
            );
        };
        self.handle([Event::Client {
            from: socket.participant,
            msg,
            owner: socket.owner,
            fresh_token: random::resume_token()?,
        }])
        .await
    }

    // Completes the close handshake even if the room fails to handle the close. Deployed
    // Workers with compatibility date >= 2026-04-07 reply to Close frames themselves
    // (`web_socket_auto_reply_to_close`; calling `close` stays safe), but local `wrangler dev`
    // 4.147.0 does not, and clients then wait for the Close frame forever.
    async fn websocket_close(
        &self,
        ws: WebSocket,
        _code: usize,
        _reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        let gone = self.socket_gone(&ws).await;
        if let Err(e) = ws.close(Some(CLOSE_NORMAL), Some("bye")) {
            console_warn!("websocket_close: close failed: {e}");
        }
        gone
    }

    async fn websocket_error(&self, ws: WebSocket, _error: worker::Error) -> Result<()> {
        self.socket_gone(&ws).await
    }

    /// Fires for the core's alarm and for the keepalive check, whichever is earlier. Lets the
    /// core handle whatever is due (a spurious `AlarmFired` is harmless), reports and closes
    /// sockets whose client stopped pinging, and arms the next alarm.
    ///
    /// A stale socket is closed only once the step that reports it is stored: if storing fails,
    /// the runtime retries the alarm, which must still find the socket open and stale.
    async fn alarm(&self) -> Result<Response> {
        let now = now();
        let stale = self
            .open_sockets()
            .filter(|(ws, socket)| is_stale(now, socket.since, last_ping(&self.state, ws)))
            .map(|(ws, socket)| Input {
                event: Event::SocketClosed {
                    who: socket.participant,
                },
                close_stale: Some(ws),
            });
        let inputs: VecDeque<Input> = std::iter::once(Input::from(Event::AlarmFired))
            .chain(stale)
            .collect();
        self.run(inputs).await?;
        if let Some(Record::Open(open)) = self.load().await? {
            self.arm(open.alarm).await?;
        }
        Response::empty()
    }
}

impl RoomObject {
    async fn init(&self, init: InitRoom) -> Result<Response> {
        if self.load().await?.is_some() {
            return Response::error("room exists", 409);
        }
        let (room, effects) =
            RoomState::new(init.meeting, init.transcript, init.recording_policy, now());
        self.store(&Record::created(room, &effects)).await?;
        let replies = self.execute_step(effects, &mut 0).await?;
        self.handle(replies.into_iter().map(Event::Sfu)).await?;
        Response::empty()
    }

    async fn info(&self) -> Result<Response> {
        let Some(record) = self.load().await? else {
            return Response::error("unknown meeting", 404);
        };
        Response::from_json(&RoomInfo::new(record.recording_policy(), record.phase()))
    }

    /// Accepts a socket for a scheduled or running meeting and gives it the next participant
    /// id; `owner` says whether the router verified the owner's session on it.
    async fn accept(&self, owner: bool) -> Result<Response> {
        let open = match self.load().await? {
            None => return Response::error("unknown meeting", 404),
            Some(Record::Ended { .. }) => return Response::error("meeting ended", 410),
            Some(Record::Open(open)) => open,
        };
        let alarm = open.alarm;
        let (record, participant) = open.connect();
        self.store(&record).await?;
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        self.state.accept_web_socket(&server);
        server.serialize_attachment(Attachment::Open(OpenSocket {
            participant,
            since: now(),
            owner,
        }))?;
        self.arm(alarm).await?;
        Response::from_websocket(client)
    }

    /// Reports a socket that closed or failed to the core, unless the room closed it itself.
    /// The socket is marked closed only once the report is stored, so a failed store leaves it
    /// to be reported again. The runtime may report both an error and a close for the same
    /// socket; a second report in between is harmless, because the participant already left.
    async fn socket_gone(&self, ws: &WebSocket) -> Result<()> {
        let Some(Attachment::Open(socket)) = attachment(ws)? else {
            return Ok(());
        };
        self.handle([Event::SocketClosed {
            who: socket.participant,
        }])
        .await?;
        ws.serialize_attachment(Attachment::Closed)
    }

    /// Steps the core through `events` and every SFU reply that follows from them.
    async fn handle(&self, events: impl IntoIterator<Item = Event>) -> Result<()> {
        self.run(events.into_iter().map(Input::from).collect())
            .await
    }

    /// Steps the core through `inputs` and every SFU reply that follows from them. After each
    /// step it stores the record, closes the input's stale socket, if any, and carries out the
    /// effects.
    async fn run(&self, inputs: VecDeque<Input>) -> Result<()> {
        let mut queue = inputs;
        let mut sfu_calls = 0;
        while let Some(Input { event, close_stale }) = queue.pop_front() {
            let Some(Record::Open(open)) = self.load().await? else {
                // Ended, or never initialised: nothing listens any more.
                return Ok(());
            };
            let now = now();
            let (record, effects) = open.step(now, event);
            self.store(&record).await?;
            if let Some(ws) = close_stale {
                close(&ws, CLOSE_STALE, "no ping");
            }
            let replies = self.execute_step(effects, &mut sfu_calls).await?;
            queue.extend(
                replies
                    .into_iter()
                    .map(|reply| Input::from(Event::Sfu(reply))),
            );
        }
        Ok(())
    }

    /// Carries out one step's effects and returns the SFU replies to feed back.
    ///
    /// The socket, alarm, recording and lifecycle effects run first, in order, then the SFU
    /// calls, in order. Each SFU call awaits a fetch with the input gate open, so other events
    /// arrive meanwhile: a `CloseSocket` or `Rebind` behind an SFU call would let a kicked or
    /// replaced socket send a `Join` first and come back as a participant whose close is never
    /// reported. The core gets SFU replies back as events, so nothing it emits depends on how
    /// the two groups interleave.
    async fn execute_step(
        &self,
        effects: Vec<Effect>,
        sfu_calls: &mut usize,
    ) -> Result<Vec<SfuReply>> {
        let (sfu, local): (Vec<Effect>, Vec<Effect>) = effects.into_iter().partition(is_sfu_call);
        let mut replies = Vec::new();
        for effect in local.into_iter().chain(sfu) {
            replies.extend(self.execute(effect, sfu_calls).await?);
        }
        Ok(replies)
    }

    /// Carries out one effect; returns the SFU reply to feed back, if any.
    async fn execute(&self, effect: Effect, sfu_calls: &mut usize) -> Result<Option<SfuReply>> {
        match effect {
            Effect::Send { to, msg } => self.send_to(&[to], &msg)?,
            Effect::Broadcast { to, msg } => self.send_to(&to, &msg)?,
            Effect::CloseSocket { who } => {
                for (ws, _) in self.open_sockets().filter(|(_, s)| s.participant == who) {
                    close(&ws, CLOSE_NORMAL, "closed by the room");
                }
            }
            Effect::Rebind {
                socket,
                participant,
            } => {
                // The socket keeps its owner flag: the owner's socket stays the owner's.
                for (ws, open) in self.open_sockets().filter(|(_, s)| s.participant == socket) {
                    ws.serialize_attachment(Attachment::Open(OpenSocket {
                        participant,
                        ..open
                    }))?;
                }
            }
            Effect::SetAlarm { at } => self.arm(Some(at)).await?,
            Effect::StartRecording { .. }
            | Effect::RecordTrack { .. }
            | Effect::StopRecordingTrack { .. }
            | Effect::StopRecording => {
                // The RecorderObject comes in phase 3.
                console_log!("recording not implemented yet: {effect:?}");
            }
            Effect::ReleaseMeetingId { meeting } => {
                // Logged, not returned: `EndMeeting` must still close the sockets. The KV entry
                // expires with its TTL, and the room answers "ended" until then.
                if let Err(e) = self.release(&meeting).await {
                    console_error!("releasing {meeting:?} failed: {e}");
                }
            }
            Effect::EndMeeting => {
                for (ws, _) in self.open_sockets() {
                    close(&ws, CLOSE_NORMAL, "meeting ended");
                }
                self.state.storage().delete_alarm().await?;
            }
            Effect::SfuNewSession { .. }
            | Effect::SfuPublish { .. }
            | Effect::SfuSubscribe { .. }
            | Effect::SfuRenegotiate { .. }
            | Effect::SfuCloseTracks { .. }
            | Effect::SfuUpdateLayer { .. } => return Ok(self.call_sfu(&effect, sfu_calls).await),
        }
        Ok(None)
    }

    /// Makes the SFU call for `effect`; returns the reply to feed back, if the core expects one.
    async fn call_sfu(&self, effect: &Effect, sfu_calls: &mut usize) -> Option<SfuReply> {
        if *sfu_calls >= MAX_SFU_CALLS {
            // Fed back as a failure, so the participant is not left waiting for a reply; the
            // core retries through its alarm, not at once.
            console_error!("more than {MAX_SFU_CALLS} SFU calls in one handler; failed {effect:?}");
            return failure_reply(effect);
        }
        *sfu_calls += 1;
        let Some(client) = sfu::client(&self.env) else {
            console_warn!("CALLS_APP_ID or CALLS_APP_SECRET is not set; SFU calls fail");
            return failure_reply(effect);
        };
        match client.perform(effect).await {
            Ok(reply) => reply,
            Err(Failure { error, reply }) => {
                console_error!("SFU call failed: {error}");
                reply
            }
        }
    }

    /// Deletes the words' KV entry if it still points at this room; after the TTL the words
    /// may already belong to another meeting.
    async fn release(&self, meeting: &MeetingId) -> Result<()> {
        let kv = self.env.kv(MEETINGS)?;
        let this = self.state.id().to_string();
        if kv.get(meeting.as_str()).text().await?.as_deref() == Some(this.as_str()) {
            kv.delete(meeting.as_str()).await?;
        }
        Ok(())
    }

    /// Sets the object's alarm to the earlier of `core` and the next keepalive check.
    async fn arm(&self, core: Option<Millis>) -> Result<()> {
        let now = now();
        let open_sockets = self.open_sockets().next().is_some();
        if let Some(at) = alarm_time(core, open_sockets, now) {
            let offset = i64::try_from(at.0.saturating_sub(now.0)).unwrap_or(i64::MAX);
            self.state.storage().set_alarm(offset).await?;
        }
        Ok(())
    }

    /// Sends `msg` to the open sockets of `to`. A failed send is logged, so one dead socket
    /// does not stop delivery to the others.
    fn send_to(&self, to: &[ParticipantId], msg: &ServerMsg) -> Result<()> {
        let text = serde_json::to_string(msg)?;
        for (ws, _) in self
            .open_sockets()
            .filter(|(_, s)| to.contains(&s.participant))
        {
            if let Err(e) = ws.send_with_str(&text) {
                console_warn!("send failed: {e}");
            }
        }
        Ok(())
    }

    /// The sockets the room has not closed, with their attachment.
    fn open_sockets(&self) -> impl Iterator<Item = (WebSocket, OpenSocket)> {
        self.state
            .get_websockets()
            .into_iter()
            .filter_map(|ws| match attachment(&ws) {
                Ok(Some(Attachment::Open(socket))) => Some((ws, socket)),
                _ => None,
            })
    }

    async fn load(&self) -> Result<Option<Record>> {
        let json: Option<String> = self.state.storage().get(Record::KEY).await?;
        json.map(|json| serde_json::from_str(&json).map_err(worker::Error::from))
            .transpose()
    }

    async fn store(&self, record: &Record) -> Result<()> {
        self.state
            .storage()
            .put(Record::KEY, serde_json::to_string(record)?)
            .await
    }
}

/// One event for the core and, for a socket the keepalive check found stale, that socket, which
/// the room closes once the step reporting it is stored.
struct Input {
    event: Event,
    close_stale: Option<WebSocket>,
}

impl From<Event> for Input {
    fn from(event: Event) -> Self {
        Self {
            event,
            close_stale: None,
        }
    }
}

/// Whether `effect` is an SFU call, carried out after the step's other effects
/// ([`RoomObject::execute_step`]).
fn is_sfu_call(effect: &Effect) -> bool {
    match effect {
        Effect::SfuNewSession { .. }
        | Effect::SfuPublish { .. }
        | Effect::SfuSubscribe { .. }
        | Effect::SfuRenegotiate { .. }
        | Effect::SfuCloseTracks { .. }
        | Effect::SfuUpdateLayer { .. } => true,
        Effect::Send { .. }
        | Effect::Broadcast { .. }
        | Effect::CloseSocket { .. }
        | Effect::Rebind { .. }
        | Effect::SetAlarm { .. }
        | Effect::StartRecording { .. }
        | Effect::RecordTrack { .. }
        | Effect::StopRecordingTrack { .. }
        | Effect::StopRecording
        | Effect::ReleaseMeetingId { .. }
        | Effect::EndMeeting => false,
    }
}

fn attachment(ws: &WebSocket) -> Result<Option<Attachment>> {
    ws.deserialize_attachment()
}

fn send(ws: &WebSocket, msg: &ServerMsg) -> Result<()> {
    ws.send_with_str(serde_json::to_string(msg)?)
}

/// Marks the socket closed, so it delivers no events, and closes it.
fn close(ws: &WebSocket, code: u16, reason: &str) {
    let closed = ws
        .serialize_attachment(Attachment::Closed)
        .and_then(|()| ws.close(Some(code), Some(reason)));
    if let Err(e) = closed {
        console_warn!("close failed: {e}");
    }
}

/// When the socket last got an auto-response, from `getWebSocketAutoResponseTimestamp`, which
/// `worker` 0.8.7 does not bind.
fn last_ping(state: &State, ws: &WebSocket) -> Option<Millis> {
    let raw = state.as_raw();
    let method = Reflect::get(raw, &"getWebSocketAutoResponseTimestamp".into())
        .ok()?
        .dyn_into::<Function>()
        .ok()?;
    let socket: &worker::web_sys::WebSocket = ws.as_ref();
    let date = method.call1(raw, socket).ok()?.dyn_into::<JsDate>().ok()?;
    // Milliseconds since the epoch, always non-negative and integral for a real timestamp.
    Some(Millis(date.get_time() as u64))
}
