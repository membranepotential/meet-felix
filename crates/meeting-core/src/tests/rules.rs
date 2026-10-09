//! One test per room rule.

use super::support::{Room, T0, name, session, track_name, transcript};
use crate::{
    ClientMsg, EMPTY_ROOM_TIMEOUT_MS, Effect, ErrorCode, Event, HostToken, Mid, Millis,
    NEGOTIATION_TIMEOUT_MS, OfferedTrack, ParticipantId, Presence, RecordingPolicy, RemoteTrack,
    Role, RoomState, SUBSCRIBE_RETRY_MS, Sdp, ServerMsg, SessionKind, SfuCall, SfuReply, TrackKind,
};

const HOST: ParticipantId = ParticipantId(1);
const GUEST: ParticipantId = ParticipantId(2);
const OTHER: ParticipantId = ParticipantId(3);

fn error(to: ParticipantId, code: ErrorCode) -> Vec<Effect> {
    vec![Effect::Send {
        to,
        msg: ServerMsg::Error { code },
    }]
}

fn presence(room: &Room, who: ParticipantId) -> Option<Presence> {
    room.state
        .view()
        .participants
        .iter()
        .find(|p| p.id == who)
        .map(|p| p.presence)
}

/// A room with a joined host and a joined guest.
fn meeting_room() -> Room {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.enter(GUEST, false);
    room
}

#[test]
fn host_token_decides_the_role() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.enter(GUEST, false);
    let roles: Vec<Role> = room
        .state
        .view()
        .participants
        .iter()
        .map(|p| p.role)
        .collect();
    assert_eq!(roles, [Role::Host, Role::Guest]);

    let wrong = ClientMsg::Join {
        name: name("Eve"),
        consent: true,
        host_token: Some(HostToken::new("guess")),
        resume: None,
    };
    assert_eq!(
        room.client(OTHER, wrong),
        error(OTHER, ErrorCode::InvalidHostToken)
    );
    assert_eq!(presence(&room, OTHER), None);
}

#[test]
fn guests_knock_on_a_locked_room_until_a_host_admits_them() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.client(HOST, ClientMsg::Lock { on: true });

    let effects = room.join(GUEST, false, true);
    assert_eq!(
        effects[1..],
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Waiting
            },
            Effect::Broadcast {
                to: vec![HOST],
                msg: ServerMsg::Knock { who: GUEST }
            },
        ]
    );
    assert_eq!(presence(&room, GUEST), Some(Presence::Knocking));
    // Knocking guests cannot act in the room.
    assert_eq!(
        room.client(GUEST, ClientMsg::Admit { who: GUEST }),
        error(GUEST, ErrorCode::NotJoined)
    );

    let effects = room.client(HOST, ClientMsg::Admit { who: GUEST });
    assert_eq!(
        effects[1..],
        [
            Effect::SfuNewSession {
                who: GUEST,
                kind: SessionKind::Publish
            },
            Effect::SfuNewSession {
                who: GUEST,
                kind: SessionKind::Subscribe
            },
        ]
    );
    room.connect(GUEST);
    assert_eq!(presence(&room, GUEST), Some(Presence::Joined));
}

#[test]
fn unlocking_admits_everyone_knocking() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.client(HOST, ClientMsg::Lock { on: true });
    room.join(GUEST, false, true);
    room.join(OTHER, false, true);
    room.client(HOST, ClientMsg::Lock { on: false });
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
    assert_eq!(presence(&room, OTHER), Some(Presence::Connecting));
}

#[test]
fn a_guest_without_consent_waits_outside_a_recording_room_until_they_consent() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.client(HOST, ClientMsg::SetRecording { on: true });

    // The host sees the guest waiting; the guest is asked for consent.
    let effects = room.join(GUEST, false, false);
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::Broadcast { to, .. },
                Effect::Send { to: GUEST, msg: ServerMsg::ConsentRequired },
            ] if to == &[HOST]
        ),
        "{effects:#?}"
    );
    assert_eq!(presence(&room, GUEST), Some(Presence::AwaitingConsent));

    let effects = room.client(GUEST, ClientMsg::SetConsent { consent: true });
    assert!(
        matches!(
            effects.as_slice(),
            [
                Effect::Broadcast { .. },
                Effect::SfuNewSession {
                    who: GUEST,
                    kind: SessionKind::Publish
                },
                Effect::SfuNewSession {
                    who: GUEST,
                    kind: SessionKind::Subscribe
                },
            ]
        ),
        "{effects:#?}"
    );
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
}

