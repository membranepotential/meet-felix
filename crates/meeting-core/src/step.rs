//! The pure transition function.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use crate::ids::{
    DisplayName, Millis, NegotiationId, ParticipantId, ResumeToken, Sdp, SessionEpoch, SessionId,
};
use crate::protocol::{
    ClientMsg, CloseCode, ErrorCode, IncomingTrack, LocalTrack, MIN_PROTOCOL_VERSION, Recording,
    RecordingPolicy, Rid, Role, RoomView, ServerMsg, SessionEpochs, SessionKind, TrackKind,
    TrackRef,
};
use crate::shell::{Effect, Event, FailureCause, OfferedTrack, RemoteTrack, SfuCall, SfuReply};
use crate::state::{
    AlarmReason, Connection, Consent, Lifecycle, NEGOTIATION_TIMEOUT_MS, NOTICE_VERSION, Offered,
    Participant, PublishNegotiation, PublishSession, PublishedTrack, Receiving, RecordingStatus,
    Replacement, RoomState, SUBSCRIBE_RETRY_MS, SessionSlot, SubscribeNegotiation,
    SubscribeSession, Wanted,
};

/// Applies one event and returns the new state and what the shell must do, in order.
///
/// The effects start with a [`ServerMsg::Welcome`] for each participant who just joined and,
/// whenever the public [`RoomView`] changed, a [`ServerMsg::State`] broadcast to every other
/// joined participant whose socket is up, so no client misses a change (in particular a recording
/// start); a client that resumes gets the current view in its welcome. They end
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
            let sender = Sender {
                from,
                owner,
                fresh_token,
            };
            if let Err(code) = state.on_client(now, sender, msg, &mut fx) {
                fx.send(from, ServerMsg::Error { code });
            }
        }
        Event::Sfu(reply) => state.on_sfu(now, reply, &mut fx),
        Event::SocketClosed { who } => state.socket_closed(now, who, &mut fx),
        Event::AlarmFired => state.on_alarm(now, &mut fx),
    }
    let effects = fx.finish(&state, before);
    (state, effects)
}

/// The socket a client message came from, as the shell reports it in [`Event::Client`].
struct Sender {
    from: ParticipantId,
    owner: bool,
    fresh_token: ResumeToken,
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

    fn close(&mut self, who: ParticipantId, why: CloseCode) {
        self.push(Effect::CloseSocket { who, why });
    }

    /// Answers a `Join` the room does not take with `code`, then closes the socket with `why`.
    fn refuse(&mut self, from: ParticipantId, code: ErrorCode, why: CloseCode) {
        self.send(from, ServerMsg::Error { code });
        self.close(from, why);
    }

