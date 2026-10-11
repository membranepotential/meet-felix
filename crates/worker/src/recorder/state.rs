//! What a `RecorderObject` keeps in its key-value storage, the status it reports, and the pure
//! rules of its transcript job: retry times, error classes, R2 keys, gaps.

use meeting_core::http::{Progress, TranscriptStatus};
use meeting_core::recording::{ConsentRecord, RosterEntry};
use meeting_core::transcript::{Gap, GapReason, Transcript};
use meeting_core::{MeetingId, Millis, ParticipantId, SessionId, TrackName};
use serde::{Deserialize, Serialize};
use sfu_api::wire::AdapterId;

use crate::constant_time;
use crate::internal::{FinishRecording, TrackRecording, UntrackRecording};
use crate::record::{DecodeError, Stored, stored_record, stored_version};

/// The recorder's storage key.
pub const KEY: &str = "recorder";

/// The storage key of the [`BeforeStart`] requests: those that reached the recorder before its
/// `/start` created [`KEY`]. The room posts each step's requests in their own fetches, so a
/// later step's request can overtake the first `/start`.
pub const BEFORE_START_KEY: &str = "beforeStart";

/// The requests that overtook the first `/start`. `/start` takes `applied` into the new record
/// ([`Recorder::applied`]), then applies `tracks` newest first ([`BeforeStart::tracks`]), so
/// a `/track` that a later `/untrack` or `/stop` superseded is dropped as stale, and deletes the
/// key.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeStart {
    pub applied: Applied,
    tracks: Vec<Ordered<TrackRecording>>,
}

impl BeforeStart {
    /// Keeps a `/track` to apply once the record exists.
    pub fn track(&mut self, track: Ordered<TrackRecording>) {
        self.tracks.push(track);
    }

    /// The kept `/track`s, newest first: of a speaker's tracks the newest is applied and the
    /// older ones are stale.
    pub fn tracks(self) -> (Applied, Vec<Ordered<TrackRecording>>) {
        let mut tracks = self.tracks;
        tracks.sort_by_key(|track| std::cmp::Reverse(track.seq));
        (self.applied, tracks)
    }
}

/// The version of the stored recorder's shape. 1 is the bare [`Recorder`] stored before
/// 2026-10-11; 2 wraps it in the [`Stored`] envelope the room uses.
pub const RECORDER_VERSION: u32 = 2;

/// The stored JSON of `recorder`.
pub fn encode(recorder: &Recorder) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        version: RECORDER_VERSION,
        record: recorder,
    })
}

/// Reads a stored recorder. Unlike a room, a recorder lives as long as its transcript (D18),
/// and its status page needs the record, so every version ever stored keeps a reader here: a
/// change of shape adds a version and an arm that migrates the old one. Frozen records in
/// `crates/worker/fixtures/recorder/` pin every version (see `crate::record` tests).
pub fn decode(json: &str) -> Result<Recorder, DecodeError> {
    match stored_version(json, RECORDER_VERSION)? {
        // Unversioned: the record itself, of today's shape.
        1 => Ok(serde_json::from_str(json)?),
        _ => stored_record(json),
    }
}

/// How long the audio stays after the meeting ended (D18).
pub const AUDIO_RETENTION_MS: u64 = 48 * 60 * 60 * 1000;
/// Job steps (merging, writing the transcript) fail this often in a row before the transcript
/// counts as failed. Quota errors from Workers AI never count.
pub const MAX_STEP_FAILURES: u32 = 6;
/// A chunk the speech model fails on this often is left out, marked as a gap.
pub const MAX_CHUNK_ATTEMPTS: u32 = 3;

/// Everything the recorder remembers besides the media in SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recorder {
    pub meeting: MeetingId,
    /// When recording first started.
    pub started_at: Millis,
    /// The token in the adapters' endpoint URLs (128 bits, hex). Never logged.
    pub secret: String,
    /// Names and roles: of the speakers the room asked to record, and from the room's `/end` of
    /// everyone on its roster. Written only by [`Recorder::learn`].
    pub speakers: Vec<RosterEntry>,
    /// The consent record: of the speakers the room asked to record, and from the room's `/end`
    /// the whole record. Written only by [`Recorder::learn`].
    #[serde(default)]
    pub consents: Vec<ConsentRecord>,
    pub adapters: Vec<Adapter>,
    /// Stretches in which a speaker's track was to be recorded but had no adapter, because
    /// creating it failed ([`Recorder::settle`]); the transcript shows them as missing audio.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unrecorded: Vec<Unrecorded>,
    pub phase: RecorderPhase,
    /// The newest of the room's requests the recorder applied, so it can drop one that arrives
    /// after a newer one ([`RequestSeq`]). Recorders stored before it existed load as having
    /// applied none.
    #[serde(default)]
    pub applied: Applied,
}

/// The place of a request from the room among all the requests the room sends its recorder,
/// in the order the room's steps produced them.
///
/// The room carries out a step's effects after storing the step, and each request to the
/// recorder awaits a fetch with the input gate open, so a later step's `/stop` or `/untrack`
/// can reach the recorder before an earlier step's `/track`. Applied as it arrives, that
/// `/track` would record audio that `RoomState` says is not recorded (D15). The recorder
/// instead drops a request older than one it applied for the same speaker, and a `/track` older
/// than the last `/stop` ([`Recorder::begin_track`], [`Recorder::untrack`]). A request without
/// a number (from a room that ran older code) counts as [`RequestSeq::UNORDERED`], older than
/// every numbered one.
///
/// D15 does not rest on the numbers alone: a `/stop` closes every adapter, whatever its number
/// ([`Recorder::stop`]), so a wrong number can at worst leave a speaker unrecorded, never
/// recorded after a stop. A room's numbers start from its clock ([`RequestSeq::starting_at`])
/// whenever its record has none, so even a counter lost to older code (a rollback rewrites the
/// record without it) restarts above every number handed out before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestSeq(pub u64);

impl RequestSeq {
    /// What a request from a room that numbered none carries.
    pub const UNORDERED: Self = Self(0);

    /// The first number of a room that starts counting at `now`: the time in milliseconds,
    /// times 1,000. A room sends far fewer than 1,000 requests per millisecond, so a counter
    /// started later is above any started earlier, however many requests that one numbered.
    pub fn starting_at(now: Millis) -> Self {
        Self(now.0.saturating_mul(1_000))
    }

    /// The number `n` requests after this one.
    #[must_use]
    pub fn after(self, n: usize) -> Self {
        Self(self.0 + n as u64)
    }
}

impl Default for RequestSeq {
    fn default() -> Self {
        Self::UNORDERED
    }
}

/// A request from the room with its [`RequestSeq`], as the room posts it: the request's own
/// fields plus `seq`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ordered<R> {
    #[serde(default)]
    pub seq: RequestSeq,
    #[serde(flatten)]
    pub request: R,
}

/// The body of `/stop`, which has no fields besides its [`RequestSeq`].
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {}

/// The newest requests the recorder applied: per speaker the last `/track` or `/untrack`, and
/// the last `/stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub stop: RequestSeq,
    pub speakers: Vec<(ParticipantId, RequestSeq)>,
}

impl Applied {
    /// The last `/track` or `/untrack` applied for `who`.
    fn speaker(&self, who: ParticipantId) -> RequestSeq {
        self.speakers
            .iter()
            .find(|(id, _)| *id == who)
            .map_or(RequestSeq::UNORDERED, |&(_, seq)| seq)
    }

    fn set_speaker(&mut self, who: ParticipantId, seq: RequestSeq) {
        match self.speakers.iter_mut().find(|(id, _)| *id == who) {
            Some((_, known)) => *known = seq,
            None => self.speakers.push((who, seq)),
        }
    }

    /// Notes an `/untrack` for `who` numbered `seq`; `false` if it is older than a `/track` or
    /// `/untrack` applied for them, and so is to be dropped.
    pub fn untrack(&mut self, who: ParticipantId, seq: RequestSeq) -> bool {
        if seq < self.speaker(who) {
            return false;
        }
        self.set_speaker(who, seq);
        true
    }

    /// Notes a `/stop` numbered `seq`; returns the speakers whose last applied request is
    /// newer, for whom the stop came late.
    pub fn stop(&mut self, seq: RequestSeq) -> Vec<ParticipantId> {
        self.stop = self.stop.max(seq);
        self.speakers
            .iter()
            .filter(|&&(_, last)| last > seq)
            .map(|&(who, _)| who)
            .collect()
    }
}

/// What a `/track` asks of the recorder ([`Recorder::begin_track`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackPlan {
    /// The request is older than a `/stop`, `/track` or `/untrack` already applied: nothing
    /// changed.
    Stale,
    /// The speaker's open adapter carries the track already; only the name and consent changed.
    Unchanged,
    /// `adapter` is to be created at the SFU, after closing `replaced`, the speaker's previous
    /// ones.
    New {
        adapter: Adapter,
        replaced: Vec<Adapter>,
    },
}

