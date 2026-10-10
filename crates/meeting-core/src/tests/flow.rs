//! The whole life of a two-person meeting.

use super::support::{Room, T0, local_track, meeting, session, track_name};
use crate::{
    ClientMsg, EMPTY_ROOM_TIMEOUT_MS, Effect, Event, IncomingTrack, LEAVE_GRACE_MS, Mid,
    NEGOTIATION_TIMEOUT_MS, NegotiationId, OfferedTrack, ParticipantId, Phase, Presence,
    RemoteTrack, Rid, SCHEDULED_TTL_MS, Sdp, ServerMsg, SessionKind, SfuReply, TrackKind, TrackRef,
};

const HOST: ParticipantId = ParticipantId(1);
const GUEST: ParticipantId = ParticipantId(2);
const AV: [TrackKind; 2] = [TrackKind::Audio, TrackKind::Video];
/// Every session in this meeting runs one negotiation, the first, which each session counts as 1.
const FIRST: NegotiationId = NegotiationId(1);

fn remote(owner: ParticipantId, kind: TrackKind) -> RemoteTrack {
    RemoteTrack {
        session: session(owner, SessionKind::Publish),
        name: track_name(owner, kind),
    }
}

fn sub_mid(owner: ParticipantId, kind: TrackKind) -> Mid {
    Mid::new(format!("sub-{}-{kind:?}", owner.0))
}

/// Delivers the SFU's offer for `kinds` of `owner`, the client's answer and the renegotiation.
fn complete_subscription(room: &mut Room, who: ParticipantId, owner: ParticipantId) {
    let tracks = AV
        .map(|kind| OfferedTrack {
            mid: sub_mid(owner, kind),
            track: remote(owner, kind),
        })
        .to_vec();
    let effects = room.sfu(SfuReply::SubscribeOffer {
        who,
        session: session(who, SessionKind::Subscribe),
        negotiation: FIRST,
        sdp_offer: Sdp::new("sub-offer"),
        tracks,
    });
    let incoming = AV
        .map(|kind| IncomingTrack {
            mid: sub_mid(owner, kind),
            track: TrackRef { owner, kind },
        })
        .to_vec();
    assert_eq!(
        effects,
        [Effect::Send {
            to: who,
            msg: ServerMsg::SubscribeOffer {
                sdp: Sdp::new("sub-offer"),
                tracks: incoming
            }
        }]
    );
    let effects = room.client(
        who,
        ClientMsg::SubscribeAnswer {
            sdp: Sdp::new("sub-answer"),
        },
    );
    assert_eq!(
        effects,
        [Effect::SfuRenegotiate {
            who,
            session: session(who, SessionKind::Subscribe),
            negotiation: FIRST,
            sdp_answer: Sdp::new("sub-answer")
        }]
    );
    assert_eq!(
        room.sfu(SfuReply::Renegotiated {
            who,
            session: session(who, SessionKind::Subscribe),
            negotiation: FIRST,
        }),
        []
    );
}

