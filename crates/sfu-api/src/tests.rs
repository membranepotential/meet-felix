//! Wire shapes against literal samples from Cloudflare's docs (connection patterns, simulcast
//! page and OpenAPI examples, 2026-10-09), and `perform` against a canned transport.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use meeting_core::{
    Effect, FailureCause, LocalTrack, Mid, NegotiationId, OfferedTrack, ParticipantId, RemoteTrack,
    Rid, Sdp, SessionId, SessionKind, SfuCall, SfuReply, TrackKind, TrackName,
};
use serde_json::{Value, json};

use crate::wire::{
    AdapterId, AdapterRef, AdapterTrack, CloseAdaptersRequest, CloseTrackObject,
    CloseTracksRequest, LayerFallback, NewAdaptersRequest, NewSessionResponse, OutputCodec,
    RenegotiateRequest, SdpType, SessionDescription, Simulcast, TrackObject, TracksRequest,
    TracksResponse, UpdateTracksRequest,
};
use crate::{
    AppId, AppSecret, Failure, HttpRequest, HttpResponse, Method, SfuClient, SfuError, SfuTransport,
};

const PUBLISHER: &str = "2a45361d5fd7cc14eface0587c276c94";
const SUBSCRIBER: &str = "e017a2629c754fedc1f7d8587e06d126";