/// What a request from the room says about the meeting's people. The room owns names and
/// consents (the core's roster and consent record); the recorder keeps a copy so that a
/// transcript can name its speakers and list the consents even if the room's `/end` never
/// arrives, and takes it in only through [`Recorder::learn`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Learned {
    /// From a `/track`: the speaker's name, role and consent.
    Speaker {
        entry: RosterEntry,
        consent: ConsentRecord,
    },
    /// From an `/untrack`: the speaker's consent, which shows a withdrawal.
    Consent(ConsentRecord),
    /// From `/end`: the whole roster and consent record, the room's last word.
    Everyone(FinishRecording),
}

/// Replaces the item of `items` that `same` picks, or adds `item`.
fn upsert<T>(items: &mut Vec<T>, item: T, same: impl Fn(&T, &T) -> bool) {
    match items.iter_mut().find(|known| same(known, &item)) {
        Some(known) => *known = item,
        None => items.push(item),
    }
}

/// One WebSocket adapter: one speaker's audio track, streamed to
/// `/rec/{recorder}/{secret}/{who}/{generation}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub who: ParticipantId,
    /// Counts the speaker's adapters from 0; a republished or replaced track gets the next one.
    pub generation: u32,
    pub session: SessionId,
    pub track_name: TrackName,
    #[serde(flatten)]
    pub creation: Creation,
    /// Whether the recorder still accepts this adapter's stream.
    pub open: bool,
}

impl Adapter {
    /// The SFU's id, once it created the adapter.
    pub fn id(&self) -> Option<&AdapterId> {
        match &self.creation {
            Creation::Created(id) => Some(id),
            Creation::Asked | Creation::Failing(_) => None,
        }
    }

    /// Whether a failed call's retry is due at `now`.
    fn call_due(&self, now: Millis) -> bool {
        matches!(self.creation, Creation::Failing(Failing { retry_at: Some(at), .. }) if at <= now)
    }

    /// Whether the recorder stopped asking the SFU for it.
    fn gave_up(&self) -> bool {
        matches!(
            self.creation,
            Creation::Failing(Failing { retry_at: None, .. })
        )
    }
}

/// Where creating an adapter at the SFU stands. Stored as the adapter's `id` once created, and
/// as `failing` while creating it fails; neither while the first call is out, as before
/// creating could fail (2026-10-11).
///
/// Every call has its own generation, so its own endpoint: a failed call closes its
/// generation, and the next call is made for the next one ([`Recorder::settle`]). The SFU has
/// no way to list or look up adapters, and the adapter's connection carries no id, so if a
/// call failed only on the way back, the adapter it created streams to a closed generation,
/// is refused at the upgrade, and closes itself after the SFU's 15 s of reconnecting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "CreationFields", into = "CreationFields")]
pub enum Creation {
    /// The recorder asked the SFU and has no answer yet, or an eviction cut the call off.
    Asked,
    /// The SFU created it.
    Created(AdapterId),
    /// Calls for the speaker's track failed. Open: this generation is the next call's. Closed:
    /// this generation's call failed.
    Failing(Failing),
}

/// Calls for a speaker's track that failed, and when the recorder calls again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failing {
    /// When the first call failed. From then on the speaker's audio is missing, until the
    /// adapter exists or closes.
    pub since: Millis,
    /// How many calls failed.
    pub failures: u32,
    /// When the next call is due; `None` after [`MAX_ADAPTER_CALLS`] failed calls, when the
    /// recorder gave up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<Millis>,
}

/// [`Creation`] as the adapter's stored fields.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreationFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<AdapterId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failing: Option<Failing>,
}

impl From<CreationFields> for Creation {
    fn from(CreationFields { id, failing }: CreationFields) -> Self {
        match (id, failing) {
            (Some(id), _) => Self::Created(id),
            (None, Some(failing)) => Self::Failing(failing),
            (None, None) => Self::Asked,
        }
    }
}

impl From<Creation> for CreationFields {
    fn from(creation: Creation) -> Self {
        let (id, failing) = match creation {
            Creation::Asked => (None, None),
            Creation::Created(id) => (Some(id), None),
            Creation::Failing(failing) => (None, Some(failing)),
        };
        Self { id, failing }
    }
}

/// A stretch of a speaker's audio that was to be recorded but had no adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unrecorded {
    pub who: ParticipantId,
    pub start: Millis,
    pub end: Millis,
}

/// Calls to create an adapter before the recorder gives up on it, about five minutes after the
/// first ([`adapter_retry`]).
pub const MAX_ADAPTER_CALLS: u32 = 10;

/// The wait before the next call to create an adapter after the `failures`-th failed one, or
/// `None` once [`MAX_ADAPTER_CALLS`] failed: 1 s, 2 s, 5 s, 10 s, 30 s, then a minute. The first
/// wait outlasts the 0.5–1 s after a publish in which the SFU refuses to pull the new track
/// (measured for subscriber pulls on 2026-10-10; an adapter pulls the track too).
pub fn adapter_retry(failures: u32) -> Option<u64> {
    (failures < MAX_ADAPTER_CALLS).then_some(match failures {
        0 | 1 => 1_000,
        2 => 2_000,
        3 => 5_000,
        4 => 10_000,
        5 => 30_000,
        _ => 60_000,
    })
}

/// What a call to create an adapter came to ([`Recorder::settle`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The adapter exists and streams.
    Created,
    /// The SFU created the adapter after the recorder closed it (an untrack, a stop, the end):
    /// the caller closes it at the SFU.
    Orphaned(Adapter),
    /// The call failed; the next is due then.
    RetryAt(Millis),
    /// The call failed for the last time: the speaker stays unrecorded until their next track.
    GaveUp,
    /// The call failed after the recorder closed the adapter, or the adapter is unknown: there
    /// is nothing to do.
    Moot,
}

/// Where the recording stands. Not `meeting_core::Phase`, the meeting's phase: a recorder
/// ends with its meeting, but has no phase before recording starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecorderPhase {
    /// The meeting runs; adapters come and go.
    Recording,
    /// The meeting ended at `at`; the transcript job runs or ran.
    #[serde(rename_all = "camelCase")]
    Ended { at: Millis, job: Job },
}

/// Where the transcript job stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Job {
    /// Uploading and transcribing chunks. With `retry_at`, Workers AI refused for its quota or
    /// rate limit and the job waits until then ([`on_refusal`]). Records stored before
    /// 2026-10-11 also hold `waitingSince`, which is no longer read.
    #[serde(rename_all = "camelCase")]
    Transcribing { retry_at: Option<Millis> },
    /// Merging segments into paragraphs, a batch per run, after `after`.
    #[serde(rename_all = "camelCase")]
    Merging {
        after: Option<SegmentCursor>,
        failures: u32,
    },
    /// The transcript is in R2. The audio is deleted at `audio_until`.
    #[serde(rename_all = "camelCase")]
    Ready {
        audio_until: Millis,
        audio_deleted_at: Option<Millis>,
    },
    /// The job gave up.
    #[serde(rename_all = "camelCase")]
    Failed { at: Millis },
}

/// What the transcript job does next ([`Job::step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStep {
    /// Nothing is due before then.
    Wait(Millis),
    /// Transcribe the next chunks; the audio is kept until `audio_until`.
    Transcribe { audio_until: Millis },
    /// Merge the segments after `after`, then write the transcript.
    Merge {
        after: Option<SegmentCursor>,
        failures: u32,
    },
    /// Delete the audio; it was kept until `audio_until`.
    DeleteAudio { audio_until: Millis },
    /// The job is over.
    Done,
}

impl Job {
    /// The step due at `now` for a meeting that ended at `ended_at`. A wait for Workers AI ends
    /// when the audio is due to go, also one that a record stored before 2026-10-11 set later
    /// ([`on_refusal`]).
    pub fn step(&self, ended_at: Millis, now: Millis) -> JobStep {
        let keep_until = ended_at.after(AUDIO_RETENTION_MS);
        match *self {
            Self::Transcribing { retry_at: Some(at) } if at.min(keep_until) > now => {
                JobStep::Wait(at.min(keep_until))
            }
            Self::Transcribing { .. } => JobStep::Transcribe {
                audio_until: keep_until,
            },
            Self::Merging { after, failures } => JobStep::Merge { after, failures },
            Self::Ready {
                audio_until,
                audio_deleted_at: None,
            } if audio_until <= now => JobStep::DeleteAudio { audio_until },
            Self::Ready {
                audio_until,
                audio_deleted_at: None,
            } => JobStep::Wait(audio_until),
            Self::Ready { .. } | Self::Failed { .. } => JobStep::Done,
        }
    }
}

