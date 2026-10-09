//! The pure transition function.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use crate::ids::{Millis, ParticipantId, ResumeToken, Sdp, SessionEpoch, SessionId};
use crate::protocol::{
    ClientMsg, ErrorCode, IncomingTrack, LocalTrack, Recording, RecordingPolicy, Rid, Role,
    RoomView, ServerMsg, SessionEpochs, SessionKind, TrackKind, TrackRef,
};
use crate::shell::{Effect, Event, FailureCause, OfferedTrack, RemoteTrack, SfuCall, SfuReply};
use crate::state::{
    AlarmReason, Connection, Lifecycle, NEGOTIATION_TIMEOUT_MS, Offered, Participant,
    PublishNegotiation, PublishSession, PublishedTrack, Receiving, RecordingStatus, Replacement,
    RoomState, SUBSCRIBE_RETRY_MS, SessionSlot, SubscribeNegotiation, SubscribeSession,
};

/// Applies one event and returns the new state and what the shell must do, in order.
///
/// The effects start with a [`ServerMsg::Welcome`] for each participant who just joined and,
/// whenever the public [`RoomView`] changed, a [`ServerMsg::State`] broadcast to every other
/// joined participant, so no client misses a change (in particular a recording start). They end
/// with an [`Effect::SetAlarm`] whenever the room's next alarm time changed, and after every
/// alarm. After the meeting ended, every event is ignored.
pub fn step(mut state: RoomState, now: Millis, event: Event) -> (RoomState, Vec<Effect>) {
    if state.ended().is_some() {
        return (state, Vec::new());
    }
    let before = Before {
        view: state.view(),
        alarm: state.next_alarm(),
        alarm_fired: event == Event::AlarmFired,
    };
    let mut fx = Effects::default();
    match event {
        Event::Client {
            from,
            msg,
            owner,
            fresh_token,
        } => {
            if let Err(code) = state.on_client(now, from, msg, owner, fresh_token, &mut fx) {
                fx.send(from, ServerMsg::Error { code });
            }
        }
        Event::Sfu(reply) => state.on_sfu(now, reply, &mut fx),
        Event::SocketClosed { who } => state.leave(now, who, &mut fx),
        Event::AlarmFired => state.on_alarm(now, &mut fx),
    }
    let effects = fx.finish(&state, before);
    (state, effects)
}

/// What [`Effects::finish`] compares the new state with.
struct Before {
    view: RoomView,
    alarm: Option<Millis>,
    alarm_fired: bool,
}

/// Effects collected during one step, plus participants who became joined.
#[derive(Default)]
struct Effects {
    list: Vec<Effect>,
    welcomed: Vec<ParticipantId>,
}

impl Effects {
    fn push(&mut self, effect: Effect) {
        self.list.push(effect);
    }

    fn send(&mut self, to: ParticipantId, msg: ServerMsg) {
        self.push(Effect::Send { to, msg });
    }

    fn broadcast(&mut self, to: Vec<ParticipantId>, msg: ServerMsg) {
        if !to.is_empty() {
            self.push(Effect::Broadcast { to, msg });
        }
    }

    /// Prepends the welcomes and, if the view changed, the state broadcast to everyone not
    /// welcomed in this step; appends the alarm.
    fn finish(self, state: &RoomState, before: Before) -> Vec<Effect> {
        if state.ended().is_some() {
            return self.list;
        }
        let greeted = |id: &ParticipantId| {
            self.list.iter().any(
                |e| matches!(e, Effect::Send { to, msg: ServerMsg::Welcome { .. } } if to == id),
            )
        };
        let view = state.view();
        let mut effects: Vec<Effect> = self
            .welcomed
            .iter()
            .map(|&you| Effect::Send {
                to: you,
                msg: state.welcome(you),
            })
            .collect();
        if view != before.view {
            let to: Vec<_> = state
                .joined()
                .map(|(id, ..)| id)
                .filter(|id| !self.welcomed.contains(id) && !greeted(id))
                .collect();
            if !to.is_empty() {
                effects.push(Effect::Broadcast {
                    to,
                    msg: ServerMsg::State { state: view },
                });
            }
        }
        effects.extend(self.list);
        let alarm = state.next_alarm();
        if let Some(at) = alarm
            && (alarm != before.alarm || before.alarm_fired)
        {
            effects.push(Effect::SetAlarm { at });
        }
        effects
    }
}

/// The session a client message works on: `NotJoined` if the sender is not joined, and `None`
/// while the session is being replaced. Such a message is dropped without an error: the reset
/// that follows the replacement makes the client start over on the new session (D33).
fn in_use<S>(slot: Option<&mut SessionSlot<S>>) -> Result<Option<&mut S>, ErrorCode> {
    slot.map(SessionSlot::ready_mut).ok_or(ErrorCode::NotJoined)
}

/// The audio track of `owner`.
fn audio(owner: ParticipantId) -> TrackRef {
    TrackRef {
        owner,
        kind: TrackKind::Audio,
    }
}

