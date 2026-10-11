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
//!
//! The recording effects go to the meeting's `RecorderObject` (`crate::recorder`), which the
//! room creates at the first `StartRecording` and remembers in its record; they are carried out
//! with the SFU calls, after the socket effects. They carry everything the recorder needs, so
//! the room only adds the recorder's id and each request's number.

use std::collections::VecDeque;

use meeting_core::http::ApiErrorCode;
use meeting_core::{
    ClientMsg, CloseCode, Effect, ErrorCode, Event, FailureCause, MeetingId, Millis, ParticipantId,
    RoomState, ServerMsg, SfuReply, Target, TranscriptRef,
};
use sfu_api::{Failure, failure_reply};
use worker::js_sys::{Date as JsDate, Function, Reflect};
use worker::wasm_bindgen::JsCast;
use worker::{
    DurableObject, Env, Method, Request, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, WebSocketRequestResponsePair, console_error,
    console_log, console_warn, durable_object,
};

use crate::internal::{
    self, FinishRecording, INFO, INIT, InitRoom, OWNER_HEADER, OWNER_YES, RecorderId,
    RecorderRequest, RoomId, StartRecording, TrackRecording, UntrackRecording,
};
use crate::jurisdiction::{self, Jurisdiction};
use crate::record::{
    Attachment, Decoded, ENDED_RETENTION_MS, EndedAlarm, EndedRoom, FINISH_RETRY_MS, MessageBudget,
    OpenRoom, OpenSocket, PendingFinish, Record, RecorderLink, Stepped, Write, crowded_out,
    is_stale, is_unbound, join_overdue, write_for,
};
use crate::recorder::{Ordered, RequestSeq, Stop};
use crate::{RECORDERS, ROOM_JURISDICTION, is_websocket_upgrade, kv, now, random, reply, sfu};

/// The exact frame the runtime answers with [`PONG`] without waking the room
/// (`setWebSocketAutoResponse` matches the raw string). Clients send it at least every
/// [`crate::record::KEEPALIVE_MS`], or the keepalive check closes their socket.
const PING: &str = r#"{"type":"ping"}"#;
const PONG: &str = r#"{"type":"pong"}"#;

/// Upper bound on SFU effects one handler carries out, including those that follow from SFU
/// replies. The core never chains more than a few; this only stops a runaway loop.
const MAX_SFU_CALLS: usize = 64;

/// How much of a frame's parse error the log quotes: enough for the field and the problem,
/// too little for the SDP a malformed offer may carry.
const LOGGED_ERROR_CHARS: usize = 160;

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
        let Some(budget) = socket.budget.spend(now()) else {
            console_warn!(
                "{:?} sent more than its message budget; closing its socket",
                socket.participant
            );
            let input = Input {
                event: Event::SocketClosed {
                    who: socket.participant,
                },
                close: Some((ws, CloseCode::OverBudget)),
            };
            return self.run(VecDeque::from([input])).await;
        };
        ws.serialize_attachment(Attachment::Open(OpenSocket { budget, ..socket }))?;
        let parsed = match message {
            WebSocketIncomingMessage::String(text) => {
                serde_json::from_str::<ClientMsg>(&text).map_err(|e| e.to_string())
            }
            WebSocketIncomingMessage::Binary(_) => Err("a binary frame".to_owned()),
        };
        let msg = match parsed {
            Ok(msg) => msg,
            Err(error) => {
                let error: String = error.chars().take(LOGGED_ERROR_CHARS).collect();
                console_warn!("{:?} sent a malformed message: {error}", socket.participant);
                return send(
                    &ws,
                    &ServerMsg::Error {
                        code: ErrorCode::MalformedMessage,
                    },
                );
            }
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
    //
    // Logs the close code: which codes a closing tab, a phone that drops a backgrounded socket,
    // and a lost network produce is undocumented, and decides whether a close could ever skip
    // the grace period (D35 in docs/core).
    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        _reason: String,
        was_clean: bool,
    ) -> Result<()> {
        console_log!("websocket_close: code {code}, clean {was_clean}");
        let gone = self.socket_gone(&ws).await;
        // The answer to the client's close: a normal closure.
        let normal = CloseCode::Left;
        if let Err(e) = ws.close(Some(normal.code()), Some(normal.reason())) {
            console_warn!("websocket_close: close failed: {e}");
        }
        gone
    }

    async fn websocket_error(&self, ws: WebSocket, error: worker::Error) -> Result<()> {
        console_log!("websocket_error: {error}");
        self.socket_gone(&ws).await
    }

    /// In an open room, fires for the core's alarm and for the keepalive check
    /// ([`RoomObject::open_alarm`]); in an ended room, to send a failed `/end` again or to delete
    /// the storage ([`RoomObject::ended_alarm`]).
    async fn alarm(&self) -> Result<Response> {
        match self.load().await? {
            Some(Record::Open(open)) => self.open_alarm(&open).await?,
            Some(Record::Ended(ended)) => self.ended_alarm(ended).await?,
            None => {}
        }
        Response::empty()
    }
}

