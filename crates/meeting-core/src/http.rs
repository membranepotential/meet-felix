//! The bodies of the Worker's HTTP API (`/api/*`): the Worker sends and reads these types, and
//! the web app gets their TypeScript declarations (feature `wasm`) and reads every response
//! with the `parse…` functions below, which check it with these serde types. So neither side
//! restates the other's shapes.
//!
//! Every error answer has the body [`ErrorBody`], whose [`ApiErrorCode`] also decides the HTTP
//! status. No answer carries more detail than the code; the Worker logs the rest.
//!
//! A web app cached before a deploy may call a newer Worker: a change of these shapes must let
//! an older reader fail to parse, so it reports a malformed answer and recovers on reload,
//! rather than read the new shape as something else.

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use tsify::Tsify;

use crate::ids::{DisplayName, MeetingId, Millis, TranscriptRef};
use crate::protocol::{Phase, RecordingPolicy};
use crate::transcript::Transcript;

/// Body of `POST /api/login`. No `Debug`, because it holds the password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    /// The owner's password, `CREATE_SECRET`.
    pub password: String,
}

/// Body of `GET /api/me` and of a successful `POST /api/login`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify), tsify(missing_as_null))]
pub struct Owner {
    /// The owner's display name from the deployment, which the app prefills; `null` if the
    /// deployment names none, or no valid one.
    pub name: Option<DisplayName>,
}

/// Body of `POST /api/new`. An empty body asks for the defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewMeetingRequest {
    /// Whether and when the meeting records; [`RecordingPolicy::FromStart`] if left out.
    #[serde(default = "record_from_start")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub recording: RecordingPolicy,
}

impl Default for NewMeetingRequest {
    fn default() -> Self {
        Self {
            recording: record_from_start(),
        }
    }
}

fn record_from_start() -> RecordingPolicy {
    RecordingPolicy::FromStart
}

/// Body of `POST /api/new`'s answer (201): the new meeting's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct NewMeeting {
    pub meeting: MeetingId,
}

/// Body of `GET /api/meeting/:words`: where the meeting stands and, while it can be joined, how
/// it records. Tagged by `phase`, the room's [`Phase`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum MeetingLookup {
    /// No host has joined yet; guests wait for the first.
    Scheduled(OpenMeeting),
    /// A host has joined.
    Live(OpenMeeting),
    /// Over. How it recorded is nobody's business any more: the transcript link went to the
    /// participants and the owner.
    Ended,
}

/// What the pre-join screen needs to know about a meeting that can be joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct OpenMeeting {
    pub recording_policy: RecordingPolicy,
}

impl MeetingLookup {
    /// The lookup of a meeting in `phase` that records by `recording_policy`.
    pub fn new(phase: Phase, recording_policy: RecordingPolicy) -> Self {
        let open = OpenMeeting { recording_policy };
        match phase {
            Phase::Scheduled => Self::Scheduled(open),
            Phase::Live => Self::Live(open),
            Phase::Ended => Self::Ended,
        }
    }
}

/// Body of `GET /api/meetings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct Listing {
    /// The owner's last meetings, newest first.
    pub meetings: Vec<PastMeeting>,
}

/// One of the owner's meetings in a [`Listing`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct PastMeeting {
    pub meeting: MeetingId,
    /// When the owner created it.
    pub created_at: Millis,
    /// As its room reported it when the list was read.
    pub phase: Phase,
    /// The transcript link's token, once the meeting ended, if it recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub transcript: Option<TranscriptRef>,
}

/// Body of `GET /api/transcript/:token`: where the meeting's transcript job stands, tagged by
/// `status`. Every status names the meeting and when recording started; all but `recording`
/// also when the meeting ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TranscriptStatus {
    /// The meeting runs and records; the job starts when it ends.
    #[serde(rename_all = "camelCase")]
    Recording {
        meeting: MeetingId,
        started_at: Millis,
        progress: Progress,
    },
    /// The recorded audio is being transcribed.
    #[serde(rename_all = "camelCase")]
    Processing {
        meeting: MeetingId,
        started_at: Millis,
        ended_at: Millis,
        progress: Progress,
        /// When the job goes on, while the speech model's daily allowance is used up.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "wasm", tsify(optional))]
        retry_at: Option<Millis>,
    },
    /// The transcript is done.
    #[serde(rename_all = "camelCase")]
    Ready {
        meeting: MeetingId,
        started_at: Millis,
        ended_at: Millis,
        /// Until when the recorded audio is kept.
        audio_until: Millis,
        /// `transcript.json`.
        transcript: Transcript,
    },
    /// The transcript could not be made.
    #[serde(rename_all = "camelCase")]
    Failed {
        meeting: MeetingId,
        started_at: Millis,
        ended_at: Millis,
    },
}

/// Milliseconds of recorded audio the job has finished (transcribed, skipped as silent, or given
/// up on), of all audio recorded so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// Body of every error answer: `{"error": code}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct ErrorBody {
    pub error: ApiErrorCode,
}