/// The last merged segment, in merge order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SegmentCursor {
    pub start_ms: u64,
    pub id: i64,
}

impl Recorder {
    /// The speaker's open adapter, if any.
    pub fn open_adapter(&self, who: ParticipantId) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.who == who && a.open)
    }

    /// The generation `who`'s next adapter gets.
    pub fn next_generation(&self, who: ParticipantId) -> u32 {
        self.adapters
            .iter()
            .filter(|a| a.who == who)
            .map(|a| a.generation + 1)
            .max()
            .unwrap_or(0)
    }

    /// Takes in what the room said about its people; returns whether anything changed. One
    /// rule decides, whatever order the requests arrive in:
    ///
    /// - While recording, a `/track` or `/untrack` that the numbering let through
    ///   ([`Recorder::begin_track`], [`Recorder::untrack`]) is the newest the room said about
    ///   that speaker, so it replaces their entry and consent.
    /// - `/end` is the room's last request, and the core's roster and consent record hold
    ///   everyone who ever joined or consented, so it replaces both lists whole, whenever it
    ///   arrives before the transcript is written, also after the recorder ended a meeting
    ///   nobody ended.
    /// - Once the meeting ended, a per-speaker request is older than `/end` and changes
    ///   nothing.
    pub fn learn(&mut self, learned: Learned) -> bool {
        let recording = self.phase == RecorderPhase::Recording;
        match learned {
            Learned::Speaker { entry, consent } if recording => {
                upsert(&mut self.speakers, entry, |a, b| a.id == b.id);
                upsert(&mut self.consents, consent, |a, b| a.who == b.who);
            }
            Learned::Consent(consent) if recording => {
                upsert(&mut self.consents, consent, |a, b| a.who == b.who);
            }
            Learned::Everyone(FinishRecording { roster, consents })
                if !self.phase.transcript_written() =>
            {
                self.speakers = roster;
                self.consents = consents;
            }
            Learned::Speaker { .. } | Learned::Consent(_) | Learned::Everyone(_) => {
                return false;
            }
        }
        true
    }

    /// Marks the adapters that `which` selects closed at `now`; returns those that were open.
    /// An adapter that creating failed for leaves the stretch since its first failure
    /// unrecorded.
    pub fn close_adapters(
        &mut self,
        which: impl Fn(&Adapter) -> bool,
        now: Millis,
    ) -> Vec<Adapter> {
        let mut closed = Vec::new();
        for adapter in self.adapters.iter_mut().filter(|a| a.open && which(a)) {
            adapter.open = false;
            if let Creation::Failing(Failing { since, .. }) = adapter.creation {
                self.unrecorded.push(Unrecorded {
                    who: adapter.who,
                    start: since,
                    end: now,
                });
            }
            closed.push(adapter.clone());
        }
        closed
    }

    /// Stores what the call to create the adapter of `(who, generation)` came to at `now`:
    /// the SFU's id, or a failure. A failure closes the generation and opens the next one for
    /// the next call, due after the [`adapter_retry`] wait ([`Creation`] says why). A late
    /// success ends the speaker's unrecorded stretch.
    pub fn settle(
        &mut self,
        (who, generation): (ParticipantId, u32),
        created: Result<AdapterId, ()>,
        now: Millis,
    ) -> Settled {
        let Some(adapter) = self
            .adapters
            .iter_mut()
            .find(|a| a.who == who && a.generation == generation)
        else {
            return Settled::Moot;
        };
        let failing = match adapter.creation {
            Creation::Failing(failing) => Some(failing),
            Creation::Asked | Creation::Created(_) => None,
        };
        match created {
            Ok(id) => {
                adapter.creation = Creation::Created(id);
                if !adapter.open {
                    // Its unrecorded stretch ended when it closed.
                    return Settled::Orphaned(adapter.clone());
                }
                if let Some(Failing { since, .. }) = failing {
                    self.unrecorded.push(Unrecorded {
                        who,
                        start: since,
                        end: now,
                    });
                }
                Settled::Created
            }
            Err(()) if !adapter.open => Settled::Moot,
            Err(()) => {
                let failures = failing.map_or(0, |f| f.failures) + 1;
                let since = failing.map_or(now, |f| f.since);
                let retry_at = adapter_retry(failures).map(|wait| now.after(wait));
                // Closed without an unrecorded stretch: the next generation carries it on.
                adapter.open = false;
                adapter.creation = Creation::Failing(Failing {
                    since,
                    failures,
                    retry_at: None,
                });
                let (session, track_name) = (adapter.session.clone(), adapter.track_name.clone());
                let next = Adapter {
                    who,
                    generation: self.next_generation(who),
                    session,
                    track_name,
                    creation: Creation::Failing(Failing {
                        since,
                        failures,
                        retry_at,
                    }),
                    open: true,
                };
                self.adapters.push(next);
                retry_at.map_or(Settled::GaveUp, Settled::RetryAt)
            }
        }
    }

    /// The streams of the open adapters whose next call is due at `now`.
    pub fn adapters_due(&self, now: Millis) -> Vec<(ParticipantId, u32)> {
        self.adapters
            .iter()
            .filter(|a| a.open && a.call_due(now))
            .map(|a| (a.who, a.generation))
            .collect()
    }

    /// The adapter of `(who, generation)`, if the recorder is to call the SFU for it at `now`:
    /// open, and either just opened by a `/track` or failing with its next call due. Read from
    /// the stored record right before each call, so a `/stop` or `/untrack` that arrived during
    /// an earlier call makes the SFU pull nothing for a closed adapter.
    pub fn to_create(
        &self,
        (who, generation): (ParticipantId, u32),
        now: Millis,
    ) -> Option<&Adapter> {
        self.adapters.iter().find(|a| {
            a.who == who
                && a.generation == generation
                && a.open
                && (a.creation == Creation::Asked || a.call_due(now))
        })
    }

    /// Sets the transcript job's step, once the meeting ended.
    pub fn set_job(&mut self, new: Job) {
        if let RecorderPhase::Ended { job, .. } = &mut self.phase {
            *job = new;
        }
    }

    /// When the next call to create an open adapter is due, if one is.
    pub fn next_adapter_call(&self) -> Option<Millis> {
        self.adapters
            .iter()
            .filter(|a| a.open)
            .filter_map(|a| match a.creation {
                Creation::Failing(Failing { retry_at, .. }) => retry_at,
                Creation::Asked | Creation::Created(_) => None,
            })
            .min()
    }

    /// Applies the room's `/track` numbered `seq` at `now`, unless it is stale: records the
    /// speaker's name and consent, and opens an adapter for the track unless the speaker's open
    /// one carries it already and the recorder has not given up creating it. The caller creates
    /// a new adapter at the SFU.
    pub fn begin_track(
        &mut self,
        track: &TrackRecording,
        seq: RequestSeq,
        now: Millis,
    ) -> TrackPlan {
        let who = track.who;
        if seq < self.applied.stop.max(self.applied.speaker(who)) {
            return TrackPlan::Stale;
        }
        self.applied.set_speaker(who, seq);
        self.learn(Learned::Speaker {
            entry: RosterEntry {
                id: who,
                name: track.name.clone(),
                role: track.role,
            },
            consent: track.consent.clone(),
        });
        if self.open_adapter(who).is_some_and(|a| {
            a.session == track.session && a.track_name == track.track_name && !a.gave_up()
        }) {
            return TrackPlan::Unchanged;
        }
        let replaced = self.close_adapters(|a| a.who == who, now);
        let adapter = Adapter {
            who,
            generation: self.next_generation(who),
            session: track.session.clone(),
            track_name: track.track_name.clone(),
            creation: Creation::Asked,
            open: true,
        };
        self.adapters.push(adapter.clone());
        TrackPlan::New { adapter, replaced }
    }

    /// Applies the room's `/untrack` numbered `seq`, unless it is older than a `/track` or
    /// `/untrack` applied for the speaker (`None`): closes the speaker's adapters and records
    /// the consent it carries. Returns the adapters it closed.
    ///
    /// It closes all of them, not only the named track's: the room untracks only the track it
    /// records, so after the newest request for a speaker is an `/untrack`, nothing of theirs
    /// is recorded. An older `/track` for another of their tracks may have opened an adapter
    /// before this arrived.
    pub fn untrack(
        &mut self,
        untrack: &UntrackRecording,
        seq: RequestSeq,
        now: Millis,
    ) -> Option<Vec<Adapter>> {
        let who = untrack.who;
        if !self.applied.untrack(who, seq) {
            return None;
        }
        let closed = self.close_adapters(|a| a.who == who, now);
        if let Some(consent) = &untrack.consent {
            self.learn(Learned::Consent(consent.clone()));
        }
        Some(closed)
    }

    /// Applies the room's `/stop` numbered `seq`: closes every adapter, and from now on drops
    /// a `/track` numbered before it. Returns the adapters it closed and the speakers whose last
    /// applied request is numbered after the stop: for them the stop came late, or the numbers
    /// are wrong, and closing their adapter leaves them unrecorded until their next `/track`.
    ///
    /// It closes even those: whatever the numbers say, every adapter open now was created by a
    /// `/track` the room sent while it recorded, and after a stop the room records nothing
    /// until it starts again and sends new `/track`s. Leaving a speaker unrecorded is the safe
    /// mistake; recording one after a stop is the one D15 forbids.
    pub fn stop(&mut self, seq: RequestSeq, now: Millis) -> (Vec<Adapter>, Vec<ParticipantId>) {
        let late = self.applied.stop(seq);
        (self.close_adapters(|_| true, now), late)
    }

    /// What `GET /api/transcript/:token` answers: where the job stands, with the `progress` of
    /// the recorded audio, and `transcript.json` once the job is ready. `None` if the job is
    /// ready but `transcript` is missing, which it never is unless R2 lost the file.
    pub fn status(
        &self,
        progress: Progress,
        transcript: Option<Transcript>,
    ) -> Option<TranscriptStatus> {
        let meeting = self.meeting.clone();
        let started_at = self.started_at;
        let RecorderPhase::Ended { at: ended_at, job } = &self.phase else {
            return Some(TranscriptStatus::Recording {
                meeting,
                started_at,
                progress,
            });
        };
        let ended_at = *ended_at;
        let processing = |retry_at| TranscriptStatus::Processing {
            meeting: meeting.clone(),
            started_at,
            ended_at,
            progress,
            retry_at,
        };
        Some(match job {
            Job::Transcribing { retry_at } => processing(*retry_at),
            Job::Merging { .. } => processing(None),
            Job::Ready { audio_until, .. } => TranscriptStatus::Ready {
                meeting,
                started_at,
                ended_at,
                audio_until: *audio_until,
                transcript: transcript?,
            },
            Job::Failed { .. } => TranscriptStatus::Failed {
                meeting,
                started_at,
                ended_at,
            },
        })
    }
}