impl RoomObject {
    /// The alarm of an open room, for whichever is earliest of the core's alarm, a socket that
    /// may have gone stale, and a socket whose time to join is over. Lets the core handle
    /// whatever is due (a spurious `AlarmFired` is harmless), reports and closes sockets whose
    /// client stopped pinging, closes sockets that are bound to nobody in the room after
    /// [`crate::record::JOIN_TIMEOUT_MS`], reports participants who have no socket at all
    /// ([`OpenRoom::orphans`]), and arms the next alarm. If the meeting ends meanwhile,
    /// `EndMeeting` armed the ended room's alarm already.
    ///
    /// A stale socket is closed only once the step that reports it is stored: if storing fails,
    /// the runtime retries the alarm, which must still find the socket open and stale. An
    /// unbound socket is closed at once: the core knows no participant of it to report.
    async fn open_alarm(&self, open: &OpenRoom) -> Result<()> {
        let now = now();
        let present = open.present();
        for (ws, _) in self
            .open_sockets()
            .filter(|(_, socket)| is_unbound(socket, &present) && join_overdue(now, socket.since))
        {
            close(&ws, CloseCode::NoJoin);
        }
        let sockets: Vec<_> = self.open_sockets().collect();
        let attached: Vec<OpenSocket> = sockets.iter().map(|(_, socket)| *socket).collect();
        let orphans = open
            .orphans(&attached)
            .into_iter()
            .map(|who| Input::from(Event::SocketClosed { who }));
        let stale = sockets
            .into_iter()
            .filter(|(ws, socket)| is_stale(now, socket.since, last_ping(&self.state, ws)))
            .map(|(ws, socket)| Input {
                event: Event::SocketClosed {
                    who: socket.participant,
                },
                close: Some((ws, CloseCode::Stale)),
            });
        // The reports go first: they make no SFU call, while `AlarmFired` may (a grace that
        // expires closes tracks), and a client could resume on a new socket during that call.
        // `run` also drops a report whose socket the resume closed meanwhile.
        let inputs: VecDeque<Input> = orphans
            .chain(stale)
            .chain(std::iter::once(Input::from(Event::AlarmFired)))
            .collect();
        self.run(inputs).await?;
        self.arm().await
    }

    /// The alarm of an ended room: sends a pending `/end` once more and waits the retention
    /// again, or, with nothing pending, deletes all storage. With the compatibility date
    /// 2026-02-24 or later, `deleteAll()` deletes the alarm too. Afterwards the room answers
    /// like one the router never initialised: 404 to sockets and `/info`.
    async fn ended_alarm(&self, ended: EndedRoom) -> Result<()> {
        match ended.alarm() {
            EndedAlarm::Delete => self.state.storage().delete_all().await,
            EndedAlarm::FinishAgain { record, pending } => {
                self.store(&Record::Ended(record)).await?;
                self.delete_later().await?;
                let end = RecorderRequest::End(pending.finish);
                if let Err(e) = self.post_recorder(&pending.recorder, &end).await {
                    console_error!("the recorder's end failed again: {e}");
                }
                Ok(())
            }
        }
    }

    /// Arms the ended room's alarm, which deletes its storage after [`ENDED_RETENTION_MS`].
    async fn delete_later(&self) -> Result<()> {
        self.state
            .storage()
            .set_alarm(i64::try_from(ENDED_RETENTION_MS).unwrap_or(i64::MAX))
            .await
    }

    async fn init(&self, init: InitRoom) -> Result<Response> {
        if self.load().await?.is_some() {
            return Response::error("room exists", 409);
        }
        let (room, effects) =
            RoomState::new(init.meeting, init.transcript, init.recording_policy, now());
        self.store(&Record::created(room, &effects)).await?;
        let replies = self.execute_step(effects, None, &mut 0).await?;
        self.handle(replies.into_iter().map(Event::Sfu)).await?;
        Response::empty()
    }