impl RoomState {
    fn on_client(
        &mut self,
        now: Millis,
        from: ParticipantId,
        msg: ClientMsg,
        owner: bool,
        fresh_token: ResumeToken,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let present = self
            .participants
            .get(&from)
            .filter(|p| p.connection.presence().is_some());
        let Some(sender) = present else {
            let left = self.participants.contains_key(&from);
            return match msg {
                ClientMsg::Join {
                    name,
                    consent,
                    resume,
                } => {
                    if let Some(old) = resume.and_then(|token| self.holder_of(&token)) {
                        self.resume(from, old, owner, fx);
                        return Ok(());
                    }
                    let role = if owner { Role::Host } else { Role::Guest };
                    self.participants.insert(
                        from,
                        Participant {
                            name,
                            role,
                            consent_recording: consent,
                            resume: fresh_token,
                            connection: Connection::Knocking,
                        },
                    );
                    self.enter(from, fx);
                    Ok(())
                }
                // Late messages from a participant who left change nothing.
                _ if left => Ok(()),
                _ => Err(ErrorCode::NotJoined),
            };
        };
        let (is_joined, is_host) = (
            matches!(sender.connection, Connection::Joined { .. }),
            sender.role == Role::Host,
        );
        // Hosts act only once joined, so every joined client sees the effect of their commands.
        let require_host = || match (is_joined, is_host) {
            (false, _) => Err(ErrorCode::NotJoined),
            (true, false) => Err(ErrorCode::NotHost),
            (true, true) => Ok(()),
        };
        match msg {
            ClientMsg::Join { .. } => {
                if owner {
                    self.promote(from, fx);
                }
                self.greet(from, fx);
                Ok(())
            }
            ClientMsg::SetConsent { consent } => {
                self.set_consent(from, consent, fx);
                Ok(())
            }
            ClientMsg::PublishOffer { sdp, tracks } => {
                self.publish_offer(now, from, sdp, tracks, fx)
            }
            ClientMsg::Unpublish { kind } => self.unpublish(from, kind, fx),
            ClientMsg::SubscribeAnswer { sdp } => self.subscribe_answer(now, from, sdp, fx),
            ClientMsg::SetLayer { track, rid } => self.set_layer(from, track, rid, fx),
            ClientMsg::ResetSession { kind, epoch } => {
                let current = self.epochs_mut(from).map(|epochs| epochs.of(kind));
                if current == Some(epoch) {
                    self.replace_session(now, from, kind, fx);
                }
                Ok(())
            }
            ClientMsg::Leave => {
                self.leave(now, from, fx);
                fx.push(Effect::CloseSocket { who: from });
                Ok(())
            }
            ClientMsg::Lock { on } => {
                require_host()?;
                self.locked = on;
                if !on {
                    let knocking: Vec<_> = self
                        .participants
                        .iter()
                        .filter(|(_, p)| p.connection == Connection::Knocking)
                        .map(|(&id, _)| id)
                        .collect();
                    for who in knocking {
                        self.approve(who, fx);
                    }
                }
                Ok(())
            }
            ClientMsg::Admit { who } => {
                require_host()?;
                match self.participants.get(&who) {
                    Some(p) if p.connection == Connection::Knocking => {
                        self.approve(who, fx);
                        Ok(())
                    }
                    _ => Err(ErrorCode::InvalidTarget),
                }
            }
            ClientMsg::Kick { who } => {
                require_host()?;
                match self.participants.get(&who) {
                    Some(p) if who != from && p.connection.presence().is_some() => {
                        fx.send(who, ServerMsg::Kicked);
                        self.leave(now, who, fx);
                        fx.push(Effect::CloseSocket { who });
                        Ok(())
                    }
                    _ => Err(ErrorCode::InvalidTarget),
                }
            }
            ClientMsg::Mute { who } => {
                require_host()?;
                match self.participants.get(&who) {
                    Some(p) if matches!(p.connection, Connection::Joined { .. }) => {
                        fx.send(who, ServerMsg::Muted);
                        Ok(())
                    }
                    _ => Err(ErrorCode::InvalidTarget),
                }
            }
            ClientMsg::SetRecording { on } => {
                require_host()?;
                self.set_recording(now, from, on, fx);
                Ok(())
            }
            ClientMsg::End => {
                require_host()?;
                self.end(now, fx);
                Ok(())
            }
        }
    }

    /// The participant who holds `token` and has not left.
    fn holder_of(&self, token: &ResumeToken) -> Option<ParticipantId> {
        self.participants
            .iter()
            .find(|(_, p)| p.resume == *token && p.connection.presence().is_some())
            .map(|(&id, _)| id)
    }

    /// Moves participant `old` to the new socket `socket`, keeping consent and tracks, and the
    /// role unless the owner's socket takes a guest's place.
    fn resume(&mut self, socket: ParticipantId, old: ParticipantId, owner: bool, fx: &mut Effects) {
        fx.push(Effect::CloseSocket { who: old });
        fx.push(Effect::Rebind {
            socket,
            participant: old,
        });
        if owner {
            self.promote(old, fx);
        }
        self.greet(old, fx);
    }

    /// The owner's socket took `who`'s place: a guest becomes a host and, if they wait outside,
    /// is let in, as hosts never wait.
    fn promote(&mut self, who: ParticipantId, fx: &mut Effects) {
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        if p.role == Role::Host {
            return;
        }
        p.role = Role::Host;
        if matches!(
            p.connection,
            Connection::WaitingForHost | Connection::Knocking | Connection::AwaitingConsent
        ) {
            self.approve(who, fx);
        }
    }