impl RecorderPhase {
    /// Whether the job got past writing the transcript: it is ready, or the job gave up.
    pub fn transcript_written(&self) -> bool {
        matches!(
            self,
            Self::Ended {
                job: Job::Ready { .. } | Job::Failed { .. },
                ..
            }
        )
    }
}

/// Why a speech request failed, as far as the job cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiFailure {
    /// The daily allowance is used up (on Workers Free, 10,000 neurons a day, reset at 00:00
    /// UTC): wait for the reset.
    Quota,
    /// Too many requests (HTTP 429): wait an hour.
    RateLimited,
    /// Anything else: retry the chunk a few times.
    Other,
}

/// Classifies a Workers AI error by its text, since the binding throws plain errors whose
/// shape is undocumented. The caller logs the text, so the classes can be refined.
pub fn classify_ai_error(text: &str) -> AiFailure {
    let text = text.to_ascii_lowercase();
    let says = |words: &[&str]| words.iter().any(|w| text.contains(w));
    if says(&[
        "neuron",
        "quota",
        "daily",
        "allocation",
        "free tier",
        "exceeded your",
    ]) {
        AiFailure::Quota
    } else if says(&["429", "rate limit", "too many requests"]) {
        AiFailure::RateLimited
    } else {
        AiFailure::Other
    }
}

const HOUR_MS: u64 = 60 * 60 * 1000;
const DAY_MS: u64 = 24 * HOUR_MS;

/// When to try again after `failure` at `now`: five minutes after the next daily reset at
/// 00:00 UTC for the quota, an hour later for a rate limit.
pub fn quota_retry(failure: AiFailure, now: Millis) -> Option<Millis> {
    match failure {
        AiFailure::Quota => Some(Millis((now.0 / DAY_MS + 1) * DAY_MS + 5 * 60 * 1000)),
        AiFailure::RateLimited => Some(now.after(HOUR_MS)),
        AiFailure::Other => None,
    }
}

/// What the job does with a chunk Workers AI refused for `failure` at `now`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Count the attempt; after [`MAX_CHUNK_ATTEMPTS`] the chunk is not transcribed.
    Attempt,
    /// Wait for the quota or the rate limit, without counting an attempt.
    WaitUntil(Millis),
    /// Stop waiting: the wait would end after the audio is to be deleted. Every chunk still
    /// waiting is given up as not transcribed, since the same quota holds them back.
    GiveUp,
}

/// What to do after Workers AI refused a chunk for `failure` at `now`, with the audio kept until
/// `audio_until` (the meeting's end plus [`AUDIO_RETENTION_MS`], D18). The job never waits past
/// it: the audio is deleted then, and before 2026-10-11 the job waited up to seven days while
/// the R2 lifecycle rule deleted the chunks under it after three.
pub fn on_refusal(failure: AiFailure, now: Millis, audio_until: Millis) -> Refusal {
    match quota_retry(failure, now) {
        None => Refusal::Attempt,
        Some(at) if at < audio_until => Refusal::WaitUntil(at),
        Some(_) => Refusal::GiveUp,
    }
}

/// Uploads of a chunk fail this often before it is given up as missing audio.
pub const MAX_UPLOAD_ATTEMPTS: u32 = 5;

/// The wait before the next upload after the `failures`-th failed one: 2 s, 10 s, then 60 s.
pub fn upload_backoff(failures: u32) -> u64 {
    match failures {
        0 | 1 => 2_000,
        2 => 10_000,
        _ => 60_000,
    }
}

/// The wait after the `failures`-th failure in a row: 30 s, doubling, at most 30 minutes.
pub fn backoff(failures: u32) -> u64 {
    (30_000u64 << failures.min(6)).min(30 * 60 * 1000)
}

/// Whether `given` is `secret`, comparing in constant time.
pub fn secret_matches(secret: &str, given: &str) -> bool {
    constant_time::eq(secret.as_bytes(), given.as_bytes())
}

/// The R2 key of a chunk. Audio and transcripts live under separate prefixes, so an R2
/// lifecycle rule on `audio/` can delete old audio without touching transcripts (D18).
pub fn chunk_key(recorder: &str, who: ParticipantId, generation: u32, index: u32) -> String {
    format!(
        "{}{who}/{generation}-{index}.ogg",
        audio_prefix(recorder),
        who = who.0
    )
}

/// The prefix of all of a recorder's audio.
pub fn audio_prefix(recorder: &str) -> String {
    format!("audio/{recorder}/")
}

/// The R2 key of a transcript file ([`crate::internal::TRANSCRIPT_MD`] or `..._JSON`).
pub fn transcript_file_key(recorder: &str, file: &str) -> String {
    format!("transcripts/{recorder}/{file}")
}

/// A chunk as the gap rule needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSpan {
    pub who: ParticipantId,
    pub generation: u32,
    pub start: Millis,
    pub duration_ms: u64,
    /// Why the transcript lacks this chunk, if it does: its upload or its transcription
    /// failed for good.
    pub lacking: Option<GapReason>,
}

/// A hole between two chunks of one adapter stream shorter than this is not reported.
pub const MIN_GAP_MS: u64 = 1_000;

/// The audio the transcript lacks: holes of at least [`MIN_GAP_MS`] between consecutive chunks
/// of one adapter stream, chunks that never reached R2 or the speech model failed on, and
/// stretches of at least [`MIN_GAP_MS`] without an adapter because creating it failed. Time
/// between two adapters of a speaker is not a gap: the speaker had stopped publishing.
pub fn gaps(mut chunks: Vec<ChunkSpan>, unrecorded: &[Unrecorded]) -> Vec<Gap> {
    chunks.sort_by_key(|c| (c.who, c.generation, c.start));
    let holes = chunks.windows(2).filter_map(|pair| {
        let [a, b] = pair else { return None };
        let end = a.start.after(a.duration_ms);
        (a.who == b.who && a.generation == b.generation && b.start.0 >= end.0 + MIN_GAP_MS)
            .then_some(Gap {
                who: a.who,
                start: end,
                end: b.start,
                reason: GapReason::Missing,
            })
    });
    let failed = chunks.iter().filter_map(|c| {
        Some(Gap {
            who: c.who,
            start: c.start,
            end: c.start.after(c.duration_ms),
            reason: c.lacking?,
        })
    });
    let unrecorded = unrecorded
        .iter()
        .filter(|u| u.end.0 >= u.start.0 + MIN_GAP_MS)
        .map(|u| Gap {
            who: u.who,
            start: u.start,
            end: u.end,
            reason: GapReason::Missing,
        });
    holes.chain(failed).chain(unrecorded).collect()
}

