//! One test per room rule.

use super::support::{Room, T0, name, session, track_name, transcript};
use crate::recording::{ConsentRecord, RosterEntry};
use crate::{
    ClientMsg, EMPTY_ROOM_TIMEOUT_MS, Effect, ErrorCode, Event, FailureCause, Mid, Millis,
    NEGOTIATION_TIMEOUT_MS, NOTICE_VERSION, OfferedTrack, ParticipantId, Phase, Presence,
    RecordingPolicy, RemoteTrack, Role, RoomState, SCHEDULED_TTL_MS, SUBSCRIBE_RETRY_MS, Sdp,
    ServerMsg, SessionEpoch, SessionEpochs, SessionId, SessionKind, SfuCall, SfuReply, TrackKind,
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

fn muted(room: &Room, who: ParticipantId) -> Option<bool> {
    room.state
        .view()
        .participants
        .iter()
        .find(|p| p.id == who)
        .map(|p| p.muted)
}

fn role(room: &Room, who: ParticipantId) -> Option<Role> {
    room.state
        .view()
        .participants
        .iter()
        .find(|p| p.id == who)
        .map(|p| p.role)
}

#[test]
fn a_join_from_the_owners_socket_makes_a_host_and_any_other_a_guest() {
    let (mut room, _) = Room::new();
    room.enter(HOST, true);
    room.enter(GUEST, false);
    assert_eq!(role(&room, HOST), Some(Role::Host));
    assert_eq!(role(&room, GUEST), Some(Role::Guest));
}

#[test]
fn a_scheduled_room_has_no_empty_timer_and_ends_unused_after_sixty_days() {
    let (mut room, effects) = Room::new();
    let expiry = T0.after(SCHEDULED_TTL_MS);
    assert_eq!(effects, [Effect::SetAlarm { at: expiry }]);

    // Guests come and go; that neither ends the room nor moves its expiry.
    assert_eq!(
        room.join(GUEST, false, true),
        [Effect::Send {
            to: GUEST,
            msg: ServerMsg::Waiting
        }]
    );
    room.join(OTHER, false, true);
    assert_eq!(
        room.client(OTHER, ClientMsg::Leave),
        [Effect::CloseSocket { who: OTHER }]
    );
    room.now = T0.after(EMPTY_ROOM_TIMEOUT_MS);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [Effect::SetAlarm { at: expiry }]
    );
    assert_eq!(room.state.view().phase, Phase::Scheduled);

    room.now = expiry;
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::Broadcast {
                to: vec![GUEST],
                msg: ServerMsg::Ended { transcript: None }
            },
            Effect::CloseSocket { who: GUEST },
            Effect::ReleaseMeetingId {
                meeting: room.state.meeting().clone()
            },
            Effect::EndMeeting,
        ]
    );
    assert_eq!(room.state.view().phase, Phase::Ended);
    assert_eq!(room.state.ended(), Some(expiry));
}