    /// The welcome for a joined participant. It carries the current view, so send it only when
    /// this step changes the view no further, or through [`Effects::finish`].
    fn welcome(&self, you: ParticipantId) -> ServerMsg {
        let participant = &self.participants[&you];
        let Connection::Joined { epochs, .. } = participant.connection else {
            unreachable!("only joined participants are welcomed");
        };
        ServerMsg::Welcome {
            you,
            state: self.view(),
            resume: participant.resume.clone(),
            epochs,
        }
    }

    /// Answers a `Join` from a participant already in the room: tells them where they stand.
    fn greet(&self, who: ParticipantId, fx: &mut Effects) {
        match self.participants.get(&who).map(|p| &p.connection) {
            Some(Connection::Joined { .. }) => fx.send(who, self.welcome(who)),
            Some(Connection::WaitingForHost) => fx.send(who, ServerMsg::Waiting),
            Some(Connection::Knocking) => {
                fx.send(who, ServerMsg::Waiting);
                if self.lacks_consent(who) {
                    fx.send(who, ServerMsg::ConsentRequired);
                }
            }
            Some(Connection::AwaitingConsent) => fx.send(who, ServerMsg::ConsentRequired),
            // A connecting participant gets the welcome once both sessions exist.
            Some(Connection::Connecting { .. } | Connection::Left) | None => {}
        }
    }

    /// A new participant arrives: a guest waits for the first host of a scheduled room or knocks
    /// on a locked one; anyone else is let in.
    fn enter(&mut self, who: ParticipantId, fx: &mut Effects) {
        let guest = self.participants[&who].role == Role::Guest;
        let scheduled = matches!(self.lifecycle, Lifecycle::Scheduled { .. });
        if guest && scheduled {
            self.participants.get_mut(&who).expect("arrived").connection =
                Connection::WaitingForHost;
            self.greet(who, fx);
        } else if guest && self.locked {
            self.participants.get_mut(&who).expect("arrived").connection = Connection::Knocking;
            self.greet(who, fx);
            let hosts = self
                .joined()
                .map(|(id, ..)| id)
                .filter(|id| self.participants[id].role == Role::Host)
                .collect();
            fx.broadcast(hosts, ServerMsg::Knock { who });
        } else {
            self.approve(who, fx);
        }
    }

    /// Lets in a participant who waits outside or just arrived. While the room records, a guest
    /// without consent waits for it instead; a host enters anyway but is muted and not recorded.
    fn approve(&mut self, who: ParticipantId, fx: &mut Effects) {
        let Some(p) = self.participants.get(&who) else {
            return;
        };
        let guest = p.role == Role::Guest;
        if guest && self.lacks_consent(who) {
            self.participants
                .get_mut(&who)
                .expect("checked above")
                .connection = Connection::AwaitingConsent;
            fx.send(who, ServerMsg::ConsentRequired);
        } else {
            self.admit(who, fx);
            if self.lacks_consent(who) {
                self.ask_consent(who, fx);
            }
        }
    }

    /// Admits a participant by creating their SFU sessions.
    fn admit(&mut self, who: ParticipantId, fx: &mut Effects) {
        if let Some(p) = self.participants.get_mut(&who) {
            p.connection = Connection::Connecting {
                publish: None,
                subscribe: None,
            };
            if let Lifecycle::Live { empty_since } = &mut self.lifecycle {
                *empty_since = None;
            }
            fx.push(Effect::SfuNewSession {
                who,
                kind: SessionKind::Publish,
            });
            fx.push(Effect::SfuNewSession {
                who,
                kind: SessionKind::Subscribe,
            });
        }
    }

    /// Asks `who` for consent to the running recording; if they are admitted, they are muted
    /// until they give it.
    fn ask_consent(&self, who: ParticipantId, fx: &mut Effects) {
        fx.send(who, ServerMsg::ConsentRequired);
        if self
            .participants
            .get(&who)
            .is_some_and(|p| p.connection.is_admitted())
        {
            fx.send(who, ServerMsg::Muted);
        }
    }

    fn set_consent(&mut self, who: ParticipantId, consent: bool, fx: &mut Effects) {
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        if p.consent_recording == consent {
            return;
        }
        p.consent_recording = consent;
        let (awaiting, admitted) = (
            p.connection == Connection::AwaitingConsent,
            p.connection.is_admitted(),
        );
        if consent && awaiting {
            self.admit(who, fx);
        } else if consent {
            self.record_audio_of(who, fx);
        } else if admitted && self.recording.current().is_some() {
            if let Some(track) = self.resolve(audio(who)) {
                fx.push(Effect::StopRecordingTrack { who, track });
            }
            self.ask_consent(who, fx);
        }
    }

    fn publish_offer(
        &mut self,
        now: Millis,
        who: ParticipantId,
        sdp: Sdp,
        tracks: Vec<LocalTrack>,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let Some(publish) = in_use(self.publish_slot_mut(who))? else {
            return Ok(());
        };
        if publish.negotiation != PublishNegotiation::Idle {
            return Err(ErrorCode::NegotiationInProgress);
        }
        let distinct = |count: usize| count == tracks.len();
        let valid = !tracks.is_empty()
            && distinct(tracks.iter().map(|t| t.kind).collect::<BTreeSet<_>>().len())
            && distinct(tracks.iter().map(|t| &t.mid).collect::<BTreeSet<_>>().len())
            && distinct(
                tracks
                    .iter()
                    .map(|t| &t.name)
                    .collect::<BTreeSet<_>>()
                    .len(),
            )
            && tracks
                .iter()
                .all(|t| !publish.published.values().any(|p| p.name == t.name));
        if !valid {
            return Err(ErrorCode::InvalidOffer);
        }
        publish.negotiation = PublishNegotiation::OfferPending {
            tracks: tracks.clone(),
            since: now,
        };
        fx.push(Effect::SfuPublish {
            who,
            session: publish.id.clone(),
            sdp_offer: sdp,
            tracks,
        });
        Ok(())
    }