#[cfg(test)]
mod tests {
    use meeting_core::{DisplayName, Role};

    use super::*;
    use crate::record::tests::fixtures;

    #[test]
    fn quota_errors_wait_for_the_daily_reset_and_rate_limits_an_hour() {
        // 2026-10-09 22:00 UTC.
        let now = Millis(1_791_583_200_000);
        let next_reset = Millis(1_791_590_400_000 + 5 * 60 * 1000);
        for text in [
            "AiError: 4006: you have used up your daily free allocation of 10,000 neurons",
            "Quota exceeded",
        ] {
            assert_eq!(classify_ai_error(text), AiFailure::Quota, "{text}");
        }
        assert_eq!(quota_retry(AiFailure::Quota, now), Some(next_reset));
        assert_eq!(
            classify_ai_error("HTTP 429 Too Many Requests"),
            AiFailure::RateLimited
        );
        assert_eq!(
            quota_retry(AiFailure::RateLimited, now),
            Some(now.after(HOUR_MS))
        );
        assert_eq!(
            classify_ai_error("AiError: invalid audio"),
            AiFailure::Other
        );
        // An input limit is no reason to wait: retrying cannot help.
        assert_eq!(
            classify_ai_error("AiError: audio exceeds the size limit"),
            AiFailure::Other
        );
        assert_eq!(
            classify_ai_error("Capacity temporarily exceeded"),
            AiFailure::Other
        );
        assert_eq!(quota_retry(AiFailure::Other, now), None);
    }

    #[test]
    fn the_job_waits_for_workers_ai_only_while_the_audio_is_kept() {
        // 2026-10-09 22:00 UTC; the quota resets at 00:05 UTC.
        let now = Millis(1_791_583_200_000);
        let reset = quota_retry(AiFailure::Quota, now).unwrap();
        let ended = now.after(HOUR_MS);
        let audio_until = ended.after(AUDIO_RETENTION_MS);
        assert_eq!(
            on_refusal(AiFailure::Quota, now, audio_until),
            Refusal::WaitUntil(reset)
        );
        assert_eq!(
            on_refusal(AiFailure::RateLimited, now, audio_until),
            Refusal::WaitUntil(now.after(HOUR_MS))
        );
        assert_eq!(
            on_refusal(AiFailure::Other, now, audio_until),
            Refusal::Attempt
        );
        // Shortly before the audio goes, waiting for the next reset would outlast it.
        let late = audio_until.0 - HOUR_MS;
        assert_eq!(
            on_refusal(AiFailure::Quota, Millis(late), audio_until),
            Refusal::GiveUp
        );
        assert_eq!(
            on_refusal(AiFailure::RateLimited, Millis(late + 1), audio_until),
            Refusal::GiveUp
        );
    }

    #[test]
    fn uploads_back_off_from_2_s_to_a_minute() {
        let waits: Vec<u64> = (1..=MAX_UPLOAD_ATTEMPTS).map(upload_backoff).collect();
        assert_eq!(waits, [2_000, 10_000, 60_000, 60_000, 60_000]);
    }

    #[test]
    fn backoff_doubles_up_to_half_an_hour() {
        assert_eq!(backoff(0), 30_000);
        assert_eq!(backoff(1), 60_000);
        assert_eq!(backoff(20), 30 * 60 * 1000);
    }

    #[test]
    fn secrets_compare_whole() {
        assert!(secret_matches("abc", "abc"));
        assert!(!secret_matches("abc", "abd"));
        assert!(!secret_matches("abc", "ab"));
        assert!(!secret_matches("abc", ""));
    }

    #[test]
    fn audio_and_transcripts_have_separate_prefixes() {
        let key = chunk_key("r1", ParticipantId(3), 1, 12);
        assert_eq!(key, "audio/r1/3/1-12.ogg");
        assert!(key.starts_with(&audio_prefix("r1")));
        assert_eq!(
            transcript_file_key("r1", "transcript.md"),
            "transcripts/r1/transcript.md"
        );
    }

    fn span(who: u32, generation: u32, start: u64, duration_ms: u64) -> ChunkSpan {
        ChunkSpan {
            who: ParticipantId(who),
            generation,
            start: Millis(start),
            duration_ms,
            lacking: None,
        }
    }

    #[test]
    fn gaps_are_holes_within_one_adapter_stream_and_failed_chunks() {
        let mut failed = span(2, 0, 0, 5_000);
        failed.lacking = Some(GapReason::Untranscribed);
        let unrecorded = |who, start, end| Unrecorded {
            who: ParticipantId(who),
            start: Millis(start),
            end: Millis(end),
        };
        let found = gaps(
            vec![
                span(1, 0, 45_000, 10_000),
                span(1, 0, 0, 30_000),
                span(1, 0, 30_500, 9_000), // 500 ms hole: not reported
                span(1, 1, 90_000, 1_000), // a new adapter: no gap
                failed,
            ],
            &[
                unrecorded(3, 2_000, 62_000),
                unrecorded(3, 70_000, 70_600), // under a second: not reported
            ],
        );
        assert_eq!(
            found,
            [
                Gap {
                    who: ParticipantId(1),
                    start: Millis(39_500),
                    end: Millis(45_000),
                    reason: GapReason::Missing
                },
                Gap {
                    who: ParticipantId(2),
                    start: Millis(0),
                    end: Millis(5_000),
                    reason: GapReason::Untranscribed
                },
                Gap {
                    who: ParticipantId(3),
                    start: Millis(2_000),
                    end: Millis(62_000),
                    reason: GapReason::Missing
                },
            ]
        );
    }

    #[test]
    fn adapters_count_generations_per_speaker() {
        let mut recorder = Recorder {
            meeting: MeetingId::from_random([0, 0, 0]),
            started_at: Millis(0),
            secret: "s".into(),
            speakers: Vec::new(),
            consents: Vec::new(),
            adapters: Vec::new(),
            unrecorded: Vec::new(),
            phase: RecorderPhase::Recording,
            applied: Applied::default(),
        };
        let who = ParticipantId(4);
        assert_eq!(recorder.next_generation(who), 0);
        recorder.adapters.push(Adapter {
            who,
            generation: 0,
            session: SessionId::new("s"),
            track_name: TrackName::new("t"),
            creation: Creation::Asked,
            open: false,
        });
        assert_eq!(recorder.next_generation(who), 1);
        assert_eq!(recorder.open_adapter(who), None);
    }

    fn entry(who: u32, name: &str) -> RosterEntry {
        RosterEntry {
            id: ParticipantId(who),
            name: DisplayName::parse(name).unwrap(),
            role: Role::Guest,
        }
    }

    #[test]
    fn the_room_s_end_replaces_the_names_and_consents_and_is_its_last_word() {
        let mut recorder = empty_recorder();
        let speaker = |name, withdrawn| Learned::Speaker {
            entry: entry(0, name),
            consent: consent(ParticipantId(0), withdrawn),
        };
        // While recording, each request replaces the speaker's entry and consent.
        assert!(recorder.learn(speaker("Ana", false)));
        assert!(recorder.learn(speaker("Anna", false)));
        assert!(recorder.learn(Learned::Consent(consent(ParticipantId(0), true))));
        assert_eq!(recorder.speakers, [entry(0, "Anna")]);
        assert_eq!(recorder.consents, [consent(ParticipantId(0), true)]);
        // The recorder ended a meeting nobody ended; the room's `/end` still arrives.
        recorder.phase = RecorderPhase::Ended {
            at: Millis(5),
            job: Job::Transcribing { retry_at: None },
        };
        let end = FinishRecording {
            roster: vec![entry(0, "Anna"), entry(1, "Bo")],
            consents: vec![consent(ParticipantId(0), false)],
        };
        assert!(recorder.learn(Learned::Everyone(end.clone())));
        assert_eq!(recorder.speakers, end.roster);
        assert_eq!(recorder.consents, end.consents);
        // A per-speaker request that arrives after the end is older than it.
        assert!(!recorder.learn(Learned::Consent(consent(ParticipantId(0), true))));
        assert!(!recorder.learn(speaker("Ann", true)));
        assert_eq!(recorder.consents, end.consents);
        // Once the transcript is written, nothing changes it.
        recorder.phase = RecorderPhase::Ended {
            at: Millis(5),
            job: Job::Failed { at: Millis(6) },
        };
        assert!(!recorder.learn(Learned::Everyone(FinishRecording {
            roster: Vec::new(),
            consents: Vec::new(),
        })));
        assert_eq!(recorder.speakers, end.roster);
    }

    /// A request from the room, as the recorder receives it.
    #[derive(Debug)]
    enum Request {
        Track(TrackRecording),
        Untrack(UntrackRecording),
        Stop,
    }