fn to_json(value: &impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

#[test]
fn publish_request_matches_the_connection_pattern() {
    let request = TracksRequest {
        session_description: Some(SessionDescription {
            kind: SdpType::Offer,
            sdp: Sdp::new("<LOCAL_SDP>"),
        }),
        tracks: vec![TrackObject::Local {
            mid: Mid::new("<TRANSCEIVER_MID>"),
            track_name: TrackName::new("camera"),
        }],
    };
    assert_eq!(
        to_json(&request),
        json!({
          "sessionDescription": { "type": "offer", "sdp": "<LOCAL_SDP>" },
          "tracks": [
            { "location": "local", "mid": "<TRANSCEIVER_MID>", "trackName": "camera" }
          ]
        })
    );
}

#[test]
fn subscribe_request_matches_the_connection_pattern() {
    let request = TracksRequest {
        session_description: None,
        tracks: vec![TrackObject::Remote {
            session_id: SessionId::new("<PUBLISHER_SESSION_ID>"),
            track_name: TrackName::new("camera"),
            mid: None,
            simulcast: None,
        }],
    };
    assert_eq!(
        to_json(&request),
        json!({
          "tracks": [
            { "location": "remote", "sessionId": "<PUBLISHER_SESSION_ID>", "trackName": "camera" }
          ]
        })
    );
}

#[test]
fn update_request_matches_the_openapi_simulcast_example() {
    let request = UpdateTracksRequest {
        tracks: vec![TrackObject::Remote {
            session_id: SessionId::new(PUBLISHER),
            track_name: TrackName::new("simulcast-track"),
            mid: Some(Mid::new("8")),
            simulcast: Some(Simulcast {
                preferred_rid: Rid::High,
                priority_ordering: LayerFallback::Asciibetical,
                rid_not_available: LayerFallback::Asciibetical,
            }),
        }],
    };
    // The docs' sample prefers rid "h"; the core names its layers q/h/f on the wire (High = "f").
    assert_eq!(
        to_json(&request),
        json!({
          "tracks": [{
            "location": "remote",
            "sessionId": PUBLISHER,
            "trackName": "simulcast-track",
            "mid": "8",
            "simulcast": {
              "preferredRid": "f",
              "priorityOrdering": "asciibetical",
              "ridNotAvailable": "asciibetical"
            }
          }]
        })
    );
}

#[test]
fn renegotiate_and_forced_close_requests_match_the_docs() {
    let renegotiate = RenegotiateRequest {
        session_description: SessionDescription {
            kind: SdpType::Answer,
            sdp: Sdp::new("<LOCAL_ANSWER_SDP>"),
        },
    };
    assert_eq!(
        to_json(&renegotiate),
        json!({ "sessionDescription": { "type": "answer", "sdp": "<LOCAL_ANSWER_SDP>" } })
    );
    let close = CloseTracksRequest {
        tracks: vec![CloseTrackObject {
            mid: Mid::new("<TRANSCEIVER_MID>"),
        }],
        force: true,
    };
    assert_eq!(
        to_json(&close),
        json!({ "tracks": [{ "mid": "<TRANSCEIVER_MID>" }], "force": true })
    );
}

#[test]
fn responses_from_the_openapi_examples_parse() {
    let session: NewSessionResponse =
        serde_json::from_value(json!({ "sessionId": SUBSCRIBER })).unwrap();
    assert_eq!(session.session_id, SessionId::new(SUBSCRIBER));

    let remote: TracksResponse = serde_json::from_value(json!({
      "requiresImmediateRenegotiation": true,
      "tracks": [
        { "sessionId": PUBLISHER, "trackName": "2e037563-a35d-4bf6-a9ee-2d474cbb9a58", "mid": "7" }
      ],
      "sessionDescription": { "sdp": "v=0\n...", "type": "offer" }
    }))
    .unwrap();
    assert!(remote.requires_immediate_renegotiation);
    assert_eq!(remote.tracks[0].mid, Some(Mid::new("7")));
    assert_eq!(
        remote.session_description.map(|d| d.kind),
        Some(SdpType::Offer)
    );
}

/// Answers every request with the next canned response and records what was sent.
struct Canned {
    responses: RefCell<VecDeque<HttpResponse>>,
    sent: RefCell<Vec<(Method, String, Option<Value>)>>,
}

impl Canned {
    fn new(responses: impl IntoIterator<Item = (u16, Value)>) -> Self {
        Self {
            responses: RefCell::new(
                responses
                    .into_iter()
                    .map(|(status, body)| HttpResponse {
                        status,
                        body: body.to_string(),
                    })
                    .collect(),
            ),
            sent: RefCell::default(),
        }
    }
}

impl SfuTransport for &Canned {
    type Error = Infallible;

    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Infallible> {
        assert_eq!(request.authorization, "Bearer s3cret");
        self.sent.borrow_mut().push((
            request.method,
            request.url,
            request.body.map(|b| serde_json::from_str(&b).unwrap()),
        ));
        Ok(self
            .responses
            .borrow_mut()
            .pop_front()
            .expect("a canned response"))
    }
}

fn client(transport: &Canned) -> SfuClient<&Canned> {
    SfuClient::new(transport, AppId::new("app"), AppSecret::new("s3cret"))
}

/// Runs a future whose transport never waits.
fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the canned transport never waits"),
    }
}

const ALICE: ParticipantId = ParticipantId(1);
/// The negotiation the effects name; every reply must carry it back.
const NEGOTIATION: NegotiationId = NegotiationId(7);

#[test]
fn new_session_effect_creates_a_session() {
    let sfu = Canned::new([(201, json!({ "sessionId": SUBSCRIBER }))]);
    let effect = Effect::SfuNewSession {
        who: ALICE,
        kind: SessionKind::Subscribe,
    };
    assert_eq!(
        ready(client(&sfu).perform(&effect)),
        Ok(Some(SfuReply::SessionCreated {
            who: ALICE,
            kind: SessionKind::Subscribe,
            session: SessionId::new(SUBSCRIBER),
        }))
    );
    let sent = sfu.sent.borrow();
    assert_eq!(sent[0].0, Method::Post);
    assert_eq!(
        sent[0].1,
        "https://rtc.live.cloudflare.com/v1/apps/app/sessions/new"
    );
    assert_eq!(sent[0].2, None);
}