    /// Stops publishing `who`'s track of `kind`: the consumers (recorder, subscribers) first,
    /// then the source on the publish session.
    fn unpublish(
        &mut self,
        who: ParticipantId,
        kind: TrackKind,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let Some(publish) = in_use(self.publish_slot_mut(who))? else {
            return Ok(());
        };
        let track = publish
            .published
            .get(&kind)
            .ok_or(ErrorCode::NotPublished)?;
        let close = Effect::SfuCloseTracks {
            who,
            session: publish.id.clone(),
            mids: vec![track.mid.clone()],
        };
        self.withdraw(who, &[kind], fx);
        fx.push(close);
        Ok(())
    }

    fn subscribe_answer(
        &mut self,
        now: Millis,
        who: ParticipantId,
        sdp: Sdp,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let Some(subscribe) = in_use(self.subscribe_slot_mut(who))? else {
            return Ok(());
        };
        let SubscribeNegotiation::AwaitingAnswer { offered, .. } = &mut subscribe.negotiation
        else {
            return Err(ErrorCode::UnexpectedAnswer);
        };
        subscribe.negotiation = SubscribeNegotiation::Renegotiating {
            offered: mem::take(offered),
            since: now,
        };
        fx.push(Effect::SfuRenegotiate {
            who,
            session: subscribe.id.clone(),
            sdp_answer: sdp,
        });
        Ok(())
    }

    fn set_layer(
        &mut self,
        who: ParticipantId,
        track: TrackRef,
        rid: Rid,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let Some(subscribe) = in_use(self.subscribe_slot_mut(who))? else {
            return Ok(());
        };
        let receiving = subscribe
            .receiving
            .get_mut(&track)
            .ok_or(ErrorCode::NotSubscribed)?;
        if receiving.rid != rid {
            receiving.rid = rid;
            fx.push(Effect::SfuUpdateLayer {
                who,
                session: subscribe.id.clone(),
                mid: receiving.mid.clone(),
                track: receiving.remote.clone(),
                rid,
            });
        }
        Ok(())
    }

    fn set_recording(&mut self, now: Millis, by: ParticipantId, on: bool, fx: &mut Effects) {
        match (on, &self.recording) {
            (true, RecordingStatus::NeverStarted | RecordingStatus::Stopped) => {
                self.start_recording(now, by, fx);
            }
            (false, RecordingStatus::On(_)) => {
                self.recording = RecordingStatus::Stopped;
                fx.push(Effect::StopRecording);
                let awaiting: Vec<_> = self
                    .participants
                    .iter()
                    .filter(|(_, p)| p.connection == Connection::AwaitingConsent)
                    .map(|(&id, _)| id)
                    .collect();
                for who in awaiting {
                    self.admit(who, fx);
                }
            }
            _ => {}
        }
    }

    /// Starts recording: records every consenting speaker and asks everyone else for consent.
    fn start_recording(&mut self, now: Millis, by: ParticipantId, fx: &mut Effects) {
        self.recording = RecordingStatus::On(Recording { since: now, by });
        fx.push(Effect::StartRecording { since: now });
        let present: Vec<_> = self.present().collect();
        for who in present {
            if self.lacks_consent(who) {
                self.ask_consent(who, fx);
            } else {
                self.record_audio_of(who, fx);
            }
        }
    }

    /// `who`'s audio track if the recorder should record it: recording is on, they consented,
    /// and they publish audio.
    fn recordable_audio_of(&self, who: ParticipantId) -> Option<RemoteTrack> {
        let consented = self
            .participants
            .get(&who)
            .is_some_and(|p| p.consent_recording);
        (self.recording.current().is_some() && consented)
            .then(|| self.resolve(audio(who)))
            .flatten()
    }

    fn record_audio_of(&self, who: ParticipantId, fx: &mut Effects) {
        if let Some(track) = self.recordable_audio_of(who) {
            fx.push(Effect::RecordTrack { who, track });
        }
    }

    fn on_sfu(&mut self, now: Millis, reply: SfuReply, fx: &mut Effects) {
        match reply {
            SfuReply::SessionCreated { who, kind, session } => {
                self.session_created(now, who, kind, session, fx)
            }
            SfuReply::Published {
                who,
                session,
                sdp_answer,
            } => self.published(now, who, &session, sdp_answer, fx),
            SfuReply::SubscribeOffer {
                who,
                session,
                sdp_offer,
                tracks,
            } => self.subscribe_offer(now, who, &session, sdp_offer, tracks, fx),
            SfuReply::Renegotiated { who, session } => self.renegotiated(now, who, &session, fx),
            SfuReply::TrackClosed { who, kind } => self.withdraw(who, &[kind], fx),
            SfuReply::Failed { who, call, cause } => self.sfu_failed(now, who, call, cause, fx),
        }
    }

