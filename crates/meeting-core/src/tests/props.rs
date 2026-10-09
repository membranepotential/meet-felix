//! Property tests: random but plausible event sequences keep the room's invariants.
//!
//! A simulator plays the shell: it answers the SFU calls the room requested (or fails them),
//! answers subscribe offers, and delivers client messages from a small pool of participants.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::support::{Room, name, token};
use crate::state::{AlarmReason, Connection, Lifecycle};
use crate::{
    ClientMsg, Effect, ErrorCode, Event, LocalTrack, Mid, Millis, NEGOTIATION_TIMEOUT_MS,
    OfferedTrack, ParticipantId, RecordingPolicy, RemoteTrack, ResumeToken, Rid, Role, RoomState,
    RoomView, SCHEDULED_TTL_MS, Sdp, ServerMsg, SessionId, SessionKind, SfuCall, SfuReply,
    TrackKind, TrackName, TrackRef,
};

const POOL: u32 = 6;
const KINDS: [TrackKind; 3] = [TrackKind::Audio, TrackKind::Video, TrackKind::Screen];

#[derive(Debug, Clone)]
enum Action {
    /// A client joins from the owner's socket (as a host) or another socket (as a guest), into
    /// whatever phase the room is in; the room starts scheduled.
    Join {
        who: u32,
        owner: bool,
        consent: bool,
    },
    Send {
        who: u32,
        msg: Msg,
    },
    /// A client joins from socket `socket` with the resume token `whose` was welcomed with,
    /// possibly from the owner's socket.
    Resume {
        socket: u32,
        whose: u32,
        owner: bool,
    },
    /// Answer a pending SFU call.
    Reply {
        pick: usize,
        fail: bool,
    },
    /// The reply to a pending SFU call is lost.
    Lose {
        pick: usize,
    },
    /// A client answers a pending subscribe offer.
    Answer {
        pick: usize,
    },
    TrackClosed {
        who: u32,
        kind: TrackKind,
    },
    SocketClosed {
        who: u32,
    },
    /// The alarm fires at a random time (possibly early or spurious).
    Alarm,
    /// Time passes until the earliest negotiation deadline or retry, and the alarm fires.
    NegotiationDue,
    /// Time passes until a scheduled room expires, and the alarm fires.
    Expire,
    Wait {
        ms: u64,
    },
}

#[derive(Debug, Clone)]
enum Msg {
    Publish(Vec<TrackKind>),
    /// Unpublish one of the sender's published kinds, or any kind if they publish none.
    Unpublish(usize),
    StrayAnswer,
    SetLayer {
        owner: u32,
        kind: TrackKind,
        rid: Rid,
    },
    Lock(bool),
    Admit(u32),
    Kick(u32),
    Mute(u32),
    SetRecording(bool),
    SetConsent(bool),
    End,
    Leave,
}

fn who() -> impl Strategy<Value = u32> {
    1..=POOL
}

fn kind() -> impl Strategy<Value = TrackKind> {
    proptest::sample::select(KINDS.to_vec())
}

fn msg() -> impl Strategy<Value = Msg> {
    let rid = proptest::sample::select(vec![Rid::Low, Rid::Mid, Rid::High]);
    prop_oneof![
        4 => proptest::sample::subsequence(KINDS.to_vec(), 0..=3).prop_map(Msg::Publish),
        3 => any::<usize>().prop_map(Msg::Unpublish),
        1 => Just(Msg::StrayAnswer),
        2 => (who(), kind(), rid).prop_map(|(owner, kind, rid)| Msg::SetLayer { owner, kind, rid }),
        1 => any::<bool>().prop_map(Msg::Lock),
        2 => who().prop_map(Msg::Admit),
        1 => who().prop_map(Msg::Kick),
        1 => who().prop_map(Msg::Mute),
        4 => any::<bool>().prop_map(Msg::SetRecording),
        3 => any::<bool>().prop_map(Msg::SetConsent),
        1 => (0..20u8).prop_filter_map("rare", |n| (n == 0).then_some(Msg::End)),
        1 => Just(Msg::Leave),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (who(), any::<bool>(), (0..4u8).prop_map(|n| n != 0))
            .prop_map(|(who, owner, consent)| Action::Join { who, owner, consent }),
        8 => (who(), msg()).prop_map(|(who, msg)| Action::Send { who, msg }),
        1 => (who(), who(), (0..4u8).prop_map(|n| n == 0))
            .prop_map(|(socket, whose, owner)| Action::Resume { socket, whose, owner }),
        16 => (any::<usize>(), (0..20u8).prop_map(|n| n == 0)).prop_map(|(pick, fail)| Action::Reply { pick, fail }),
        1 => any::<usize>().prop_map(|pick| Action::Lose { pick }),
        6 => any::<usize>().prop_map(|pick| Action::Answer { pick }),
        1 => (who(), kind()).prop_map(|(who, kind)| Action::TrackClosed { who, kind }),
        1 => who().prop_map(|who| Action::SocketClosed { who }),
        1 => (0..8u8).prop_map(|n| if n == 0 { Action::Expire } else { Action::Alarm }),
        1 => Just(Action::NegotiationDue),
        1 => prop_oneof![0..20_000u64, 0..90_000u64].prop_map(|ms| Action::Wait { ms }),
    ]
}