#[test]
fn publish_effect_returns_the_answer_and_the_rejected_kinds_unless_every_track_is_rejected() {
    let effect = Effect::SfuPublish {
        who: ALICE,
        session: SessionId::new(PUBLISHER),
        negotiation: NEGOTIATION,
        sdp_offer: Sdp::new("offer"),
        tracks: vec![LocalTrack {
            mid: Mid::new("0"),
            kind: TrackKind::Audio,
            name: TrackName::new("mic"),
        }],
    };
    let answered = Canned::new([(
        200,
        json!({
          "requiresImmediateRenegotiation": false,
          "tracks": [{ "trackName": "mic", "mid": "0" }],
          "sessionDescription": { "sdp": "answer", "type": "answer" }
        }),
    )]);
    assert_eq!(
        ready(client(&answered).perform(&effect)),
        Ok(Some(SfuReply::Published {
            who: ALICE,
            session: SessionId::new(PUBLISHER),
            negotiation: NEGOTIATION,
            sdp_answer: Sdp::new("answer"),
            rejected: Vec::new(),
        }))
    );
    assert_eq!(
        answered.sent.borrow()[0].1,
        format!("https://rtc.live.cloudflare.com/v1/apps/app/sessions/{PUBLISHER}/tracks/new")
    );

    let two_tracks = Effect::SfuPublish {
        who: ALICE,
        session: SessionId::new(PUBLISHER),
        negotiation: NEGOTIATION,
        sdp_offer: Sdp::new("offer"),
        tracks: vec![
            LocalTrack {
                mid: Mid::new("0"),
                kind: TrackKind::Audio,
                name: TrackName::new("mic"),
            },
            LocalTrack {
                mid: Mid::new("1"),
                kind: TrackKind::Video,
                name: TrackName::new("camera"),
            },
        ],
    };
    // An entry belongs to the track whose `trackName` or `mid` it carries, whatever the order;
    // the entries that name no track belong to the tracks no entry names, by position, if their
    // counts are equal.
    let camera_error = json!({ "errorCode": "invalid_params", "errorDescription": "bad mid" });
    let named_camera_error = json!({
        "trackName": "camera", "errorCode": "invalid_params", "errorDescription": "bad mid"
    });
    let rejects_camera = [
        // Reordered: a named success, then an unnamed error.
        json!([camera_error, { "trackName": "mic", "mid": "0" }]),
        // Shorter: only the rejected track has a result.
        json!([named_camera_error]),
        // All named.
        json!([named_camera_error, { "trackName": "mic", "mid": "0" }]),
        // All unnamed, as many as the offered tracks.
        json!([{}, camera_error]),
    ];
    let partly = |tracks: &Value| {
        Canned::new([(
            200,
            json!({
              "requiresImmediateRenegotiation": false,
              "tracks": tracks,
              "sessionDescription": { "sdp": "answer", "type": "answer" }
            }),
        )])
    };
    for tracks in &rejects_camera {
        assert_eq!(
            ready(client(&partly(tracks)).perform(&two_tracks)),
            Ok(Some(SfuReply::Published {
                who: ALICE,
                session: SessionId::new(PUBLISHER),
                negotiation: NEGOTIATION,
                sdp_answer: Sdp::new("answer"),
                rejected: vec![TrackKind::Video],
            })),
            "{tracks}"
        );
    }
    // An unnamed error with no unambiguous track fails the call rather than guess.
    assert!(matches!(
        ready(client(&partly(&json!([camera_error]))).perform(&two_tracks)),
        Err(Failure {
            error: SfuError::Decode(_),
            reply: Some(SfuReply::Failed {
                cause: FailureCause::Transient,
                ..
            }),
        })
    ));

    let rejected = Canned::new([(
        200,
        json!({
          "tracks": [{ "trackName": "mic", "errorCode": "invalid_params", "errorDescription": "bad mid" }],
          "sessionDescription": { "sdp": "answer", "type": "answer" }
        }),
    )]);
    assert_eq!(
        ready(client(&rejected).perform(&effect)),
        Err(Failure {
            error: SfuError::Track {
                code: "invalid_params".to_owned(),
                description: "bad mid".to_owned(),
            },
            reply: Some(SfuReply::Failed {
                who: ALICE,
                call: SfuCall::Publish {
                    session: SessionId::new(PUBLISHER),
                    negotiation: NEGOTIATION,
                },
                cause: FailureCause::Transient,
            }),
        })
    );
}

