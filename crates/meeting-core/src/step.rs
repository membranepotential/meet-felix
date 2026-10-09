//! The pure transition function.

use std::collections::BTreeSet;
use std::mem;

use crate::ids::{HostToken, Millis, ParticipantId, ResumeToken, Sdp, SessionId};
use crate::protocol::{
    ClientMsg, ErrorCode, IncomingTrack, LocalTrack, Recording, RecordingPolicy, Rid, Role,
    RoomView, ServerMsg, TrackKind, TrackRef,
};
use crate::shell::{Effect, Event, OfferedTrack, RemoteTrack, SessionKind, SfuCall, SfuReply};
use crate::state::{
    AlarmReason, Connection, NEGOTIATION_TIMEOUT_MS, Offered, Participant, PublishNegotiation,
    PublishSession, Receiving, RecordingStatus, RoomState, SUBSCRIBE_RETRY_MS,
    SubscribeNegotiation, SubscribeSession,
};

/// Applies one event and returns the new state and what the shell must do, in order.
///
/// The effects start with a [`ServerMsg::Welcome`] for each participant who just joined and,
/// whenever the public [`RoomView`] changed, a [`ServerMsg::State`] broadcast to every other
/// joined participant, so no client misses a change (in particular a recording start). They end
/// with an [`Effect::SetAlarm`] whenever the room's next alarm time changed, and after every
/// alarm. After the meeting ended, every event is ignored.
pub fn step(mut state: RoomState, now: Millis, event: Event) -> (RoomState, Vec<Effect>) {
    if state.ended.is_some() {
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
            fresh_token,
        } => {
            if let Err(code) = state.on_client(now, from, msg, fresh_token, &mut fx) {
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

    /// Prepends the welcomes and, if the view changed, the state broadcast; appends the alarm.
    fn finish(self, state: &RoomState, before: Before) -> Vec<Effect> {
        if state.ended.is_some() {
            return self.list;
        }
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
                .filter(|id| !self.welcomed.contains(id))
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
                    host_token,
                    resume,
                } => {
                    if let Some(old) = resume.and_then(|token| self.holder_of(&token)) {
                        self.resume(from, old, fx);
                        return Ok(());
                    }
                    let role = self.role_for(host_token)?;
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
                self.greet(from, fx);
                Ok(())
            }
            ClientMsg::SetConsent { consent } => {
                self.set_consent(from, consent, fx);
                Ok(())
            }
            ClientMsg::PublishOffer { sdp, tracks } => self.publish_offer(from, sdp, tracks, fx),
            ClientMsg::SubscribeAnswer { sdp } => self.subscribe_answer(now, from, sdp, fx),
            ClientMsg::SetLayer { track, rid } => self.set_layer(from, track, rid, fx),
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

    fn role_for(&self, host_token: Option<HostToken>) -> Result<Role, ErrorCode> {
        match host_token {
            None => Ok(Role::Guest),
            Some(token) if token == self.host_token => Ok(Role::Host),
            Some(_) => Err(ErrorCode::InvalidHostToken),
        }
    }

    /// The participant who holds `token` and has not left.
    fn holder_of(&self, token: &ResumeToken) -> Option<ParticipantId> {
        self.participants
            .iter()
            .find(|(_, p)| p.resume == *token && p.connection.presence().is_some())
            .map(|(&id, _)| id)
    }

    /// Moves participant `old` to the new socket `socket`, keeping role, consent and tracks.
    fn resume(&self, socket: ParticipantId, old: ParticipantId, fx: &mut Effects) {
        fx.push(Effect::CloseSocket { who: old });
        fx.push(Effect::Rebind {
            socket,
            participant: old,
        });
        self.greet(old, fx);
    }

    /// The welcome for a joined participant. It carries the current view, so send it only when
    /// this step changes the view no further, or through [`Effects::finish`].
    fn welcome(&self, you: ParticipantId) -> ServerMsg {
        ServerMsg::Welcome {
            you,
            state: self.view(),
            resume: self.participants[&you].resume.clone(),
        }
    }

    /// Answers a `Join` from a participant already in the room: tells them where they stand.
    fn greet(&self, who: ParticipantId, fx: &mut Effects) {
        match self.participants.get(&who).map(|p| &p.connection) {
            Some(Connection::Joined { .. }) => fx.send(who, self.welcome(who)),
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

    /// A new participant arrives: a guest knocks on a locked room; anyone else is let in.
    fn enter(&mut self, who: ParticipantId, fx: &mut Effects) {
        if self.participants[&who].role == Role::Guest && self.locked {
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

    /// Lets in a participant who is knocking or just arrived. While the room records, a guest
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
            self.empty_since = None;
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
        who: ParticipantId,
        sdp: Sdp,
        tracks: Vec<LocalTrack>,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let publish = self.publish_session_mut(who).ok_or(ErrorCode::NotJoined)?;
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
                .all(|t| !publish.published.values().any(|name| *name == t.name));
        if !valid {
            return Err(ErrorCode::InvalidOffer);
        }
        publish.negotiation = PublishNegotiation::OfferPending {
            tracks: tracks.clone(),
        };
        fx.push(Effect::SfuPublish {
            who,
            session: publish.id.clone(),
            sdp_offer: sdp,
            tracks,
        });
        Ok(())
    }

    fn subscribe_answer(
        &mut self,
        now: Millis,
        who: ParticipantId,
        sdp: Sdp,
        fx: &mut Effects,
    ) -> Result<(), ErrorCode> {
        let subscribe = self
            .subscribe_session_mut(who)
            .ok_or(ErrorCode::NotJoined)?;
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
        let subscribe = self
            .subscribe_session_mut(who)
            .ok_or(ErrorCode::NotJoined)?;
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

    /// Records `who`'s audio if recording is on, they consented, and they publish audio.
    fn record_audio_of(&self, who: ParticipantId, fx: &mut Effects) {
        let consented = self
            .participants
            .get(&who)
            .is_some_and(|p| p.consent_recording);
        if self.recording.current().is_some()
            && consented
            && let Some(track) = self.resolve(audio(who))
        {
            fx.push(Effect::RecordTrack { who, track });
        }
    }

    fn on_sfu(&mut self, now: Millis, reply: SfuReply, fx: &mut Effects) {
        match reply {
            SfuReply::SessionCreated { who, kind, session } => {
                self.session_created(now, who, kind, session, fx)
            }
            SfuReply::Published { who, sdp_answer } => self.published(now, who, sdp_answer, fx),
            SfuReply::SubscribeOffer {
                who,
                sdp_offer,
                tracks,
            } => self.subscribe_offer(now, who, sdp_offer, tracks, fx),
            SfuReply::Renegotiated { who } => self.renegotiated(now, who, fx),
            SfuReply::TrackClosed { who, kind } => {
                let removed = self
                    .publish_session_mut(who)
                    .and_then(|p| p.published.remove(&kind));
                if removed.is_some() {
                    self.unsubscribe_everyone(who, &[kind], fx);
                }
            }
            SfuReply::Failed { who, call } => self.sfu_failed(now, who, call, fx),
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
            return;
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
        let wanted = self
            .joined()
            .filter(|&(owner, ..)| owner != who)
            .flat_map(|(owner, publish, _)| {
                publish
                    .published
                    .keys()
                    .map(move |&kind| (TrackRef { owner, kind }, now))
            })
            .collect();
        self.participants
            .get_mut(&who)
            .expect("checked above")
            .connection = Connection::Joined {
            publish: PublishSession {
                id: publish,
                published: Default::default(),
                negotiation: PublishNegotiation::Idle,
            },
            subscribe: SubscribeSession {
                id: subscribe,
                receiving: Default::default(),
                wanted,
                negotiation: SubscribeNegotiation::Idle,
            },
        };
        fx.welcomed.push(who);
        if self.recording_policy == RecordingPolicy::FromStart
            && self.recording == RecordingStatus::NeverStarted
            && role == Role::Host
        {
            self.start_recording(now, who, fx);
        }
        self.request_wanted(now, who, fx);
    }

    fn published(&mut self, now: Millis, who: ParticipantId, sdp_answer: Sdp, fx: &mut Effects) {
        let Some(publish) = self.publish_session_mut(who) else {
            return;
        };
        let PublishNegotiation::OfferPending { tracks } =
            mem::replace(&mut publish.negotiation, PublishNegotiation::Idle)
        else {
            return;
        };
        let kinds: Vec<TrackKind> = tracks.iter().map(|t| t.kind).collect();
        let replaced: Vec<TrackKind> = tracks
            .into_iter()
            .filter_map(|t| publish.published.insert(t.kind, t.name).map(|_| t.kind))
            .collect();
        self.unsubscribe_everyone(who, &replaced, fx);
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
        sdp: Sdp,
        tracks: Vec<OfferedTrack>,
        fx: &mut Effects,
    ) {
        let Some(subscribe) = self.subscribe_session_mut(who) else {
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

    fn renegotiated(&mut self, now: Millis, who: ParticipantId, fx: &mut Effects) {
        let Some(subscribe) = self.subscribe_session_mut(who) else {
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

    fn sfu_failed(&mut self, now: Millis, who: ParticipantId, call: SfuCall, fx: &mut Effects) {
        match call {
            SfuCall::NewSession(_) => {
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
                }
            }
            SfuCall::Publish => {
                let Some(publish) = self.publish_session_mut(who) else {
                    return;
                };
                if publish.negotiation != PublishNegotiation::Idle {
                    publish.negotiation = PublishNegotiation::Idle;
                    fx.send(
                        who,
                        ServerMsg::Error {
                            code: ErrorCode::PublishFailed,
                        },
                    );
                }
            }
            SfuCall::Subscribe | SfuCall::Renegotiate => {
                let Some(subscribe) = self.subscribe_session_mut(who) else {
                    return;
                };
                let running = matches!(
                    (call, &subscribe.negotiation),
                    (
                        SfuCall::Subscribe,
                        SubscribeNegotiation::AwaitingOffer { .. }
                    ) | (
                        SfuCall::Renegotiate,
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
            connection: Connection::Joined { subscribe, .. },
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
            let Connection::Joined { subscribe, .. } = &mut p.connection else {
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
        let Some(p) = self.participants.get_mut(&who) else {
            return;
        };
        let published = match &p.connection {
            Connection::Left => return,
            Connection::Knocking | Connection::AwaitingConsent | Connection::Connecting { .. } => {
                Vec::new()
            }
            Connection::Joined { publish, .. } => publish.published.keys().copied().collect(),
        };
        p.connection = Connection::Left;
        self.unsubscribe_everyone(who, &published, fx);
        let anyone_admitted = self
            .participants
            .values()
            .any(|p| p.connection.is_admitted());
        if self.empty_since.is_none() && !anyone_admitted {
            self.empty_since = Some(now);
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
                AlarmReason::EmptyRoom => return self.end(now, fx),
                AlarmReason::Subscribe(who) => {
                    let timed_out = self
                        .joined()
                        .find(|&(id, ..)| id == who)
                        .and_then(|(.., subscribe)| subscribe.negotiation.since())
                        .is_some_and(|since| since.after(NEGOTIATION_TIMEOUT_MS) <= now);
                    if timed_out {
                        self.subscription_failed(now, who, fx);
                    }
                    self.request_wanted(now, who, fx);
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
        self.ended = Some(now);
        fx.push(Effect::ReleaseMeetingId {
            meeting: self.meeting.clone(),
        });
        fx.push(Effect::EndMeeting);
    }
}