    /// Prepends the welcomes and, if the view changed, the state broadcast to everyone with a
    /// socket who was not welcomed in this step; appends the alarm.
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
                .filter(|id| state.participants[id].connection.socket_lost().is_none())
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
    /// A message from `sender`'s socket. A `Join` from a socket that holds no place is refused
    /// if the client is too old or the room is full, resumes the place its token names, or adds
    /// a participant; everything else needs a participant in the room, and host commands a
    /// joined host ([`RoomState::host_command`]).
    fn on_client(
        &mut self,
        now: Millis,
        sender: Sender,
        msg: ClientMsg,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let from = sender.from;
        let Some(participant) = self
            .participants
            .get_mut(&from)
            .filter(|p| p.connection.presence().is_some())
        else {
            return match msg {
                ClientMsg::Join {
                    name,
                    consent,
                    resume,
                    protocol,
                } => {
                    if protocol < MIN_PROTOCOL_VERSION {
                        fx.refuse(from, ErrorCode::OutdatedClient, CloseCode::Outdated);
                    } else if let Some(old) = resume.and_then(|token| self.holder_of(&token)) {
                        self.resume(now, from, old, sender.owner, fx);
                    } else if !sender.owner && self.is_full() {
                        fx.refuse(from, ErrorCode::RoomFull, CloseCode::RoomFull);
                    } else {
                        self.arrive(now, sender, name, consent, fx);
                    }
                    Ok(())
                }
                // Late messages from a participant who left change nothing.
                _ if self.participants.contains_key(&from) => Ok(()),
                _ => Err(ErrorCode::NotJoined),
            };
        };
        let (is_joined, is_host) = (
            matches!(participant.connection, Connection::Joined { .. }),
            participant.role == Role::Host,
        );
        match msg {
            ClientMsg::SetMuted { muted } => {
                // Whoever is in a recording room without consent stays muted until they consent
                // (D15): an unmute is answered like the mute that asked for consent. Muting is
                // always allowed.
                if !muted
                    && self.recording.current().is_some()
                    && participant.connection.is_admitted()
                    && !participant.consents()
                {
                    self.ask_consent(from, fx);
                } else {
                    participant.muted = muted;
                }
                Ok(())
            }
            ClientMsg::Join { .. } => {
                if sender.owner {
                    self.promote(now, from, fx);
                }
                self.greet(from, fx);
                Ok(())
            }
            ClientMsg::SetConsent { consent } => {
                self.set_consent(now, from, consent, fx);
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
                fx.close(from, CloseCode::Left);
                Ok(())
            }
            ClientMsg::Lock { .. }
            | ClientMsg::Admit { .. }
            | ClientMsg::Kick { .. }
            | ClientMsg::Mute { .. }
            | ClientMsg::SetRecording { .. }
            | ClientMsg::End => match (is_joined, is_host) {
                // Hosts act only once joined, so every joined client sees the effect of their
                // commands.
                (false, _) => Err(ErrorCode::NotJoined),
                (true, false) => Err(ErrorCode::NotHost),
                (true, true) => self.host_command(now, from, msg, fx),
            },
        }
    }

    /// Carries out a joined host's command.
    fn host_command(
        &mut self,
        now: Millis,
        host: ParticipantId,
        msg: ClientMsg,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        // The participant a command names, if they are in a state it applies to.
        let target =
            |state: &Self, who: ParticipantId, applies: fn(&Connection) -> bool| match state
                .participants
                .get(&who)
            {
                Some(p) if applies(&p.connection) => Ok(()),
                _ => Err(ErrorCode::InvalidTarget),
            };
        match msg {
            ClientMsg::Lock { on } => {
                self.locked = on;
                if !on {
                    let knocking: Vec<_> = self
                        .participants
                        .iter()
                        .filter(|(_, p)| p.connection == Connection::Knocking)
                        .map(|(&id, _)| id)
                        .collect();
                    for who in knocking {
                        self.approve(now, who, fx);
                    }
                }
            }
            ClientMsg::Admit { who } => {
                target(self, who, |c| *c == Connection::Knocking)?;
                self.approve(now, who, fx);
            }
            ClientMsg::Kick { who } => {
                if who == host {
                    return Err(ErrorCode::InvalidTarget);
                }
                target(self, who, |c| c.presence().is_some())?;
                fx.send(who, ServerMsg::Kicked);
                self.leave(now, who, fx);
                fx.close(who, CloseCode::Kicked);
            }
            ClientMsg::Mute { who } => {
                target(self, who, |c| matches!(c, Connection::Joined { .. }))?;
                self.mute(who, fx);
            }
            ClientMsg::SetRecording { on } => self.set_recording(now, host, on, fx),
            ClientMsg::End => self.end(now, fx),
            // Not host commands; `on_client` handles them.
            ClientMsg::Join { .. }
            | ClientMsg::SetConsent { .. }
            | ClientMsg::SetMuted { .. }
            | ClientMsg::PublishOffer { .. }
            | ClientMsg::Unpublish { .. }
            | ClientMsg::SubscribeAnswer { .. }
            | ClientMsg::SetLayer { .. }
            | ClientMsg::ResetSession { .. }
            | ClientMsg::Leave => {}
        }
        Ok(())
    }

    /// The participant who holds `token` and has not left.
    fn holder_of(&self, token: &ResumeToken) -> Option<ParticipantId> {
        self.participants
            .iter()
            .find(|(_, p)| p.resume == *token && p.connection.presence().is_some())
            .map(|(&id, _)| id)
    }