    fn session_created(
        &mut self,
        now: Millis,
        who: ParticipantId,
        kind: SessionKind,
        session: SessionId,
        fx: &mut Effects,
    ) {
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        let role = p.role;
        let Connection::Connecting { publish, subscribe } = &mut p.connection else {
            return self.session_replaced(now, who, kind, session, fx);
        };
        let slot = match kind {
            SessionKind::Publish => &mut *publish,
            SessionKind::Subscribe => &mut *subscribe,
        };
        if slot.is_none() {
            *slot = Some(session);
        }
        let (Some(publish), Some(subscribe)) = (publish.clone(), subscribe.clone()) else {
            return;
        };
        let wanted = self.others_tracks(now, who);
        self.participants
            .get_mut(&who)
            .expect("checked above")
            .connection = Connection::Joined {
            publish: SessionSlot::Ready(PublishSession::new(publish)),
            subscribe: SessionSlot::Ready(SubscribeSession::new(subscribe, wanted)),
            epochs: SessionEpochs::default(),
        };
        fx.welcomed.push(who);
        if role == Role::Host && matches!(self.lifecycle, Lifecycle::Scheduled { .. }) {
            self.go_live(now, who, fx);
        }
        self.request_wanted(now, who, fx);
    }

    /// Every track the others publish, wanted by `who` from `now`.
    fn others_tracks(&self, now: Millis, who: ParticipantId) -> BTreeMap<TrackRef, Millis> {
        self.published_tracks()
            .filter(|track| track.owner != who)
            .map(|track| (track, now))
            .collect()
    }

    /// Installs the new session that replaces a joined participant's lost one, and tells the
    /// client to start over on a new peer connection. A new subscribe session wants every track
    /// the others publish. Ignored unless that session is being replaced.
    fn session_replaced(
        &mut self,
        now: Millis,
        who: ParticipantId,
        kind: SessionKind,
        session: SessionId,
        fx: &mut Effects,
    ) {
        match kind {
            SessionKind::Publish => {
                let Some(slot @ SessionSlot::Replacing(_)) = self.publish_slot_mut(who) else {
                    return;
                };
                *slot = SessionSlot::Ready(PublishSession::new(session));
                let epoch = self.next_epoch(who, kind);
                fx.send(who, ServerMsg::PublishReset { epoch });
            }
            SessionKind::Subscribe => {
                let wanted = self.others_tracks(now, who);
                let Some(slot @ SessionSlot::Replacing(_)) = self.subscribe_slot_mut(who) else {
                    return;
                };
                *slot = SessionSlot::Ready(SubscribeSession::new(session, wanted));
                let epoch = self.next_epoch(who, kind);
                fx.send(who, ServerMsg::SubscribeReset { epoch });
                self.request_wanted(now, who, fx);
            }
        }
    }

    /// Counts a new session of `kind` for joined participant `who`; returns its epoch.
    fn next_epoch(&mut self, who: ParticipantId, kind: SessionKind) -> SessionEpoch {
        let epoch = self
            .epochs_mut(who)
            .expect("only joined participants replace sessions")
            .of_mut(kind);
        *epoch = epoch.next();
        *epoch
    }

    /// Gives up `who`'s session of `kind` and asks the SFU for a new one (D33 in `docs/media`).
    /// Nothing is closed on the lost session, as the SFU no longer serves it, but a lost publish
    /// session takes its tracks with it: they stop being recorded and reach no subscriber any
    /// more. Ignored unless `who` is joined and the session is in use.
    fn replace_session(
        &mut self,
        now: Millis,
        who: ParticipantId,
        kind: SessionKind,
        fx: &mut Effects,
    ) {
        let requested = Replacement::Requested { since: now };
        match kind {
            SessionKind::Publish => {
                let Some(publish) = self.publish_session_mut(who) else {
                    return;
                };
                let kinds = publish.kinds();
                self.withdraw(who, &kinds, fx);
                *self.publish_slot_mut(who).expect("checked above") =
                    SessionSlot::Replacing(requested);
            }
            SessionKind::Subscribe => {
                let Some(slot @ SessionSlot::Ready(_)) = self.subscribe_slot_mut(who) else {
                    return;
                };
                *slot = SessionSlot::Replacing(requested);
            }
        }
        fx.push(Effect::SfuNewSession { who, kind });
    }

    /// The SFU could not create a replacement session: tell the participant after the first
    /// failure, and ask again after [`SUBSCRIBE_RETRY_MS`] through the alarm.
    fn replacement_failed(
        &mut self,
        now: Millis,
        who: ParticipantId,
        kind: SessionKind,
        fx: &mut Effects,
    ) {
        let Some(replacement) = self.replacement_mut(who, kind) else {
            return;
        };
        let first = match replacement {
            Replacement::Requested { .. } => true,
            Replacement::Retrying { .. } => false,
            Replacement::Failed { .. } => return,
        };
        *replacement = Replacement::Failed {
            retry_at: now.after(SUBSCRIBE_RETRY_MS),
        };
        if first {
            fx.send(
                who,
                ServerMsg::Error {
                    code: ErrorCode::SfuUnavailable,
                },
            );
        }
    }