/// Publishes audio and video for `who` and checks that `other` is asked to subscribe.
fn publish_av(room: &mut Room, who: ParticipantId, other: ParticipantId) {
    let tracks = AV.map(|kind| local_track(who, kind)).to_vec();
    let effects = room.client(
        who,
        ClientMsg::PublishOffer {
            sdp: Sdp::new("offer"),
            tracks: tracks.clone(),
        },
    );
    // The offer fails if the SFU does not answer in time.
    assert_eq!(
        effects,
        [
            Effect::SfuPublish {
                who,
                session: session(who, SessionKind::Publish),
                negotiation: FIRST,
                sdp_offer: Sdp::new("offer"),
                tracks
            },
            Effect::SetAlarm {
                at: room.now.after(NEGOTIATION_TIMEOUT_MS)
            },
        ]
    );
    room.now = room.now.after(1_000);
    let effects = room.sfu(SfuReply::Published {
        who,
        session: session(who, SessionKind::Publish),
        negotiation: FIRST,
        sdp_answer: Sdp::new("answer"),
        rejected: Vec::new(),
    });
    let [
        Effect::Broadcast {
            to,
            msg: ServerMsg::State { state },
        },
        answer,
        subscribe,
        alarm,
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(to, &[HOST, GUEST]);
    assert_eq!(
        state
            .participants
            .iter()
            .find(|p| p.id == who)
            .map(|p| p.tracks.clone()),
        Some(AV.to_vec())
    );
    assert_eq!(
        answer,
        &Effect::Send {
            to: who,
            msg: ServerMsg::PublishAnswer {
                sdp: Sdp::new("answer"),
                rejected: Vec::new()
            }
        }
    );
    assert_eq!(
        subscribe,
        &Effect::SfuSubscribe {
            who: other,
            session: session(other, SessionKind::Subscribe),
            negotiation: FIRST,
            tracks: AV.map(|kind| remote(who, kind)).to_vec()
        }
    );
    // The subscribe negotiation fails if it takes too long.
    assert_eq!(
        alarm,
        &Effect::SetAlarm {
            at: room.now.after(NEGOTIATION_TIMEOUT_MS)
        }
    );
}

#[test]
fn host_and_guest_meet_publish_subscribe_leave_and_the_room_ends() {
    // The owner created the meeting ahead of time; it waits for its first host.
    let (mut room, effects) = Room::new();
    assert_eq!(
        effects,
        [Effect::SetAlarm {
            at: T0.after(SCHEDULED_TTL_MS)
        }]
    );
    assert_eq!(room.state.view().phase, Phase::Scheduled);

    // Days later, the owner joins from their authenticated socket and becomes the host.
    room.now = T0.after(3 * 24 * 60 * 60 * 1000);
    assert_eq!(
        room.join(HOST, true, true),
        [
            Effect::SfuNewSession {
                who: HOST,
                kind: SessionKind::Publish
            },
            Effect::SfuNewSession {
                who: HOST,
                kind: SessionKind::Subscribe
            },
        ]
    );
    // Once the host is in, the meeting is live and the expiry alarm no longer applies.
    let effects = room.connect(HOST);
    let [
        Effect::Send {
            to: HOST,
            msg: ServerMsg::Welcome {
                you: HOST, state, ..
            },
        },
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(state.meeting, meeting());
    assert_eq!(state.phase, Phase::Live);
    assert_eq!(state.participants[0].presence, Presence::Joined);

    // The guest joins; the host sees them connect, then join.
    let effects = room.join(GUEST, false, true);
    assert!(matches!(effects.as_slice(), [Effect::Broadcast { to, .. }, _, _] if to == &[HOST]));
    let effects = room.connect(GUEST);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Send { to: GUEST, msg: ServerMsg::Welcome { you: GUEST, .. } }, Effect::Broadcast { to, .. }]
            if to == &[HOST]
    ));

    // Both publish; each subscribes to the other.
    publish_av(&mut room, HOST, GUEST);
    complete_subscription(&mut room, GUEST, HOST);
    publish_av(&mut room, GUEST, HOST);
    complete_subscription(&mut room, HOST, GUEST);

    // The guest lowers the host's video layer.
    let video = TrackRef {
        owner: HOST,
        kind: TrackKind::Video,
    };
    assert_eq!(
        room.client(
            GUEST,
            ClientMsg::SetLayer {
                track: video,
                rid: Rid::Low
            }
        ),
        [Effect::SfuUpdateLayer {
            who: GUEST,
            session: session(GUEST, SessionKind::Subscribe),
            mid: sub_mid(HOST, TrackKind::Video),
            track: remote(HOST, TrackKind::Video),
            rid: Rid::Low,
        }]
    );

    // The guest leaves: the host stops receiving their tracks.
    let effects = room.client(GUEST, ClientMsg::Leave);
    let [
        Effect::Broadcast {
            to,
            msg: ServerMsg::State { state },
        },
        close,
        Effect::CloseSocket { who: GUEST },
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(to, &[HOST]);
    assert_eq!(state.participants.len(), 1);
    assert_eq!(
        close,
        &Effect::SfuCloseTracks {
            who: HOST,
            session: session(HOST, SessionKind::Subscribe),
            mids: AV.map(|kind| sub_mid(GUEST, kind)).to_vec(),
        }
    );

    // The host's socket drops: the host keeps their place for the grace period, so the room is
    // not empty yet; the alarm waits for the grace to end.
    room.now = room.now.after(60_000);
    let grace_ends = room.now.after(LEAVE_GRACE_MS);
    assert_eq!(
        room.apply(Event::SocketClosed { who: HOST }),
        [Effect::SetAlarm { at: grace_ends }]
    );

    // No client takes the place: the host leaves when the grace ends, and the empty time starts
    // then.
    room.now = grace_ends;
    let left_at = room.now;
    assert_eq!(
        room.apply(Event::AlarmFired),
        [Effect::SetAlarm {
            at: left_at.after(EMPTY_ROOM_TIMEOUT_MS)
        }]
    );

    // An early alarm re-arms; the due alarm ends the meeting.
    room.now = left_at.after(EMPTY_ROOM_TIMEOUT_MS - 1);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [Effect::SetAlarm {
            at: left_at.after(EMPTY_ROOM_TIMEOUT_MS)
        }]
    );
    room.now = left_at.after(EMPTY_ROOM_TIMEOUT_MS);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::ReleaseMeetingId { meeting: meeting() },
            Effect::EndMeeting
        ]
    );
    assert_eq!(room.state.ended(), Some(room.now));

    // Nothing happens after the end.
    assert_eq!(room.join(ParticipantId(3), false, true), []);
}