#[test]
fn guests_wait_for_the_first_host_and_enter_with_it_if_they_consent_to_its_recording() {
    let (mut room, _) = Room::with_policy(RecordingPolicy::FromStart);
    room.join(GUEST, false, true);
    room.join(OTHER, false, false);
    assert_eq!(presence(&room, GUEST), Some(Presence::WaitingForHost));
    assert_eq!(presence(&room, OTHER), Some(Presence::WaitingForHost));
    // Nobody can lock a scheduled room, as only a joined host may; the first host's arrival is
    // never held back by a lock.
    assert_eq!(
        room.client(GUEST, ClientMsg::Lock { on: true }),
        error(GUEST, ErrorCode::NotJoined)
    );

    // A host who is still connecting does not let anyone in yet.
    room.join(HOST, true, true);
    assert_eq!(presence(&room, GUEST), Some(Presence::WaitingForHost));
    assert_eq!(room.state.view().phase, Phase::Scheduled);

    let effects = room.connect(HOST);
    let [
        Effect::Send {
            to: HOST,
            msg: ServerMsg::Welcome { state, .. },
        },
        rest @ ..,
    ] = effects.as_slice()
    else {
        panic!("unexpected effects {effects:#?}");
    };
    assert_eq!(state.phase, Phase::Live);
    assert_eq!(state.recording.map(|r| r.by), Some(HOST));
    assert_eq!(
        rest,
        [
            Effect::StartRecording { since: room.now },
            Effect::Send {
                to: OTHER,
                msg: ServerMsg::ConsentRequired
            },
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
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
    assert_eq!(presence(&room, OTHER), Some(Presence::AwaitingConsent));
}

#[test]
fn without_recording_every_waiting_guest_enters_with_the_first_host() {
    let (mut room, _) = Room::new();
    room.join(GUEST, false, false);
    room.enter(HOST, true);
    assert_eq!(presence(&room, GUEST), Some(Presence::Connecting));
    assert_eq!(room.state.view().recording, None);
}

#[test]
fn the_owners_socket_turns_the_guest_whose_place_it_takes_into_a_host() {
    const NEW_SOCKET: ParticipantId = ParticipantId(9);
    let mut room = meeting_room();
    let token = room.state.participants[&GUEST].resume.clone();
    let effects = room.client_from(
        NEW_SOCKET,
        ClientMsg::Join {
            name: name("Owner"),
            consent: true,
            resume: Some(token),
        },
        true,
    );
    let view = room.state.view();
    assert_eq!(role(&room, GUEST), Some(Role::Host));
    assert_eq!(
        effects,
        [
            Effect::Broadcast {
                to: vec![HOST],
                msg: ServerMsg::State {
                    state: view.clone()
                }
            },
            Effect::CloseSocket { who: GUEST },
            Effect::Rebind {
                socket: NEW_SOCKET,
                participant: GUEST
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Welcome {
                    you: GUEST,
                    state: view,
                    resume: room.state.participants[&GUEST].resume.clone(),
                    epochs: SessionEpochs::default(),
                }
            },
        ]
    );

    // A knocking guest joining again from the owner's socket is a host, and hosts never knock.
    room.client(HOST, ClientMsg::Lock { on: true });
    room.join(OTHER, false, true);
    assert_eq!(presence(&room, OTHER), Some(Presence::Knocking));
    room.join(OTHER, true, true);
    assert_eq!(role(&room, OTHER), Some(Role::Host));
    assert_eq!(presence(&room, OTHER), Some(Presence::Connecting));
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
    assert_eq!(muted(&room, GUEST), Some(false));
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
    assert_eq!(state, &room.state.view(), "shows the guest muted");
    assert_eq!(muted(&room, GUEST), Some(true));
    assert_eq!(muted(&room, HOST), Some(false));
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
    assert_eq!(muted(&room, GUEST), Some(true));
}

#[test]
fn a_host_without_consent_enters_a_recording_room_muted() {
    let (mut room, _) = Room::with_policy(RecordingPolicy::FromStart);
    room.join(HOST, true, false);
    room.connect(HOST);
    assert_eq!(presence(&room, HOST), Some(Presence::Joined));
    assert_eq!(muted(&room, HOST), Some(true));
}

#[test]
fn everyone_sees_a_participants_microphone_switch() {
    let mut room = meeting_room();
    assert_eq!(muted(&room, GUEST), Some(false));

    let effects = room.client(GUEST, ClientMsg::SetMuted { muted: true });
    assert_eq!(
        effects,
        [Effect::Broadcast {
            to: vec![HOST, GUEST],
            msg: ServerMsg::State {
                state: room.state.view()
            }
        }]
    );
    assert_eq!(muted(&room, GUEST), Some(true));
    // The same switch again changes nothing.
    assert_eq!(room.client(GUEST, ClientMsg::SetMuted { muted: true }), []);

    room.client(GUEST, ClientMsg::SetMuted { muted: false });
    assert_eq!(muted(&room, GUEST), Some(false));
}

#[test]
fn a_host_mute_shows_the_participant_muted_at_once_and_tells_them() {
    let mut room = meeting_room();
    let effects = room.client(HOST, ClientMsg::Mute { who: GUEST });
    assert_eq!(
        effects,
        [
            Effect::Broadcast {
                to: vec![HOST, GUEST],
                msg: ServerMsg::State {
                    state: room.state.view()
                }
            },
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Muted
            },
        ]
    );
    assert_eq!(muted(&room, GUEST), Some(true));
    assert_eq!(
        room.client(HOST, ClientMsg::Mute { who: OTHER }),
        error(HOST, ErrorCode::InvalidTarget)
    );
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
    // The guest withdraws consent, gives it again, and withdraws it for good: the record keeps
    // the first consent and the last withdrawal.
    let withdrawn = room.now.after(9_000);
    room.client(GUEST, ClientMsg::SetConsent { consent: false });
    room.now = room.now.after(5_000);
    room.client(GUEST, ClientMsg::SetConsent { consent: true });
    room.now = withdrawn;
    room.client(GUEST, ClientMsg::SetConsent { consent: false });
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
            Effect::FinishRecording {
                roster: vec![
                    RosterEntry {
                        id: HOST,
                        name: name("P1"),
                        role: Role::Host
                    },
                    RosterEntry {
                        id: GUEST,
                        name: name("P2"),
                        role: Role::Guest
                    },
                ],
                consents: vec![
                    ConsentRecord {
                        who: HOST,
                        name: name("P1"),
                        at: T0,
                        notice_version: NOTICE_VERSION,
                        withdrawn_at: None
                    },
                    ConsentRecord {
                        who: GUEST,
                        name: name("P2"),
                        at: T0,
                        notice_version: NOTICE_VERSION,
                        withdrawn_at: Some(withdrawn)
                    },
                ],
            },
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

    // A participant stored before the mute flag existed loads unmuted.
    room.client(GUEST, ClientMsg::SetMuted { muted: true });
    let mut stored = serde_json::to_value(&room.state).expect("serializable");
    for p in stored["participants"]
        .as_object_mut()
        .expect("participants")
        .values_mut()
    {
        p.as_object_mut().expect("participant").remove("muted");
    }
    let loaded: RoomState = serde_json::from_value(stored).expect("deserializable");
    assert!(!loaded.participants[&GUEST].muted);
}

#[test]
fn a_client_takes_its_place_again_on_a_new_socket() {
    const NEW_SOCKET: ParticipantId = ParticipantId(9);
    let mut room = meeting_room();
    room.publish(GUEST, &[TrackKind::Audio]);
    room.client(GUEST, ClientMsg::SetMuted { muted: true });
    let before = room.state.clone();
    let token = before.participants[&GUEST].resume.clone();

    let effects = room.client(
        NEW_SOCKET,
        ClientMsg::Join {
            name: name("Someone else"),
            consent: false,
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
                    resume: token,
                    epochs: SessionEpochs::default(),
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
        call: SfuCall::Subscribe {
            session: session(GUEST, SessionKind::Subscribe),
        },
        cause: FailureCause::Transient,
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
        session: session(GUEST, SessionKind::Subscribe),
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
        session: session(GUEST, SessionKind::Subscribe),
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
    room.sfu(SfuReply::Renegotiated {
        who: GUEST,
        session: session(GUEST, SessionKind::Subscribe),
    });
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
            session: session(GUEST, SessionKind::Publish),
            sdp_answer: Sdp::new("late")
        }),
        []
    );
    assert!(matches!(
        room.client(GUEST, offer()).as_slice(),
        [Effect::SfuPublish { .. }, Effect::SetAlarm { .. }]
    ));
}