    /// The first host joined: the room goes live, starts recording if the meeting records from
    /// the start, and lets in the guests waiting for the host. A guest without consent to that
    /// recording waits for it instead; [`RoomState::start_recording`] already asked them.
    fn go_live(&mut self, now: Millis, host: ParticipantId, fx: &mut Effects) {
        self.lifecycle = Lifecycle::Live { empty_since: None };
        if self.recording_policy == RecordingPolicy::FromStart {
            self.start_recording(now, host, fx);
        }
        let waiting: Vec<_> = self
            .participants
            .iter()
            .filter(|(_, p)| p.connection == Connection::WaitingForHost)
            .map(|(&id, _)| id)
            .collect();
        for guest in waiting {
            if self.lacks_consent(guest) {
                self.participants
                    .get_mut(&guest)
                    .expect("waiting")
                    .connection = Connection::AwaitingConsent;
            } else {
                self.admit(guest, fx);
            }
        }
    }

    fn published(
        &mut self,
        now: Millis,
        who: ParticipantId,
        session: &SessionId,
        sdp_answer: Sdp,
        fx: &mut Effects,
    ) {
        let Some(publish) = self
            .publish_session_mut(who)
            .filter(|publish| publish.id == *session)
        else {
            return;
        };
        let PublishNegotiation::OfferPending { tracks, .. } =
            mem::replace(&mut publish.negotiation, PublishNegotiation::Idle)
        else {
            return;
        };
        let kinds: Vec<TrackKind> = tracks.iter().map(|t| t.kind).collect();
        // A new track replaces the published track of its kind.
        self.withdraw(who, &kinds, fx);
        let publish = self.publish_session_mut(who).expect("checked above");
        publish.published.extend(tracks.into_iter().map(|t| {
            let track = PublishedTrack {
                name: t.name,
                mid: t.mid,
            };
            (t.kind, track)
        }));
        fx.send(who, ServerMsg::PublishAnswer { sdp: sdp_answer });
        let subscribers: Vec<_> = self
            .joined()
            .map(|(id, ..)| id)
            .filter(|&id| id != who)
            .collect();
        for subscriber in subscribers {
            if let Some(subscribe) = self.subscribe_session_mut(subscriber) {
                subscribe.wanted.extend(
                    kinds
                        .iter()
                        .map(|&kind| (TrackRef { owner: who, kind }, now)),
                );
            }
            self.request_wanted(now, subscriber, fx);
        }
        if kinds.contains(&TrackKind::Audio) {
            self.record_audio_of(who, fx);
        }
    }

    fn subscribe_offer(
        &mut self,
        now: Millis,
        who: ParticipantId,
        session: &SessionId,
        sdp: Sdp,
        tracks: Vec<OfferedTrack>,
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.current_subscribe_session(who, session) else {
            return;
        };
        let SubscribeNegotiation::AwaitingOffer { requested, .. } = &subscribe.negotiation else {
            return;
        };
        let offered: Vec<Offered> = tracks
            .into_iter()
            .filter_map(|o| {
                let (track, _) = requested.iter().find(|(_, remote)| *remote == o.track)?;
                Some(Offered {
                    track: *track,
                    remote: o.track,
                    mid: o.mid,
                })
            })
            .collect();
        // Requested tracks the SFU left out stay wanted and are retried later.
        for (track, _) in requested {
            if !offered.iter().any(|o| o.track == *track)
                && let Some(due) = subscribe.wanted.get_mut(track)
            {
                *due = now.after(SUBSCRIBE_RETRY_MS);
            }
        }
        let incoming = offered
            .iter()
            .map(|o| IncomingTrack {
                mid: o.mid.clone(),
                track: o.track,
            })
            .collect();
        subscribe.negotiation = SubscribeNegotiation::AwaitingAnswer {
            offered,
            since: now,
        };
        fx.send(
            who,
            ServerMsg::SubscribeOffer {
                sdp,
                tracks: incoming,
            },
        );
    }

    fn renegotiated(
        &mut self,
        now: Millis,
        who: ParticipantId,
        session: &SessionId,
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.current_subscribe_session(who, session) else {
            return;
        };
        let SubscribeNegotiation::Renegotiating { offered, .. } = &mut subscribe.negotiation else {
            return;
        };
        let offered = mem::take(offered);
        subscribe.negotiation = SubscribeNegotiation::Idle;
        // Tracks closed or replaced while the negotiation ran arrive anyway; close them again.
        // A replaced track stays wanted under its new name.
        let (live, stale): (Vec<_>, Vec<_>) = offered
            .into_iter()
            .partition(|o| self.resolve(o.track).as_ref() == Some(&o.remote));
        let subscribe = self.subscribe_session_mut(who).expect("checked above");
        for o in live {
            subscribe.wanted.remove(&o.track);
            subscribe.receiving.insert(
                o.track,
                Receiving {
                    remote: o.remote,
                    mid: o.mid,
                    rid: Rid::High,
                },
            );
        }
        if !stale.is_empty() {
            let mids = stale.into_iter().map(|o| o.mid).collect();
            fx.push(Effect::SfuCloseTracks {
                who,
                session: subscribe.id.clone(),
                mids,
            });
        }
        self.request_wanted(now, who, fx);
    }