    async fn info(&self) -> Result<Response> {
        let Some(record) = self.load().await? else {
            return Response::error("unknown meeting", 404);
        };
        Response::from_json(&record.lookup())
    }

    /// Accepts a socket for a scheduled or running meeting and gives it the next participant
    /// id; `owner` says whether the router verified the owner's session on it. Never refuses
    /// one, as it may be a client resuming its place (D35): to keep the sockets bounded, it
    /// closes the oldest sockets bound to nobody in the room instead ([`crowded_out`]), and the
    /// alarm closes such a socket once [`crate::record::JOIN_TIMEOUT_MS`] passed without a `Join`.
    async fn accept(&self, owner: bool) -> Result<Response> {
        let open = match self.load().await? {
            None => return reply::error(ApiErrorCode::NotFound),
            Some(Record::Ended(_)) => return reply::error(ApiErrorCode::MeetingEnded),
            Some(Record::Open(open)) => open,
        };
        let present = open.present();
        let unbound = self
            .open_sockets()
            .filter(|(_, socket)| is_unbound(socket, &present))
            .collect();
        for ws in crowded_out(unbound) {
            close(&ws, CloseCode::Crowded);
        }
        let (record, participant) = open.connect();
        self.store(&record).await?;
        let WebSocketPair { client, server } = WebSocketPair::new()?;
        self.state.accept_web_socket(&server);
        server.serialize_attachment(Attachment::Open(OpenSocket {
            participant,
            since: now(),
            owner,
            budget: MessageBudget::default(),
        }))?;
        // Wakes the room when the socket's time to join is over, unless the alarm is due
        // earlier anyway; a spurious `AlarmFired` is harmless to the core.
        self.arm().await?;
        Response::from_websocket(client)
    }

    /// Reports a socket that closed or failed to the core, unless the room closed it itself.
    /// The socket is marked closed only once the report is stored, so a failed store leaves it
    /// to be reported again. The runtime may report both an error and a close for the same
    /// socket; a second report in between is harmless, because the core ignores it: the participant
    /// left already, or waits to resume since the first report.
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
    /// step it stores the record if the step changed it, closes the input's socket, if any, and
    /// carries out the effects. A step that changed nothing (a repeated `SetMuted`, a message
    /// the core refused) writes nothing, so it costs no row of the daily write allowance.
    async fn run(&self, inputs: VecDeque<Input>) -> Result<()> {
        let mut queue = inputs;
        let mut sfu_calls = 0;
        while let Some(Input { event, close }) = queue.pop_front() {
            // A socket that a resume on a new socket closed meanwhile (`CloseSocket` marks it
            // `Closed`) must not report the resumed participant as lost.
            if let Some((ws, _)) = &close
                && !matches!(attachment(ws)?, Some(Attachment::Open(_)))
            {
                continue;
            }
            let Some((Record::Open(open), stored)) = self.load_stored().await? else {
                // Ended, or never initialised: nothing listens any more.
                return Ok(());
            };
            let now = now();
            let writer = match &event {
                Event::Client { from, .. } => Some(*from),
                _ => None,
            };
            let Stepped {
                mut record,
                effects,
                recorder,
            } = open.step(now, event, || self.new_recorder());
            match write_for(&stored, &mut record, writer, now)? {
                Write::Skip => {}
                Write::Store(json) => self.state.storage().put(Record::KEY, json).await?,
                Write::Refuse(who) => {
                    // Dropped as if never received; the client resumes and catches up from its
                    // welcome, its budget still spent.
                    console_warn!("{who:?} used up its write budget; closing its sockets");
                    queue.extend(
                        self.open_sockets()
                            .filter(|(_, s)| s.participant == who)
                            .map(|(ws, _)| Input {
                                event: Event::SocketClosed { who },
                                close: Some((ws, CloseCode::OverBudget)),
                            }),
                    );
                    continue;
                }
            }
            if let Some((ws, why)) = close {
                self::close(&ws, why);
            }
            let replies = self.execute_step(effects, recorder, &mut sfu_calls).await?;
            queue.extend(
                replies
                    .into_iter()
                    .map(|reply| Input::from(Event::Sfu(reply))),
            );
        }
        Ok(())
    }