#[test]
fn neither_admitting_nor_unlocking_lets_in_a_guest_without_consent_while_recording() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.client(HOST, ClientMsg::Lock { on: true });
    room.client(HOST, ClientMsg::SetRecording { on: true });

    let effects = room.join(GUEST, false, false);
    assert_eq!(
        effects[1..],
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Waiting
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::ConsentRequired
            },
            Effect::Broadcast {
                to: vec![HOST],
                msg: ServerMsg::Knock { who: GUEST }
            },
        ]
    );
    let effects = room.client(HOST, ClientMsg::Admit { who: GUEST });
    assert_eq!(
        effects[1..],
        [Effect::Send {
            to: GUEST,
            msg: ServerMsg::ConsentRequired
        }]
    );
    assert_eq!(presence(&room, GUEST), Some(Presence::AwaitingConsent));

    room.join(OTHER, false, false);
    room.client(HOST, ClientMsg::Lock { on: false });
    assert_eq!(presence(&room, OTHER), Some(Presence::AwaitingConsent));

    // Once recording stops, nothing keeps them out.
    room.client(HOST, ClientMsg::SetRecording { on: false });
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
    assert_eq!(presence(&room, OTHER), Some(Presence::Connecting));
}

#[test]
fn only_hosts_run_the_room() {
    let mut room = meeting_room();
    let before = room.state.clone();
    for msg in [
        ClientMsg::Lock { on: true },
        ClientMsg::Admit { who: OTHER },
        ClientMsg::Kick { who: HOST },
        ClientMsg::Mute { who: HOST },
        ClientMsg::SetRecording { on: true },
        ClientMsg::End,
    ] {
        assert_eq!(room.client(GUEST, msg), error(GUEST, ErrorCode::NotHost));
    }
    assert_eq!(room.state, before);
}

#[test]
fn starting_a_recording_announces_it_first_and_records_consenting_speakers_only() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.join(GUEST, false, false);
    room.connect(GUEST);
    room.publish(HOST, &[TrackKind::Audio]);
    room.publish(GUEST, &[TrackKind::Audio]);

    let effects = room.client(HOST, ClientMsg::SetRecording { on: true });
    let [
        Effect::Broadcast {
            to,
            msg: ServerMsg::State { state },
        },
        start,
        record,
        ask,
        mute,
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(to, &[HOST, GUEST]);
    let recording = state.recording.expect("announced");
    assert_eq!((recording.since, recording.by), (room.now, HOST));
    assert_eq!(start, &Effect::StartRecording { since: room.now });
    assert_eq!(
        record,
        &Effect::RecordTrack {
            who: HOST,
            track: RemoteTrack {
                session: session(HOST, SessionKind::Publish),
                name: track_name(HOST, TrackKind::Audio)
            },
        }
    );
    // The guest who did not consent is asked and muted.
    assert_eq!(
        [ask, mute],
        [
            &Effect::Send {
                to: GUEST,
                msg: ServerMsg::ConsentRequired
            },
            &Effect::Send {
                to: GUEST,
                msg: ServerMsg::Muted
            },
        ]
    );
}

#[test]
fn consent_given_later_records_and_withdrawn_consent_stops_recording_and_mutes() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.join(GUEST, false, false);
    room.connect(GUEST);
    room.publish(GUEST, &[TrackKind::Audio]);
    room.client(HOST, ClientMsg::SetRecording { on: true });
    let track = RemoteTrack {
        session: session(GUEST, SessionKind::Publish),
        name: track_name(GUEST, TrackKind::Audio),
    };

    let effects = room.client(GUEST, ClientMsg::SetConsent { consent: true });
    assert!(matches!(effects.as_slice(), [Effect::Broadcast { .. }, _]));
    assert_eq!(
        effects[1],
        Effect::RecordTrack {
            who: GUEST,
            track: track.clone()
        }
    );

    let effects = room.client(GUEST, ClientMsg::SetConsent { consent: false });
    assert!(matches!(effects.as_slice(), [Effect::Broadcast { .. }, ..]));
    assert_eq!(
        effects[1..],
        [
            Effect::StopRecordingTrack { who: GUEST, track },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::ConsentRequired
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Muted
            },
        ]
    );
}