    /// `who`'s subscribe session if it is `session`; `None` for a reply about a session the
    /// room has replaced since.
    fn current_subscribe_session(
        &mut self,
        who: ParticipantId,
        session: &SessionId,
    ) -> Option<&mut SubscribeSession> {
        self.subscribe_session_mut(who)
            .filter(|subscribe| subscribe.id == *session)
    }

    /// A failed call on a session the SFU no longer serves replaces that session; any other
    /// failed call is retried later. Failures about a replaced session are ignored.
    fn sfu_failed(
        &mut self,
        now: Millis,
        who: ParticipantId,
        call: SfuCall,
        cause: FailureCause,
        fx: &mut Effects,
    ) {
        match call {
            SfuCall::NewSession(kind) => {
                let connecting = self
                    .participants
                    .get(&who)
                    .is_some_and(|p| matches!(p.connection, Connection::Connecting { .. }));
                if connecting {
                    fx.send(
                        who,
                        ServerMsg::Error {
                            code: ErrorCode::SfuUnavailable,
                        },
                    );
                    self.leave(now, who, fx);
                    fx.push(Effect::CloseSocket { who });
                } else {
                    self.replacement_failed(now, who, kind, fx);
                }
            }
            SfuCall::Publish { session } => {
                if self
                    .publish_session_mut(who)
                    .is_none_or(|publish| publish.id != session)
                {
                    return;
                }
                match cause {
                    FailureCause::SessionGone => {
                        self.replace_session(now, who, SessionKind::Publish, fx)
                    }
                    FailureCause::Transient => self.publish_failed(who, fx),
                }
            }
            SfuCall::Subscribe { ref session } | SfuCall::Renegotiate { ref session } => {
                let Some(subscribe) = self.current_subscribe_session(who, session) else {
                    return;
                };
                if cause == FailureCause::SessionGone {
                    return self.replace_session(now, who, SessionKind::Subscribe, fx);
                }
                let running = matches!(
                    (&call, &subscribe.negotiation),
                    (
                        SfuCall::Subscribe { .. },
                        SubscribeNegotiation::AwaitingOffer { .. }
                    ) | (
                        SfuCall::Renegotiate { .. },
                        SubscribeNegotiation::Renegotiating { .. }
                    )
                );
                if running {
                    self.subscription_failed(now, who, fx);
                    self.request_wanted(now, who, fx);
                }
            }
        }
    }

    /// Drops `who`'s pending publish offer, if any, and tells the client it failed, so it may
    /// offer again.
    fn publish_failed(&mut self, who: ParticipantId, fx: &mut Effects) {
        let Some(publish) = self.publish_session_mut(who) else {
            return;
        };
        if mem::replace(&mut publish.negotiation, PublishNegotiation::Idle)
            != PublishNegotiation::Idle
        {
            fx.send(
                who,
                ServerMsg::Error {
                    code: ErrorCode::PublishFailed,
                },
            );
        }
    }

    /// Ends `who`'s running subscribe negotiation as failed: closes the transceivers it added,
    /// keeps its tracks wanted for a retry after [`SUBSCRIBE_RETRY_MS`], and tells the client to
    /// drop any subscribe offer it holds.
    fn subscription_failed(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let Some(subscribe) = self.subscribe_session_mut(who) else {
            return;
        };
        let negotiation = mem::replace(&mut subscribe.negotiation, SubscribeNegotiation::Idle);
        let retry = now.after(SUBSCRIBE_RETRY_MS);
        for track in negotiation.tracks() {
            if let Some(due) = subscribe.wanted.get_mut(&track) {
                *due = retry;
            }
        }
        let mids: Vec<_> = match negotiation {
            SubscribeNegotiation::AwaitingAnswer { offered, .. }
            | SubscribeNegotiation::Renegotiating { offered, .. } => {
                offered.into_iter().map(|o| o.mid).collect()
            }
            SubscribeNegotiation::Idle | SubscribeNegotiation::AwaitingOffer { .. } => Vec::new(),
        };
        if !mids.is_empty() {
            fx.push(Effect::SfuCloseTracks {
                who,
                session: subscribe.id.clone(),
                mids,
            });
        }
        fx.send(
            who,
            ServerMsg::Error {
                code: ErrorCode::SubscribeFailed,
            },
        );
    }

    /// Starts a subscribe negotiation for the wanted tracks that are due, unless one is running.
    /// Drops wanted tracks that are gone or already received.
    fn request_wanted(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let Some(Participant {
            connection:
                Connection::Joined {
                    subscribe: SessionSlot::Ready(subscribe),
                    ..
                },
            ..
        }) = self.participants.get(&who)
        else {
            return;
        };
        if subscribe.negotiation != SubscribeNegotiation::Idle {
            return;
        }
        let mut requested: Vec<(TrackRef, RemoteTrack)> = Vec::new();
        let mut settled = Vec::new();
        for (&track, &due) in &subscribe.wanted {
            if due > now {
                continue;
            }
            match self.resolve(track) {
                Some(remote)
                    if subscribe
                        .receiving
                        .get(&track)
                        .is_none_or(|r| r.remote != remote) =>
                {
                    requested.push((track, remote));
                }
                _ => settled.push(track),
            }
        }
        let subscribe = self.subscribe_session_mut(who).expect("checked above");
        for track in settled {
            subscribe.wanted.remove(&track);
        }
        if !requested.is_empty() {
            let tracks = requested.iter().map(|(_, remote)| remote.clone()).collect();
            fx.push(Effect::SfuSubscribe {
                who,
                session: subscribe.id.clone(),
                tracks,
            });
            subscribe.negotiation = SubscribeNegotiation::AwaitingOffer {
                requested,
                since: now,
            };
        }
    }