    fn consent(who: ParticipantId, withdrawn: bool) -> ConsentRecord {
        ConsentRecord {
            who,
            name: DisplayName::parse("Ada").unwrap(),
            at: Millis(0),
            notice_version: 1,
            withdrawn_at: withdrawn.then_some(Millis(1)),
        }
    }

    fn track(who: u32, session: &str, name: &str) -> Request {
        let who = ParticipantId(who);
        Request::Track(TrackRecording {
            who,
            name: DisplayName::parse("Ada").unwrap(),
            role: Role::Guest,
            consent: consent(who, false),
            session: SessionId::new(session),
            track_name: TrackName::new(name),
        })
    }

    fn untrack(who: u32, session: &str, name: &str) -> Request {
        let who = ParticipantId(who);
        Request::Untrack(UntrackRecording {
            who,
            session: SessionId::new(session),
            track_name: TrackName::new(name),
            consent: Some(consent(who, true)),
        })
    }

    fn empty_recorder() -> Recorder {
        Recorder {
            meeting: MeetingId::from_random([0, 0, 0]),
            started_at: Millis(0),
            secret: "s".into(),
            speakers: Vec::new(),
            consents: Vec::new(),
            adapters: Vec::new(),
            unrecorded: Vec::new(),
            phase: RecorderPhase::Recording,
            applied: Applied::default(),
        }
    }

    /// Applies `(seq, request)` the way `RecorderObject` does, with adapters created at once.
    fn apply(recorder: &mut Recorder, (seq, request): &(RequestSeq, Request)) {
        match request {
            Request::Track(track) => {
                recorder.begin_track(track, *seq, Millis(0));
            }
            Request::Untrack(untrack) => {
                recorder.untrack(untrack, *seq, Millis(0));
            }
            Request::Stop => {
                recorder.stop(*seq, Millis(0));
            }
        }
    }

    /// The tracks the recorder records: its open adapters.
    fn recorded(recorder: &Recorder) -> Vec<(ParticipantId, String)> {
        let mut open: Vec<_> = recorder
            .adapters
            .iter()
            .filter(|a| a.open)
            .map(|a| (a.who, a.track_name.as_str().to_owned()))
            .collect();
        open.sort();
        open
    }