#[test]
fn subscribe_effect_maps_mids_and_drops_rejected_tracks() {
    let camera = RemoteTrack {
        session: SessionId::new(PUBLISHER),
        name: TrackName::new("camera"),
    };
    let gone = RemoteTrack {
        session: SessionId::new(PUBLISHER),
        name: TrackName::new("gone"),
    };
    let sfu = Canned::new([(
        200,
        json!({
          "requiresImmediateRenegotiation": true,
          "tracks": [
            { "sessionId": PUBLISHER, "trackName": "camera", "mid": "5" },
            { "sessionId": PUBLISHER, "trackName": "gone",
              "errorCode": "not_found_track_error", "errorDescription": "no such track" }
          ],
          "sessionDescription": { "sdp": "offer", "type": "offer" }
        }),
    )]);
    let effect = Effect::SfuSubscribe {
        who: ALICE,
        session: SessionId::new(SUBSCRIBER),
        negotiation: NEGOTIATION,
        tracks: vec![camera.clone(), gone],
    };
    assert_eq!(
        ready(client(&sfu).perform(&effect)),
        Ok(Some(SfuReply::SubscribeOffer {
            who: ALICE,
            session: SessionId::new(SUBSCRIBER),
            negotiation: NEGOTIATION,
            sdp_offer: Sdp::new("offer"),
            tracks: vec![OfferedTrack {
                mid: Mid::new("5"),
                track: camera,
            }],
        }))
    );
}

/// The shape the live SFU returned on 2026-10-09 when the publisher was not connected yet
/// (`docs/spikes/02-first-call.md`): every track missing, and still an offer to answer.
#[test]
fn subscribe_effect_returns_the_offer_when_every_track_is_missing() {
    let sfu = Canned::new([(
        200,
        json!({
          "requiresImmediateRenegotiation": true,
          "tracks": [{
            "sessionId": PUBLISHER, "trackName": "camera",
            "errorCode": "not_found_track_error",
            "errorDescription": "Track not found on remote peer. Make sure the publisher peer is connected and sending packets for this track"
          }],
          "sessionDescription": { "sdp": "offer", "type": "offer" }
        }),
    )]);
    let effect = Effect::SfuSubscribe {
        who: ALICE,
        session: SessionId::new(SUBSCRIBER),
        negotiation: NEGOTIATION,
        tracks: vec![RemoteTrack {
            session: SessionId::new(PUBLISHER),
            name: TrackName::new("camera"),
        }],
    };
    assert_eq!(
        ready(client(&sfu).perform(&effect)),
        Ok(Some(SfuReply::SubscribeOffer {
            who: ALICE,
            session: SessionId::new(SUBSCRIBER),
            negotiation: NEGOTIATION,
            sdp_offer: Sdp::new("offer"),
            tracks: Vec::new(),
        }))
    );

    let no_offer = Canned::new([(
        200,
        json!({
          "tracks": [{ "sessionId": PUBLISHER, "trackName": "camera",
                       "errorCode": "not_found_track_error", "errorDescription": "gone" }]
        }),
    )]);
    assert_eq!(
        ready(client(&no_offer).perform(&effect)),
        Err(Failure {
            error: SfuError::Track {
                code: "not_found_track_error".to_owned(),
                description: "gone".to_owned(),
            },
            reply: Some(SfuReply::Failed {
                who: ALICE,
                call: SfuCall::Subscribe {
                    session: SessionId::new(SUBSCRIBER),
                    negotiation: NEGOTIATION,
                },
                cause: FailureCause::Transient,
            }),
        })
    );
}

