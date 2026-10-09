//! Property tests: random but plausible event sequences keep the room's invariants.
//!
//! A simulator plays the shell: it answers the SFU calls the room requested (or fails them),
//! answers subscribe offers, and delivers client messages from a small pool of participants.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::support::{Room, name, token};
use crate::state::Connection;
use crate::{
    ClientMsg, Effect, ErrorCode, Event, HostToken, LocalTrack, Mid, Millis, OfferedTrack,
    ParticipantId, RecordingPolicy, RemoteTrack, ResumeToken, Rid, Role, RoomState, RoomView, Sdp,
    ServerMsg, SessionId, SessionKind, SfuCall, SfuReply, TrackKind, TrackName, TrackRef,
};

const POOL: u32 = 6;
const KINDS: [TrackKind; 3] = [TrackKind::Audio, TrackKind::Video, TrackKind::Screen];

#[derive(Debug, Clone)]
enum Action {
    Join {
        who: u32,
        host: bool,
        wrong_token: bool,
        consent: bool,
    },
    Send {
        who: u32,
        msg: Msg,
    },
    /// A client joins from socket `socket` with the resume token `whose` was welcomed with.
    Resume {
        socket: u32,
        whose: u32,
    },
    /// Answer a pending SFU call.
    Reply {
        pick: usize,
        fail: bool,
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
    Alarm,
    Wait {
        ms: u64,
    },
}

#[derive(Debug, Clone)]
enum Msg {
    Publish(Vec<TrackKind>),
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
        4 => (who(), any::<bool>(), (0..8u8).prop_map(|n| n == 0), (0..4u8).prop_map(|n| n != 0))
            .prop_map(|(who, host, wrong_token, consent)| Action::Join { who, host, wrong_token, consent }),
        8 => (who(), msg()).prop_map(|(who, msg)| Action::Send { who, msg }),
        1 => (who(), who()).prop_map(|(socket, whose)| Action::Resume { socket, whose }),
        16 => (any::<usize>(), (0..20u8).prop_map(|n| n == 0)).prop_map(|(pick, fail)| Action::Reply { pick, fail }),
        6 => any::<usize>().prop_map(|pick| Action::Answer { pick }),
        1 => (who(), kind()).prop_map(|(who, kind)| Action::TrackClosed { who, kind }),
        1 => who().prop_map(|who| Action::SocketClosed { who }),
        1 => Just(Action::Alarm),
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
            ended: false,
            counter: 0,
        }
    }

    /// A client message with a fresh token, as the shell sends it.
    fn client(&mut self, from: ParticipantId, msg: ClientMsg) -> Event {
        self.counter += 1;
        Event::Client {
            from,
            msg,
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
                host,
                wrong_token,
                consent,
            } => {
                let host_token = match (host, wrong_token) {
                    (_, true) => Some(HostToken::new("wrong")),
                    (true, false) => Some(super::support::host_token()),
                    (false, false) => None,
                };
                self.client(
                    id(who),
                    ClientMsg::Join {
                        name: name("Someone"),
                        consent,
                        host_token,
                        resume: None,
                    },
                )
            }
            Action::Resume { socket, whose } => {
                let resume = Some(self.tokens.get(&id(whose))?.clone());
                self.client(
                    id(socket),
                    ClientMsg::Join {
                        name: name("Someone"),
                        consent: true,
                        host_token: None,
                        resume,
                    },
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
                self.client(id(who), msg)
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
                )
            }
            Action::TrackClosed { who, kind } => {
                Event::Sfu(SfuReply::TrackClosed { who: id(who), kind })
            }
            Action::SocketClosed { who } => Event::SocketClosed { who: id(who) },
            Action::Alarm => Event::AlarmFired,
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
            && before.ended.is_none()
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
        let sender = match event {
            Event::Client { from, .. } => Some(*from),
            _ => None,
        };
        let known =
            |who: &ParticipantId| after.participants.contains_key(who) || Some(*who) == sender;
        let present = |who: &ParticipantId| {
            after
                .participants
                .get(who)
                .is_some_and(|p| p.connection.presence().is_some())
        };
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
                    // A resume moves a participant to the sender's socket and changes nothing else.
                    prop_assert!(
                        Some(*socket) == sender && present(participant),
                        "{effect:?}"
                    );
                    prop_assert_eq!(before, after);
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
                Effect::RecordTrack { who, .. } | Effect::StopRecordingTrack { who, .. } => {
                    prop_assert!(present(who), "{effect:?}")
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
                                && publish.published.values().any(|n| *n == track.name)
                        });
                        prop_assert!(published, "{track:?} is not published");
                    }
                }
                _ => {}
            }
        }

        // Whoever leaves or is kicked stops reaching every other subscriber.
        if after.ended.is_none() {
            let left = before.participants.iter().filter(|&(who, p)| {
                p.connection.presence().is_some()
                    && after.participants.get(who).map(|p| &p.connection) == Some(&Connection::Left)
            });
            for (&gone, _) in left {
                for (subscriber, _, subscribe) in before.joined() {
                    let received: BTreeSet<_> = subscribe
                        .receiving
                        .iter()
                        .filter(|(track, _)| track.owner == gone)
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
                        "{subscriber:?} still receives {received:?} of {gone:?}: {effects:?}"
                    );
                }
            }
        }

        let recording = after.recording.current().is_some();
        for (&who, p) in &after.participants {
            let was = before.participants.get(&who);
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
                // Only a locked room has guests knocking.
                Connection::Knocking => prop_assert!(after.locked && p.role == Role::Guest),
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

        // The empty timer runs exactly while nobody is admitted.
        if after.ended.is_none() {
            let admitted = after
                .participants
                .values()
                .any(|p| p.connection.is_admitted());
            prop_assert_eq!(after.empty_since.is_none(), admitted);
        }
        Ok(())
    }

    /// While recording, exactly the consenting joined speakers' current audio is recorded.
    fn check_recorded(&self) -> Result<(), TestCaseError> {
        let state = &self.room.state;
        if self.ended || state.recording.current().is_none() {
            return Ok(());
        }
        for (who, ..) in state.joined() {
            let audio = TrackRef {
                owner: who,
                kind: TrackKind::Audio,
            };
            if let Some(track) = state.resolve(audio) {
                prop_assert_eq!(
                    self.recorded.contains(&track),
                    state.participants[&who].consent_recording,
                    "{:?}",
                    who
                );
            }
        }
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
        actions in proptest::collection::vec(action(), 1..200),
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
fn random_sessions_reach_joined_publishing_and_recording_rooms() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let (mut receiving, mut recording, mut ended) = (0, 0, 0);
    let (mut awaiting, mut resumed, mut retried) = (0, 0, 0);
    for case in 0..200 {
        let actions = proptest::collection::vec(action(), 1..200)
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
    }
    assert!(
        [receiving, recording, ended, awaiting, resumed, retried]
            .iter()
            .all(|&n| n >= 20),
        "receiving {receiving}, recording {recording}, ended {ended}, \
         awaiting consent {awaiting}, resumed {resumed}, retry pending {retried}"
    );
}