/// Why the Worker refused or failed a request. Each code has one HTTP status
/// ([`ApiErrorCode::status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub enum ApiErrorCode {
    /// 400: the body is not what the route takes.
    BadRequest,
    /// 401: an owner-only route without the owner's session or password, or a wrong password.
    Unauthorized,
    /// 403: a browser request from another origin that the owner's session cookie would
    /// authorize.
    CrossOrigin,
    /// 404: no such meeting, transcript, file or route.
    NotFound,
    /// 405: the route exists, for another method.
    MethodNotAllowed,
    /// 415: a browser's `POST /api/new` that is not `application/json`.
    NotJson,
    /// 426: a route for WebSocket upgrades, requested without one.
    UpgradeRequired,
    /// 429: the client is over a rate limit; `Retry-After` says when to try again.
    RateLimited,
    /// 500: the deployment lacks a secret or has an invalid setting; the Worker's log says
    /// which.
    NotConfigured,
    /// 500: the Worker failed; its log has the detail.
    Internal,
    /// 503: no free meeting words were found; trying again draws new ones.
    NoFreeWords,
}

impl ApiErrorCode {
    /// The HTTP status of an answer with this code.
    pub const fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::CrossOrigin => 403,
            Self::NotFound => 404,
            Self::MethodNotAllowed => 405,
            Self::NotJson => 415,
            Self::UpgradeRequired => 426,
            Self::RateLimited => 429,
            Self::NotConfigured | Self::Internal => 500,
            Self::NoFreeWords => 503,
        }
    }
}

/// The web app's readers: each checks a response body with the serde type and throws if it
/// is not one.
#[cfg(feature = "wasm")]
mod wasm {
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use tsify::{Ts, Tsify};
    use wasm_bindgen::prelude::*;

    use super::{ErrorBody, Listing, MeetingLookup, NewMeeting, Owner, TranscriptStatus};

    fn parse<T: DeserializeOwned + Serialize + Tsify>(json: &str) -> Result<Ts<T>, JsError> {
        Ok(serde_json::from_str::<T>(json)?.into_ts()?)
    }

    /// Reads an [`Owner`] (`GET /api/me`, `POST /api/login`).
    #[wasm_bindgen(js_name = parseOwner)]
    pub fn parse_owner(json: &str) -> Result<Ts<Owner>, JsError> {
        parse(json)
    }

    /// Reads a [`NewMeeting`] (`POST /api/new`).
    #[wasm_bindgen(js_name = parseNewMeeting)]
    pub fn parse_new_meeting(json: &str) -> Result<Ts<NewMeeting>, JsError> {
        parse(json)
    }

    /// Reads a [`MeetingLookup`] (`GET /api/meeting/:words`).
    #[wasm_bindgen(js_name = parseMeetingLookup)]
    pub fn parse_meeting_lookup(json: &str) -> Result<Ts<MeetingLookup>, JsError> {
        parse(json)
    }

    /// Reads a [`Listing`] (`GET /api/meetings`).
    #[wasm_bindgen(js_name = parseListing)]
    pub fn parse_listing(json: &str) -> Result<Ts<Listing>, JsError> {
        parse(json)
    }

    /// Reads a [`TranscriptStatus`] (`GET /api/transcript/:token`).
    #[wasm_bindgen(js_name = parseTranscriptStatus)]
    pub fn parse_transcript_status(json: &str) -> Result<Ts<TranscriptStatus>, JsError> {
        parse(json)
    }