fn session_gone(who: ParticipantId, call: SfuCall) -> SfuReply {
    SfuReply::Failed {
        who,
        call,
        cause: FailureCause::SessionGone,
    }
}

#[test]
fn a_subscribe_session_the_sfu_lost_is_replaced_and_receives_every_track_again() {
    let mut room = meeting_room();
    let (audio, _) = guest_receives_host_audio(&mut room);
    room.publish(HOST, &[TrackKind::Video]);
    let video = RemoteTrack {
        session: session(HOST, SessionKind::Publish),
        name: track_name(HOST, TrackKind::Video),
    };
    let old = session(GUEST, SessionKind::Subscribe);

    // The pull of the host's video meets a session the SFU declared gone.
    assert_eq!(
        room.sfu(session_gone(
            GUEST,
            SfuCall::Subscribe {
                session: old.clone()
            }
        )),
        [Effect::SfuNewSession {
            who: GUEST,
            kind: SessionKind::Subscribe
        }]
    );
    assert_eq!(presence(&room, GUEST), Some(Presence::Joined));
    // A late offer for the lost session changes nothing.
    assert_eq!(
        room.sfu(SfuReply::SubscribeOffer {
            who: GUEST,
            session: old.clone(),
            sdp_offer: Sdp::new("late"),
            tracks: Vec::new(),
        }),
        []
    );

    let new = SessionId::new("subscribe-again");
    assert_eq!(
        room.sfu(SfuReply::SessionCreated {
            who: GUEST,
            kind: SessionKind::Subscribe,
            session: new.clone(),
        }),
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::SubscribeReset {
                    epoch: SessionEpoch(1)
                }
            },
            // The alarm stays: the subscribe offer times out when the request would have.
            Effect::SfuSubscribe {
                who: GUEST,
                session: new.clone(),
                tracks: vec![audio, video]
            },
        ]
    );
    // Replies about the lost session can no longer touch the new one.
    for late in [
        SfuReply::SubscribeOffer {
            who: GUEST,
            session: old.clone(),
            sdp_offer: Sdp::new("late"),
            tracks: Vec::new(),
        },
        session_gone(
            GUEST,
            SfuCall::Renegotiate {
                session: old.clone(),
            },
        ),
    ] {
        assert_eq!(room.sfu(late), []);
    }
    assert!(matches!(
        room.sfu(SfuReply::SubscribeOffer {
            who: GUEST,
            session: new,
            sdp_offer: Sdp::new("offer"),
            tracks: Vec::new(),
        })
        .as_slice(),
        [
            Effect::Send {
                msg: ServerMsg::SubscribeOffer { .. },
                ..
            },
            ..
        ]
    ));
}

