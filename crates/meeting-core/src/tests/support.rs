//! Helpers that drive a room through events.

use crate::{
    ClientMsg, DisplayName, Effect, Event, LocalTrack, MeetingId, Mid, Millis, ParticipantId,
    RecordingPolicy, ResumeToken, RoomState, Sdp, SessionId, SessionKind, SfuReply, TrackKind,
    TrackName, TranscriptRef, step,
};

pub(super) const T0: Millis = Millis(1_700_000_000_000);

pub(super) fn transcript() -> TranscriptRef {
    TranscriptRef::new("t-123")
}

pub(super) fn meeting() -> MeetingId {
    MeetingId::parse("amber-otter-ocean").expect("listed words")
}

pub(super) fn name(s: &str) -> DisplayName {
    DisplayName::parse(s).expect("valid name")
}

pub(super) fn session(who: ParticipantId, kind: SessionKind) -> SessionId {
    SessionId::new(format!("{kind:?}-{}", who.0))
}

pub(super) fn track_name(who: ParticipantId, kind: TrackKind) -> TrackName {
    TrackName::new(format!("{kind:?}-{}", who.0))
}

pub(super) fn local_track(who: ParticipantId, kind: TrackKind) -> LocalTrack {
    LocalTrack {
        mid: Mid::new(format!("{kind:?}")),
        kind,
        name: track_name(who, kind),
    }
}

/// A distinct token per counter value, standing in for the shell's random bytes.
pub(super) fn token(n: u32) -> ResumeToken {
    let mut bytes = [0; 16];
    bytes[..4].copy_from_slice(&n.to_be_bytes());
    ResumeToken::from_random(bytes)
}

/// A room plus a clock and a source of fresh tokens.
pub(super) struct Room {
    pub(super) state: RoomState,
    pub(super) now: Millis,
    tokens: u32,
}

impl Room {
    /// A scheduled room where the host decides when to record.
    pub(super) fn new() -> (Self, Vec<Effect>) {
        Self::with_policy(RecordingPolicy::HostDecides)
    }

    pub(super) fn with_policy(policy: RecordingPolicy) -> (Self, Vec<Effect>) {
        let (state, effects) = RoomState::new(meeting(), transcript(), policy, T0);
        let room = Self {
            state,
            now: T0,
            tokens: 0,
        };
        (room, effects)
    }

    /// The token the next client message carries; [`Room::client_from`] uses it up.
    pub(super) fn next_token(&self) -> ResumeToken {
        token(self.tokens)
    }

    pub(super) fn apply(&mut self, event: Event) -> Vec<Effect> {
        let (state, effects) = step(self.state.clone(), self.now, event);
        self.state = state;
        effects
    }

    /// A message from a socket that is not the owner's.
    pub(super) fn client(&mut self, from: ParticipantId, msg: ClientMsg) -> Vec<Effect> {
        self.client_from(from, msg, false)
    }

    /// A message from a socket the shell reports as the owner's or not.
    pub(super) fn client_from(
        &mut self,
        from: ParticipantId,
        msg: ClientMsg,
        owner: bool,
    ) -> Vec<Effect> {
        let fresh_token = self.next_token();
        self.tokens += 1;
        self.apply(Event::Client {
            from,
            msg,
            owner,
            fresh_token,
        })
    }

    pub(super) fn sfu(&mut self, reply: SfuReply) -> Vec<Effect> {
        self.apply(Event::Sfu(reply))
    }

    /// Joins from the owner's socket (as a host) or another socket (as a guest).
    pub(super) fn join(&mut self, who: ParticipantId, host: bool, consent: bool) -> Vec<Effect> {
        self.client_from(
            who,
            ClientMsg::Join {
                name: name(&format!("P{}", who.0)),
                consent,
                resume: None,
            },
            host,
        )
    }

    /// Delivers both session replies; returns the effects of the second.
    pub(super) fn connect(&mut self, who: ParticipantId) -> Vec<Effect> {
        let kind = SessionKind::Publish;
        self.sfu(SfuReply::SessionCreated {
            who,
            kind,
            session: session(who, kind),
        });
        let kind = SessionKind::Subscribe;
        self.sfu(SfuReply::SessionCreated {
            who,
            kind,
            session: session(who, kind),
        })
    }

    /// Joins and connects; returns the effects of the connection.
    pub(super) fn enter(&mut self, who: ParticipantId, host: bool) -> Vec<Effect> {
        self.join(who, host, true);
        self.connect(who)
    }

    /// Publishes tracks and delivers the SFU's answer; returns the effects of the answer.
    pub(super) fn publish(&mut self, who: ParticipantId, kinds: &[TrackKind]) -> Vec<Effect> {
        let tracks = kinds.iter().map(|&k| local_track(who, k)).collect();
        self.client(
            who,
            ClientMsg::PublishOffer {
                sdp: Sdp::new("offer"),
                tracks,
            },
        );
        self.sfu(SfuReply::Published {
            who,
            sdp_answer: Sdp::new("answer"),
        })
    }
}