#[test]
fn a_from_start_meeting_records_once_the_first_host_joins() {
    let (mut room, _) = Room::with_policy(RecordingPolicy::FromStart);
    assert_eq!(
        room.state.view().recording_policy,
        RecordingPolicy::FromStart
    );
    room.enter(GUEST, false);
    assert_eq!(room.state.view().recording, None);

    room.join(HOST, true, true);
    let effects = room.connect(HOST);
    let [
        Effect::Send {
            to: HOST,
            msg: ServerMsg::Welcome { state, .. },
        },
        Effect::Broadcast { to, .. },
        Effect::StartRecording { since },
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(to, &[GUEST]);
    assert_eq!(*since, room.now);
    assert_eq!(state.recording.map(|r| r.by), Some(HOST));
}

#[test]
fn knocking_guests_neither_keep_the_room_alive_nor_move_its_end() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.client(HOST, ClientMsg::Lock { on: true });
    room.now = Millis(T0.0 + 10_000);
    let emptied = room.now;
    let effects = room.client(HOST, ClientMsg::Leave);
    assert_eq!(
        effects.last(),
        Some(&Effect::SetAlarm {
            at: emptied.after(EMPTY_ROOM_TIMEOUT_MS)
        })
    );

    room.now = emptied.after(30_000);
    room.join(GUEST, false, true);
    room.now = emptied.after(60_000);
    assert_eq!(
        room.client(GUEST, ClientMsg::Leave),
        [Effect::CloseSocket { who: GUEST }]
    );
    room.now = emptied.after(EMPTY_ROOM_TIMEOUT_MS);
    assert_eq!(
        room.apply(Event::AlarmFired).last(),
        Some(&Effect::EndMeeting)
    );
}

#[test]
fn kicked_participants_are_told_and_disconnected() {
    let mut room = meeting_room();
    let effects = room.client(HOST, ClientMsg::Kick { who: GUEST });
    assert!(matches!(
        effects.as_slice(),
        [
            Effect::Broadcast { .. },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Kicked
            },
            Effect::CloseSocket { who: GUEST },
        ]
    ));
    assert_eq!(presence(&room, GUEST), None);
    assert_eq!(
        room.client(HOST, ClientMsg::Kick { who: HOST }),
        error(HOST, ErrorCode::InvalidTarget)
    );
}

#[test]
fn ending_tells_everyone_and_releases_the_link() {
    let mut room = meeting_room();
    room.client(HOST, ClientMsg::SetRecording { on: true });
    let effects = room.client(HOST, ClientMsg::End);
    assert_eq!(
        effects,
        [
            Effect::Broadcast {
                to: vec![HOST, GUEST],
                msg: ServerMsg::Ended {
                    transcript: Some(transcript())
                }
            },
            Effect::CloseSocket { who: HOST },
            Effect::CloseSocket { who: GUEST },
            Effect::StopRecording,
            Effect::ReleaseMeetingId {
                meeting: room.state.meeting().clone()
            },
            Effect::EndMeeting,
        ]
    );
    assert_eq!(room.client(HOST, ClientMsg::Leave), []);
}

#[test]
fn an_unrecorded_meeting_ends_without_transcript() {
    let mut room = meeting_room();
    let effects = room.client(HOST, ClientMsg::End);
    assert_eq!(
        effects[0],
        Effect::Broadcast {
            to: vec![HOST, GUEST],
            msg: ServerMsg::Ended { transcript: None }
        }
    );
}

#[test]
fn publish_offers_are_validated_and_serialized() {
    let mut room = meeting_room();
    let offer = |kinds: &[TrackKind]| ClientMsg::PublishOffer {
        sdp: crate::Sdp::new("offer"),
        tracks: kinds
            .iter()
            .map(|&k| super::support::local_track(GUEST, k))
            .collect(),
    };
    assert_eq!(
        room.client(GUEST, offer(&[])),
        error(GUEST, ErrorCode::InvalidOffer)
    );
    assert_eq!(
        room.client(GUEST, offer(&[TrackKind::Audio, TrackKind::Audio])),
        error(GUEST, ErrorCode::InvalidOffer)
    );
    assert!(matches!(
        room.client(GUEST, offer(&[TrackKind::Audio])).as_slice(),
        [Effect::SfuPublish { .. }, Effect::SetAlarm { .. }]
    ));
    assert_eq!(
        room.client(GUEST, offer(&[TrackKind::Video])),
        error(GUEST, ErrorCode::NegotiationInProgress)
    );
}

#[test]
fn state_survives_a_json_round_trip() {
    let mut room = meeting_room();
    room.publish(HOST, &[TrackKind::Audio, TrackKind::Video]);
    let json = serde_json::to_string(&room.state).expect("serializable");
    assert_eq!(
        serde_json::from_str::<RoomState>(&json).expect("deserializable"),
        room.state
    );
}