#[test]
fn a_publish_session_the_sfu_lost_takes_its_tracks_with_it_and_is_replaced() {
    let mut room = meeting_room();
    let (audio, mid) = guest_receives_host_audio(&mut room);
    room.client(HOST, ClientMsg::SetRecording { on: true });
    let old = session(HOST, SessionKind::Publish);
    room.client(
        HOST,
        ClientMsg::PublishOffer {
            sdp: Sdp::new("offer"),
            tracks: vec![super::support::local_track(HOST, TrackKind::Video)],
        },
    );

    let effects = room.sfu(session_gone(
        HOST,
        SfuCall::Publish {
            session: old.clone(),
        },
    ));
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
    // Nothing is closed on the lost session itself, and the client hears nothing yet.
    assert_eq!(
        rest,
        [
            Effect::StopRecordingTrack {
                who: HOST,
                track: audio
            },
            Effect::SfuCloseTracks {
                who: GUEST,
                session: session(GUEST, SessionKind::Subscribe),
                mids: vec![mid]
            },
            Effect::SfuNewSession {
                who: HOST,
                kind: SessionKind::Publish
            },
        ]
    );
    // An offer before the new session exists is dropped; the reset makes the client offer again.
    assert_eq!(
        room.client(
            HOST,
            ClientMsg::PublishOffer {
                sdp: Sdp::new("offer"),
                tracks: vec![super::support::local_track(HOST, TrackKind::Audio)],
            }
        ),
        []
    );

    let new = SessionId::new("publish-again");
    assert_eq!(
        room.sfu(SfuReply::SessionCreated {
            who: HOST,
            kind: SessionKind::Publish,
            session: new.clone(),
        }),
        [Effect::Send {
            to: HOST,
            msg: ServerMsg::PublishReset {
                epoch: SessionEpoch(1)
            }
        }]
    );
    let tracks = vec![super::support::local_track(HOST, TrackKind::Audio)];
    assert!(matches!(
        room.client(
            HOST,
            ClientMsg::PublishOffer {
                sdp: Sdp::new("offer"),
                tracks: tracks.clone(),
            }
        )
        .as_slice(),
        [Effect::SfuPublish { session, .. }, Effect::SetAlarm { .. }] if *session == new
    ));
    // The late answer for the lost session is not taken for the new one.
    assert_eq!(
        room.sfu(SfuReply::Published {
            who: HOST,
            session: old,
            sdp_answer: Sdp::new("late"),
        }),
        []
    );
    let effects = room.sfu(SfuReply::Published {
        who: HOST,
        session: new.clone(),
        sdp_answer: Sdp::new("answer"),
    });
    let track = RemoteTrack {
        session: new,
        name: track_name(HOST, TrackKind::Audio),
    };
    assert!(effects.contains(&Effect::RecordTrack { who: HOST, track }));
}