    /// Moves participant `old` to the new socket `socket`, keeping consent and tracks, and the
    /// role unless the owner's socket takes a guest's place. Ends their wait to resume, if their
    /// old socket is gone (D35). Everything sent to them meanwhile went nowhere: the welcome and
    /// a repeated consent request catch up on the room, and a subscribe offer still waiting for
    /// its answer fails at once, so its tracks are offered again after [`SUBSCRIBE_RETRY_MS`]
    /// instead of after its timeout.
    fn resume(
        &mut self,
        now: Millis,
        socket: ParticipantId,
        old: ParticipantId,
        owner: bool,
        fx: &mut Effects,
    ) {
        fx.close(old, CloseCode::Replaced);
        fx.push(Effect::Rebind {
            socket,
            participant: old,
        });
        let was_lost = match &mut self.participants.get_mut(&old).expect("holder").connection {
            Connection::Joined { socket_lost, .. } => socket_lost.take().is_some(),
            _ => false,
        };
        if owner {
            self.promote(now, old, fx);
        }
        self.greet(old, fx);
        let unanswered = self.subscribe_session_mut(old).is_some_and(|subscribe| {
            matches!(
                subscribe.negotiation,
                SubscribeNegotiation::AwaitingAnswer { .. }
            )
        });
        if was_lost && unanswered {
            self.subscription_failed(now, old, Wanted::failed, fx);
        }
    }