#[test]
fn close_effect_treats_already_closed_tracks_as_closed() {
    let effect = Effect::SfuCloseTracks {
        who: ALICE,
        session: SessionId::new(SUBSCRIBER),
        mids: vec![Mid::new("0"), Mid::new("1")],
    };
    let closed = Canned::new([(
        200,
        json!({
          "requiresImmediateRenegotiation": false,
          "tracks": [
            { "mid": "0" },
            { "mid": "1", "errorCode": "close_track_error",
              "errorDescription": "Track doesn't exist or was already closed" }
          ]
        }),
    )]);
    assert_eq!(ready(client(&closed).perform(&effect)), Ok(None));
    assert_eq!(closed.sent.borrow()[0].0, Method::Put);

    let failed = Canned::new([(
        200,
        json!({
          "requiresImmediateRenegotiation": false,
          "tracks": [{ "mid": "0", "errorCode": "internal_error", "errorDescription": "Backend error" }]
        }),
    )]);
    assert!(matches!(
        ready(client(&failed).perform(&effect)),
        Err(Failure {
            error: SfuError::Track { .. },
            reply: None,
        })
    ));
}

#[test]
fn api_and_http_errors_become_failed_replies() {
    let effect = Effect::SfuRenegotiate {
        who: ALICE,
        session: SessionId::new(SUBSCRIBER),
        negotiation: NEGOTIATION,
        sdp_answer: Sdp::new("answer"),
    };
    let accepted = Canned::new([(200, json!({}))]);
    assert_eq!(
        ready(client(&accepted).perform(&effect)),
        Ok(Some(SfuReply::Renegotiated {
            who: ALICE,
            session: SessionId::new(SUBSCRIBER),
            negotiation: NEGOTIATION,
        }))
    );

    let api_error = Canned::new([(
        406,
        json!({ "errorCode": "invalid_session_description", "errorDescription": "wrong order" }),
    )]);
    assert_eq!(
        ready(client(&api_error).perform(&effect)),
        Err(Failure {
            error: SfuError::Api {
                status: 406,
                code: "invalid_session_description".to_owned(),
                description: "wrong order".to_owned(),
            },
            reply: Some(SfuReply::Failed {
                who: ALICE,
                call: SfuCall::Renegotiate {
                    session: SessionId::new(SUBSCRIBER),
                    negotiation: NEGOTIATION,
                },
                cause: FailureCause::Transient,
            }),
        })
    );

    let bare = Canned::new([(503, json!(null))]);
    assert!(matches!(
        ready(client(&bare).perform(&effect)),
        Err(Failure {
            error: SfuError::Status { status: 503 },
            ..
        })
    ));
}

/// The answer the live SFU gave on 2026-10-09 to every subscribe after the phone's peer
/// connection dropped: the session is gone, and the room must replace it.
#[test]
fn a_session_the_sfu_declares_gone_is_reported_as_such() {
    let effect = Effect::SfuSubscribe {
        who: ALICE,
        session: SessionId::new(SUBSCRIBER),
        negotiation: NEGOTIATION,
        tracks: vec![RemoteTrack {
            session: SessionId::new(PUBLISHER),
            name: TrackName::new("camera"),
        }],
    };
    let gone = Canned::new([(
        410,
        json!({
          "errorCode": "session_error",
          "errorDescription": "Session appears to be disconnected. Please check if the PeerConnection is connected."
        }),
    )]);
    assert_eq!(
        ready(client(&gone).perform(&effect)).map_err(|f| f.reply),
        Err(Some(SfuReply::Failed {
            who: ALICE,
            call: SfuCall::Subscribe {
                session: SessionId::new(SUBSCRIBER),
                negotiation: NEGOTIATION,
            },
            cause: FailureCause::SessionGone,
        }))
    );

    let api = |status, code: &str| SfuError::Api {
        status,
        code: code.to_owned(),
        description: String::new(),
    };
    for (error, cause) in [
        (SfuError::Status { status: 410 }, FailureCause::SessionGone),
        (api(406, "session_error"), FailureCause::SessionGone),
        (
            api(406, "invalid_session_description"),
            FailureCause::Transient,
        ),
        (SfuError::Status { status: 503 }, FailureCause::Transient),
        (
            SfuError::Transport("reset".to_owned()),
            FailureCause::Transient,
        ),
    ] {
        assert_eq!(error.cause(), cause, "{error}");
    }
}