/// A client's report that its peer connection for the current session of `kind` failed.
fn reset(kind: SessionKind, epoch: u32) -> ClientMsg {
    ClientMsg::ResetSession {
        kind,
        epoch: SessionEpoch(epoch),
    }
}

#[test]
fn a_client_whose_peer_connection_failed_gets_a_new_session_once() {
    let mut room = meeting_room();
    // The request times out, so the first one sets the alarm.
    let deadline = Effect::SetAlarm {
        at: room.now.after(NEGOTIATION_TIMEOUT_MS),
    };
    for (kind, alarm) in [
        (SessionKind::Subscribe, vec![deadline]),
        (SessionKind::Publish, vec![]),
    ] {
        let mut expected = vec![Effect::SfuNewSession { who: GUEST, kind }];
        expected.extend(alarm);
        assert_eq!(room.client(GUEST, reset(kind, 0)), expected);
        // A reset while the replacement runs changes nothing.
        assert_eq!(room.client(GUEST, reset(kind, 0)), []);
    }

    // Someone who is not in yet has no session to reset.
    room.client(HOST, ClientMsg::Lock { on: true });
    room.join(OTHER, false, true);
    assert_eq!(room.client(OTHER, reset(SessionKind::Subscribe, 0)), []);
}

#[test]
fn a_late_report_about_a_replaced_sessions_connection_changes_nothing() {
    let mut room = meeting_room();
    // The SFU lost the guest's subscribe session, and the room replaced it.
    room.sfu(session_gone(
        GUEST,
        SfuCall::Subscribe {
            session: session(GUEST, SessionKind::Subscribe),
        },
    ));
    let effects = room.sfu(SfuReply::SessionCreated {
        who: GUEST,
        kind: SessionKind::Subscribe,
        session: SessionId::new("subscribe-again"),
    });
    assert!(effects.contains(&Effect::Send {
        to: GUEST,
        msg: ServerMsg::SubscribeReset {
            epoch: SessionEpoch(1)
        }
    }));
    // The old peer connection fails only now; its report names the old epoch.
    let before = room.state.clone();
    assert_eq!(room.client(GUEST, reset(SessionKind::Subscribe, 0)), []);
    assert_eq!(room.state, before);
    // A report about the new session's connection replaces it.
    assert!(matches!(
        room.client(GUEST, reset(SessionKind::Subscribe, 1))
            .as_slice(),
        [Effect::SfuNewSession { .. }, ..]
    ));
    // A client that reconnects learns the current epochs from its welcome.
    let effects = room.join(GUEST, false, true);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Send {
            msg: ServerMsg::Welcome { epochs, .. },
            ..
        }] if *epochs == SessionEpochs { publish: SessionEpoch(0), subscribe: SessionEpoch(1) }
    ));
}