    /// The owner's socket took `who`'s place: a guest becomes a host and, if they wait outside,
    /// is let in, as hosts never wait.
    fn promote(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
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
            self.approve(now, who, fx);
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

    /// Answers a `Join` from a participant already in the room: tells them where they stand,
    /// and asks a joined participant again for consent the room records without, in case the
    /// first request went to a socket that was gone.
    fn greet(&self, who: ParticipantId, fx: &mut Effects) {
        match self.participants.get(&who).map(|p| &p.connection) {
            Some(Connection::Joined { .. }) => {
                fx.send(who, self.welcome(who));
                if self.lacks_consent(who) {
                    fx.send(who, ServerMsg::ConsentRequired);
                }
            }
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

    /// A new participant arrives from `sender`'s socket: a guest waits for the first host of a
    /// scheduled room, knocks on a locked one, or, while the room records without their consent,
    /// waits for it; anyone else is let in.
    fn arrive(
        &mut self,
        now: Millis,
        sender: Sender,
        name: DisplayName,
        consent: bool,
        fx: &mut Effects,
    ) {
        let who = sender.from;
        let role = if sender.owner {
            Role::Host
        } else {
            Role::Guest
        };
        let guest = role == Role::Guest;
        let connection = if guest && matches!(self.lifecycle, Lifecycle::Scheduled { .. }) {
            Connection::WaitingForHost
        } else if guest && self.locked {
            Connection::Knocking
        } else if guest && self.recording.current().is_some() && !consent {
            Connection::AwaitingConsent
        } else {
            Connection::connecting(now)
        };
        let admitted = connection.is_admitted();
        let knocking = connection == Connection::Knocking;
        self.participants.insert(
            who,
            Participant {
                name,
                role,
                consent: consent.then_some(Consent {
                    at: now,
                    notice_version: NOTICE_VERSION,
                    withdrawn_at: None,
                }),
                resume: sender.fresh_token,
                connection,
                muted: false,
            },
        );
        if admitted {
            self.start_connecting(who, fx);
            if self.lacks_consent(who) {
                self.ask_consent(who, fx);
            }
            return;
        }
        self.greet(who, fx);
        if knocking {
            let hosts = self
                .joined()
                .map(|(id, ..)| id)
                .filter(|id| self.participants[id].role == Role::Host)
                .collect();
            fx.broadcast(hosts, ServerMsg::Knock { who });
        }
    }

    /// Lets in a participant who waits outside. While the room records, a guest without consent
    /// waits for it instead; a host enters anyway but is muted and not recorded.
    fn approve(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let recording = self.recording.current().is_some();
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        if p.role == Role::Guest && recording && !p.consents() {
            p.connection = Connection::AwaitingConsent;
            fx.send(who, ServerMsg::ConsentRequired);
        } else {
            self.admit(now, who, fx);
            if self.lacks_consent(who) {
                self.ask_consent(who, fx);
            }
        }
    }

    /// Admits a participant: they connect from `now` on.
    fn admit(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        if let Some(p) = self.participants.get_mut(&who) {
            p.connection = Connection::connecting(now);
            self.start_connecting(who, fx);
        }
    }

    /// Asks the SFU for the sessions of `who`, who just started connecting. The room is no
    /// longer empty.
    fn start_connecting(&mut self, who: ParticipantId, fx: &mut Effects) {
        if let Lifecycle::Live { empty_since } = &mut self.lifecycle {
            *empty_since = None;
        }
        for kind in [SessionKind::Publish, SessionKind::Subscribe] {
            fx.push(Effect::SfuNewSession { who, kind });
        }
    }

    /// Asks `who` for consent to the running recording; if they are admitted, they are muted
    /// until they give it.
    fn ask_consent(&mut self, who: ParticipantId, fx: &mut Effects) {
        fx.send(who, ServerMsg::ConsentRequired);
        if self
            .participants
            .get(&who)
            .is_some_and(|p| p.connection.is_admitted())
        {
            self.mute(who, fx);
        }
    }

    /// Mutes `who`: shows them muted at once and tells their client to switch the microphone off.
    fn mute(&mut self, who: ParticipantId, fx: &mut Effects) {
        if let Some(p) = self.participants.get_mut(&who) {
            p.muted = true;
            fx.send(who, ServerMsg::Muted);
        }
    }

    fn set_consent(&mut self, now: Millis, who: ParticipantId, consent: bool, fx: &mut Effects) {
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        if p.consents() == consent {
            return;
        }
        // The first consent's time stays; a withdrawal is recorded, and consenting again clears it.
        p.consent = Some(match p.consent {
            Some(given) => Consent {
                withdrawn_at: (!consent).then_some(now),
                ..given
            },
            None => Consent {
                at: now,
                notice_version: NOTICE_VERSION,
                withdrawn_at: None,
            },
        });
        let (awaiting, admitted) = (
            p.connection == Connection::AwaitingConsent,
            p.connection.is_admitted(),
        );
        if consent && awaiting {
            self.admit(now, who, fx);
        } else if consent {
            self.record_audio_of(who, fx);
        } else if admitted && self.recording.current().is_some() {
            if let Some(track) = self.resolve(audio(who)) {
                fx.push(self.stop_recording_track(who, track));
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
            negotiation: publish.next_negotiation(),
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
            negotiation: subscribe.last_negotiation,
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
                    self.admit(now, who, fx);
                }
            }
            _ => {}
        }
    }

    /// Starts recording: records every consenting speaker and asks everyone else for consent.
    fn start_recording(&mut self, now: Millis, by: ParticipantId, fx: &mut Effects) {
        self.recording = RecordingStatus::On(Recording { since: now, by });
        fx.push(Effect::StartRecording {
            since: now,
            meeting: self.meeting.clone(),
            transcript: self.transcript.clone(),
        });
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
            .is_some_and(Participant::consents);
        (self.recording.current().is_some() && consented)
            .then(|| self.resolve(audio(who)))
            .flatten()
    }

    /// Records `who`'s audio track if [`RoomState::recordable_audio_of`] says so; the request
    /// carries their name and consent, for the transcript.
    fn record_audio_of(&self, who: ParticipantId, fx: &mut Effects) {
        let Some(track) = self.recordable_audio_of(who) else {
            return;
        };
        let p = &self.participants[&who];
        // Always there: only a participant who consents is recordable.
        let Some(consent) = p.consent_record(who) else {
            return;
        };
        fx.push(Effect::RecordTrack {
            speaker: p.roster_entry(who),
            consent,
            track,
        });
    }

    /// The request to stop recording `who`'s audio `track`, with their consent as it stands.
    fn stop_recording_track(&self, who: ParticipantId, track: RemoteTrack) -> Effect {
        let consent = self
            .participants
            .get(&who)
            .and_then(|p| p.consent_record(who));
        Effect::StopRecordingTrack {
            who,
            track,
            consent,
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
                negotiation,
                sdp_answer,
                rejected,
            } => self.published(now, who, &session, negotiation, sdp_answer, &rejected, fx),
            SfuReply::SubscribeOffer {
                who,
                session,
                negotiation,
                sdp_offer,
                tracks,
            } => self.subscribe_offer(now, who, &session, negotiation, sdp_offer, tracks, fx),
            SfuReply::Renegotiated {
                who,
                session,
                negotiation,
            } => self.renegotiated(now, who, &session, negotiation, fx),
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
        let Connection::Connecting {
            publish, subscribe, ..
        } = &mut p.connection
        else {
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
            socket_lost: None,
        };
        fx.welcomed.push(who);
        if role == Role::Host && matches!(self.lifecycle, Lifecycle::Scheduled { .. }) {
            self.go_live(now, who, fx);
        }
        self.request_wanted(now, who, fx);
    }

    /// Every track the others publish, wanted by `who` from `now`.
    fn others_tracks(&self, now: Millis, who: ParticipantId) -> BTreeMap<TrackRef, Wanted> {
        self.published_tracks()
            .filter(|track| track.owner != who)
            .map(|track| (track, Wanted::at(now)))
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
        let recording = self.recording.current().is_some();
        let mut admitted = Vec::new();
        for (&guest, p) in &mut self.participants {
            if p.connection != Connection::WaitingForHost {
                continue;
            }
            if recording && !p.consents() {
                p.connection = Connection::AwaitingConsent;
            } else {
                admitted.push(guest);
            }
        }
        for guest in admitted {
            self.admit(now, guest, fx);
        }
    }

    /// The SFU answered `who`'s pending offer: the accepted tracks are published and reach every
    /// other joined participant; the rejected ones stay unpublished, and the client hears which.
    /// An answer that rejects every offered track fails the offer. Ignored unless `negotiation`
    /// is the offer pending on `session`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the fields of SfuReply::Published plus the step's context"
    )]
    fn published(
        &mut self,
        now: Millis,
        who: ParticipantId,
        session: &SessionId,
        negotiation: NegotiationId,
        sdp_answer: Sdp,
        rejected: &[TrackKind],
        fx: &mut Effects,
    ) {
        let Some(publish) = self
            .publish_session_mut(who)
            .filter(|publish| publish.id == *session && publish.pending(negotiation))
        else {
            return;
        };
        let PublishNegotiation::OfferPending { tracks, .. } =
            mem::replace(&mut publish.negotiation, PublishNegotiation::Idle)
        else {
            return;
        };
        let (refused, accepted): (Vec<LocalTrack>, Vec<LocalTrack>) =
            tracks.into_iter().partition(|t| rejected.contains(&t.kind));
        if accepted.is_empty() {
            fx.send(
                who,
                ServerMsg::Error {
                    code: ErrorCode::PublishFailed,
                },
            );
            return;
        }
        let kinds: Vec<TrackKind> = accepted.iter().map(|t| t.kind).collect();
        // A new track replaces the published track of its kind.
        self.withdraw(who, &kinds, fx);
        let publish = self.publish_session_mut(who).expect("checked above");
        publish.published.extend(accepted.into_iter().map(|t| {
            let track = PublishedTrack {
                name: t.name,
                mid: t.mid,
            };
            (t.kind, track)
        }));
        fx.send(
            who,
            ServerMsg::PublishAnswer {
                sdp: sdp_answer,
                rejected: refused.into_iter().map(|t| t.kind).collect(),
            },
        );
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
                        .map(|&kind| (TrackRef { owner: who, kind }, Wanted::at(now))),
                );
            }
            self.request_wanted(now, subscriber, fx);
        }
        if kinds.contains(&TrackKind::Audio) {
            self.record_audio_of(who, fx);
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the fields of SfuReply::SubscribeOffer plus the step's context"
    )]
    fn subscribe_offer(
        &mut self,
        now: Millis,
        who: ParticipantId,
        session: &SessionId,
        negotiation: NegotiationId,
        sdp: Sdp,
        tracks: Vec<OfferedTrack>,
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.running_negotiation(who, session, negotiation) else {
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
        // Requested tracks the SFU left out stay wanted and are retried soon: it leaves out the
        // tracks it did not find, usually because their publisher's connection is not up yet.
        for (track, _) in requested {
            if !offered.iter().any(|o| o.track == *track)
                && let Some(wanted) = subscribe.wanted.get_mut(track)
            {
                wanted.not_found(now);
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
        negotiation: NegotiationId,
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.running_negotiation(who, session, negotiation) else {
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

    /// `who`'s subscribe session if it is `session` and `negotiation` runs on it; `None` for a
    /// reply about a replaced session, or about a negotiation that failed or timed out since.
    fn running_negotiation(
        &mut self,
        who: ParticipantId,
        session: &SessionId,
        negotiation: NegotiationId,
    ) -> Option<&mut SubscribeSession> {
        self.current_subscribe_session(who, session)
            .filter(|subscribe| subscribe.running(negotiation))
    }

    /// A failed call on a session the SFU no longer serves replaces that session, whichever
    /// negotiation it belonged to; any other failed call ends its negotiation, which is retried
    /// later. Failures about a replaced session, and other failures about a negotiation that is
    /// no longer running, are ignored.
    fn sfu_failed(
        &mut self,
        now: Millis,
        who: ParticipantId,
        call: SfuCall,
        cause: FailureCause,
        fx: &mut Effects,
    ) {
        match call {
            SfuCall::NewSession(kind) => match self.participants.get(&who).map(|p| &p.connection) {
                Some(Connection::Connecting { .. }) => self.connect_failed(now, who, fx),
                _ => self.replacement_failed(now, who, kind, fx),
            },
            SfuCall::Publish {
                session,
                negotiation,
            } => {
                let Some(publish) = self
                    .publish_session_mut(who)
                    .filter(|publish| publish.id == session)
                else {
                    return;
                };
                match cause {
                    FailureCause::SessionGone => {
                        self.replace_session(now, who, SessionKind::Publish, fx)
                    }
                    FailureCause::TrackNotFound | FailureCause::Transient
                        if publish.pending(negotiation) =>
                    {
                        self.publish_failed(who, fx)
                    }
                    FailureCause::TrackNotFound | FailureCause::Transient => {}
                }
            }
            SfuCall::Subscribe { ref session, .. } | SfuCall::Renegotiate { ref session, .. } => {
                let Some(subscribe) = self.current_subscribe_session(who, session) else {
                    return;
                };
                if cause == FailureCause::SessionGone {
                    return self.replace_session(now, who, SessionKind::Subscribe, fx);
                }
                if subscribe.awaits(&call) {
                    let retry = match cause {
                        FailureCause::TrackNotFound => Wanted::not_found,
                        FailureCause::SessionGone | FailureCause::Transient => Wanted::failed,
                    };
                    self.subscription_failed(now, who, retry, fx);
                    self.request_wanted(now, who, fx);
                }
            }
        }
    }

    /// The SFU did not create the sessions of `who`, who is connecting: it failed, or did not
    /// answer within [`NEGOTIATION_TIMEOUT_MS`]. They leave, and their client may come back
    /// later ([`CloseCode::TryLater`]).
    fn connect_failed(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        fx.send(
            who,
            ServerMsg::Error {
                code: ErrorCode::SfuUnavailable,
            },
        );
        self.leave(now, who, fx);
        fx.close(who, CloseCode::TryLater);
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
    /// keeps its tracks wanted for a retry that `retry` schedules ([`Wanted::failed`], or
    /// [`Wanted::not_found`] when the SFU did not find them), and tells the client to drop any
    /// subscribe offer it holds.
    fn subscription_failed(
        &mut self,
        now: Millis,
        who: ParticipantId,
        retry: fn(&mut Wanted, Millis),
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.subscribe_session_mut(who) else {
            return;
        };
        let negotiation = mem::replace(&mut subscribe.negotiation, SubscribeNegotiation::Idle);
        for track in negotiation.tracks() {
            if let Some(wanted) = subscribe.wanted.get_mut(&track) {
                retry(wanted, now);
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
        for (&track, wanted) in &subscribe.wanted {
            if wanted.due > now {
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
                negotiation: subscribe.next_negotiation(),
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
            fx.push(self.stop_recording_track(owner, track));
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

    /// `who`'s socket closed without a `Leave`. A joined participant keeps their place, sessions
    /// and tracks until their client resumes or [`crate::LEAVE_GRACE_MS`] pass (D35); a second
    /// report keeps the first time. Anyone else leaves at once: they publish nothing, and their
    /// client holds no resume token, as only the welcome carries it.
    fn socket_closed(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        match self.participants.get_mut(&who).map(|p| &mut p.connection) {
            Some(Connection::Joined { socket_lost, .. }) => {
                socket_lost.get_or_insert(now);
            }
            _ => self.leave(now, who, fx),
        }
    }

    /// `who` leaves: their tracks stop reaching everyone and stop being recorded, and the empty
    /// timer starts if nobody admitted is left. A participant who never joined is forgotten: they
    /// spoke in no recording, and a client whose join the SFU refuses again and again must not
    /// grow the roster with each attempt.
    fn leave(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let Some(p) = self.participants.get(&who) else {
            return;
        };
        match &p.connection {
            Connection::Left => return,
            Connection::WaitingForHost
            | Connection::Knocking
            | Connection::AwaitingConsent
            | Connection::Connecting { .. } => {
                self.participants.remove(&who);
            }
            Connection::Joined { publish, .. } => {
                let published = publish
                    .ready()
                    .map(PublishSession::kinds)
                    .unwrap_or_default();
                self.withdraw(who, &published, fx);
                if let Some(p) = self.participants.get_mut(&who) {
                    p.connection = Connection::Left;
                }
            }
        }
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
                AlarmReason::Connect(who) => {
                    let connecting = self
                        .participants
                        .get(&who)
                        .is_some_and(|p| matches!(p.connection, Connection::Connecting { .. }));
                    if connecting {
                        self.connect_failed(now, who, fx);
                    }
                }
                AlarmReason::Grace(who) => self.leave(now, who, fx),
                // Due only once the pending offer timed out.
                AlarmReason::Publish(who) => self.publish_failed(who, fx),
                AlarmReason::Subscribe(who) => {
                    let timed_out = self
                        .joined()
                        .find(|&(id, ..)| id == who)
                        .and_then(|(.., subscribe)| subscribe.ready()?.negotiation.since())
                        .is_some_and(|since| since.after(NEGOTIATION_TIMEOUT_MS) <= now);
                    if timed_out {
                        self.subscription_failed(now, who, Wanted::failed, fx);
                    }
                    self.request_wanted(now, who, fx);
                }
                AlarmReason::Replace(who, kind) => {
                    let Some(replacement) = self.replacement_mut(who, kind) else {
                        continue;
                    };
                    match *replacement {
                        _ if replacement.due() > now => {}
                        Replacement::Failed { .. } => {
                            *replacement = Replacement::Retrying { since: now };
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

    /// Ends the meeting. Everyone still there hears it; only those who were let in (connecting
    /// or joined, also during the grace) get the transcript link of a recorded meeting (D19).
    /// A guest who waited for the host, knocked or never consented was never in it.
    fn end(&mut self, now: Millis, fx: &mut Effects) {
        let present: Vec<_> = self.present().collect();
        if self.recording == RecordingStatus::NeverStarted {
            fx.broadcast(present.clone(), ServerMsg::Ended { transcript: None });
        } else {
            let (admitted, outside): (Vec<_>, Vec<_>) = present
                .iter()
                .partition(|who| self.participants[who].connection.is_admitted());
            let transcript = Some(self.transcript.clone());
            fx.broadcast(admitted, ServerMsg::Ended { transcript });
            fx.broadcast(outside, ServerMsg::Ended { transcript: None });
        }
        for (&who, p) in &mut self.participants {
            if p.connection.presence().is_some() {
                fx.close(who, CloseCode::Ended);
                p.connection = Connection::Left;
            }
        }
        if self.recording.current().is_some() {
            self.recording = RecordingStatus::Stopped;
            fx.push(Effect::StopRecording);
        }
        if self.recording != RecordingStatus::NeverStarted {
            fx.push(Effect::FinishRecording {
                roster: self.roster(),
                consents: self.consents(),
            });
        }
        self.lifecycle = Lifecycle::Ended { at: now };
        fx.push(Effect::ReleaseMeetingId {
            meeting: self.meeting.clone(),
        });
        fx.push(Effect::EndMeeting);
    }
}