#[test]
fn a_client_takes_its_place_again_on_a_new_socket() {
    const NEW_SOCKET: ParticipantId = ParticipantId(9);
    let mut room = meeting_room();
    room.publish(GUEST, &[TrackKind::Audio]);
    let before = room.state.clone();
    let token = before.participants[&GUEST].resume.clone();

    let effects = room.client(
        NEW_SOCKET,
        ClientMsg::Join {
            name: name("Someone else"),
            consent: false,
            host_token: None,
            resume: Some(token.clone()),
        },
    );
    assert_eq!(
        effects,
        [
            Effect::CloseSocket { who: GUEST },
            Effect::Rebind {
                socket: NEW_SOCKET,
                participant: GUEST
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Welcome {
                    you: GUEST,
                    state: before.view(),
                    resume: token
                }
            },
        ]
    );
    assert_eq!(room.state, before);
}

#[test]
fn the_resume_token_of_someone_who_left_starts_a_fresh_join() {
    const NEW_SOCKET: ParticipantId = ParticipantId(9);
    let mut room = meeting_room();
    let token = room.state.participants[&GUEST].resume.clone();
    room.client(GUEST, ClientMsg::Leave);
    let effects = room.client(
        NEW_SOCKET,
        ClientMsg::Join {
            name: name("Guest"),
            consent: true,
            host_token: None,
            resume: Some(token),
        },
    );
    assert!(!effects.iter().any(|e| matches!(e, Effect::Rebind { .. })));
    assert_eq!(presence(&room, NEW_SOCKET), Some(Presence::Connecting));
}

#[test]
fn a_repeated_join_is_answered_again_and_a_join_after_leaving_starts_fresh() {
    let mut room = meeting_room();
    let before = room.state.clone();
    let effects = room.join(GUEST, false, true);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Send {
                to: GUEST,
                msg: ServerMsg::Welcome { you: GUEST, .. }
            }]
        ),
        "{effects:#?}"
    );
    assert_eq!(room.state, before);

    room.client(GUEST, ClientMsg::Leave);
    room.join(GUEST, false, true);
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
}

/// The host publishes audio; returns the track the guest is asked to receive.
fn host_publishes_audio(room: &mut Room) -> RemoteTrack {
    room.publish(HOST, &[TrackKind::Audio]);
    RemoteTrack {
        session: session(HOST, SessionKind::Publish),
        name: track_name(HOST, TrackKind::Audio),
    }
}

#[test]
fn a_failed_subscription_is_retried_after_a_pause() {
    let mut room = meeting_room();
    let track = host_publishes_audio(&mut room);
    let effects = room.sfu(SfuReply::Failed {
        who: GUEST,
        call: SfuCall::Subscribe,
    });
    assert_eq!(
        effects,
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Error {
                    code: ErrorCode::SubscribeFailed
                }
            },
            Effect::SetAlarm {
                at: room.now.after(SUBSCRIBE_RETRY_MS)
            },
        ]
    );

    room.now = room.now.after(SUBSCRIBE_RETRY_MS);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::SfuSubscribe {
                who: GUEST,
                session: session(GUEST, SessionKind::Subscribe),
                tracks: vec![track]
            },
            Effect::SetAlarm {
                at: room.now.after(NEGOTIATION_TIMEOUT_MS)
            },
        ]
    );
}

#[test]
fn an_unanswered_subscribe_offer_times_out_and_is_requested_again() {
    let mut room = meeting_room();
    let track = host_publishes_audio(&mut room);
    let mid = Mid::new("sub-audio");
    room.sfu(SfuReply::SubscribeOffer {
        who: GUEST,
        sdp_offer: Sdp::new("offer"),
        tracks: vec![OfferedTrack {
            mid: mid.clone(),
            track: track.clone(),
        }],
    });

    room.now = room.now.after(NEGOTIATION_TIMEOUT_MS);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::SfuCloseTracks {
                who: GUEST,
                session: session(GUEST, SessionKind::Subscribe),
                mids: vec![mid]
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Error {
                    code: ErrorCode::SubscribeFailed
                }
            },
            Effect::SetAlarm {
                at: room.now.after(SUBSCRIBE_RETRY_MS)
            },
        ]
    );
    // A late answer to the dropped offer is rejected.
    assert_eq!(
        room.client(
            GUEST,
            ClientMsg::SubscribeAnswer {
                sdp: Sdp::new("late")
            }
        ),
        error(GUEST, ErrorCode::UnexpectedAnswer)
    );

    room.now = room.now.after(SUBSCRIBE_RETRY_MS);
    assert!(matches!(
        room.apply(Event::AlarmFired).as_slice(),
        [Effect::SfuSubscribe { tracks, .. }, Effect::SetAlarm { .. }] if tracks == &[track]
    ));
}