/// The shell's side of the conversation.
struct Sim {
    room: Room,
    /// SFU calls the room requested and nobody answered yet.
    pending: Vec<Effect>,
    /// Clients holding a subscribe offer.
    offers: Vec<ParticipantId>,
    /// Sessions the SFU created, with the participant and whether it is a publish session.
    created: BTreeSet<(ParticipantId, bool, SessionId)>,
    /// The last snapshot each client received.
    seen: BTreeMap<ParticipantId, RoomView>,
    /// The resume token each client was welcomed with.
    tokens: BTreeMap<ParticipantId, ResumeToken>,
    /// Tracks recorded since the recorder last started.
    recorded: Vec<RemoteTrack>,
    /// The alarm the shell holds.
    alarm: Option<Millis>,
    /// How many clients resumed on a new socket.
    rebinds: usize,
    /// How many tracks were unpublished.
    unpublished: usize,
    /// How many publish offers timed out.
    publish_timeouts: usize,
    /// Whether a host was ever joined.
    host_joined: bool,
    /// How many guests waited for the first host and were let in, or asked for consent, when it
    /// joined.
    released: usize,
    /// How many guests became hosts because the owner's socket took their place.
    promoted: usize,
    /// Whether the room ended unused at its scheduled expiry.
    expired: bool,
    ended: bool,
    counter: u32,
}

impl Sim {
    fn new(policy: RecordingPolicy) -> Self {
        let (room, effects) = Room::with_policy(policy);
        let alarm = effects.iter().find_map(|e| match e {
            Effect::SetAlarm { at } => Some(*at),
            _ => None,
        });
        Self {
            room,
            pending: Vec::new(),
            offers: Vec::new(),
            created: BTreeSet::new(),
            seen: BTreeMap::new(),
            tokens: BTreeMap::new(),
            recorded: Vec::new(),
            alarm,
            rebinds: 0,
            unpublished: 0,
            publish_timeouts: 0,
            host_joined: false,
            released: 0,
            promoted: 0,
            expired: false,
            ended: false,
            counter: 0,
        }
    }