    /// Forgets `owner`'s published tracks of these kinds, if any: stops recording the audio
    /// track if it is recorded and stops sending the tracks to everyone else. Leaves the publish
    /// session to the caller.
    fn withdraw(&mut self, owner: ParticipantId, kinds: &[TrackKind], fx: &mut Effects) {
        if kinds.contains(&TrackKind::Audio)
            && let Some(track) = self.recordable_audio_of(owner)
        {
            fx.push(Effect::StopRecordingTrack { who: owner, track });
        }
        let Some(publish) = self.publish_session_mut(owner) else {
            return;
        };
        let removed: Vec<TrackKind> = kinds
            .iter()
            .copied()
            .filter(|kind| publish.published.remove(kind).is_some())
            .collect();
        self.unsubscribe_everyone(owner, &removed, fx);
    }

    /// Stops sending `owner`'s tracks of these kinds to everyone else.
    fn unsubscribe_everyone(
        &mut self,
        owner: ParticipantId,
        kinds: &[TrackKind],
        fx: &mut Effects,
    ) {
        if kinds.is_empty() {
            return;
        }
        for (&who, p) in &mut self.participants {
            let Connection::Joined {
                subscribe: SessionSlot::Ready(subscribe),
                ..
            } = &mut p.connection
            else {
                continue;
            };
            subscribe
                .wanted
                .retain(|t, _| t.owner != owner || !kinds.contains(&t.kind));
            let mids: Vec<_> = kinds
                .iter()
                .filter_map(|&kind| subscribe.receiving.remove(&TrackRef { owner, kind }))
                .map(|r| r.mid)
                .collect();
            if !mids.is_empty() {
                fx.push(Effect::SfuCloseTracks {
                    who,
                    session: subscribe.id.clone(),
                    mids,
                });
            }
        }
    }

    fn leave(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let Some(p) = self.participants.get(&who) else {
            return;
        };
        let published = match &p.connection {
            Connection::Left => return,
            Connection::WaitingForHost
            | Connection::Knocking
            | Connection::AwaitingConsent
            | Connection::Connecting { .. } => Vec::new(),
            Connection::Joined { publish, .. } => publish
                .ready()
                .map(PublishSession::kinds)
                .unwrap_or_default(),
        };
        self.withdraw(who, &published, fx);
        self.participants
            .get_mut(&who)
            .expect("checked above")
            .connection = Connection::Left;
        let anyone_admitted = self
            .participants
            .values()
            .any(|p| p.connection.is_admitted());
        if let Lifecycle::Live { empty_since } = &mut self.lifecycle
            && empty_since.is_none()
            && !anyone_admitted
        {
            *empty_since = Some(now);
        }
    }

    fn on_alarm(&mut self, now: Millis, fx: &mut Effects) {
        let due: Vec<_> = self
            .alarms()
            .filter(|&(at, _)| at <= now)
            .map(|(_, reason)| reason)
            .collect();
        for reason in due {
            match reason {
                AlarmReason::Expire | AlarmReason::EmptyRoom => return self.end(now, fx),
                // Due only once the pending offer timed out.
                AlarmReason::Publish(who) => self.publish_failed(who, fx),
                AlarmReason::Subscribe(who) => {
                    let timed_out = self
                        .joined()
                        .find(|&(id, ..)| id == who)
                        .and_then(|(.., subscribe)| subscribe.ready()?.negotiation.since())
                        .is_some_and(|since| since.after(NEGOTIATION_TIMEOUT_MS) <= now);
                    if timed_out {
                        self.subscription_failed(now, who, fx);
                    }
                    self.request_wanted(now, who, fx);
                }
                AlarmReason::Replace(who, kind) => {
                    let Some(&mut replacement) = self.replacement_mut(who, kind) else {
                        continue;
                    };
                    match replacement {
                        _ if replacement.due() > now => {}
                        Replacement::Failed { .. } => {
                            *self.replacement_mut(who, kind).expect("checked above") =
                                Replacement::Retrying { since: now };
                            fx.push(Effect::SfuNewSession { who, kind });
                        }
                        // No reply in time: the request counts as failed.
                        Replacement::Requested { .. } | Replacement::Retrying { .. } => {
                            self.replacement_failed(now, who, kind, fx);
                        }
                    }
                }
            }
        }
    }

    fn end(&mut self, now: Millis, fx: &mut Effects) {
        let present: Vec<_> = self.present().collect();
        let transcript =
            (self.recording != RecordingStatus::NeverStarted).then(|| self.transcript.clone());
        fx.broadcast(present.clone(), ServerMsg::Ended { transcript });
        for &who in &present {
            fx.push(Effect::CloseSocket { who });
            self.participants.get_mut(&who).expect("present").connection = Connection::Left;
        }
        if self.recording.current().is_some() {
            self.recording = RecordingStatus::Stopped;
            fx.push(Effect::StopRecording);
        }
        self.lifecycle = Lifecycle::Ended { at: now };
        fx.push(Effect::ReleaseMeetingId {
            meeting: self.meeting.clone(),
        });
        fx.push(Effect::EndMeeting);
    }
}
