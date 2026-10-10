//! The JSON on the wire and the generated TypeScript declarations agree.

use crate::{ClientMsg, ServerMsg};

#[cfg(feature = "wasm")]
#[test]
fn declarations_use_camel_case_tags_fields_and_optional_options() {
    use tsify::Tsify;

    use crate::RoomView;

    assert!(ClientMsg::DECL.contains(
        r#"{ type: "join"; name: DisplayName; consent: boolean; resume?: ResumeToken }"#
    ));
    assert!(ClientMsg::DECL.contains(r#"{ type: "setConsent"; consent: boolean }"#));
    assert!(ClientMsg::DECL.contains(r#"{ type: "setMuted"; muted: boolean }"#));
    assert!(ServerMsg::DECL.contains(
        r#"{ type: "welcome"; you: ParticipantId; state: RoomView; resume: ResumeToken; epochs: SessionEpochs }"#
    ));
    assert!(ServerMsg::DECL.contains(r#"{ type: "consentRequired" }"#));
    assert!(ServerMsg::DECL.contains(r#"{ type: "ended"; transcript?: TranscriptRef }"#));
    assert!(RoomView::DECL.contains("recording?: Recording;"));
    assert!(RoomView::DECL.contains("recordingPolicy: RecordingPolicy;"));
    assert!(RoomView::DECL.contains("phase: Phase;"));
    assert!(crate::Phase::DECL.contains(r#""scheduled" | "live" | "ended""#));
    assert!(crate::Presence::DECL.contains(r#""waitingForHost""#));
    assert!(ClientMsg::DECL.contains(r#"{ type: "unpublish"; kind: TrackKind }"#));
    assert!(crate::Rid::DECL.contains(r#""q" | "h" | "f""#));
    assert!(
        ClientMsg::DECL
            .contains(r#"{ type: "resetSession"; kind: SessionKind; epoch: SessionEpoch }"#)
    );
    assert!(crate::SessionKind::DECL.contains(r#""publish" | "subscribe""#));
    assert!(ServerMsg::DECL.contains(r#"{ type: "subscribeReset"; epoch: SessionEpoch }"#));
}

/// Every [`ServerMsg`] variant survives JSON, as `parseServerMsg` reads it.
#[cfg(feature = "wasm")]
#[test]
fn every_server_msg_round_trips_through_json() {
    use crate::{
        ErrorCode, IncomingTrack, MeetingId, Mid, Millis, ParticipantId, Phase, Presence,
        Recording, RecordingPolicy, ResumeToken, Role, RoomView, Sdp, TrackKind, TrackRef,
        TranscriptRef,
    };

    let you = ParticipantId(7);
    let state = RoomView {
        meeting: MeetingId::parse("amber-otter-ocean").expect("listed words"),
        phase: Phase::Live,
        locked: true,
        recording_policy: RecordingPolicy::FromStart,
        recording: Some(Recording {
            since: Millis(1_700_000_000_000),
            by: you,
        }),
        participants: vec![crate::ParticipantView {
            id: you,
            name: super::support::name("Ada"),
            role: Role::Host,
            consent_recording: true,
            presence: Presence::Joined,
            tracks: vec![TrackKind::Audio, TrackKind::Video],
            muted: true,
        }],
    };
    let all = [
        ServerMsg::Welcome {
            you,
            state: state.clone(),
            resume: ResumeToken::from_random([7; 16]),
            epochs: crate::SessionEpochs::default(),
        },
        ServerMsg::State { state },
        ServerMsg::Waiting,
        ServerMsg::PublishAnswer {
            sdp: Sdp::new("answer"),
        },
        ServerMsg::SubscribeOffer {
            sdp: Sdp::new("offer"),
            tracks: vec![IncomingTrack {
                mid: Mid::new("0"),
                track: TrackRef {
                    owner: you,
                    kind: TrackKind::Screen,
                },
            }],
        },
        ServerMsg::PublishReset {
            epoch: crate::SessionEpoch(1),
        },
        ServerMsg::SubscribeReset {
            epoch: crate::SessionEpoch(2),
        },
        ServerMsg::Knock { who: you },
        ServerMsg::ConsentRequired,
        ServerMsg::Muted,
        ServerMsg::Kicked,
        ServerMsg::Ended {
            transcript: Some(TranscriptRef::new("t-1")),
        },
        ServerMsg::Error {
            code: ErrorCode::NotPublished,
        },
    ];
    // Adding a variant breaks this match; add it to `all` as well.
    let covered: std::collections::BTreeSet<_> = all
        .iter()
        .map(|msg| match msg {
            ServerMsg::Welcome { .. } => 0,
            ServerMsg::State { .. } => 1,
            ServerMsg::Waiting => 2,
            ServerMsg::PublishAnswer { .. } => 3,
            ServerMsg::SubscribeOffer { .. } => 4,
            ServerMsg::Knock { .. } => 5,
            ServerMsg::ConsentRequired => 6,
            ServerMsg::Muted => 7,
            ServerMsg::Kicked => 8,
            ServerMsg::Ended { .. } => 9,
            ServerMsg::Error { .. } => 10,
            ServerMsg::PublishReset { .. } => 11,
            ServerMsg::SubscribeReset { .. } => 12,
        })
        .collect();
    assert_eq!(covered.len(), all.len());
    for msg in all {
        let json = serde_json::to_string(&msg).expect("serializable");
        assert_eq!(
            serde_json::from_str::<ServerMsg>(&json).expect("parses"),
            msg,
            "{json}"
        );
    }
}

#[test]
fn json_uses_camel_case_tags_and_omits_absent_options() {
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"join","name":" Ada ","consent":true}"#).expect("valid");
    assert!(
        matches!(msg, ClientMsg::Join { resume: None, ref name, .. } if name.as_str() == "Ada")
    );
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"setRecording","on":true}"#).expect("valid");
    assert_eq!(msg, ClientMsg::SetRecording { on: true });
    assert!(
        serde_json::from_str::<ClientMsg>(r#"{"type":"join","name":"","consent":true}"#).is_err()
    );
    let ended =
        serde_json::to_string(&ServerMsg::Ended { transcript: None }).expect("serializable");
    assert_eq!(ended, r#"{"type":"ended"}"#);
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"setConsent","consent":false}"#).expect("valid");
    assert_eq!(msg, ClientMsg::SetConsent { consent: false });
    let policy = serde_json::to_string(&crate::RecordingPolicy::FromStart).expect("serializable");
    assert_eq!(policy, r#""fromStart""#);
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"setMuted","muted":true}"#).expect("valid");
    assert_eq!(msg, ClientMsg::SetMuted { muted: true });
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"unpublish","kind":"video"}"#).expect("valid");
    assert_eq!(
        msg,
        ClientMsg::Unpublish {
            kind: crate::TrackKind::Video
        }
    );
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"resetSession","kind":"subscribe","epoch":2}"#)
            .expect("valid");
    assert_eq!(
        msg,
        ClientMsg::ResetSession {
            kind: crate::SessionKind::Subscribe,
            epoch: crate::SessionEpoch(2),
        }
    );
}

#[test]
fn rids_sort_from_the_best_layer_to_the_worst() {
    use crate::Rid;

    let names = [Rid::High, Rid::Mid, Rid::Low]
        .map(|rid| serde_json::to_string(&rid).expect("serializable"));
    assert_eq!(names, [r#""f""#, r#""h""#, r#""q""#]);
    assert!(names.is_sorted(), "the SFU falls back asciibetically");
}
