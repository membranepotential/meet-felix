//! Property tests: random but plausible event sequences keep the room's invariants.
//!
//! A simulator plays the shell: it answers the SFU calls the room requested (or fails them),
//! answers subscribe offers, and delivers client messages from a small pool of participants.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::support::{Room, name, token};
use crate::state::{
    AlarmReason, Connection, Lifecycle, PublishNegotiation, SessionSlot, SubscribeNegotiation,
};
use crate::{
    ClientMsg, CloseCode, Effect, ErrorCode, Event, FIRST_PULL_RETRIES, FIRST_PULL_RETRY_MS,
    FailureCause, LEAVE_GRACE_MS, LocalTrack, Mid, Millis, NEGOTIATION_TIMEOUT_MS, NegotiationId,
    OfferedTrack, ParticipantId, RecordingPolicy, RemoteTrack, ResumeToken, Rid, Role, RoomState,
    RoomView, SCHEDULED_TTL_MS, SUBSCRIBE_RETRY_MS, Sdp, ServerMsg, SessionEpochs, SessionId,
    SessionKind, SfuCall, SfuReply, TrackKind, TrackName, TrackRef,
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
    /// Answer a pending SFU call, or fail it. An answer to a publish offer rejects the offered
    /// tracks whose bit is set in `refuse` (bit 0 for the first track); a subscribe offer leaves
    /// out the requested tracks whose bit is set, as the SFU leaves out tracks it did not find.
    Reply {
        pick: usize,
        fail: Option<FailureCause>,
        refuse: u8,
    },
    /// The SFU holds a pending call past the room's next negotiation deadline: `Reply` and `Lose`
    /// no longer pick it, time passes as for [`Action::NegotiationDue`], and the alarm fires.
    Stall {
        pick: usize,
    },
    /// The SFU answers the call it held longest, which by now may belong to a negotiation that
    /// timed out while a newer one runs on the same session.
    Unstall,
    /// A client offers `kinds`, the SFU holds the publish call until the offer times out, the
    /// client offers `kinds` again (as after a device change), and only then does the SFU answer
    /// the held call, or fail it: a late reply while the new offer is pending.
    Reoffer {
        who: u32,
        kinds: Vec<TrackKind>,
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
    SetMuted(bool),
    /// The client's peer connection for this session failed; `stale` names an epoch other than
    /// the current one, as a report about a replaced session's connection does.
    ResetSession {
        kind: SessionKind,
        stale: bool,
    },
    End,
    Leave,
}

/// Whether bit `i` of `refuse` is set: the `i`-th track of an SFU call is refused. Tracks past
/// the mask's width are never refused; a subscribe call can carry more tracks than it has bits.
fn refused(refuse: u8, i: usize) -> bool {
    u32::try_from(i)
        .ok()
        .and_then(|i| 1u8.checked_shl(i))
        .is_some_and(|bit| refuse & bit != 0)
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
        4 => any::<usize>().prop_map(Msg::Unpublish),
        1 => Just(Msg::StrayAnswer),
        2 => (who(), kind(), rid).prop_map(|(owner, kind, rid)| Msg::SetLayer { owner, kind, rid }),
        1 => any::<bool>().prop_map(Msg::Lock),
        2 => who().prop_map(Msg::Admit),
        1 => who().prop_map(Msg::Kick),
        1 => who().prop_map(Msg::Mute),
        4 => any::<bool>().prop_map(Msg::SetRecording),
        3 => any::<bool>().prop_map(Msg::SetConsent),
        2 => any::<bool>().prop_map(Msg::SetMuted),
        1 => (proptest::sample::select(vec![SessionKind::Publish, SessionKind::Subscribe]), any::<bool>())
            .prop_map(|(kind, stale)| Msg::ResetSession { kind, stale }),
        1 => (0..20u8).prop_filter_map("rare", |n| (n == 0).then_some(Msg::End)),
        1 => Just(Msg::Leave),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (who(), any::<bool>(), (0..4u8).prop_map(|n| n != 0))
            .prop_map(|(who, owner, consent)| Action::Join { who, owner, consent }),
        8 => (who(), msg()).prop_map(|(who, msg)| Action::Send { who, msg }),
        2 => (who(), who(), (0..4u8).prop_map(|n| n == 0))
            .prop_map(|(socket, whose, owner)| Action::Resume { socket, whose, owner }),
        16 => (any::<usize>(), (0..40u8).prop_map(|n| match n {
            0 | 1 => Some(FailureCause::Transient),
            2 => Some(FailureCause::SessionGone),
            3 => Some(FailureCause::TrackNotFound),
            _ => None,
        }), prop_oneof![3 => Just(0u8), 1 => 1..8u8])
            .prop_map(|(pick, fail, refuse)| Action::Reply { pick, fail, refuse }),
        2 => any::<usize>().prop_map(|pick| Action::Stall { pick }),
        2 => Just(Action::Unstall),
        1 => (who(), proptest::sample::subsequence(KINDS.to_vec(), 1..=3), (0..4u8).prop_map(|n| n == 0))
            .prop_map(|(who, kinds, fail)| Action::Reoffer { who, kinds, fail }),
        1 => any::<usize>().prop_map(|pick| Action::Lose { pick }),
        6 => any::<usize>().prop_map(|pick| Action::Answer { pick }),
        2 => who().prop_map(|who| Action::SocketClosed { who }),
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
    /// SFU calls the SFU holds, oldest first (see [`Action::Stall`]).
    stalled: Vec<Effect>,
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
    /// Tracks the SFU accepted in an answer to a publish offer.
    accepted: Vec<RemoteTrack>,
    /// How many publish answers the room applied with some offered tracks rejected and some
    /// accepted.
    partly_rejected: usize,
    /// How many publish replies about a negotiation other than the running one, on a session in
    /// use while a newer negotiation runs, reached the room.
    late_publish_replies: usize,
    /// The same for subscribe and renegotiate replies.
    late_subscribe_replies: usize,
    /// The alarm the shell holds.
    alarm: Option<Millis>,
    /// How many clients resumed on a new socket.
    rebinds: usize,
    /// Joined participants whose socket closed and who have not resumed: the shell delivers
    /// nothing to them, and their client sends nothing until it resumes.
    detached: BTreeSet<ParticipantId>,
    /// How many joined participants lost their socket.
    graced: usize,
    /// How many of them resumed during the grace.
    resumed_in_grace: usize,
    /// How many of them left because the grace ended.
    grace_expired: usize,
    /// How many tracks were unpublished.
    unpublished: usize,
    /// How many publish offers timed out.
    publish_timeouts: usize,
    /// How many times the room scheduled a track again because the SFU did not find it.
    not_found: usize,
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
            stalled: Vec::new(),
            offers: Vec::new(),
            created: BTreeSet::new(),
            seen: BTreeMap::new(),
            tokens: BTreeMap::new(),
            recorded: Vec::new(),
            accepted: Vec::new(),
            partly_rejected: 0,
            late_publish_replies: 0,
            late_subscribe_replies: 0,
            alarm,
            rebinds: 0,
            detached: BTreeSet::new(),
            graced: 0,
            resumed_in_grace: 0,
            grace_expired: 0,
            unpublished: 0,
            publish_timeouts: 0,
            not_found: 0,
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
        // A closed socket sends nothing.
        if let Action::Join { who, .. }
        | Action::Send { who, .. }
        | Action::Resume { socket: who, .. } = action
            && self.detached.contains(&id(who))
        {
            return None;
        }
        Some(match action {
            Action::Join {
                who,
                owner,
                consent,
            } => self.client(
                id(who),
                ClientMsg::Join {
                    protocol: crate::PROTOCOL_VERSION,
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
                        protocol: crate::PROTOCOL_VERSION,
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
                            .and_then(|(_, publish, _)| Some(publish.ready()?.kinds()))
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
                    Msg::SetMuted(muted) => ClientMsg::SetMuted { muted },
                    Msg::ResetSession { kind, stale } => {
                        let epoch = current_epochs(&self.room.state, id(who))
                            .unwrap_or_default()
                            .of(kind);
                        ClientMsg::ResetSession {
                            kind,
                            epoch: if stale { epoch.next() } else { epoch },
                        }
                    }
                    Msg::End => ClientMsg::End,
                    Msg::Leave => ClientMsg::Leave,
                };
                self.client(id(who), msg, false)
            }
            Action::Reply { pick, fail, refuse } => {
                if self.pending.is_empty() {
                    return None;
                }
                let call = self.pending.remove(pick % self.pending.len());
                let failed = |who, call, cause| SfuReply::Failed { who, call, cause };
                Event::Sfu(match (call, fail) {
                    (Effect::SfuNewSession { who, kind }, None) => SfuReply::SessionCreated {
                        who,
                        kind,
                        session: SessionId::new(self.fresh("s")),
                    },
                    (Effect::SfuNewSession { who, kind }, Some(cause)) => {
                        failed(who, SfuCall::NewSession(kind), cause)
                    }
                    (
                        Effect::SfuPublish {
                            who,
                            session,
                            negotiation,
                            tracks,
                            ..
                        },
                        None,
                    ) => {
                        let (rejected, accepted): (Vec<_>, Vec<_>) = tracks
                            .into_iter()
                            .enumerate()
                            .partition(|&(i, _)| refused(refuse, i));
                        self.accepted
                            .extend(accepted.into_iter().map(|(_, t)| RemoteTrack {
                                session: session.clone(),
                                name: t.name,
                            }));
                        SfuReply::Published {
                            who,
                            session,
                            negotiation,
                            sdp_answer: Sdp::new("a"),
                            rejected: rejected.into_iter().map(|(_, t)| t.kind).collect(),
                        }
                    }
                    (
                        Effect::SfuPublish {
                            who,
                            session,
                            negotiation,
                            ..
                        },
                        Some(cause),
                    ) => failed(
                        who,
                        SfuCall::Publish {
                            session,
                            negotiation,
                        },
                        cause,
                    ),
                    (
                        Effect::SfuSubscribe {
                            who,
                            session,
                            negotiation,
                            tracks,
                        },
                        None,
                    ) => {
                        let found: Vec<_> = tracks
                            .into_iter()
                            .enumerate()
                            .filter(|&(i, _)| !refused(refuse, i))
                            .map(|(_, track)| track)
                            .collect();
                        let tracks = found
                            .into_iter()
                            .map(|track| OfferedTrack {
                                mid: Mid::new(self.fresh("r")),
                                track,
                            })
                            .collect();
                        SfuReply::SubscribeOffer {
                            who,
                            session,
                            negotiation,
                            sdp_offer: Sdp::new("o"),
                            tracks,
                        }
                    }
                    (
                        Effect::SfuSubscribe {
                            who,
                            session,
                            negotiation,
                            ..
                        },
                        Some(cause),
                    ) => failed(
                        who,
                        SfuCall::Subscribe {
                            session,
                            negotiation,
                        },
                        cause,
                    ),
                    (
                        Effect::SfuRenegotiate {
                            who,
                            session,
                            negotiation,
                            ..
                        },
                        None,
                    ) => SfuReply::Renegotiated {
                        who,
                        session,
                        negotiation,
                    },
                    (
                        Effect::SfuRenegotiate {
                            who,
                            session,
                            negotiation,
                            ..
                        },
                        Some(cause),
                    ) => failed(
                        who,
                        SfuCall::Renegotiate {
                            session,
                            negotiation,
                        },
                        cause,
                    ),
                    (other, _) => unreachable!("not an SFU call: {other:?}"),
                })
            }
            Action::Stall { pick } => {
                if !self.pending.is_empty() {
                    let call = self.pending.remove(pick % self.pending.len());
                    self.stalled.push(call);
                }
                return self.event(Action::NegotiationDue);
            }
            Action::Unstall => {
                if self.stalled.is_empty() {
                    return None;
                }
                let call = self.stalled.remove(0);
                self.pending.insert(0, call);
                return self.event(Action::Reply {
                    pick: 0,
                    fail: None,
                    refuse: 0,
                });
            }
            Action::Reoffer { .. } => unreachable!("`run` plays it as several events"),
            Action::Lose { pick } => {
                if !self.pending.is_empty() {
                    self.pending.remove(pick % self.pending.len());
                }
                return None;
            }
            Action::Answer { pick } => {
                let connected: Vec<usize> = (0..self.offers.len())
                    .filter(|&i| !self.detached.contains(&self.offers[i]))
                    .collect();
                if connected.is_empty() {
                    return None;
                }
                let from = self.offers.remove(connected[pick % connected.len()]);
                self.client(
                    from,
                    ClientMsg::SubscribeAnswer {
                        sdp: Sdp::new("answer"),
                    },
                    false,
                )
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
        if let Action::Reoffer { who, kinds, fail } = action {
            return self.reoffer(who, kinds, fail);
        }
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
        self.not_found += self.check_not_found(&before, &event)?;
        let after = &self.room.state;
        if let Event::Sfu(reply) = &event
            && let Some((who, kind, session, Some(negotiation))) = reply_session(reply)
            && current_session(&before, who, kind).as_ref() == Some(session)
            && running_negotiation(&before, who, kind).is_some_and(|running| running != negotiation)
        {
            match kind {
                SessionKind::Publish => self.late_publish_replies += 1,
                SessionKind::Subscribe => self.late_subscribe_replies += 1,
            }
        }
        let partly = effects.iter().any(|e| {
            matches!(
                e,
                Effect::Send { msg: ServerMsg::PublishAnswer { rejected, .. }, .. }
                    if !rejected.is_empty()
            )
        });
        self.partly_rejected += usize::from(partly);
        if let Event::SocketClosed { who } = event
            && matches!(
                before.participants.get(&who).map(|p| &p.connection),
                Some(Connection::Joined {
                    socket_lost: None,
                    ..
                })
            )
        {
            self.detached.insert(who);
            self.graced += 1;
        }
        if event == Event::AlarmFired {
            self.grace_expired += before
                .participants
                .iter()
                .filter(|&(who, p)| {
                    p.connection.socket_lost().is_some()
                        && after.participants[who].connection == Connection::Left
                })
                .count();
        }
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
                Effect::Rebind { participant, .. } => {
                    self.rebinds += 1;
                    self.resumed_in_grace += usize::from(self.detached.remove(&participant));
                }
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
        let after = &self.room.state;
        self.detached.retain(|who| {
            after
                .participants
                .get(who)
                .is_some_and(|p| p.connection.presence().is_some())
        });
        self.check_views()?;
        self.check_recorded()?;
        self.check_alarm()
    }

    /// Plays [`Action::Reoffer`] as the events it consists of.
    fn reoffer(
        &mut self,
        who: u32,
        kinds: Vec<TrackKind>,
        fail: bool,
    ) -> Result<(), TestCaseError> {
        let offer = |kinds: &Vec<TrackKind>| Action::Send {
            who,
            msg: Msg::Publish(kinds.clone()),
        };
        self.run(offer(&kinds))?;
        let Some(held) = self.pending.iter().rposition(
            |call| matches!(call, Effect::SfuPublish { who: w, .. } if *w == ParticipantId(who)),
        ) else {
            return Ok(());
        };
        let held = self.pending.remove(held);
        let offered = self
            .room
            .state
            .joined()
            .find(|&(owner, ..)| owner == ParticipantId(who))
            .and_then(|(_, publish, _)| match publish.ready()?.negotiation {
                PublishNegotiation::OfferPending { since, .. } => Some(since),
                PublishNegotiation::Idle => None,
            });
        if let Some(since) = offered {
            self.room.now = self.room.now.max(since.after(NEGOTIATION_TIMEOUT_MS));
            self.run(Action::Alarm)?;
        }
        self.run(offer(&kinds))?;
        self.pending.insert(0, held);
        self.run(Action::Reply {
            pick: 0,
            fail: fail.then_some(FailureCause::Transient),
            refuse: 0,
        })
    }

    fn receive(&mut self, to: &[ParticipantId], msg: ServerMsg) {
        for &who in to.iter().filter(|who| !self.detached.contains(who)) {
            match &msg {
                ServerMsg::Welcome { state, resume, .. } => {
                    self.seen.insert(who, state.clone());
                    self.tokens.insert(who, resume.clone());
                }
                ServerMsg::State { state } => {
                    self.seen.insert(who, state.clone());
                }
                ServerMsg::SubscribeOffer { .. } => self.offers.push(who),
                // A client drops its subscriber peer connection, and the offer it holds, when
                // the room replaced the subscribe session.
                ServerMsg::SubscribeReset { .. } => self.offers.retain(|&o| o != who),
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

        // The transcript link reaches only those who were let in (D19).
        for effect in effects {
            if let Effect::Broadcast {
                to,
                msg: ServerMsg::Ended {
                    transcript: Some(_),
                },
            } = effect
            {
                for who in to {
                    prop_assert!(
                        before.participants[who].connection.is_admitted(),
                        "{who:?} gets the transcript link without being let in"
                    );
                }
            }
        }

        // A recording start reaches every joined client before the recorder starts.
        if let (None, Some(recording)) = (before.recording.current(), after.recording.current()) {
            let started = effects
                .iter()
                .position(|e| matches!(e, Effect::StartRecording { .. }));
            prop_assert!(started.is_some(), "{effects:?}");
            // A client waiting to resume hears of it in its welcome.
            let connected = after
                .joined()
                .filter(|(who, ..)| after.participants[who].connection.socket_lost().is_none());
            for (who, ..) in connected {
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
        // A participant who leaves before they joined is no longer in `after`.
        let known = |who: &ParticipantId| {
            after.participants.contains_key(who)
                || before.participants.contains_key(who)
                || Some(*who) == sender
        };
        let present_in = |state: &RoomState, who: &ParticipantId| {
            state
                .participants
                .get(who)
                .is_some_and(|p| p.connection.presence().is_some())
        };
        let present = |who: &ParticipantId| present_in(after, who);
        let session_of =
            |who: &ParticipantId, kind: SessionKind| current_session(after, *who, kind);

        // A joined client's reset for a session epoch that is not the current one changes
        // nothing.
        if let Event::Client {
            from,
            msg: ClientMsg::ResetSession { kind, epoch },
            ..
        } = event
            && current_epochs(before, *from).is_some_and(|epochs| epochs.of(*kind) != *epoch)
        {
            prop_assert_eq!(before, after);
            prop_assert!(effects.is_empty(), "{effects:?}");
        }

        // A reply about a session the participant no longer uses changes nothing.
        if let Event::Sfu(reply) = event
            && let Some((who, kind, session, _)) = reply_session(reply)
            && current_session(before, who, kind).as_ref() != Some(session)
        {
            prop_assert_eq!(before, after);
            prop_assert!(effects.is_empty(), "{effects:?}");
        }

        // A reply about a negotiation that is not the one running on its session (it failed or
        // timed out, and perhaps a newer one runs) changes nothing, unless it reports the session
        // gone.
        if let Event::Sfu(reply) = event
            && let Some((who, kind, _, Some(negotiation))) = reply_session(reply)
            && running_negotiation(before, who, kind) != Some(negotiation)
            && !matches!(
                reply,
                SfuReply::Failed {
                    cause: FailureCause::SessionGone,
                    ..
                }
            )
        {
            prop_assert_eq!(before, after);
            prop_assert!(effects.is_empty(), "{effects:?}");
        }

        // Every track the room publishes was accepted by the SFU, so no subscriber pulls a track
        // the SFU rejected.
        for track in after.published_tracks() {
            let remote = after.resolve(track).expect("published");
            prop_assert!(self.accepted.contains(&remote), "{remote:?} was rejected");
        }
        for effect in effects {
            match effect {
                Effect::Send { to, .. } => prop_assert!(known(to), "{effect:?}"),
                Effect::Broadcast { to, .. } => {
                    prop_assert!(!to.is_empty() && to.iter().all(known), "{effect:?}")
                }
                Effect::CloseSocket { who, .. } => prop_assert!(known(who), "{effect:?}"),
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
                    let mut expected = resumed(before, *participant, self.room.now);
                    if owner_join {
                        prop_assert_eq!(after.participants[participant].role, Role::Host);
                        expected
                            .participants
                            .get_mut(participant)
                            .expect("present")
                            .role = Role::Host;
                    }
                    prop_assert_eq!(&expected, after);
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
                            .and_then(|(_, publish, _)| publish.ready()?.published.get(kind))
                            .map(|track| track.mid.clone()),
                        _ => None,
                    };
                    prop_assert_eq!(Some(mids.clone()), unpublished.map(|mid| vec![mid]));
                }
                // Several participants whose grace ended leave in one alarm: one who leaves
                // later in the step may still have the earlier ones' tracks closed on their
                // session.
                Effect::SfuCloseTracks { who, session, .. }
                    if !present(who)
                        && current_session(before, *who, SessionKind::Subscribe).as_ref()
                            == Some(session) => {}
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
                Effect::RecordTrack { speaker, .. } => {
                    prop_assert!(present(&speaker.id), "{effect:?}")
                }
                // A participant who leaves in this step stops being recorded.
                Effect::StopRecordingTrack { who, .. } => {
                    prop_assert!(present_in(before, who), "{effect:?}")
                }
                Effect::SfuSubscribe {
                    who,
                    session,
                    tracks,
                    ..
                } => {
                    prop_assert_eq!(
                        session_of(who, SessionKind::Subscribe),
                        Some(session.clone()),
                        "{:?}",
                        effect
                    );
                    // Every requested track is published right now, and the SFU accepted it.
                    for track in tracks {
                        prop_assert!(self.accepted.contains(track), "{track:?} was rejected");
                        let published = after.joined().any(|(_, publish, _)| {
                            publish.ready().is_some_and(|publish| {
                                publish.id == track.session
                                    && publish.published.values().any(|p| p.name == track.name)
                            })
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
                if let SessionSlot::Ready(subscribe) = subscribe
                    && still_joined(subscriber)
                {
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
            // A participant's mute flag follows their own report, and otherwise only turns on,
            // together with a `Muted` to them; whoever is sent `Muted` is shown muted. (A join
            // after leaving is a new participant, unmuted.)
            let told_muted = effects.contains(&Effect::Send {
                to: who,
                msg: ServerMsg::Muted,
            });
            if let Some(was) = was
                && was.connection.presence().is_some()
                && was.muted != p.muted
            {
                let reported = matches!(
                    event,
                    Event::Client { from, msg: ClientMsg::SetMuted { muted }, .. }
                        if *from == who && *muted == p.muted
                );
                prop_assert!(
                    reported || (p.muted && told_muted),
                    "{who:?}'s mute flag changed: {effects:?}"
                );
            } else if p.connection.presence().is_some()
                && was.is_none_or(|was| was.connection.presence().is_none())
            {
                prop_assert!(
                    !p.muted || told_muted,
                    "{who:?} joined muted without being told: {effects:?}"
                );
            }
            prop_assert!(
                !told_muted || p.muted,
                "{who:?} told to mute but shown unmuted"
            );
            match &p.connection {
                // Nobody joins before the SFU created both sessions, and every session in use
                // is one the SFU created for its participant.
                Connection::Joined {
                    publish, subscribe, ..
                } => {
                    if let SessionSlot::Ready(publish) = publish {
                        prop_assert!(self.created.contains(&(who, true, publish.id.clone())));
                    }
                    if let SessionSlot::Ready(subscribe) = subscribe {
                        prop_assert!(self.created.contains(&(who, false, subscribe.id.clone())));
                        // Every received track is still published under the same name.
                        for (&track, receiving) in &subscribe.receiving {
                            prop_assert_eq!(after.resolve(track), Some(receiving.remote.clone()));
                        }
                        // No subscription is lost: every other joined participant's track is
                        // received under its current name or still wanted.
                        for track in after.published_tracks().filter(|t| t.owner != who) {
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
                    prop_assert!(recording && p.role == Role::Guest && !p.consents())
                }
                Connection::Connecting { .. } | Connection::Left => {}
            }
            // No guest is admitted without consent while the room records.
            let newly_admitted =
                p.connection.is_admitted() && !was.is_some_and(|w| w.connection.is_admitted());
            if newly_admitted && p.role == Role::Guest {
                prop_assert!(!recording || p.consents(), "{who:?} admitted");
            }
            // Whoever is in a recording room without consent was asked for it and muted.
            let unconsented = |r: &RoomState, p: &crate::state::Participant| {
                r.recording.current().is_some() && p.connection.is_admitted() && !p.consents()
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

        self.check_grace(before, event, effects)?;

        // No negotiation older than the timeout, and no due request for a replacement session,
        // survives an alarm at or after its deadline.
        if *event == Event::AlarmFired {
            let now = self.room.now;
            for (who, publish, subscribe) in after.joined() {
                let since = [
                    publish.ready().and_then(|p| p.negotiation.since()),
                    subscribe.ready().and_then(|s| s.negotiation.since()),
                ];
                for since in since {
                    prop_assert!(
                        since.is_none_or(|since| since.after(NEGOTIATION_TIMEOUT_MS) > now),
                        "{who:?}'s negotiation since {since:?} survived the alarm at {now:?}"
                    );
                }
                // A request for a replacement session is never older than the timeout.
                let replacements = [
                    match publish {
                        SessionSlot::Replacing(r) => Some(r.due()),
                        SessionSlot::Ready(_) => None,
                    },
                    match subscribe {
                        SessionSlot::Replacing(r) => Some(r.due()),
                        SessionSlot::Ready(_) => None,
                    },
                ];
                for due in replacements.into_iter().flatten() {
                    prop_assert!(due > now, "{who:?}'s replacement due at {due:?}");
                }
            }
        }

        self.check_phases(before, event, effects)
    }

    /// No wanted track waits longer than [`SUBSCRIBE_RETRY_MS`]. A wanted track's not-found count
    /// grows only when the SFU did not find it (it left the track out of a subscribe offer, or failed
    /// the pull with [`FailureCause::TrackNotFound`]), by one, and the track is then due
    /// [`FIRST_PULL_RETRY_MS`] later for the first [`FIRST_PULL_RETRIES`] times, else
    /// [`SUBSCRIBE_RETRY_MS`] later. Returns how many counts grew.
    fn check_not_found(&self, before: &RoomState, event: &Event) -> Result<usize, TestCaseError> {
        let now = self.room.now;
        let mut grown = 0;
        for (who, _, subscribe) in self.room.state.joined() {
            let Some(subscribe) = subscribe.ready() else {
                continue;
            };
            let was = before
                .joined()
                .find(|&(id, ..)| id == who)
                .and_then(|(.., s)| s.ready())
                .filter(|s| s.id == subscribe.id);
            for (track, wanted) in &subscribe.wanted {
                prop_assert!(
                    wanted.due <= now.after(SUBSCRIBE_RETRY_MS),
                    "{who:?} waits for {track:?} until {:?}",
                    wanted.due
                );
                let count = was
                    .and_then(|s| s.wanted.get(track))
                    .map_or(0, |w| w.not_found);
                // A new track under the same kind starts over at 0.
                if wanted.not_found <= count {
                    continue;
                }
                let refused = matches!(
                    event,
                    Event::Sfu(
                        SfuReply::SubscribeOffer { who: to, .. }
                            | SfuReply::Failed { who: to, cause: FailureCause::TrackNotFound, .. }
                    ) if *to == who
                );
                prop_assert!(
                    refused,
                    "{who:?}'s {track:?} counted as not found on {event:?}"
                );
                prop_assert_eq!(wanted.not_found, count + 1);
                let pause = if wanted.not_found <= FIRST_PULL_RETRIES {
                    FIRST_PULL_RETRY_MS
                } else {
                    SUBSCRIBE_RETRY_MS
                };
                prop_assert_eq!(wanted.due, now.after(pause));
                grown += 1;
            }
        }
        Ok(grown)
    }

    /// A joined participant whose socket closes keeps everything for the grace period: the step
    /// only marks them, closes no track and stops no recording. The mark never moves later, and
    /// no grace outlives an alarm at or after its end. Anyone else whose socket closes leaves.
    fn check_grace(
        &self,
        before: &RoomState,
        event: &Event,
        effects: &[Effect],
    ) -> Result<(), TestCaseError> {
        let after = &self.room.state;
        let now = self.room.now;
        for (who, p) in &after.participants {
            if let Some(lost) = before
                .participants
                .get(who)
                .and_then(|w| w.connection.socket_lost())
            {
                let still = p.connection.socket_lost();
                prop_assert!(
                    still.is_none_or(|t| t == lost),
                    "{who:?}'s grace moved: {still:?}"
                );
            }
        }
        if let Event::SocketClosed { who } = event
            && after.ended().is_none()
        {
            match before.participants.get(who).map(|p| &p.connection) {
                Some(Connection::Joined {
                    socket_lost: Some(_),
                    ..
                }) => {
                    prop_assert_eq!(before, after);
                    prop_assert!(effects.is_empty(), "{effects:?}");
                }
                Some(Connection::Joined {
                    socket_lost: None, ..
                }) => {
                    let mut expected = before.clone();
                    if let Connection::Joined { socket_lost, .. } = &mut expected
                        .participants
                        .get_mut(who)
                        .expect("joined")
                        .connection
                    {
                        *socket_lost = Some(now);
                    }
                    prop_assert_eq!(&expected, after);
                    for effect in effects {
                        let allowed = match effect {
                            Effect::Broadcast {
                                to,
                                msg: ServerMsg::State { .. },
                            } => !to.contains(who),
                            Effect::SetAlarm { .. } => true,
                            _ => false,
                        };
                        prop_assert!(allowed, "{effect:?} for {who:?}'s lost socket");
                    }
                }
                _ => prop_assert!(
                    after
                        .participants
                        .get(who)
                        .is_none_or(|p| p.connection == Connection::Left)
                ),
            }
        }
        if *event == Event::AlarmFired {
            for (who, p) in &after.participants {
                if let Some(lost) = p.connection.socket_lost() {
                    prop_assert!(
                        lost.after(LEAVE_GRACE_MS) > now,
                        "{who:?}'s grace since {lost:?} survived the alarm at {now:?}"
                    );
                }
            }
        }
        Ok(())
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
            // Nobody is joined in a scheduled room, and its only timer for the room is the
            // expiry; a host who connects has a timer of their own.
            Lifecycle::Scheduled { since } => {
                prop_assert!(after.joined().next().is_none());
                let expected = |(at, reason): (Millis, AlarmReason)| match reason {
                    AlarmReason::Expire => at == since.after(SCHEDULED_TTL_MS),
                    AlarmReason::Connect(_) => true,
                    _ => false,
                };
                prop_assert!(after.alarms().all(expected));
                prop_assert!(after.next_alarm() <= Some(since.after(SCHEDULED_TTL_MS)));
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
                // A guest who leaves before they joined is forgotten.
                for (who, _) in waiting {
                    let Some(now) = after.participants.get(who) else {
                        continue;
                    };
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
            .filter(|(who, ..)| state.participants[who].consents())
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

    /// Every connected joined client's last snapshot is the current view. The room counts as
    /// reconnecting exactly the participants whose socket the shell lost.
    fn check_views(&self) -> Result<(), TestCaseError> {
        if self.ended {
            return Ok(());
        }
        let state = &self.room.state;
        let view = state.view();
        let lost: BTreeSet<_> = state
            .participants
            .iter()
            .filter(|(_, p)| p.connection.socket_lost().is_some())
            .map(|(&who, _)| who)
            .collect();
        prop_assert_eq!(&lost, &self.detached);
        let shown: BTreeSet<_> = view
            .participants
            .iter()
            .filter(|p| p.reconnecting)
            .map(|p| p.id)
            .collect();
        prop_assert_eq!(&shown, &lost);
        for (who, ..) in state.joined().filter(|(who, ..)| !lost.contains(who)) {
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

/// `before` as a resume of `who` at `now` leaves it: their wait to resume is over, and a subscribe
/// offer sent to them meanwhile failed, so its tracks are requested again after a pause.
fn resumed(before: &RoomState, who: ParticipantId, now: Millis) -> RoomState {
    let mut expected = before.clone();
    let p = expected.participants.get_mut(&who).expect("present");
    if let Connection::Joined {
        socket_lost,
        subscribe,
        ..
    } = &mut p.connection
        && socket_lost.take().is_some()
        && let SessionSlot::Ready(subscribe) = subscribe
        && let SubscribeNegotiation::AwaitingAnswer { offered, .. } = &subscribe.negotiation
    {
        let retry = now.after(SUBSCRIBE_RETRY_MS);
        for o in offered {
            if let Some(wanted) = subscribe.wanted.get_mut(&o.track) {
                wanted.due = retry;
            }
        }
        subscribe.negotiation = SubscribeNegotiation::Idle;
    }
    expected
}

/// The id of `who`'s session of `kind`, if they are joined and it is in use.
fn current_session(state: &RoomState, who: ParticipantId, kind: SessionKind) -> Option<SessionId> {
    match &state.participants.get(&who)?.connection {
        Connection::Joined {
            publish, subscribe, ..
        } => match kind {
            SessionKind::Publish => publish.ready().map(|p| p.id.clone()),
            SessionKind::Subscribe => subscribe.ready().map(|s| s.id.clone()),
        },
        _ => None,
    }
}

/// The epochs of `who`'s sessions, if they are joined.
fn current_epochs(state: &RoomState, who: ParticipantId) -> Option<SessionEpochs> {
    match state.participants.get(&who)?.connection {
        Connection::Joined { epochs, .. } => Some(epochs),
        _ => None,
    }
}

/// The id of the negotiation running on `who`'s session of `kind`, if they are joined, the session
/// is in use and a negotiation runs on it.
fn running_negotiation(
    state: &RoomState,
    who: ParticipantId,
    kind: SessionKind,
) -> Option<NegotiationId> {
    match &state.participants.get(&who)?.connection {
        Connection::Joined {
            publish, subscribe, ..
        } => match kind {
            SessionKind::Publish => publish
                .ready()
                .filter(|p| p.negotiation != PublishNegotiation::Idle)
                .map(|p| p.last_negotiation),
            SessionKind::Subscribe => subscribe
                .ready()
                .filter(|s| s.negotiation != SubscribeNegotiation::Idle)
                .map(|s| s.last_negotiation),
        },
        _ => None,
    }
}

/// The participant, kind and id of the session a reply is about, for replies that name one,
/// with the negotiation it names, if any.
fn reply_session(
    reply: &SfuReply,
) -> Option<(
    ParticipantId,
    SessionKind,
    &SessionId,
    Option<NegotiationId>,
)> {
    match reply {
        SfuReply::Published {
            who,
            session,
            negotiation,
            ..
        }
        | SfuReply::Failed {
            who,
            call:
                SfuCall::Publish {
                    session,
                    negotiation,
                },
            ..
        } => Some((*who, SessionKind::Publish, session, Some(*negotiation))),
        SfuReply::SubscribeOffer {
            who,
            session,
            negotiation,
            ..
        }
        | SfuReply::Renegotiated {
            who,
            session,
            negotiation,
        }
        | SfuReply::Failed {
            who,
            call:
                SfuCall::Subscribe {
                    session,
                    negotiation,
                }
                | SfuCall::Renegotiate {
                    session,
                    negotiation,
                },
            ..
        } => Some((*who, SessionKind::Subscribe, session, Some(*negotiation))),
        SfuReply::SessionCreated { .. }
        | SfuReply::Failed {
            call: SfuCall::NewSession(_),
            ..
        } => None,
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
    let (mut released, mut expired, mut promoted, mut replaced) = (0, 0, 0, 0);
    let (mut graced, mut resumed_in_grace, mut grace_expired) = (0, 0, 0);
    let (mut partly_rejected, mut late_publish, mut late_subscribe) = (0, 0, 0);
    let mut not_found = 0;
    for case in 0..1000 {
        let actions = proptest::collection::vec(action(), 1..250)
            .new_tree(&mut runner)
            .expect("tree")
            .current();
        let mut sim = Sim::new(policy(case % 2 == 0));
        let mut saw = [false; 6];
        for action in actions {
            sim.run(action).expect("invariants hold");
            let state = &sim.room.state;
            saw[0] |= state
                .joined()
                .any(|(_, _, s)| s.ready().is_some_and(|s| !s.receiving.is_empty()));
            saw[1] |= state.recording.current().is_some();
            saw[2] |= state
                .participants
                .values()
                .any(|p| p.connection == Connection::AwaitingConsent);
            saw[3] |= sim.rebinds > 0;
            saw[4] |= state.joined().any(|(_, _, s)| {
                s.ready().is_some_and(|s| {
                    s.negotiation.since().is_none()
                        && s.wanted.values().any(|w| w.due > sim.room.now)
                })
            });
            saw[5] |= state.joined().any(|(_, p, s)| {
                matches!(p, SessionSlot::Replacing(_)) || matches!(s, SessionSlot::Replacing(_))
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
        replaced += usize::from(saw[5]);
        graced += usize::from(sim.graced > 0);
        resumed_in_grace += usize::from(sim.resumed_in_grace > 0);
        grace_expired += usize::from(sim.grace_expired > 0);
        partly_rejected += usize::from(sim.partly_rejected > 0);
        late_publish += usize::from(sim.late_publish_replies > 0);
        late_subscribe += usize::from(sim.late_subscribe_replies > 0);
        not_found += usize::from(sim.not_found > 0);
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
            replaced,
            graced,
            resumed_in_grace,
            grace_expired,
            partly_rejected,
            late_publish,
            late_subscribe,
            not_found,
        ]
        .iter()
        .all(|&n| n >= 20),
        "receiving {receiving}, recording {recording}, ended {ended}, \
         awaiting consent {awaiting}, resumed {resumed}, retry pending {retried}, \
         unpublished {unpublished}, publish timed out {publish_timeouts}, \
         waiting guests let in by the first host {released}, expired unused {expired}, \
         promoted by the owner {promoted}, session replaced {replaced}, \
         socket lost in grace {graced}, resumed in grace {resumed_in_grace}, \
         left when the grace ended {grace_expired}, publish partly rejected {partly_rejected}, \
         late publish reply during a newer offer {late_publish}, \
         late subscribe reply during a newer negotiation {late_subscribe}, \
         track not found {not_found}"
    );
}

/// What link holders do to a room's size: guests join from new sockets, participants leave or
/// lose their socket, the owner joins, a host locks or unlocks.
#[derive(Debug, Clone)]
enum Crowd {
    Guest { consent: bool },
    Owner,
    Leave(usize),
    Drop(usize),
    Lock(bool),
}

fn crowd() -> impl Strategy<Value = Crowd> {
    prop_oneof![
        6 => any::<bool>().prop_map(|consent| Crowd::Guest { consent }),
        1 => Just(Crowd::Owner),
        3 => any::<usize>().prop_map(Crowd::Leave),
        1 => any::<usize>().prop_map(Crowd::Drop),
        1 => any::<bool>().prop_map(Crowd::Lock),
    ]
}

/// The `pick`-th of `who`, if any.
fn nth(who: Vec<ParticipantId>, pick: usize) -> Option<ParticipantId> {
    (!who.is_empty()).then(|| who[pick % who.len()])
}

/// Runs `actions` and checks the caps after every step; returns whether a join was refused
/// because of [`crate::MAX_PRESENT`] and because of [`crate::MAX_ROSTER`].
fn run_crowd(from_start: bool, actions: Vec<Crowd>) -> Result<(bool, bool), TestCaseError> {
    let (mut room, _) = Room::with_policy(policy(from_start));
    let (mut next, mut owner_joins) = (0, 0);
    let (mut refused_present, mut refused_roster) = (false, false);
    for action in actions {
        let before = room.state.clone();
        match action {
            Crowd::Guest { consent } => {
                next += 1;
                let who = ParticipantId(next);
                let effects = room.join(who, false, consent);
                if before.is_full() {
                    prop_assert_eq!(
                        effects,
                        vec![
                            Effect::Send {
                                to: who,
                                msg: ServerMsg::Error {
                                    code: ErrorCode::RoomFull
                                }
                            },
                            Effect::CloseSocket {
                                who,
                                why: CloseCode::RoomFull
                            }
                        ]
                    );
                    prop_assert_eq!(&room.state, &before, "a refused join changes nothing");
                    refused_present |= before.present().count() >= crate::MAX_PRESENT;
                    refused_roster |= before.participants.len() >= crate::MAX_ROSTER;
                } else {
                    let connection = &room.state.participants[&who].connection;
                    prop_assert!(connection.presence().is_some());
                    // Only those who joined stay on the roster once they leave.
                    if matches!(connection, Connection::Connecting { .. }) {
                        room.connect(who);
                    }
                }
            }
            Crowd::Owner => {
                next += 1;
                owner_joins += 1;
                let who = ParticipantId(next);
                room.join(who, true, true);
                room.connect(who);
                prop_assert!(
                    room.state.participants[&who].connection.is_admitted(),
                    "the owner always gets in"
                );
            }
            Crowd::Leave(pick) => {
                if let Some(who) = nth(room.state.present().collect(), pick) {
                    room.client(who, ClientMsg::Leave);
                }
            }
            Crowd::Drop(pick) => {
                if let Some(who) = nth(room.state.present().collect(), pick) {
                    room.apply(Event::SocketClosed { who });
                }
            }
            Crowd::Lock(on) => {
                let hosts = room
                    .state
                    .joined()
                    .map(|(id, ..)| id)
                    .filter(|id| room.state.participants[id].role == Role::Host)
                    .collect();
                if let Some(host) = nth(hosts, 0) {
                    room.client(host, ClientMsg::Lock { on });
                }
            }
        }
        // Only the owner's joins may go beyond the caps.
        prop_assert!(room.state.present().count() <= crate::MAX_PRESENT + owner_joins);
        prop_assert!(room.state.participants.len() <= crate::MAX_ROSTER + owner_joins);
    }
    Ok((refused_present, refused_roster))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn a_room_never_grows_beyond_its_caps(
        from_start in any::<bool>(),
        actions in proptest::collection::vec(crowd(), 1..1500),
    ) {
        run_crowd(from_start, actions)?;
    }
}

/// The crowds above fill rooms to both caps; otherwise the property proves little.
#[test]
fn random_crowds_reach_both_caps() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let (mut present, mut roster) = (0, 0);
    for case in 0..40 {
        let actions = proptest::collection::vec(crowd(), 1000..1500)
            .new_tree(&mut runner)
            .expect("tree")
            .current();
        let (p, r) = run_crowd(case % 2 == 0, actions).expect("the caps hold");
        present += usize::from(p);
        roster += usize::from(r);
    }
    assert!(
        present >= 20 && roster >= 20,
        "refused at the present cap {present}, at the roster cap {roster}"
    );
}
