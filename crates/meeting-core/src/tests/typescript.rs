//! The JSON on the wire and the generated TypeScript declarations agree.

use crate::{ClientMsg, ServerMsg};

#[cfg(feature = "wasm")]
#[test]
fn declarations_use_camel_case_tags_fields_and_optional_options() {
    use tsify::Tsify;

    use crate::RoomView;

    assert!(ClientMsg::DECL.contains(
        r#"{ type: "join"; name: DisplayName; consent: boolean; hostToken?: HostToken; resume?: ResumeToken }"#
    ));
    assert!(ClientMsg::DECL.contains(r#"{ type: "setConsent"; consent: boolean }"#));
    assert!(ServerMsg::DECL.contains(
        r#"{ type: "welcome"; you: ParticipantId; state: RoomView; resume: ResumeToken }"#
    ));
    assert!(ServerMsg::DECL.contains(r#"{ type: "consentRequired" }"#));
    assert!(ServerMsg::DECL.contains(r#"{ type: "ended"; transcript?: TranscriptRef }"#));
    assert!(RoomView::DECL.contains("recording?: Recording;"));
    assert!(RoomView::DECL.contains("recordingPolicy: RecordingPolicy;"));
}

#[test]
fn json_uses_camel_case_tags_and_omits_absent_options() {
    let msg: ClientMsg =
        serde_json::from_str(r#"{"type":"join","name":" Ada ","consent":true}"#).expect("valid");
    assert!(
        matches!(msg, ClientMsg::Join { host_token: None, ref name, .. } if name.as_str() == "Ada")
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
}