    /// A client message with a fresh token, as the shell sends it.
    fn client(&mut self, from: ParticipantId, msg: ClientMsg, owner: bool) -> Event {
        self.counter += 1;
        Event::Client {
            from,
            msg,
            owner,
            fresh_token: token(self.counter),
        }
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}{}", self.counter)
    }

    /// Turns an action into an event, or `None` if it does not apply right now.
    fn event(&mut self, action: Action) -> Option<Event> {
        let id = ParticipantId;
        Some(match action {
            Action::Join {
                who,
                owner,
                consent,
            } => self.client(
                id(who),
                ClientMsg::Join {
                    name: name("Someone"),
                    consent,
                    resume: None,
                },
                owner,
            ),
            Action::Resume {
                socket,
                whose,
                owner,
            } => {
                let resume = Some(self.tokens.get(&id(whose))?.clone());
                self.client(
                    id(socket),
                    ClientMsg::Join {
                        name: name("Someone"),
                        consent: true,
                        resume,
                    },
                    owner,
                )
            }
            Action::Send { who, msg } => {
                let msg = match msg {
                    Msg::Publish(kinds) => {
                        let tracks = kinds
                            .into_iter()
                            .map(|kind| LocalTrack {
                                mid: Mid::new(self.fresh("m")),
                                kind,
                                name: TrackName::new(self.fresh("t")),
                            })
                            .collect();
                        ClientMsg::PublishOffer {
                            sdp: Sdp::new("offer"),
                            tracks,
                        }
                    }
                    Msg::Unpublish(pick) => {
                        let published = self
                            .room
                            .state
                            .joined()
                            .find(|&(owner, ..)| owner == id(who))
                            .map(|(_, publish, _)| publish.kinds())
                            .filter(|kinds| !kinds.is_empty())
                            .unwrap_or_else(|| KINDS.to_vec());
                        ClientMsg::Unpublish {
                            kind: published[pick % published.len()],
                        }
                    }
                    Msg::StrayAnswer => ClientMsg::SubscribeAnswer {
                        sdp: Sdp::new("stray"),
                    },
                    Msg::SetLayer { owner, kind, rid } => ClientMsg::SetLayer {
                        track: TrackRef {
                            owner: id(owner),
                            kind,
                        },
                        rid,
                    },
                    Msg::Lock(on) => ClientMsg::Lock { on },
                    Msg::Admit(w) => ClientMsg::Admit { who: id(w) },
                    Msg::Kick(w) => ClientMsg::Kick { who: id(w) },
                    Msg::Mute(w) => ClientMsg::Mute { who: id(w) },
                    Msg::SetRecording(on) => ClientMsg::SetRecording { on },
                    Msg::SetConsent(consent) => ClientMsg::SetConsent { consent },
                    Msg::End => ClientMsg::End,
                    Msg::Leave => ClientMsg::Leave,
                };
                self.client(id(who), msg, false)
            }
            Action::Reply { pick, fail } => {
                if self.pending.is_empty() {
                    return None;
                }
                let call = self.pending.remove(pick % self.pending.len());
                Event::Sfu(match (call, fail) {
                    (Effect::SfuNewSession { who, kind }, false) => SfuReply::SessionCreated {
                        who,
                        kind,
                        session: SessionId::new(self.fresh("s")),
                    },
                    (Effect::SfuNewSession { who, kind }, true) => SfuReply::Failed {
                        who,
                        call: SfuCall::NewSession(kind),
                    },
                    (Effect::SfuPublish { who, .. }, false) => SfuReply::Published {
                        who,
                        sdp_answer: Sdp::new("a"),
                    },
                    (Effect::SfuPublish { who, .. }, true) => SfuReply::Failed {
                        who,
                        call: SfuCall::Publish,
                    },
                    (Effect::SfuSubscribe { who, tracks, .. }, false) => {
                        let tracks = tracks.into_iter().map(|track| OfferedTrack {
                            mid: Mid::new(self.fresh("r")),
                            track,
                        });
                        SfuReply::SubscribeOffer {
                            who,
                            sdp_offer: Sdp::new("o"),
                            tracks: tracks.collect(),
                        }
                    }
                    (Effect::SfuSubscribe { who, .. }, true) => SfuReply::Failed {
                        who,
                        call: SfuCall::Subscribe,
                    },
                    (Effect::SfuRenegotiate { who, .. }, false) => SfuReply::Renegotiated { who },
                    (Effect::SfuRenegotiate { who, .. }, true) => SfuReply::Failed {
                        who,
                        call: SfuCall::Renegotiate,
                    },
                    (other, _) => unreachable!("not an SFU call: {other:?}"),
                })
            }
            Action::Lose { pick } => {
                if !self.pending.is_empty() {
                    self.pending.remove(pick % self.pending.len());
                }
                return None;
            }
            Action::Answer { pick } => {
                if self.offers.is_empty() {
                    return None;
                }
                let from = self.offers.remove(pick % self.offers.len());
                self.client(
                    from,
                    ClientMsg::SubscribeAnswer {
                        sdp: Sdp::new("answer"),
                    },
                    false,
                )
            }
            Action::TrackClosed { who, kind } => {
                Event::Sfu(SfuReply::TrackClosed { who: id(who), kind })
            }
            Action::SocketClosed { who } => Event::SocketClosed { who: id(who) },
            Action::Alarm => Event::AlarmFired,
            Action::NegotiationDue => {
                let due = self
                    .room
                    .state
                    .alarms()
                    .filter(|&(_, reason)| {
                        !matches!(reason, AlarmReason::EmptyRoom | AlarmReason::Expire)
                    })
                    .map(|(at, _)| at)
                    .min()?;
                self.room.now = self.room.now.max(due);
                Event::AlarmFired
            }
            Action::Expire => {
                let Lifecycle::Scheduled { since } = self.room.state.lifecycle else {
                    return None;
                };
                self.room.now = self.room.now.max(since.after(SCHEDULED_TTL_MS));
                Event::AlarmFired
            }
            Action::Wait { ms } => {
                self.room.now = self.room.now.after(ms);
                return None;
            }
        })
    }

    fn run(&mut self, action: Action) -> Result<(), TestCaseError> {
        let Some(event) = self.event(action) else {
            return Ok(());
        };
        let before = self.room.state.clone();
        if let Event::Sfu(SfuReply::SessionCreated { who, kind, session }) = &event {
            self.created
                .insert((*who, *kind == SessionKind::Publish, session.clone()));
        }
        let effects = self.room.apply(event.clone());
        self.check(&before, &event, &effects)?;
        let after = &self.room.state;
        self.host_joined |= after
            .joined()
            .any(|(id, ..)| after.participants[&id].role == Role::Host);
        match (&before.lifecycle, &after.lifecycle) {
            (Lifecycle::Scheduled { .. }, Lifecycle::Live { .. }) => {
                self.released += before
                    .participants
                    .values()
                    .filter(|p| p.connection == Connection::WaitingForHost)
                    .count();
            }
            (Lifecycle::Scheduled { .. }, Lifecycle::Ended { .. }) => self.expired = true,
            _ => {}
        }
        self.promoted += after
            .participants
            .iter()
            .filter(|&(who, p)| {
                p.role == Role::Host
                    && before
                        .participants
                        .get(who)
                        .is_some_and(|w| w.role == Role::Guest && w.connection.presence().is_some())
            })
            .count();
        let publish_failed = |e: &Effect| {
            matches!(
                e,
                Effect::Send {
                    msg: ServerMsg::Error {
                        code: ErrorCode::PublishFailed
                    },
                    ..
                }
            )
        };
        match &event {
            Event::Client {
                msg: ClientMsg::Unpublish { .. },
                ..
            } if effects
                .iter()
                .any(|e| matches!(e, Effect::SfuCloseTracks { .. })) =>
            {
                self.unpublished += 1;
            }
            Event::AlarmFired if effects.iter().any(publish_failed) => self.publish_timeouts += 1,
            _ => {}
        }
        for effect in effects {
            match effect {
                Effect::SfuNewSession { .. }
                | Effect::SfuPublish { .. }
                | Effect::SfuSubscribe { .. }
                | Effect::SfuRenegotiate { .. } => self.pending.push(effect),
                Effect::Send { to, msg } => self.receive(&[to], msg),
                Effect::Broadcast { to, msg } => self.receive(&to, msg),
                Effect::SetAlarm { at } => self.alarm = Some(at),
                Effect::Rebind { .. } => self.rebinds += 1,
                Effect::EndMeeting => self.ended = true,
                Effect::StartRecording { .. } => self.recorded.clear(),
                Effect::RecordTrack { track, .. } => {
                    prop_assert!(!self.recorded.contains(&track), "{track:?} recorded twice");
                    self.recorded.push(track);
                }
                Effect::StopRecordingTrack { track, .. } => {
                    prop_assert!(self.recorded.contains(&track), "{track:?} not recorded");
                    self.recorded.retain(|t| *t != track);
                }
                _ => {}
            }
        }
        self.check_views()?;
        self.check_recorded()?;
        self.check_alarm()
    }

    fn receive(&mut self, to: &[ParticipantId], msg: ServerMsg) {
        for &who in to {
            match &msg {
                ServerMsg::Welcome { state, resume, .. } => {
                    self.seen.insert(who, state.clone());
                    self.tokens.insert(who, resume.clone());
                }
                ServerMsg::State { state } => {
                    self.seen.insert(who, state.clone());
                }
                ServerMsg::SubscribeOffer { .. } => self.offers.push(who),
                // A client drops the offer it holds when the subscription failed.
                ServerMsg::Error {
                    code: ErrorCode::SubscribeFailed,
                } => self.offers.retain(|&o| o != who),
                _ => {}
            }
        }
    }

    fn check(
        &self,
        before: &RoomState,
        event: &Event,
        effects: &[Effect],
    ) -> Result<(), TestCaseError> {
        let after = &self.room.state;

        // After EndMeeting nothing is sent, in this step or any later one.
        if self.ended {
            prop_assert!(effects.is_empty(), "effects after the end: {effects:?}");
        }
        if let Some(end) = effects.iter().position(|e| *e == Effect::EndMeeting) {
            prop_assert_eq!(end, effects.len() - 1, "EndMeeting is last");
        }

        // A recording start reaches every joined client before the recorder starts.
        if let (None, Some(recording)) = (before.recording.current(), after.recording.current()) {
            let started = effects
                .iter()
                .position(|e| matches!(e, Effect::StartRecording { .. }));
            prop_assert!(started.is_some(), "{effects:?}");
            for (who, ..) in after.joined() {
                let announced = effects[..started.unwrap_or(0)].iter().any(|e| match e {
                    Effect::Send {
                        to,
                        msg: ServerMsg::Welcome { state, .. },
                    } => *to == who && state.recording == Some(recording),
                    Effect::Broadcast {
                        to,
                        msg: ServerMsg::State { state },
                    } => to.contains(&who) && state.recording == Some(recording),
                    _ => false,
                });
                prop_assert!(announced, "{who:?} not told before recording: {effects:?}");
            }
        }

        // Only joined hosts lock, admit, kick, mute, record and end; anyone else changes nothing
        // and, unless they left, gets an error.
        if let Event::Client { from, msg, .. } = event
            && before.ended().is_none()
            && matches!(
                msg,
                ClientMsg::Lock { .. }
                    | ClientMsg::Admit { .. }
                    | ClientMsg::Kick { .. }
                    | ClientMsg::Mute { .. }
                    | ClientMsg::SetRecording { .. }
                    | ClientMsg::End
            )
        {
            let sender = before.participants.get(from);
            let joined_host = sender.is_some_and(|p| {
                p.role == Role::Host && matches!(p.connection, Connection::Joined { .. })
            });
            if !joined_host {
                prop_assert_eq!(before, after);
                if sender.is_some_and(|p| p.connection == Connection::Left) {
                    prop_assert!(effects.is_empty(), "{effects:?}");
                } else {
                    prop_assert!(
                        matches!(
                            effects,
                            [Effect::Send { to, msg: ServerMsg::Error { .. } }] if to == from
                        ),
                        "{effects:?}"
                    );
                }
            }
        }

        // Every id in an effect is a participant (or the sender being rejected); SFU and recorder
        // effects only concern participants still in the room, on their own sessions.
        let (sender, owner_join) = match event {
            Event::Client {
                from, msg, owner, ..
            } => (Some(*from), *owner && matches!(msg, ClientMsg::Join { .. })),
            _ => (None, false),
        };
        let known =
            |who: &ParticipantId| after.participants.contains_key(who) || Some(*who) == sender;
        let present_in = |state: &RoomState, who: &ParticipantId| {
            state
                .participants
                .get(who)
                .is_some_and(|p| p.connection.presence().is_some())
        };
        let present = |who: &ParticipantId| present_in(after, who);
        let session_of = |who: &ParticipantId, kind: SessionKind| match after
            .participants
            .get(who)
            .map(|p| &p.connection)
        {
            Some(Connection::Joined { publish, subscribe }) => Some(match kind {
                SessionKind::Publish => publish.id.clone(),
                SessionKind::Subscribe => subscribe.id.clone(),
            }),
            _ => None,
        };
        for effect in effects {
            match effect {
                Effect::Send { to, .. } => prop_assert!(known(to), "{effect:?}"),
                Effect::Broadcast { to, .. } => {
                    prop_assert!(!to.is_empty() && to.iter().all(known), "{effect:?}")
                }
                Effect::CloseSocket { who } => prop_assert!(known(who), "{effect:?}"),
                Effect::Rebind {
                    socket,
                    participant,
                } => {
                    // A resume moves a participant to the sender's socket and changes nothing else,
                    // except that the owner's socket turns a guest into a host.
                    prop_assert!(
                        Some(*socket) == sender && present(participant),
                        "{effect:?}"
                    );
                    if owner_join {
                        prop_assert_eq!(after.participants[participant].role, Role::Host);
                    } else {
                        prop_assert_eq!(before, after);
                    }
                }
                Effect::SfuNewSession { who, .. } => prop_assert!(present(who), "{effect:?}"),
                Effect::SfuPublish { who, session, .. } => {
                    prop_assert_eq!(
                        session_of(who, SessionKind::Publish),
                        Some(session.clone()),
                        "{:?}",
                        effect
                    );
                }
                // On the publish session, only an unpublish closes a track: the sender's own,
                // published one.
                Effect::SfuCloseTracks { who, session, mids }
                    if session_of(who, SessionKind::Publish).as_ref() == Some(session) =>
                {
                    let unpublished = match event {
                        Event::Client {
                            from,
                            msg: ClientMsg::Unpublish { kind },
                            ..
                        } if from == who => before
                            .joined()
                            .find(|&(id, ..)| id == *who)
                            .and_then(|(_, publish, _)| publish.published.get(kind))
                            .map(|track| track.mid.clone()),
                        _ => None,
                    };
                    prop_assert_eq!(Some(mids.clone()), unpublished.map(|mid| vec![mid]));
                }
                Effect::SfuRenegotiate { who, session, .. }
                | Effect::SfuCloseTracks { who, session, .. }
                | Effect::SfuUpdateLayer { who, session, .. } => {
                    prop_assert_eq!(
                        session_of(who, SessionKind::Subscribe),
                        Some(session.clone()),
                        "{:?}",
                        effect
                    );
                }
                Effect::RecordTrack { who, .. } => prop_assert!(present(who), "{effect:?}"),
                // A participant who leaves in this step stops being recorded.
                Effect::StopRecordingTrack { who, .. } => {
                    prop_assert!(present_in(before, who), "{effect:?}")
                }
                Effect::SfuSubscribe {
                    who,
                    session,
                    tracks,
                } => {
                    prop_assert_eq!(
                        session_of(who, SessionKind::Subscribe),
                        Some(session.clone()),
                        "{:?}",
                        effect
                    );
                    // Every requested track is published right now.
                    for track in tracks {
                        let published = after.joined().any(|(_, publish, _)| {
                            publish.id == track.session
                                && publish.published.values().any(|p| p.name == track.name)
                        });
                        prop_assert!(published, "{track:?} is not published");
                    }
                }
                _ => {}
            }
        }

        // A track that stops being published under its name (unpublished, replaced, closed by
        // the SFU, or its owner left or was kicked) stops reaching every subscriber.
        if after.ended().is_none() {
            let still_joined = |who: ParticipantId| after.joined().any(|(id, ..)| id == who);
            for (subscriber, _, subscribe) in before.joined() {
                if still_joined(subscriber) {
                    let received: BTreeSet<_> = subscribe
                        .receiving
                        .iter()
                        .filter(|&(&track, r)| after.resolve(track).as_ref() != Some(&r.remote))
                        .map(|(_, r)| r.mid.clone())
                        .collect();
                    let closed: BTreeSet<_> = effects
                        .iter()
                        .filter_map(|e| match e {
                            Effect::SfuCloseTracks { who, mids, .. } if *who == subscriber => {
                                Some(mids.iter().cloned())
                            }
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    prop_assert!(
                        received.is_subset(&closed),
                        "{subscriber:?} still receives {received:?}: {effects:?}"
                    );
                }
            }
        }

        let recording = after.recording.current().is_some();
        let scheduled = matches!(after.lifecycle, Lifecycle::Scheduled { .. });
        for (&who, p) in &after.participants {
            let was = before.participants.get(&who);
            // Only the owner's socket turns a guest into a host, and nothing turns a host into a
            // guest. (A join after leaving is a new participant.)
            if let Some(was) = was
                && was.connection.presence().is_some()
                && was.role != p.role
            {
                prop_assert!(owner_join && p.role == Role::Host, "{who:?} changed role");
            }
            match &p.connection {
                // Nobody joins before the SFU created both sessions.
                Connection::Joined { publish, subscribe } => {
                    prop_assert!(self.created.contains(&(who, true, publish.id.clone())));
                    prop_assert!(self.created.contains(&(who, false, subscribe.id.clone())));
                    // Every received track is still published under the same name.
                    for (&track, receiving) in &subscribe.receiving {
                        prop_assert_eq!(after.resolve(track), Some(receiving.remote.clone()));
                    }
                    // No subscription is lost: every other joined participant's track is
                    // received under its current name or still wanted.
                    for (owner, publish, _) in after.joined().filter(|&(o, ..)| o != who) {
                        for &kind in publish.published.keys() {
                            let track = TrackRef { owner, kind };
                            let received = subscribe
                                .receiving
                                .get(&track)
                                .is_some_and(|r| Some(&r.remote) == after.resolve(track).as_ref());
                            prop_assert!(
                                received || subscribe.wanted.contains_key(&track),
                                "{who:?} lost {track:?}"
                            );
                        }
                    }
                }
                // Only a scheduled room has guests waiting for the host.
                Connection::WaitingForHost => prop_assert!(scheduled && p.role == Role::Guest),
                // Only a locked live room has guests knocking.
                Connection::Knocking => {
                    prop_assert!(!scheduled && after.locked && p.role == Role::Guest)
                }
                // Only a recording room keeps a guest waiting for consent.
                Connection::AwaitingConsent => {
                    prop_assert!(recording && p.role == Role::Guest && !p.consent_recording)
                }
                Connection::Connecting { .. } | Connection::Left => {}
            }
            // No guest is admitted without consent while the room records.
            let newly_admitted =
                p.connection.is_admitted() && !was.is_some_and(|w| w.connection.is_admitted());
            if newly_admitted && p.role == Role::Guest {
                prop_assert!(!recording || p.consent_recording, "{who:?} admitted");
            }
            // Whoever is in a recording room without consent was asked for it and muted.
            let unconsented = |r: &RoomState, p: &crate::state::Participant| {
                r.recording.current().is_some()
                    && p.connection.is_admitted()
                    && !p.consent_recording
            };
            if unconsented(after, p) && !was.is_some_and(|w| unconsented(before, w)) {
                for asked in [ServerMsg::ConsentRequired, ServerMsg::Muted] {
                    prop_assert!(
                        effects.contains(&Effect::Send {
                            to: who,
                            msg: asked.clone()
                        }),
                        "{who:?} not sent {asked:?}: {effects:?}"
                    );
                }
            }
        }

        // No negotiation older than the timeout survives an alarm at or after its deadline.
        if *event == Event::AlarmFired {
            let now = self.room.now;
            for (who, publish, subscribe) in after.joined() {
                for since in [publish.negotiation.since(), subscribe.negotiation.since()] {
                    prop_assert!(
                        since.is_none_or(|since| since.after(NEGOTIATION_TIMEOUT_MS) > now),
                        "{who:?}'s negotiation since {since:?} survived the alarm at {now:?}"
                    );
                }
            }
        }

        self.check_phases(before, event, effects)
    }

    /// A room is scheduled until its first host joins, then live, and never goes back. Its
    /// timers and its waiting guests follow the phase.
    fn check_phases(
        &self,
        before: &RoomState,
        event: &Event,
        effects: &[Effect],
    ) -> Result<(), TestCaseError> {
        let after = &self.room.state;
        let host_joined = after
            .joined()
            .any(|(id, ..)| after.participants[&id].role == Role::Host);
        match after.lifecycle {
            // Nobody is joined in a scheduled room, and its only timer is the expiry.
            Lifecycle::Scheduled { since } => {
                prop_assert!(after.joined().next().is_none());
                prop_assert!(
                    after
                        .alarms()
                        .all(|(_, reason)| reason != AlarmReason::EmptyRoom)
                );
                prop_assert_eq!(after.next_alarm(), Some(since.after(SCHEDULED_TTL_MS)));
            }
            // No room is live without a host having joined. The empty timer runs exactly while
            // nobody is admitted.
            Lifecycle::Live { empty_since } => {
                prop_assert!(self.host_joined || host_joined);
                let admitted = after
                    .participants
                    .values()
                    .any(|p| p.connection.is_admitted());
                prop_assert_eq!(empty_since.is_none(), admitted);
            }
            Lifecycle::Ended { .. } => {}
        }
        let waiting = before
            .participants
            .iter()
            .filter(|(_, p)| p.connection == Connection::WaitingForHost);
        match (&before.lifecycle, &after.lifecycle) {
            (Lifecycle::Scheduled { .. }, Lifecycle::Live { .. }) => {
                // The first host joined in this step and let in every guest waiting for it,
                // except those who lack consent to the recording that starts with it: they are
                // asked for it.
                prop_assert!(host_joined);
                for (who, _) in waiting {
                    let now = &after.participants[who];
                    if after.lacks_consent(*who) {
                        prop_assert_eq!(&now.connection, &Connection::AwaitingConsent);
                        let asked = Effect::Send {
                            to: *who,
                            msg: ServerMsg::ConsentRequired,
                        };
                        prop_assert!(effects.contains(&asked), "{who:?} not asked: {effects:?}");
                    } else {
                        prop_assert!(now.connection.is_admitted(), "{who:?} not let in");
                    }
                }
            }
            (Lifecycle::Scheduled { .. }, after_lifecycle) => {
                // Otherwise waiting guests keep waiting until they leave, the room expires, or
                // the owner's socket makes them a host.
                for (who, _) in waiting {
                    let now = &after.participants[who];
                    prop_assert!(
                        matches!(
                            now.connection,
                            Connection::WaitingForHost | Connection::Left
                        ) || now.role == Role::Host,
                        "{who:?} let in before the host"
                    );
                }
                // A scheduled room ends only at its expiry.
                if let (Lifecycle::Ended { at }, Lifecycle::Scheduled { since }) =
                    (after_lifecycle, &before.lifecycle)
                {
                    prop_assert_eq!(event, &Event::AlarmFired);
                    prop_assert!(*at >= since.after(SCHEDULED_TTL_MS));
                }
            }
            (Lifecycle::Live { .. }, Lifecycle::Scheduled { .. }) => {
                prop_assert!(false, "a live room became scheduled")
            }
            _ => {}
        }
        Ok(())
    }

    /// While recording, exactly the consenting joined speakers' current audio is recorded:
    /// nothing is missing and nothing is left over from a track that is gone.
    fn check_recorded(&self) -> Result<(), TestCaseError> {
        let state = &self.room.state;
        if self.ended || state.recording.current().is_none() {
            return Ok(());
        }
        let expected: BTreeSet<_> = state
            .joined()
            .filter(|(who, ..)| state.participants[who].consent_recording)
            .filter_map(|(owner, ..)| {
                state.resolve(TrackRef {
                    owner,
                    kind: TrackKind::Audio,
                })
            })
            .map(|track| (track.session, track.name))
            .collect();
        let recorded: BTreeSet<_> = self
            .recorded
            .iter()
            .map(|track| (track.session.clone(), track.name.clone()))
            .collect();
        prop_assert_eq!(recorded, expected);
        Ok(())
    }

    /// The shell's alarm is set for the room's next due time.
    fn check_alarm(&self) -> Result<(), TestCaseError> {
        if let Some(next) = self.room.state.next_alarm() {
            prop_assert_eq!(self.alarm, Some(next));
        }
        Ok(())
    }

    /// Every joined client's last snapshot is the current view.
    fn check_views(&self) -> Result<(), TestCaseError> {
        if self.ended {
            return Ok(());
        }
        let view = self.room.state.view();
        for (who, ..) in self.room.state.joined() {
            prop_assert_eq!(
                self.seen.get(&who),
                Some(&view),
                "participant {:?} has a stale view",
                who
            );
        }
        Ok(())
    }
}

fn policy(from_start: bool) -> RecordingPolicy {
    if from_start {
        RecordingPolicy::FromStart
    } else {
        RecordingPolicy::HostDecides
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn random_sessions_keep_the_invariants(
        from_start in any::<bool>(),
        actions in proptest::collection::vec(action(), 1..250),
    ) {
        let mut sim = Sim::new(policy(from_start));
        for action in actions {
            sim.run(action)?;
        }
        let state = &sim.room.state;
        let json = serde_json::to_string(state).expect("serializable");
        prop_assert_eq!(&serde_json::from_str::<RoomState>(&json).expect("deserializable"), state);
    }
}

/// The simulator reaches the interesting states; otherwise the properties above prove little.
#[test]
fn random_sessions_reach_joined_publishing_recording_and_scheduled_rooms() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let (mut receiving, mut recording, mut ended) = (0, 0, 0);
    let (mut awaiting, mut resumed, mut retried) = (0, 0, 0);
    let (mut unpublished, mut publish_timeouts) = (0, 0);
    let (mut released, mut expired, mut promoted) = (0, 0, 0);
    for case in 0..400 {
        let actions = proptest::collection::vec(action(), 1..250)
            .new_tree(&mut runner)
            .expect("tree")
            .current();
        let mut sim = Sim::new(policy(case % 2 == 0));
        let mut saw = [false; 5];
        for action in actions {
            sim.run(action).expect("invariants hold");
            let state = &sim.room.state;
            saw[0] |= state.joined().any(|(_, _, s)| !s.receiving.is_empty());
            saw[1] |= state.recording.current().is_some();
            saw[2] |= state
                .participants
                .values()
                .any(|p| p.connection == Connection::AwaitingConsent);
            saw[3] |= sim.rebinds > 0;
            saw[4] |= state.joined().any(|(_, _, s)| {
                s.negotiation.since().is_none() && s.wanted.values().any(|&due| due > sim.room.now)
            });
        }
        receiving += usize::from(saw[0]);
        recording += usize::from(saw[1]);
        awaiting += usize::from(saw[2]);
        resumed += usize::from(saw[3]);
        retried += usize::from(saw[4]);
        ended += usize::from(sim.ended);
        unpublished += usize::from(sim.unpublished > 0);
        publish_timeouts += usize::from(sim.publish_timeouts > 0);
        released += usize::from(sim.released > 0);
        expired += usize::from(sim.expired);
        promoted += usize::from(sim.promoted > 0);
    }
    assert!(
        [
            receiving,
            recording,
            ended,
            awaiting,
            resumed,
            retried,
            unpublished,
            publish_timeouts,
            released,
            expired,
            promoted,
        ]
        .iter()
        .all(|&n| n >= 20),
        "receiving {receiving}, recording {recording}, ended {ended}, \
         awaiting consent {awaiting}, resumed {resumed}, retry pending {retried}, \
         unpublished {unpublished}, publish timed out {publish_timeouts}, \
         waiting guests let in by the first host {released}, expired unused {expired}, \
         promoted by the owner {promoted}"
    );
}