fn adapter_track(name: &str) -> AdapterTrack {
    AdapterTrack::Remote {
        session_id: SessionId::new(PUBLISHER),
        track_name: TrackName::new(name),
        endpoint: format!("wss://example.com/rec/{name}"),
        output_codec: OutputCodec::Pcm,
    }
}

#[test]
fn adapter_request_matches_the_websocket_adapter_docs() {
    let request = NewAdaptersRequest {
        tracks: vec![adapter_track("mic")],
    };
    assert_eq!(
        to_json(&request),
        json!({ "tracks": [{
            "location": "remote",
            "sessionId": PUBLISHER,
            "trackName": "mic",
            "endpoint": "wss://example.com/rec/mic",
            "outputCodec": "pcm"
        }]})
    );
    let close = CloseAdaptersRequest {
        tracks: vec![AdapterRef {
            adapter_id: AdapterId::new("a1"),
        }],
    };
    assert_eq!(
        to_json(&close),
        json!({ "tracks": [{ "adapterId": "a1" }] })
    );
}

#[test]
fn adapter_creation_reports_each_entry_in_order() {
    let partial = Canned::new([(
        200,
        json!({ "tracks": [
            { "trackName": "a", "adapterId": "id-a", "endpoint": "wss://example.com/rec/a" },
            { "trackName": "b", "errorCode": "websocket_handshake_failed",
              "errorDescription": "handshake failed" },
        ]}),
    )]);
    let results =
        ready(client(&partial).create_adapters(vec![adapter_track("a"), adapter_track("b")]))
            .unwrap();
    assert_eq!(results[0], Ok(AdapterId::new("id-a")));
    assert!(
        matches!(&results[1], Err(SfuError::Track { code, .. }) if code == "websocket_handshake_failed")
    );
    let sent = partial.sent.borrow();
    assert_eq!(
        sent[0].1,
        "https://rtc.live.cloudflare.com/v1/apps/app/adapters/websocket/new"
    );

    // HTTP 503 when every entry failed still names the reason per entry.
    let all_failed = Canned::new([(
        503,
        json!({ "tracks": [{ "trackName": "a", "errorCode": "websocket_handshake_failed",
            "errorDescription": "no" }] }),
    )]);
    let results = ready(client(&all_failed).create_adapters(vec![adapter_track("a")])).unwrap();
    assert!(matches!(&results[0], Err(SfuError::Track { .. })));

    let down = Canned::new([(503, json!({}))]);
    assert_eq!(
        ready(client(&down).create_adapters(vec![adapter_track("a")])),
        Err(SfuError::Status { status: 503 })
    );
}

#[test]
fn closing_adapters_is_idempotent_and_reports_entry_errors() {
    let ok = Canned::new([(
        200,
        json!({ "tracks": [{ "adapterId": "a1", "bytesProcessed": 9 }] }),
    )]);
    assert_eq!(
        ready(client(&ok).close_adapters(&[AdapterId::new("a1")])),
        Ok(())
    );
    assert_eq!(
        ok.sent.borrow()[0].1,
        "https://rtc.live.cloudflare.com/v1/apps/app/adapters/websocket/close"
    );
    let failed = Canned::new([(
        200,
        json!({ "tracks": [{ "adapterId": "a1", "errorCode": "internal", "errorDescription": "x" }] }),
    )]);
    assert!(ready(client(&failed).close_adapters(&[AdapterId::new("a1")])).is_err());
}