    /// Every order of `items`.
    fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        (0..items.len())
            .flat_map(|i| {
                let mut rest = items.to_vec();
                let first = rest.remove(i);
                permutations(&rest).into_iter().map(move |mut tail| {
                    tail.insert(0, first);
                    tail
                })
            })
            .collect()
    }

    #[test]
    fn a_track_older_than_a_stop_or_untrack_that_arrived_first_records_nothing() {
        // Recording starts with speaker 0's track (1) and stops (2); the stop arrives first.
        let mut recorder = empty_recorder();
        recorder.stop(RequestSeq(2), Millis(0));
        let Request::Track(late) = track(0, "p0", "a0") else {
            unreachable!()
        };
        assert_eq!(
            recorder.begin_track(&late, RequestSeq(1), Millis(0)),
            TrackPlan::Stale
        );
        assert_eq!(recorder, {
            let mut stopped = empty_recorder();
            stopped.applied.stop = RequestSeq(2);
            stopped
        });
        // Speaker 0 withdraws consent (4) after their track (3); the untrack arrives first.
        let mut recorder = empty_recorder();
        let Request::Untrack(withdrawn) = untrack(0, "p0", "a0") else {
            unreachable!()
        };
        assert_eq!(
            recorder.untrack(&withdrawn, RequestSeq(4), Millis(0)),
            Some(Vec::new())
        );
        assert_eq!(
            recorder.begin_track(&late, RequestSeq(3), Millis(0)),
            TrackPlan::Stale
        );
        assert!(recorded(&recorder).is_empty());
        assert_eq!(recorder.consents, [consent(ParticipantId(0), true)]);
    }

    #[test]
    fn the_recorder_never_records_more_than_in_order_and_as_much_unless_a_stop_is_overtaken() {
        // What a room sends: speakers 0 and 1 are recorded; 0 withdraws consent; recording
        // stops and starts again with 0, who consented again, and 1, who republished; then 1
        // unpublishes.
        let first = RequestSeq::starting_at(Millis(1_791_583_200_000));
        let history: Vec<(RequestSeq, Request)> = [
            track(0, "p0", "a0"),
            track(1, "p1", "a1"),
            untrack(0, "p0", "a0"),
            Request::Stop,
            track(0, "p0", "a0"),
            track(1, "p1b", "a1b"),
            untrack(1, "p1b", "a1b"),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, request)| (first.after(i), request))
        .collect();
        let stop = history
            .iter()
            .position(|(_, r)| matches!(r, Request::Stop))
            .expect("a stop");
        let mut in_order = empty_recorder();
        for request in &history {
            apply(&mut in_order, request);
        }
        let expected = recorded(&in_order);
        assert_eq!(expected, [(ParticipantId(0), "a0".to_owned())]);
        let indices: Vec<usize> = (0..history.len()).collect();
        let mut equal = 0;
        for arrival in permutations(&indices) {
            let mut recorder = empty_recorder();
            for &i in &arrival {
                apply(&mut recorder, &history[i]);
            }
            let got = recorded(&recorder);
            // D15: nothing is recorded that in-order delivery would not record.
            assert!(
                got.iter().all(|track| expected.contains(track)),
                "arrival {arrival:?} records {got:?}"
            );
            // A request numbered after the stop that arrives before it is closed by it: the
            // safe mistake. Otherwise the result is the in-order one.
            let at = |i| arrival.iter().position(|&j| j == i).expect("in arrival");
            let overtaken = (stop + 1..history.len()).any(|later| at(later) < at(stop));
            if !overtaken {
                assert_eq!(got, expected, "arrival {arrival:?}");
                equal += 1;
            }
        }
        assert!(equal > 0);
    }

    #[test]
    fn adapter_calls_back_off_from_1_s_to_a_minute_and_stop_after_ten() {
        let waits: Vec<u64> = (1..MAX_ADAPTER_CALLS).filter_map(adapter_retry).collect();
        assert_eq!(
            waits,
            [
                1_000, 2_000, 5_000, 10_000, 30_000, 60_000, 60_000, 60_000, 60_000
            ]
        );
        assert_eq!(adapter_retry(MAX_ADAPTER_CALLS), None);
    }

    /// A recording recorder with speaker 0's track asked for at `Millis(0)`.
    fn asked_for_one_track() -> (Recorder, (ParticipantId, u32)) {
        let Request::Track(request) = track(0, "p0", "a0") else {
            unreachable!()
        };
        let mut recorder = empty_recorder();
        let TrackPlan::New { adapter, .. } =
            recorder.begin_track(&request, RequestSeq(1), Millis(0))
        else {
            panic!("a new adapter")
        };
        (recorder, (adapter.who, adapter.generation))
    }

    #[test]
    fn a_failed_adapter_is_called_for_again_and_the_audio_it_missed_is_a_gap() {
        let (mut recorder, key) = asked_for_one_track();
        assert_eq!(
            recorder.to_create(key, Millis(0)).map(|a| a.generation),
            Some(0)
        );
        assert_eq!(
            recorder.settle(key, Err(()), Millis(100)),
            Settled::RetryAt(Millis(1_100))
        );
        // The failed call's generation is closed; the next call has its own.
        let next = (ParticipantId(0), 1);
        assert_eq!(recorder.to_create(key, Millis(1_100)), None);
        assert_eq!(recorder.next_adapter_call(), Some(Millis(1_100)));
        assert!(recorder.adapters_due(Millis(1_099)).is_empty());
        assert_eq!(recorder.to_create(next, Millis(1_099)), None);
        assert_eq!(recorder.adapters_due(Millis(1_100)), [next]);
        assert_eq!(
            recorder.settle(next, Err(()), Millis(1_100)),
            Settled::RetryAt(Millis(3_100))
        );
        let last = (ParticipantId(0), 2);
        let id = AdapterId::new("adapter-0");
        assert_eq!(
            recorder.settle(last, Ok(id.clone()), Millis(3_100)),
            Settled::Created
        );
        assert_eq!(recorder.adapters[2].id(), Some(&id));
        assert_eq!(recorder.to_create(last, Millis(3_100)), None);
        assert_eq!(recorder.next_adapter_call(), None);
        // Only the generation that exists is open, so a duplicate the SFU made for an earlier
        // call is refused at the upgrade.
        let open: Vec<u32> = recorder
            .adapters
            .iter()
            .filter(|a| a.open)
            .map(|a| a.generation)
            .collect();
        assert_eq!(open, [2]);
        // The speaker was not recorded from the first failure until the adapter existed.
        assert_eq!(
            recorder.unrecorded,
            [Unrecorded {
                who: ParticipantId(0),
                start: Millis(100),
                end: Millis(3_100),
            }]
        );
    }

    #[test]
    fn a_given_up_adapter_misses_audio_until_it_closes_and_the_same_track_asks_again() {
        let (mut recorder, _) = asked_for_one_track();
        let mut now = Millis(100);
        let mut last = Settled::Moot;
        for generation in 0..MAX_ADAPTER_CALLS {
            last = recorder.settle((ParticipantId(0), generation), Err(()), now);
            now = now.after(60_000);
        }
        assert_eq!(last, Settled::GaveUp);
        assert!(recorder.adapters_due(now).is_empty());
        assert!(recorder.unrecorded.is_empty(), "still open");
        // The room sends the same track again (consent given again, recording restarted):
        // the recorder asks for a new adapter instead of keeping the given-up one.
        let Request::Track(again) = track(0, "p0", "a0") else {
            unreachable!()
        };
        let TrackPlan::New { adapter, replaced } = recorder.begin_track(&again, RequestSeq(2), now)
        else {
            panic!("a new adapter")
        };
        assert_eq!((adapter.generation, replaced.len()), (11, 1));
        assert_eq!(
            recorder.unrecorded,
            [Unrecorded {
                who: ParticipantId(0),
                start: Millis(100),
                end: now,
            }]
        );
    }

    #[test]
    fn a_retry_due_while_a_stop_arrived_calls_nothing() {
        let (mut recorder, key) = asked_for_one_track();
        recorder.settle(key, Err(()), Millis(100));
        let next = (ParticipantId(0), 1);
        // The alarm found the retry due, then a `/stop` arrived during another call.
        assert_eq!(recorder.adapters_due(Millis(1_100)), [next]);
        recorder.stop(RequestSeq(2), Millis(1_050));
        assert_eq!(recorder.to_create(next, Millis(1_100)), None);
        assert_eq!(recorder.unrecorded.len(), 1);
    }

    #[test]
    fn a_job_step_stored_on_the_record_as_it_is_now_keeps_an_end_that_arrived_meanwhile() {
        // A meeting the recorder ended itself; a transcribe step loads it and awaits Whisper.
        let mut stored = empty_recorder();
        stored.phase = RecorderPhase::Ended {
            at: Millis(5),
            job: Job::Transcribing { retry_at: None },
        };
        let loaded_by_the_step = stored.clone();
        // Meanwhile the room's `/end` arrives and is stored.
        let end = FinishRecording {
            roster: vec![entry(0, "Ada")],
            consents: vec![consent(ParticipantId(0), false)],
        };
        assert!(stored.learn(Learned::Everyone(end.clone())));
        // The step stores its job on the record as it is now, not on what it loaded.
        let job = Job::Merging {
            after: None,
            failures: 0,
        };
        stored.set_job(job.clone());
        assert_eq!(stored.speakers, end.roster);
        assert_eq!(stored.consents, end.consents);
        assert_eq!(stored.phase, RecorderPhase::Ended { at: Millis(5), job });
        assert!(
            loaded_by_the_step.speakers.is_empty(),
            "what the old code saved"
        );
    }

    #[test]
    fn a_wait_stored_by_older_code_ends_when_the_audio_is_due_to_go() {
        let (_, json) = fixtures::read("recorder", "older")
            .into_iter()
            .find(|(name, _)| name == "v2-transcribing-with-waiting-since.json")
            .expect("the fixture");
        let recorder = decode(&json).unwrap();
        let RecorderPhase::Ended { at, job } = recorder.phase else {
            panic!("ended")
        };
        let Job::Transcribing {
            retry_at: Some(retry_at),
        } = job
        else {
            panic!("waiting")
        };
        let audio_until = at.after(AUDIO_RETENTION_MS);
        assert_eq!(job.step(at, at), JobStep::Wait(retry_at));
        assert_eq!(job.step(at, retry_at), JobStep::Transcribe { audio_until });
        // A week-long wait from before 2026-10-11 ends when the audio is due to go.
        let week = Job::Transcribing {
            retry_at: Some(at.after(7 * DAY_MS)),
        };
        assert_eq!(week.step(at, retry_at), JobStep::Wait(audio_until));
        assert_eq!(
            week.step(at, audio_until),
            JobStep::Transcribe { audio_until }
        );
    }

    #[test]
    fn a_call_that_returns_after_the_adapter_closed_changes_nothing_but_the_sfu() {
        let (mut recorder, key) = asked_for_one_track();
        recorder.stop(RequestSeq(2), Millis(50));
        assert_eq!(recorder.settle(key, Err(()), Millis(60)), Settled::Moot);
        assert_eq!(recorder.next_adapter_call(), None);
        let Settled::Orphaned(adapter) =
            recorder.settle(key, Ok(AdapterId::new("late")), Millis(70))
        else {
            panic!("orphaned")
        };
        assert_eq!(adapter.id(), Some(&AdapterId::new("late")));
        assert!(recorder.unrecorded.is_empty());
        assert_eq!(
            recorder.settle((ParticipantId(9), 0), Err(()), Millis(80)),
            Settled::Moot
        );
    }

    #[test]
    fn a_stop_closes_every_adapter_whatever_its_number() {
        // A room whose counter restarted below the recorder's (older code dropped it): its
        // stop still closes what the newer numbers opened.
        let Request::Track(track) = track(0, "p0", "a0") else {
            unreachable!()
        };
        let mut recorder = empty_recorder();
        assert!(matches!(
            recorder.begin_track(&track, RequestSeq(9_000), Millis(0)),
            TrackPlan::New { .. }
        ));
        let (closed, late) = recorder.stop(RequestSeq(5), Millis(0));
        assert_eq!(closed.len(), 1);
        assert_eq!(late, [ParticipantId(0)]);
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn a_track_that_overtook_the_start_is_applied_unless_a_later_request_superseded_it() {
        let ordered = |seq, request| match request {
            Request::Track(request) => Ordered {
                seq: RequestSeq(seq),
                request,
            },
            _ => unreachable!(),
        };
        // Two tracks of speaker 0 and one of speaker 1 overtook the start; 1 then withdrew.
        let mut before = BeforeStart::default();
        before.track(ordered(2, track(0, "p0", "a0")));
        before.track(ordered(4, track(0, "p0", "a0b")));
        before.track(ordered(3, track(1, "p1", "a1")));
        assert!(before.applied.untrack(ParticipantId(1), RequestSeq(5)));
        let (applied, tracks) = before.tracks();
        assert_eq!(
            tracks.iter().map(|t| t.seq.0).collect::<Vec<_>>(),
            [4, 3, 2],
            "newest first"
        );
        let mut recorder = Recorder {
            applied,
            ..empty_recorder()
        };
        for track in &tracks {
            recorder.begin_track(&track.request, track.seq, Millis(0));
        }
        assert_eq!(recorded(&recorder), [(ParticipantId(0), "a0b".to_owned())]);
    }

    #[test]
    fn an_untrack_or_stop_that_overtook_the_start_still_drops_the_older_track() {
        let Request::Track(late) = track(0, "p0", "a0") else {
            unreachable!()
        };
        // `/start` (1) and `/track` (2) from one step; `/untrack` (3) of a later step arrives
        // before the record exists, so the recorder notes it on its own.
        let mut before_start = Applied::default();
        assert!(before_start.untrack(ParticipantId(0), RequestSeq(3)));
        let mut recorder = Recorder {
            applied: before_start,
            ..empty_recorder()
        };
        assert_eq!(
            recorder.begin_track(&late, RequestSeq(2), Millis(0)),
            TrackPlan::Stale
        );
        // The same for a `/stop` (3).
        let mut before_start = Applied::default();
        before_start.stop(RequestSeq(3));
        let mut recorder = Recorder {
            applied: before_start,
            ..empty_recorder()
        };
        assert_eq!(
            recorder.begin_track(&late, RequestSeq(2), Millis(0)),
            TrackPlan::Stale
        );
        assert!(recorded(&recorder).is_empty());
    }

    #[test]
    fn requests_without_a_number_count_as_older_than_numbered_ones() {
        let Request::Track(track) = track(0, "p0", "a0") else {
            unreachable!()
        };
        // A room that ran older code: its unnumbered requests apply as they arrive.
        let mut recorder = empty_recorder();
        assert!(matches!(
            recorder.begin_track(&track, RequestSeq::UNORDERED, Millis(0)),
            TrackPlan::New { .. }
        ));
        assert_eq!(recorder.stop(RequestSeq::UNORDERED, Millis(0)).0.len(), 1);
        // Once a numbered request was applied, an unnumbered one is stale.
        recorder.stop(RequestSeq(1), Millis(0));
        assert_eq!(
            recorder.begin_track(&track, RequestSeq::UNORDERED, Millis(0)),
            TrackPlan::Stale
        );
    }

    #[test]
    fn the_rooms_requests_carry_their_number_beside_their_fields() {
        let Request::Track(track) = track(0, "p0", "a0") else {
            unreachable!()
        };
        let json = serde_json::to_value(Ordered {
            seq: RequestSeq(5),
            request: track,
        })
        .unwrap();
        assert_eq!(json["seq"], 5);
        assert_eq!(json["trackName"], "a0");
        // A body without a number, as a room on older code posts it.
        let mut old = json;
        old.as_object_mut().unwrap().remove("seq");
        let parsed: Ordered<TrackRecording> = serde_json::from_value(old).unwrap();
        assert_eq!(parsed.seq, RequestSeq::UNORDERED);
        let stop = serde_json::to_string(&Ordered {
            seq: RequestSeq(6),
            request: Stop {},
        })
        .unwrap();
        assert_eq!(stop, r#"{"seq":6}"#);
        let old_stop: Option<Ordered<Stop>> = serde_json::from_str("null").unwrap();
        assert!(old_stop.is_none());
    }

    /// What the recorder serves in every phase is a `TranscriptStatus` the web app reads: the
    /// job's state, the meeting's times, and the transcript as the job writes it.
    #[test]
    fn the_status_of_every_phase_reads_as_a_transcript_status() {
        let progress = Progress {
            done: 30_000,
            total: 60_000,
        };
        for (name, recorder) in fixture_recorders() {
            let ended_at = match recorder.phase {
                RecorderPhase::Ended { at, .. } => Some(at),
                RecorderPhase::Recording => None,
            };
            let transcript = ended_at.map(|ended_at| {
                Transcript::new(
                    recorder.meeting.clone(),
                    recorder.started_at,
                    ended_at,
                    meeting_core::recording::label_speakers(&recorder.speakers),
                    recorder.consents.clone(),
                    Vec::new(),
                    Vec::new(),
                )
            });
            let status = recorder
                .status(progress, transcript.clone())
                .expect("a status");
            let served = serde_json::to_string(&status).expect("serializable");
            let read: TranscriptStatus = serde_json::from_str(&served).expect(name);
            assert_eq!(read, status, "{name}");
            let tag = serde_json::from_str::<serde_json::Value>(&served).expect("JSON")["status"]
                .as_str()
                .map(str::to_owned);
            let expected = match name {
                "recording" | "retrying" => "recording",
                "transcribing" | "merging" => "processing",
                other => other,
            };
            assert_eq!(tag.as_deref(), Some(expected), "{name}");
            if let TranscriptStatus::Ready {
                transcript: served, ..
            } = read
            {
                assert_eq!(Some(served), transcript);
            }
        }
        let ready = fixture_recorders()
            .into_iter()
            .find(|(name, _)| *name == "ready")
            .expect("a ready fixture")
            .1;
        assert_eq!(ready.status(progress, None), None, "ready without its file");
    }

    /// Recorders the fixtures freeze: one per phase and job step, with speakers, a withdrawn
    /// consent, and adapters open, closed and being created; and one whose adapters failed to
    /// be created, with the stretches they left unrecorded.
    fn fixture_recorders() -> Vec<(&'static str, Recorder)> {
        let (ada, bo) = (ParticipantId(0), ParticipantId(1));
        let recording = Recorder {
            meeting: MeetingId::from_random([1, 2, 3]),
            started_at: Millis(1_791_583_200_000),
            secret: "00112233445566778899aabbccddeeff".into(),
            speakers: vec![
                RosterEntry {
                    id: ada,
                    name: DisplayName::parse("Ada").unwrap(),
                    role: Role::Host,
                },
                RosterEntry {
                    id: bo,
                    name: DisplayName::parse("Bo").unwrap(),
                    role: Role::Guest,
                },
            ],
            consents: vec![
                ConsentRecord {
                    who: ada,
                    name: DisplayName::parse("Ada").unwrap(),
                    at: Millis(1_791_583_200_000),
                    notice_version: 1,
                    withdrawn_at: None,
                },
                ConsentRecord {
                    who: bo,
                    name: DisplayName::parse("Bo").unwrap(),
                    at: Millis(1_791_583_260_000),
                    notice_version: 1,
                    withdrawn_at: Some(Millis(1_791_583_500_000)),
                },
            ],
            adapters: vec![
                Adapter {
                    who: ada,
                    generation: 0,
                    session: SessionId::new("publish-0"),
                    track_name: TrackName::new("audio-0"),
                    creation: Creation::Created(AdapterId::new("adapter-0")),
                    open: true,
                },
                Adapter {
                    who: bo,
                    generation: 0,
                    session: SessionId::new("publish-1"),
                    track_name: TrackName::new("audio-1"),
                    creation: Creation::Created(AdapterId::new("adapter-1")),
                    open: false,
                },
                Adapter {
                    who: bo,
                    generation: 1,
                    session: SessionId::new("publish-1b"),
                    track_name: TrackName::new("audio-1b"),
                    creation: Creation::Asked,
                    open: true,
                },
            ],
            unrecorded: Vec::new(),
            phase: RecorderPhase::Recording,
            applied: Applied {
                stop: RequestSeq(4),
                speakers: vec![(ada, RequestSeq(3)), (bo, RequestSeq(7))],
            },
        };
        let since = Millis(1_791_583_300_000);
        let failing = |failures, retry_at| {
            Creation::Failing(Failing {
                since,
                failures,
                retry_at,
            })
        };
        let mut retrying = recording.clone();
        retrying.adapters[0].creation = failing(2, Some(since.after(3_000)));
        retrying.adapters[1].creation = failing(MAX_ADAPTER_CALLS, None);
        retrying.adapters[2].creation = Creation::Created(AdapterId::new("adapter-1b"));
        retrying.unrecorded = vec![Unrecorded {
            who: bo,
            start: since,
            end: since.after(290_000),
        }];
        let at = Millis(1_791_586_800_000);
        let ended = |job| Recorder {
            phase: RecorderPhase::Ended { at, job },
            ..recording.clone()
        };
        vec![
            ("recording", recording.clone()),
            ("retrying", retrying),
            (
                "transcribing",
                ended(Job::Transcribing {
                    retry_at: Some(at.after(HOUR_MS)),
                }),
            ),
            (
                "merging",
                ended(Job::Merging {
                    after: Some(SegmentCursor {
                        start_ms: 1_791_583_300_000,
                        id: 42,
                    }),
                    failures: 1,
                }),
            ),
            (
                "ready",
                ended(Job::Ready {
                    audio_until: at.after(AUDIO_RETENTION_MS),
                    audio_deleted_at: Some(at.after(AUDIO_RETENTION_MS + 60_000)),
                }),
            ),
            (
                "failed",
                ended(Job::Failed {
                    at: at.after(HOUR_MS),
                }),
            ),
        ]
    }

    /// Writes the `current/` fixtures from [`fixture_recorders`]; see
    /// `crate::record::tests::fixtures` for when.
    #[test]
    #[ignore = "writes the fixtures of the current shape; run it only after a shape change"]
    fn capture_recorder_fixtures() {
        for (name, recorder) in fixture_recorders() {
            fixtures::capture(
                "recorder",
                &format!("{name}.json"),
                &encode(&recorder).unwrap(),
            );
        }
    }

    #[test]
    fn the_current_code_stores_the_frozen_recorders_unchanged() {
        for (name, json) in fixtures::read("recorder", "current") {
            assert_eq!(
                stored_version(&json, RECORDER_VERSION).unwrap(),
                RECORDER_VERSION,
                "{name}"
            );
            let recorder = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(
                fixtures::same_json(&json, &encode(&recorder).unwrap()),
                "{name} no longer stores as frozen: the stored shape changed"
            );
        }
    }

    #[test]
    fn every_recorder_deployed_code_stored_still_loads() {
        let built = fixture_recorders();
        for (name, json) in fixtures::read("recorder", "older") {
            let version = fixtures::version_in_name(&name);
            assert_eq!(stored_version(&json, RECORDER_VERSION).unwrap(), version);
            let recorder = decode(&json).unwrap_or_else(|e| panic!("{name}: {e}"));
            // The unversioned records were captured from these scenarios, before the recorder
            // kept the requests it applied.
            if let Some((_, expected)) = built.iter().find(|(n, _)| format!("v1-{n}.json") == name)
            {
                let before_applied = Recorder {
                    applied: Applied::default(),
                    ..expected.clone()
                };
                assert_eq!(recorder, before_applied, "{name}");
            }
        }
    }

    #[test]
    fn a_recorder_from_newer_code_is_refused() {
        assert!(matches!(
            decode(r#"{"version":3,"record":{}}"#),
            Err(DecodeError::Newer {
                found: 3,
                supported: RECORDER_VERSION
            })
        ));
    }
}