    /// A new `RecorderObject` id in the rooms' jurisdiction; `None`, logged, if the runtime
    /// cannot make one. The meeting then goes on without its recording.
    fn new_recorder(&self) -> Option<String> {
        let id = || -> Result<String> {
            let jurisdiction =
                Jurisdiction::parse_var(&self.env.var(ROOM_JURISDICTION)?.to_string())?;
            let recorders = self.env.durable_object(RECORDERS)?;
            Ok(jurisdiction::unique_id(&recorders, jurisdiction)?.to_string())
        };
        id().inspect_err(|e| console_error!("creating the recorder failed: {e}"))
            .ok()
    }

    /// Carries out one step's effects and returns the SFU replies to feed back.
    ///
    /// The socket, alarm and lifecycle effects run first, in order, then the SFU and recorder
    /// calls, in order. Each SFU call awaits a fetch with the input gate open, so other events
    /// arrive meanwhile: a `CloseSocket` or `Rebind` behind an SFU call would let a kicked or
    /// replaced socket send a `Join` first and come back as a participant whose close is never
    /// reported. The core gets SFU replies back as events, so nothing it emits depends on how
    /// the two groups interleave. Recorder calls of different steps may still reach the
    /// recorder out of order; each carries its number for that
    /// ([`RecorderLink::first_request`]).
    async fn execute_step(
        &self,
        effects: Vec<Effect>,
        recorder: Option<RecorderLink>,
        sfu_calls: &mut usize,
    ) -> Result<Vec<SfuReply>> {
        // `partition` keeps the order, and every recorder call is remote, so the recorder calls
        // run in effect order and take their numbers in the order `OpenRoom::step` counted.
        let (remote, local): (Vec<Effect>, Vec<Effect>) = effects
            .into_iter()
            .partition(|effect| effect.target() != Target::Room);
        let mut requests = recorder.map(|link| Requests {
            next: link.first_request,
            recorder: link.id,
        });
        let mut replies = Vec::new();
        for effect in local.into_iter().chain(remote) {
            replies.extend(self.execute(effect, requests.as_mut(), sfu_calls).await?);
        }
        Ok(replies)
    }

