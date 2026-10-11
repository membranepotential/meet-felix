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
//! with the SFU calls, after the socket effects.

use std::collections::VecDeque;

use meeting_core::recording::{ConsentRecord, RosterEntry};
use meeting_core::{
    ClientMsg, Effect, ErrorCode, Event, FailureCause, MAX_PRESENT, MeetingId, Millis,
    ParticipantId, RecordingPolicy, RoomState, ServerMsg, SfuReply, TranscriptRef,
};
use serde::Serialize;
use sfu_api::{Failure, failure_reply};
use worker::js_sys::{Date as JsDate, Function, Reflect};
use worker::wasm_bindgen::JsCast;
use worker::{
    DurableObject, Env, Method, Request, RequestInit, Response, Result, State, WebSocket,
    WebSocketIncomingMessage, WebSocketPair, WebSocketRequestResponsePair, console_error,
    console_log, console_warn, durable_object,
};

use crate::internal::{
    FinishRecording, INFO, INIT, InitRoom, ORIGIN, OWNER_HEADER, OWNER_YES, RECORDER_END,
    RECORDER_START, RECORDER_STOP, RECORDER_TRACK, RECORDER_UNTRACK, RoomInfo, StartRecording,
    TrackRecording, UntrackRecording,
};
use crate::record::{
    Attachment, Decoded, ENDED_RETENTION_MS, EndedAlarm, EndedRoom, FINISH_RETRY_MS, OpenSocket,
    PendingFinish, Record, alarm_time, is_stale,
};
use crate::{
    Jurisdiction, MEETINGS, RECORDERS, ROOM_JURISDICTION, is_websocket_upgrade, now, random, sfu,
    transcript_key,
};

/// The exact frame the runtime answers with [`PONG`] without waking the room
/// (`setWebSocketAutoResponse` matches the raw string). Clients send it at least every
/// [`crate::record::KEEPALIVE_MS`], or the keepalive check closes their socket.
const PING: &str = r#"{"type":"ping"}"#;
const PONG: &str = r#"{"type":"pong"}"#;

/// Upper bound on SFU effects one handler carries out, including those that follow from SFU
/// replies. The core never chains more than a few; this only stops a runaway loop.
const MAX_SFU_CALLS: usize = 64;

/// How many sockets a room keeps open besides the owner's: two per participant the core takes
/// at once ([`MAX_PRESENT`]), for a client that resumes on a new socket before the room noticed
/// that its old one is gone.
const MAX_OPEN_SOCKETS: usize = 2 * MAX_PRESENT;

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
        if let Err(e) = ws.close(Some(CLOSE_NORMAL), Some("bye")) {
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
            Some(Record::Open(_)) => self.open_alarm().await?,
            Some(Record::Ended(ended)) => self.ended_alarm(ended).await?,
            None => {}
        }
        Response::empty()
    }
}