    /// Reads an [`ErrorBody`], the body of every error answer.
    #[wasm_bindgen(js_name = parseErrorBody)]
    pub fn parse_error_body(json: &str) -> Result<Ts<ErrorBody>, JsError> {
        parse(json)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn wire(value: &impl Serialize) -> Value {
        serde_json::to_value(value).expect("serializable")
    }

    #[test]
    fn a_lookup_tells_the_recording_policy_only_while_the_meeting_can_be_joined() {
        assert_eq!(
            wire(&MeetingLookup::new(Phase::Live, RecordingPolicy::FromStart)),
            json!({ "phase": "live", "recordingPolicy": "fromStart" })
        );
        assert_eq!(
            wire(&MeetingLookup::new(
                Phase::Scheduled,
                RecordingPolicy::HostDecides
            )),
            json!({ "phase": "scheduled", "recordingPolicy": "hostDecides" })
        );
        assert_eq!(
            wire(&MeetingLookup::new(Phase::Ended, RecordingPolicy::FromStart)),
            json!({ "phase": "ended" })
        );
        let open: Result<MeetingLookup, _> = serde_json::from_value(json!({ "phase": "live" }));
        assert!(open.is_err(), "an open meeting without its policy");
    }

    #[test]
    fn a_past_meeting_leaves_out_a_missing_transcript() {
        let meeting = MeetingId::parse("amber-otter-ocean").expect("listed words");
        let past = |transcript| PastMeeting {
            meeting: meeting.clone(),
            created_at: Millis(1),
            phase: Phase::Ended,
            transcript,
        };
        assert_eq!(
            wire(&past(None)),
            json!({ "meeting": meeting, "createdAt": 1, "phase": "ended" })
        );
        assert_eq!(
            wire(&past(Some(TranscriptRef::new("t")))),
            json!({ "meeting": meeting, "createdAt": 1, "phase": "ended", "transcript": "t" })
        );
    }

    /// A ready status as the recorder sends it: with the progress, which a ready status does
    /// not need, and `transcript.json` as written, with its version and without `withdrawnAt`
    /// while consent stands.
    #[test]
    fn a_transcript_status_reads_what_the_recorder_sends() {
        let ready = json!({
            "status": "ready", "meeting": "amber-otter-ocean", "startedAt": 1_000,
            "endedAt": 9_000, "progress": { "done": 4, "total": 4 }, "audioUntil": 99,
            "transcript": {
                "version": crate::transcript::TRANSCRIPT_VERSION, "meeting": "amber-otter-ocean",
                "startedAt": 1_000, "endedAt": 9_000,
                "speakers": [{ "who": 0, "label": "Robin (host)", "role": "host" }],
                "consents": [{ "who": 0, "name": "Robin", "at": 800, "noticeVersion": 1 }],
                "gaps": [{ "who": 0, "start": 2_000, "end": 3_000, "reason": "missing" }],
                "paragraphs": [{ "who": 0, "label": "Robin (host)", "startMs": 1_000,
                    "endMs": 1_500, "text": "Guten Tag." }]
            }
        });
        let status: TranscriptStatus = serde_json::from_value(ready.clone()).expect("ready");
        let TranscriptStatus::Ready {
            audio_until,
            ref transcript,
            ..
        } = status
        else {
            panic!("{status:?}");
        };
        assert_eq!((audio_until, transcript.paragraphs.len()), (Millis(99), 1));

        let recording = json!({ "status": "recording", "meeting": "amber-otter-ocean",
            "startedAt": 1_000, "progress": { "done": 0, "total": 0 } });
        let status: TranscriptStatus = serde_json::from_value(recording.clone()).expect("recording");
        assert_eq!(wire(&status), recording);

        for broken in [
            json!({ "status": "processing", "meeting": "amber-otter-ocean", "startedAt": 1,
                "progress": { "done": 0, "total": 0 } }),
            json!({ "status": "ready", "meeting": "amber-otter-ocean", "startedAt": 1,
                "endedAt": 2, "transcript": ready["transcript"] }),
            json!({ "status": "lost", "meeting": "amber-otter-ocean", "startedAt": 1 }),
        ] {
            assert!(
                serde_json::from_value::<TranscriptStatus>(broken.clone()).is_err(),
                "{broken}"
            );
        }
    }

    #[test]
    fn an_error_is_its_code_alone() {
        let body = ErrorBody {
            error: ApiErrorCode::RateLimited,
        };
        assert_eq!(wire(&body), json!({ "error": "rateLimited" }));
        assert_eq!(body.error.status(), 429);
        assert_eq!(ApiErrorCode::NotFound.status(), 404);
    }

    #[test]
    fn a_new_meeting_records_from_the_start_unless_asked_otherwise() {
        let parse = |json| serde_json::from_str::<NewMeetingRequest>(json).map(|r| r.recording);
        assert_eq!(parse("{}").ok(), Some(RecordingPolicy::FromStart));
        assert_eq!(
            parse(r#"{"recording":"hostDecides"}"#).ok(),
            Some(RecordingPolicy::HostDecides)
        );
        assert!(parse(r#"{"recording":"never"}"#).is_err());
        assert!(parse(r#"{"recording":"fromStart","title":"x"}"#).is_err());
        assert_eq!(
            NewMeetingRequest::default().recording,
            RecordingPolicy::FromStart
        );
    }

    #[test]
    fn a_login_is_the_password_alone() {
        let parse = |json| serde_json::from_str::<LoginRequest>(json).map(|r| r.password);
        assert_eq!(parse(r#"{"password":"pw"}"#).ok().as_deref(), Some("pw"));
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"password":"pw","user":"x"}"#).is_err());
    }

    #[test]
    fn the_owner_without_a_name_is_null() {
        assert_eq!(wire(&Owner { name: None }), json!({ "name": null }));
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn declarations_match_the_wire() {
        use tsify::Tsify;

        assert!(MeetingLookup::DECL.contains(
            r#"({ phase: "scheduled" } & OpenMeeting) | ({ phase: "live" } & OpenMeeting) | { phase: "ended" }"#
        ));
        assert!(Owner::DECL.contains("name: DisplayName | null;"));
        assert!(PastMeeting::DECL.contains("transcript?: TranscriptRef;"));
        assert!(TranscriptStatus::DECL.contains(
            r#"{ status: "recording"; meeting: MeetingId; startedAt: Millis; progress: Progress }"#
        ));
        assert!(TranscriptStatus::DECL.contains("retryAt?: Millis }"));
        assert!(
            TranscriptStatus::DECL.contains("audioUntil: Millis; transcript: Transcript }")
        );
        assert!(ApiErrorCode::DECL.contains(r#""notFound""#));
    }
}