    /// Carries out one effect; returns the SFU reply to feed back, if any.
    async fn execute(
        &self,
        effect: Effect,
        recorder: Option<&mut Requests>,
        sfu_calls: &mut usize,
    ) -> Result<Option<SfuReply>> {
        match effect {
            Effect::Send { to, msg } => self.send_to(&[to], &msg)?,
            Effect::Broadcast { to, msg } => self.send_to(&to, &msg)?,
            Effect::CloseSocket { who, why } => {
                for (ws, _) in self.open_sockets().filter(|(_, s)| s.participant == who) {
                    close(&ws, why);
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
            // The stored record holds the core's time; `arm` adds the sockets' times.
            Effect::SetAlarm { .. } => self.arm().await?,
            Effect::StartRecording { .. }
            | Effect::RecordTrack { .. }
            | Effect::StopRecordingTrack { .. }
            | Effect::StopRecording
            | Effect::FinishRecording { .. } => match recorder {
                Some(recorder) => self.tell_recorder(recorder, effect).await,
                None => console_error!("no recorder for a recording effect"),
            },
            Effect::ReleaseMeetingId { meeting } => {
                // Logged, not returned: `EndMeeting` must still close the sockets. The KV entry
                // expires with its TTL; the room answers "ended" until it deletes its storage,
                // then "unknown meeting".
                if let Err(e) = self.release(&meeting).await {
                    console_error!("releasing {meeting:?} failed: {e}");
                }
            }
            Effect::EndMeeting => {
                for (ws, _) in self.open_sockets() {
                    close(&ws, CloseCode::Ended);
                }
                // Replaces the core's last alarm; a failed `/end` re-arms it sooner.
                self.delete_later().await?;
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

    /// Carries out a recording effect at the recorder, with the effect's number. Failures are
    /// logged, not returned: the meeting goes on without its recording rather than failing.
    async fn tell_recorder(&self, requests: &mut Requests, effect: Effect) {
        // Taken first, so every recording effect uses up its number as `OpenRoom::step` counted.
        let seq = requests.next;
        requests.next = seq.after(1);
        let recorder = requests.recorder.as_str();
        if let Effect::StartRecording {
            meeting,
            transcript,
            ..
        } = &effect
        {
            // The link works from now on, showing "recording" until the meeting ends.
            self.link_transcript(recorder, meeting, transcript).await;
        }
        let Some(request) = recorder_request(effect, seq) else {
            return;
        };
        let told = self.post_recorder(recorder, &request).await;
        if let (Err(_), RecorderRequest::End(finish)) = (&told, request) {
            let pending = PendingFinish {
                recorder: recorder.to_owned(),
                finish,
            };
            if let Err(e) = self.finish_later(pending).await {
                console_error!("keeping the recorder's end for later failed: {e}");
            }
        }
        if let Err(e) = told {
            console_error!("the recorder failed: {e}");
        }
    }

    /// Keeps a failed `/end` in the ended record and arms the alarm to send it once more, which
    /// also postpones the deletion of the storage ([`RoomObject::ended_alarm`]).
    async fn finish_later(&self, pending: PendingFinish) -> Result<()> {
        let Some(record) = self.load().await? else {
            return Ok(());
        };
        self.store(&record.with_pending_finish(pending)).await?;
        self.state
            .storage()
            .set_alarm(i64::try_from(FINISH_RETRY_MS).unwrap_or(i64::MAX))
            .await
    }

    async fn post_recorder(&self, recorder: &str, request: &RecorderRequest) -> Result<()> {
        let stub = RecorderId::new(recorder).stub(&self.env.durable_object(RECORDERS)?)?;
        internal::post(&stub, request.path(), request)
            .await
            .map(drop)
    }

    /// Maps the transcript token to the recorder in KV, for `GET /api/transcript/:token`. The
    /// entry has no expiry: transcripts stay until the owner deletes them (D18). Logged on failure
    /// without the token, which is the link's only secret.
    async fn link_transcript(
        &self,
        recorder: &str,
        meeting: &MeetingId,
        transcript: &TranscriptRef,
    ) {
        let linked = async {
            let kv = kv::open(&self.env)?;
            kv::link_transcript(&kv, transcript, &RecorderId::new(recorder)).await
        };
        if let Err(e) = linked.await {
            console_error!("linking the transcript of {meeting:?} failed: {e}");
        }
    }

    /// Makes the SFU call for `effect`; returns the reply to feed back, if the core expects one.
    async fn call_sfu(&self, effect: &Effect, sfu_calls: &mut usize) -> Option<SfuReply> {
        if *sfu_calls >= MAX_SFU_CALLS {
            // Fed back as a failure, so the participant is not left waiting for a reply; the
            // core retries through its alarm, not at once.
            console_error!(
                "more than {MAX_SFU_CALLS} SFU calls in one handler; failed {}",
                describe_call(effect)
            );
            return failure_reply(effect, FailureCause::Transient);
        }
        *sfu_calls += 1;
        let Some(client) = sfu::client(&self.env) else {
            console_warn!("CALLS_APP_ID or CALLS_APP_SECRET is not set; SFU calls fail");
            return failure_reply(effect, FailureCause::Transient);
        };
        match client.perform(effect).await {
            Ok(reply) => reply,
            // A track the SFU does not know yet is expected on a first pull; the core pulls
            // again soon, so it is no error.
            Err(Failure { error, reply }) if error.cause() == FailureCause::TrackNotFound => {
                console_log!("{} failed, retried soon: {error}", describe_call(effect));
                reply
            }
            Err(Failure { error, reply }) => {
                console_error!("{} failed: {error}", describe_call(effect));
                reply
            }
        }
    }

    /// Deletes the words' KV entry if it still points at this room; after the TTL the words
    /// may already belong to another meeting.
    async fn release(&self, meeting: &MeetingId) -> Result<()> {
        let this = RoomId::new(self.state.id().to_string());
        kv::release_words(&kv::open(&self.env)?, meeting, &this).await
    }

    /// Sets the object's alarm of an open room to [`OpenRoom::alarm_time`] over the stored
    /// record and the open sockets. That time covers every reason the alarm serves, so it
    /// replaces whatever was set before: an earlier time that is still needed is part of it.
    async fn arm(&self) -> Result<()> {
        let Some(Record::Open(open)) = self.load().await? else {
            return Ok(());
        };
        let sockets: Vec<_> = self
            .open_sockets()
            .map(|(ws, socket)| (socket, last_ping(&self.state, &ws)))
            .collect();
        if let Some(at) = open.alarm_time(&sockets) {
            let offset = i64::try_from(at.0.saturating_sub(now().0)).unwrap_or(i64::MAX);
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

    /// The stored record.
    async fn load(&self) -> Result<Option<Record>> {
        Ok(self.load_stored().await?.map(|(record, _)| record))
    }

    /// The stored record and its JSON as stored. A record of an older version is logged and
    /// replaced by an ended one ([`crate::record::decode`]), which deletes itself like any other.
    async fn load_stored(&self) -> Result<Option<(Record, String)>> {
        let json: Option<String> = self.state.storage().get(Record::KEY).await?;
        let Some(json) = json else {
            return Ok(None);
        };
        match crate::record::decode(&json)? {
            Decoded::Current(record) => Ok(Some((record, json))),
            Decoded::Outdated { version } => {
                console_warn!(
                    "room {}: its record has the old version {version}; the meeting counts as ended",
                    self.state.id()
                );
                let ended = Record::Ended(EndedRoom::default());
                let json = crate::record::encode(&ended)?;
                self.state.storage().put(Record::KEY, &json).await?;
                self.delete_later().await?;
                Ok(Some((ended, json)))
            }
        }
    }

    async fn store(&self, record: &Record) -> Result<()> {
        self.state
            .storage()
            .put(Record::KEY, crate::record::encode(record)?)
            .await
    }
}

/// One event for the core and, for a socket the room closes itself (the keepalive check found
/// it stale, or it overspent its message budget), that socket, which the room closes once the
/// step reporting it is stored.
struct Input {
    event: Event,
    close: Option<(WebSocket, CloseCode)>,
}

impl From<Event> for Input {
    fn from(event: Event) -> Self {
        Self { event, close: None }
    }
}

/// Where one step's recorder requests go: the recorder, and the number the next one carries.
struct Requests {
    /// The `RecorderObject`'s id (hex).
    recorder: String,
    next: RequestSeq,
}

/// The request a recording effect makes of the recorder, numbered `seq` where the recorder
/// orders requests; `None` for any other effect.
fn recorder_request(effect: Effect, seq: RequestSeq) -> Option<RecorderRequest> {
    Some(match effect {
        Effect::StartRecording { since, meeting, .. } => {
            RecorderRequest::Start(StartRecording { meeting, since })
        }
        Effect::RecordTrack {
            speaker,
            consent,
            track,
        } => RecorderRequest::Track(Ordered {
            seq,
            request: TrackRecording {
                who: speaker.id,
                name: speaker.name,
                role: speaker.role,
                consent,
                session: track.session,
                track_name: track.name,
            },
        }),
        Effect::StopRecordingTrack {
            who,
            track,
            consent,
        } => RecorderRequest::Untrack(Ordered {
            seq,
            request: UntrackRecording {
                who,
                session: track.session,
                track_name: track.name,
                consent,
            },
        }),
        Effect::StopRecording => RecorderRequest::Stop(Ordered {
            seq,
            request: Stop {},
        }),
        Effect::FinishRecording { roster, consents } => {
            RecorderRequest::End(FinishRecording { roster, consents })
        }
        _ => return None,
    })
}

/// Names the SFU call an effect makes, its participant and its session, for the log, e.g.
/// `SFU Subscribe for ParticipantId(0) on session 2a45…`.
fn describe_call(effect: &Effect) -> String {
    let (call, who, session) = match effect {
        Effect::SfuNewSession { who, kind } => {
            return format!("SFU NewSession({kind:?}) for {who:?}");
        }
        Effect::SfuPublish { who, session, .. } => ("Publish", who, session),
        Effect::SfuSubscribe { who, session, .. } => ("Subscribe", who, session),
        Effect::SfuRenegotiate { who, session, .. } => ("Renegotiate", who, session),
        Effect::SfuCloseTracks { who, session, .. } => ("CloseTracks", who, session),
        Effect::SfuUpdateLayer { who, session, .. } => ("UpdateLayer", who, session),
        other => return format!("{other:?}"),
    };
    format!("SFU {call} for {who:?} on session {}", session.as_str())
}

fn attachment(ws: &WebSocket) -> Result<Option<Attachment>> {
    ws.deserialize_attachment()
}

fn send(ws: &WebSocket, msg: &ServerMsg) -> Result<()> {
    ws.send_with_str(serde_json::to_string(msg)?)
}

/// Marks the socket closed, so it delivers no events, and closes it with the code and reason
/// of `why`.
fn close(ws: &WebSocket, why: CloseCode) {
    let closed = ws
        .serialize_attachment(Attachment::Closed)
        .and_then(|()| ws.close(Some(why.code()), Some(why.reason())));
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

#[cfg(test)]
mod tests {
    use meeting_core::recording::{ConsentRecord, RosterEntry};
    use meeting_core::{DisplayName, RemoteTrack, Role, SessionId, TrackName};
    use serde_json::json;

    use super::*;

    /// Every recording effect becomes the request the recorder reads at that path, with the
    /// effect's data, and the numbered ones carry the room's number beside their own fields.
    #[test]
    fn the_recorder_reads_each_request_the_room_posts() {
        let seq = RequestSeq(7);
        let who = ParticipantId(3);
        let name = DisplayName::parse("Robin").expect("a valid name");
        let meeting = MeetingId::parse("amber-otter-ocean").expect("listed words");
        let speaker = RosterEntry {
            id: who,
            name: name.clone(),
            role: Role::Host,
        };
        let consent = ConsentRecord {
            who,
            name: name.clone(),
            at: Millis(5),
            notice_version: 1,
            withdrawn_at: None,
        };
        let withdrawn = ConsentRecord {
            withdrawn_at: Some(Millis(9)),
            ..consent.clone()
        };
        let track = RemoteTrack {
            session: SessionId::new("session"),
            name: TrackName::new("audio"),
        };
        let cases = [
            (
                Effect::StartRecording {
                    since: Millis(1),
                    meeting: meeting.clone(),
                    transcript: TranscriptRef::parse("0123456789abcdef0123456789abcdef")
                        .expect("a token"),
                },
                "/start",
                RecorderRequest::Start(StartRecording {
                    meeting,
                    since: Millis(1),
                }),
            ),
            (
                Effect::RecordTrack {
                    speaker: speaker.clone(),
                    consent: consent.clone(),
                    track: track.clone(),
                },
                "/track",
                RecorderRequest::Track(Ordered {
                    seq,
                    request: TrackRecording {
                        who,
                        name: name.clone(),
                        role: Role::Host,
                        consent: consent.clone(),
                        session: track.session.clone(),
                        track_name: track.name.clone(),
                    },
                }),
            ),
            (
                Effect::StopRecordingTrack {
                    who,
                    track: track.clone(),
                    consent: Some(withdrawn.clone()),
                },
                "/untrack",
                RecorderRequest::Untrack(Ordered {
                    seq,
                    request: UntrackRecording {
                        who,
                        session: track.session.clone(),
                        track_name: track.name.clone(),
                        consent: Some(withdrawn.clone()),
                    },
                }),
            ),
            (
                Effect::StopRecording,
                "/stop",
                RecorderRequest::Stop(Ordered {
                    seq,
                    request: Stop {},
                }),
            ),
            (
                Effect::FinishRecording {
                    roster: vec![speaker.clone()],
                    consents: vec![withdrawn.clone()],
                },
                "/end",
                RecorderRequest::End(FinishRecording {
                    roster: vec![speaker],
                    consents: vec![withdrawn],
                }),
            ),
        ];
        for (effect, path, expected) in cases {
            let request = recorder_request(effect, seq).expect("a recording effect");
            assert_eq!((request.path(), &request), (path, &expected));
            let body = serde_json::to_string(&request).expect("serializable");
            let read = RecorderRequest::parse(path, &body).expect("the recorder reads it");
            assert_eq!(read.as_ref(), Some(&expected), "{path} {body}");
        }
        assert_eq!(recorder_request(Effect::EndMeeting, seq), None);
    }

    #[test]
    fn a_numbered_request_is_its_fields_plus_seq() {
        let stop = RecorderRequest::Stop(Ordered {
            seq: RequestSeq(7),
            request: Stop {},
        });
        assert_eq!(
            serde_json::to_value(&stop).expect("serializable"),
            json!({ "seq": 7 })
        );
        // A room that ran older code posts `null` to /stop; the recorder still stops.
        let RecorderRequest::Stop(old) = RecorderRequest::parse("/stop", "null")
            .expect("readable")
            .expect("a stop")
        else {
            panic!("not a stop");
        };
        assert_eq!(old.seq, RequestSeq::UNORDERED);
        assert_eq!(RecorderRequest::parse("/transcript", "{}").ok(), Some(None));
    }
}