impl RoomObject {
    /// The alarm of an open room, for the core's alarm or the keepalive check, whichever is
    /// earlier. Lets the core handle whatever is due (a spurious `AlarmFired` is harmless),
    /// reports and closes sockets whose client stopped pinging, and arms the next alarm. If the
    /// meeting ends meanwhile, `EndMeeting` armed the ended room's alarm already.
    ///
    /// A stale socket is closed only once the step that reports it is stored: if storing fails,
    /// the runtime retries the alarm, which must still find the socket open and stale.
    async fn open_alarm(&self) -> Result<()> {
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
        // The stale reports go first: they make no SFU call, while `AlarmFired` may (a grace
        // that expires closes tracks), and a client could resume on a new socket during that
        // call. `run` also drops a report whose socket the resume closed meanwhile.
        let inputs: VecDeque<Input> = stale
            .chain(std::iter::once(Input::from(Event::AlarmFired)))
            .collect();
        self.run(inputs).await?;
        if let Some(Record::Open(open)) = self.load().await? {
            self.arm(open.alarm).await?;
        }
        Ok(())
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
                if let Err(e) = self
                    .post_recorder(&pending.recorder, RECORDER_END, &pending.finish)
                    .await
                {
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
        Response::from_json(&RoomInfo::new(record.recording_policy(), record.phase()))
    }

    /// Accepts a socket for a scheduled or running meeting and gives it the next participant
    /// id; `owner` says whether the router verified the owner's session on it. Refuses a socket
    /// that is not the owner's while [`MAX_OPEN_SOCKETS`] are open, so nobody can hold more
    /// sockets than the core takes participants.
    async fn accept(&self, owner: bool) -> Result<Response> {
        let open = match self.load().await? {
            None => return Response::error("unknown meeting", 404),
            Some(Record::Ended(_)) => return Response::error("meeting ended", 410),
            Some(Record::Open(open)) => open,
        };
        if !owner && self.open_sockets().count() >= MAX_OPEN_SOCKETS {
            return Response::error("the meeting is full", 429);
        }
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
    /// step it stores the record, closes the input's stale socket, if any, and carries out the
    /// effects.
    async fn run(&self, inputs: VecDeque<Input>) -> Result<()> {
        let mut queue = inputs;
        let mut sfu_calls = 0;
        while let Some(Input { event, close_stale }) = queue.pop_front() {
            // A stale socket that a resume on a new socket closed meanwhile (`CloseSocket`
            // marks it `Closed`) must not report the resumed participant as lost.
            if let Some(ws) = &close_stale
                && !matches!(attachment(ws)?, Some(Attachment::Open(_)))
            {
                continue;
            }
            let Some(Record::Open(open)) = self.load().await? else {
                // Ended, or never initialised: nothing listens any more.
                return Ok(());
            };
            let now = now();
            let before = (
                open.recorder.clone(),
                open.room.meeting().clone(),
                open.room.transcript().clone(),
            );
            let (mut record, effects) = open.step(now, event);
            let recorder = self.recorder_link(before, &mut record, &effects);
            self.store(&record).await?;
            if let Some(ws) = close_stale {
                close(&ws, CLOSE_STALE, "no ping");
            }
            let replies = self
                .execute_step(effects, recorder.as_ref(), &mut sfu_calls)
                .await?;
            queue.extend(
                replies
                    .into_iter()
                    .map(|reply| Input::from(Event::Sfu(reply))),
            );
        }
        Ok(())
    }

    /// What the room needs to carry out a step's recording effects, or `None` if it has none.
    /// Creates the recorder's id at the first `StartRecording` and keeps it in the record;
    /// `before` is the recorder id, meeting and transcript token before the step, which an
    /// ending step's record no longer holds.
    fn recorder_link(
        &self,
        (id, meeting, transcript): (Option<String>, MeetingId, TranscriptRef),
        record: &mut Record,
        effects: &[Effect],
    ) -> Option<RecorderLink> {
        if !effects.iter().any(is_recording) {
            return None;
        }
        let id = match id {
            Some(id) => id,
            None if effects
                .iter()
                .any(|e| matches!(e, Effect::StartRecording { .. })) =>
            {
                match self.new_recorder() {
                    Ok(id) => id,
                    Err(e) => {
                        console_error!("creating the recorder failed: {e}");
                        return None;
                    }
                }
            }
            None => {
                console_error!("a recording effect without a recorder");
                return None;
            }
        };
        let (roster, consents) = match record {
            Record::Open(open) => {
                open.recorder = Some(id.clone());
                (open.room.roster(), open.room.consents())
            }
            Record::Ended(_) => (Vec::new(), Vec::new()),
        };
        Some(RecorderLink {
            id,
            meeting,
            transcript,
            roster,
            consents,
        })
    }

    /// A new `RecorderObject` id in the rooms' jurisdiction.
    fn new_recorder(&self) -> Result<String> {
        let jurisdiction = Jurisdiction::from_var(&self.env.var(ROOM_JURISDICTION)?.to_string())
            .map_err(worker::Error::RustError)?;
        let recorders = self.env.durable_object(RECORDERS)?;
        let id = match jurisdiction {
            None => recorders.unique_id()?,
            Some(jurisdiction) => recorders.unique_id_with_jurisdiction(jurisdiction.as_str())?,
        };
        Ok(id.to_string())
    }

    /// Carries out one step's effects and returns the SFU replies to feed back.
    ///
    /// The socket, alarm and lifecycle effects run first, in order, then the SFU and recorder
    /// calls, in order. Each SFU call awaits a fetch with the input gate open, so other events
    /// arrive meanwhile: a `CloseSocket` or `Rebind` behind an SFU call would let a kicked or
    /// replaced socket send a `Join` first and come back as a participant whose close is never
    /// reported. The core gets SFU replies back as events, so nothing it emits depends on how
    /// the two groups interleave.
    async fn execute_step(
        &self,
        effects: Vec<Effect>,
        recorder: Option<&RecorderLink>,
        sfu_calls: &mut usize,
    ) -> Result<Vec<SfuReply>> {
        let (remote, local): (Vec<Effect>, Vec<Effect>) =
            effects.into_iter().partition(is_remote_call);
        let mut replies = Vec::new();
        for effect in local.into_iter().chain(remote) {
            replies.extend(self.execute(effect, recorder, sfu_calls).await?);
        }
        Ok(replies)
    }

    /// Carries out one effect; returns the SFU reply to feed back, if any.
    async fn execute(
        &self,
        effect: Effect,
        recorder: Option<&RecorderLink>,
        sfu_calls: &mut usize,
    ) -> Result<Option<SfuReply>> {
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
                    close(&ws, CLOSE_NORMAL, "meeting ended");
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

    /// Carries out a recording effect at the recorder. Failures are logged, not returned: the
    /// meeting goes on without its recording rather than failing.
    async fn tell_recorder(&self, recorder: &RecorderLink, effect: Effect) {
        let told = match effect {
            Effect::StartRecording { since } => {
                // The link works from now on, showing "recording" until the meeting ends.
                self.link_transcript(recorder).await;
                let start = StartRecording {
                    meeting: recorder.meeting.clone(),
                    since,
                };
                self.post_recorder(&recorder.id, RECORDER_START, &start)
                    .await
            }
            Effect::RecordTrack { who, track } => {
                let entry = recorder.roster.iter().find(|e| e.id == who);
                let consent = recorder.consents.iter().find(|c| c.who == who);
                let (Some(entry), Some(consent)) = (entry, consent) else {
                    console_error!("recording {who:?}, who has no name or consent on record");
                    return;
                };
                let body = TrackRecording {
                    who,
                    name: entry.name.clone(),
                    role: entry.role,
                    consent: consent.clone(),
                    session: track.session,
                    track_name: track.name,
                };
                self.post_recorder(&recorder.id, RECORDER_TRACK, &body)
                    .await
            }
            Effect::StopRecordingTrack { who, track } => {
                let body = UntrackRecording {
                    who,
                    session: track.session,
                    track_name: track.name,
                    consent: recorder.consents.iter().find(|c| c.who == who).cloned(),
                };
                self.post_recorder(&recorder.id, RECORDER_UNTRACK, &body)
                    .await
            }
            Effect::StopRecording => self.post_recorder(&recorder.id, RECORDER_STOP, &()).await,
            Effect::FinishRecording { roster, consents } => {
                let finish = FinishRecording { roster, consents };
                let finished = self
                    .post_recorder(&recorder.id, RECORDER_END, &finish)
                    .await;
                if finished.is_err() {
                    let pending = PendingFinish {
                        recorder: recorder.id.clone(),
                        finish,
                    };
                    if let Err(e) = self.finish_later(pending).await {
                        console_error!("keeping the recorder's end for later failed: {e}");
                    }
                }
                finished
            }
            _ => return,
        };
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

    async fn post_recorder(&self, recorder: &str, path: &str, body: &impl Serialize) -> Result<()> {
        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_body(Some(serde_json::to_string(body)?.into()));
        let response = self
            .env
            .durable_object(RECORDERS)?
            .id_from_string(recorder)?
            .get_stub()?
            .fetch_with_request(Request::new_with_init(&format!("{ORIGIN}{path}"), &init)?)
            .await?;
        match response.status_code() {
            200..300 => Ok(()),
            status => Err(worker::Error::RustError(format!(
                "{path} answered {status}"
            ))),
        }
    }

    /// Maps the transcript token to the recorder in KV, for `GET /api/transcript/:token`. The
    /// entry has no expiry: transcripts stay until the owner deletes them (D18). Logged on failure
    /// without the token, which is the link's only secret.
    async fn link_transcript(&self, recorder: &RecorderLink) {
        let linked = async {
            self.env
                .kv(MEETINGS)?
                .put(&transcript_key(&recorder.transcript), &recorder.id)?
                .execute()
                .await?;
            Ok::<(), worker::Error>(())
        };
        if let Err(e) = linked.await {
            console_error!(
                "linking the transcript of {:?} failed: {e}",
                recorder.meeting
            );
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

    /// The stored record. A record of an older version is logged and replaced by an ended
    /// one ([`crate::record::decode`]), which deletes itself like any other.
    async fn load(&self) -> Result<Option<Record>> {
        let json: Option<String> = self.state.storage().get(Record::KEY).await?;
        let Some(json) = json else {
            return Ok(None);
        };
        match crate::record::decode(&json)? {
            Decoded::Current(record) => Ok(Some(record)),
            Decoded::Outdated { version } => {
                console_warn!(
                    "room {}: its record has the old version {version}; the meeting counts as ended",
                    self.state.id()
                );
                let ended = Record::Ended(EndedRoom {
                    recording_policy: RecordingPolicy::HostDecides,
                    pending_finish: None,
                });
                self.store(&ended).await?;
                self.delete_later().await?;
                Ok(Some(ended))
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

/// What the room needs to carry out one step's recording effects.
struct RecorderLink {
    /// The `RecorderObject`'s id (hex).
    id: String,
    meeting: MeetingId,
    transcript: TranscriptRef,
    /// The room's roster after the step, for the speakers' names.
    roster: Vec<RosterEntry>,
    /// The room's consent record after the step.
    consents: Vec<ConsentRecord>,
}

/// Whether `effect` is an SFU or recorder call, carried out after the step's other effects
/// ([`RoomObject::execute_step`]).
fn is_remote_call(effect: &Effect) -> bool {
    match effect {
        Effect::SfuNewSession { .. }
        | Effect::SfuPublish { .. }
        | Effect::SfuSubscribe { .. }
        | Effect::SfuRenegotiate { .. }
        | Effect::SfuCloseTracks { .. }
        | Effect::SfuUpdateLayer { .. }
        | Effect::StartRecording { .. }
        | Effect::RecordTrack { .. }
        | Effect::StopRecordingTrack { .. }
        | Effect::StopRecording
        | Effect::FinishRecording { .. } => true,
        Effect::Send { .. }
        | Effect::Broadcast { .. }
        | Effect::CloseSocket { .. }
        | Effect::Rebind { .. }
        | Effect::SetAlarm { .. }
        | Effect::ReleaseMeetingId { .. }
        | Effect::EndMeeting => false,
    }
}

/// Whether `effect` is for the recorder.
fn is_recording(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::StartRecording { .. }
            | Effect::RecordTrack { .. }
            | Effect::StopRecordingTrack { .. }
            | Effect::StopRecording
            | Effect::FinishRecording { .. }
    )
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