/// The host publishes audio and the guest receives it; returns the track and the guest's mid.
fn guest_receives_host_audio(room: &mut Room) -> (RemoteTrack, Mid) {
    let track = host_publishes_audio(room);
    let mid = Mid::new("sub-audio");
    room.sfu(SfuReply::SubscribeOffer {
        who: GUEST,
        sdp_offer: Sdp::new("offer"),
        tracks: vec![OfferedTrack {
            mid: mid.clone(),
            track: track.clone(),
        }],
    });
    room.client(
        GUEST,
        ClientMsg::SubscribeAnswer {
            sdp: Sdp::new("answer"),
        },
    );
    room.sfu(SfuReply::Renegotiated { who: GUEST });
    (track, mid)
}

#[test]
fn unpublishing_closes_the_track_everywhere_and_stops_recording_it() {
    let mut room = meeting_room();
    let (track, mid) = guest_receives_host_audio(&mut room);
    let effects = room.client(HOST, ClientMsg::SetRecording { on: true });
    assert!(effects.contains(&Effect::RecordTrack {
        who: HOST,
        track: track.clone()
    }));

    let effects = room.client(
        HOST,
        ClientMsg::Unpublish {
            kind: TrackKind::Audio,
        },
    );
    let [
        Effect::Broadcast {
            msg: ServerMsg::State { state },
            ..
        },
        rest @ ..,
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert!(state.participants.iter().all(|p| p.tracks.is_empty()));
    assert_eq!(
        rest,
        [
            Effect::StopRecordingTrack { who: HOST, track },
            Effect::SfuCloseTracks {
                who: GUEST,
                session: session(GUEST, SessionKind::Subscribe),
                mids: vec![mid]
            },
            Effect::SfuCloseTracks {
                who: HOST,
                session: session(HOST, SessionKind::Publish),
                mids: vec![Mid::new("Audio")]
            },
        ]
    );
    assert_eq!(
        room.client(
            HOST,
            ClientMsg::Unpublish {
                kind: TrackKind::Audio
            }
        ),
        error(HOST, ErrorCode::NotPublished)
    );
    // The kind may be published again, under the same name.
    assert!(matches!(
        room.publish(HOST, &[TrackKind::Audio]).as_slice(),
        [Effect::Broadcast { .. }, ..]
    ));
}

#[test]
fn a_recorded_speaker_who_leaves_or_is_kicked_is_no_longer_recorded() {
    for (who, msg) in [
        (GUEST, ClientMsg::Leave),
        (HOST, ClientMsg::Kick { who: GUEST }),
    ] {
        let mut room = meeting_room();
        room.client(HOST, ClientMsg::SetRecording { on: true });
        room.publish(GUEST, &[TrackKind::Audio]);
        let track = RemoteTrack {
            session: session(GUEST, SessionKind::Publish),
            name: track_name(GUEST, TrackKind::Audio),
        };
        assert!(
            room.client(who, msg)
                .contains(&Effect::StopRecordingTrack { who: GUEST, track })
        );
    }
}

#[test]
fn an_unanswered_publish_offer_times_out_and_a_new_offer_goes_through() {
    let mut room = meeting_room();
    let offer = || ClientMsg::PublishOffer {
        sdp: Sdp::new("offer"),
        tracks: vec![super::support::local_track(GUEST, TrackKind::Audio)],
    };
    let deadline = room.now.after(NEGOTIATION_TIMEOUT_MS);
    assert!(matches!(
        room.client(GUEST, offer()).as_slice(),
        [Effect::SfuPublish { .. }, Effect::SetAlarm { at }] if *at == deadline
    ));
    room.now = Millis(deadline.0 - 1);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [Effect::SetAlarm { at: deadline }]
    );

    room.now = deadline;
    assert_eq!(
        room.apply(Event::AlarmFired),
        error(GUEST, ErrorCode::PublishFailed)
    );
    // A late answer to the dropped offer changes nothing.
    assert_eq!(
        room.sfu(SfuReply::Published {
            who: GUEST,
            sdp_answer: Sdp::new("late")
        }),
        []
    );
    assert!(matches!(
        room.client(GUEST, offer()).as_slice(),
        [Effect::SfuPublish { .. }, Effect::SetAlarm { .. }]
    ));
}