#[test]
fn a_replacement_the_sfu_cannot_create_is_requested_again_through_the_alarm() {
    let mut room = meeting_room();
    room.client(GUEST, reset(SessionKind::Subscribe, 0));
    let failed = || SfuReply::Failed {
        who: GUEST,
        call: SfuCall::NewSession(SessionKind::Subscribe),
        cause: FailureCause::Transient,
    };
    let retry = room.now.after(SUBSCRIBE_RETRY_MS);
    assert_eq!(
        room.sfu(failed()),
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Error {
                    code: ErrorCode::SfuUnavailable
                }
            },
            Effect::SetAlarm { at: retry },
        ]
    );
    // The guest stays in the meeting.
    assert_eq!(presence(&room, GUEST), Some(Presence::Joined));

    room.now = retry;
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::SfuNewSession {
                who: GUEST,
                kind: SessionKind::Subscribe
            },
            Effect::SetAlarm {
                at: retry.after(NEGOTIATION_TIMEOUT_MS)
            },
        ]
    );
    // The guest was told once; a further failure only sets the alarm again.
    assert_eq!(
        room.sfu(failed()),
        [Effect::SetAlarm {
            at: room.now.after(SUBSCRIBE_RETRY_MS)
        }]
    );
}

#[test]
fn a_request_for_a_replacement_that_gets_no_reply_times_out_and_is_sent_again() {
    let mut room = meeting_room();
    let requested = room.now;
    assert_eq!(
        room.client(GUEST, reset(SessionKind::Publish, 0)),
        [
            Effect::SfuNewSession {
                who: GUEST,
                kind: SessionKind::Publish
            },
            Effect::SetAlarm {
                at: requested.after(NEGOTIATION_TIMEOUT_MS)
            },
        ]
    );
    // The SFU's reply is lost. Once the request timed out, it counts as failed.
    room.now = requested.after(NEGOTIATION_TIMEOUT_MS);
    let retry = room.now.after(SUBSCRIBE_RETRY_MS);
    assert_eq!(
        room.apply(Event::AlarmFired),
        [
            Effect::Send {
                to: GUEST,
                msg: ServerMsg::Error {
                    code: ErrorCode::SfuUnavailable
                }
            },
            Effect::SetAlarm { at: retry },
        ]
    );
    room.now = retry;
    assert!(matches!(
        room.apply(Event::AlarmFired).as_slice(),
        [
            Effect::SfuNewSession {
                who: GUEST,
                kind: SessionKind::Publish
            },
            Effect::SetAlarm { .. }
        ]
    ));
    // A late reply to the first request still brings the new session.
    assert_eq!(
        room.sfu(SfuReply::SessionCreated {
            who: GUEST,
            kind: SessionKind::Publish,
            session: SessionId::new("publish-again"),
        }),
        [Effect::Send {
            to: GUEST,
            msg: ServerMsg::PublishReset {
                epoch: SessionEpoch(1)
            }
        }]
    );
}

#[test]
fn messages_about_a_session_being_replaced_are_dropped_without_an_error() {
    let mut room = meeting_room();
    guest_receives_host_audio(&mut room);
    room.publish(GUEST, &[TrackKind::Audio]);
    room.client(GUEST, reset(SessionKind::Subscribe, 0));
    room.client(GUEST, reset(SessionKind::Publish, 0));
    let host_audio = crate::TrackRef {
        owner: HOST,
        kind: TrackKind::Audio,
    };
    for msg in [
        ClientMsg::SetLayer {
            track: host_audio,
            rid: crate::Rid::Low,
        },
        ClientMsg::SubscribeAnswer {
            sdp: Sdp::new("late"),
        },
        ClientMsg::Unpublish {
            kind: TrackKind::Audio,
        },
        ClientMsg::PublishOffer {
            sdp: Sdp::new("offer"),
            tracks: vec![super::support::local_track(GUEST, TrackKind::Video)],
        },
    ] {
        let before = room.state.clone();
        assert_eq!(room.client(GUEST, msg.clone()), [], "{msg:?}");
        assert_eq!(room.state, before, "{msg:?}");
    }
}

#[test]
fn the_roster_keeps_everyone_who_left_and_the_consents_only_standing_ones() {
    let mut room = meeting_room();
    let third = ParticipantId(3);
    room.join(third, false, false);
    room.client(GUEST, ClientMsg::Leave);
    let ids: Vec<_> = room.state.roster().iter().map(|e| e.id).collect();
    assert_eq!(ids, [HOST, GUEST, third], "the guest who left stays named");
    let consented: Vec<_> = room.state.consents().iter().map(|c| c.who).collect();
    assert_eq!(
        consented,
        [HOST, GUEST],
        "joined without consent: no record"
    );
}
