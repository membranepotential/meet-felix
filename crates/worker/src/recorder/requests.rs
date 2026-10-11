//! The room's requests (`/start`, `/track`, `/untrack`, `/stop`, `/end`) and the housekeeping
//! while the meeting is recorded. The rules for which request applies are pure, in
//! [`super::state::Recorder`]; this file loads the record, applies them, and carries out what
//! they decide.

use std::rc::Rc;

use meeting_core::Millis;
use worker::{Response, Result, console_error, console_log, console_warn};

use super::state::{
    self, BeforeStart, Job, Ordered, Recorder, RecorderPhase, RequestSeq, TrackPlan,
};
use super::streams::StreamKey;
use super::{Inner, no_content, not_found};
use crate::internal::{FinishRecording, StartRecording, TrackRecording, UntrackRecording};
use crate::{now, random};

/// How often the alarm looks after a recording meeting.
pub(super) const HOUSEKEEPING_MS: u64 = 5 * 60 * 1000;
/// A stream without messages this long has its chunk closed; new messages start a new one.
const IDLE_STREAM_MS: u64 = 30 * 1000;
/// A recorder that heard nothing this long without an `/end` treats the meeting as over.
const ABANDONED_MS: u64 = 6 * 60 * 60 * 1000;

impl Inner {
    /// Creates the record on the first `/start`, with the requests that overtook it
    /// ([`state::BeforeStart`]); later calls change nothing.
    pub(super) async fn start(self: &Rc<Self>, start: StartRecording) -> Result<Response> {
        if self.load().await?.is_none() {
            let (applied, tracks) = self.before_start().await?.tracks();
            let recorder = Recorder {
                meeting: start.meeting,
                started_at: start.since,
                secret: random::secret()?,
                speakers: Vec::new(),
                consents: Vec::new(),
                adapters: Vec::new(),
                phase: RecorderPhase::Recording,
                applied,
            };
            self.save(&recorder).await?;
            self.state.storage().delete(state::BEFORE_START_KEY).await?;
            self.arm(now().after(HOUSEKEEPING_MS)).await?;
            console_log!("recorder {}: recording started", self.id());
            // Logged, not returned: the start itself succeeded.
            for track in tracks {
                let who = track.request.who;
                if let Err(e) = self.track(track).await {
                    console_error!("recorder: a /track for {who:?} kept before /start failed: {e}");
                }
            }
        }
        no_content()
    }

    /// Records a speaker's audio track: creates an adapter for it, after closing the speaker's
    /// previous one if the track changed (a republish or a replaced session). A `/track` older
    /// than a `/stop` or `/untrack` already applied changes nothing ([`Recorder::begin_track`]).
    pub(super) async fn track(self: &Rc<Self>, track: Ordered<TrackRecording>) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return self.note_before_start(|before| before.track(track)).await;
        };
        let Ordered {
            seq,
            request: track,
        } = track;
        if recorder.phase != RecorderPhase::Recording {
            return Response::error("the meeting ended", 409);
        }
        let who = track.who;
        let (generation, replaced) = match recorder.begin_track(&track, seq) {
            TrackPlan::Stale => {
                console_log!("recorder: dropped a /track for {who:?} older than a later request");
                return no_content();
            }
            TrackPlan::Unchanged => {
                self.save(&recorder).await?;
                return no_content();
            }
            TrackPlan::New {
                generation,
                replaced,
            } => (generation, replaced),
        };
        // Stored before the SFU call, so the adapter's socket is accepted the moment it opens.
        self.save(&recorder).await?;
        self.end_streams(&replaced);
        self.close_sfu_adapters(&replaced).await;

        let key: StreamKey = (who, generation);
        let created = match self.endpoint(&recorder.secret, key) {
            Ok(endpoint) => {
                self.create_adapter(track.session, track.track_name, endpoint)
                    .await
            }
            Err(e) => Err(e),
        };
        // Reloaded: the record may have changed while the SFU call was out.
        let mut recorder = self.load().await?.unwrap_or(recorder);
        let adapter = recorder
            .adapters
            .iter_mut()
            .find(|a| a.who == who && a.generation == generation);
        let mut orphaned = None;
        let response = match (created, adapter) {
            (Ok(id), Some(adapter)) => {
                adapter.id = Some(id);
                if !adapter.open {
                    // Closed (an untrack, a stop, the end) while the SFU created it.
                    orphaned = Some(adapter.clone());
                }
                no_content()
            }
            (Err(e), Some(adapter)) => {
                adapter.open = false;
                console_error!("recorder: no adapter for {who:?} generation {generation}: {e}");
                Response::error("could not create the adapter", 502)
            }
            (_, None) => Response::error("adapter vanished", 409),
        };
        self.save(&recorder).await?;
        if let Some(adapter) = orphaned {
            self.close_sfu_adapters(&[adapter]).await;
        }
        response
    }

    /// The requests noted before `/start` created the record ([`state::BEFORE_START_KEY`]).
    async fn before_start(&self) -> Result<BeforeStart> {
        let json: Option<String> = self.state.storage().get(state::BEFORE_START_KEY).await?;
        Ok(json
            .map(|json| serde_json::from_str(&json))
            .transpose()?
            .unwrap_or_default())
    }

    /// Notes a request that overtook the first `/start`, for `/start` to apply
    /// ([`state::BeforeStart`]).
    async fn note_before_start(&self, note: impl FnOnce(&mut BeforeStart)) -> Result<Response> {
        let mut before = self.before_start().await?;
        note(&mut before);
        self.state
            .storage()
            .put(state::BEFORE_START_KEY, serde_json::to_string(&before)?)
            .await?;
        Response::empty().map(|r| r.with_status(202))
    }

    pub(super) async fn untrack(
        self: &Rc<Self>,
        untrack: Ordered<UntrackRecording>,
    ) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return self
                .note_before_start(|before| {
                    before.applied.untrack(untrack.request.who, untrack.seq);
                })
                .await;
        };
        let Some(closed) = recorder.untrack(&untrack.request, untrack.seq) else {
            console_log!(
                "recorder: dropped an /untrack for {:?} older than a later request",
                untrack.request.who
            );
            return no_content();
        };
        self.save(&recorder).await?;
        self.end_streams(&closed);
        self.close_sfu_adapters(&closed).await;
        no_content()
    }

    pub(super) async fn stop(self: &Rc<Self>, seq: RequestSeq) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return self
                .note_before_start(|before| {
                    before.applied.stop(seq);
                })
                .await;
        };
        let (closed, late) = recorder.stop(seq);
        if !late.is_empty() {
            console_warn!(
                "recorder: /stop {seq:?} arrived after newer requests of {late:?}; \
                 their adapters are closed too and stay closed until their next /track"
            );
        }
        self.save(&recorder).await?;
        self.end_streams(&closed);
        self.close_sfu_adapters(&closed).await;
        no_content()
    }

    /// The meeting is over: closes every stream, keeps the roster's names and the consent
    /// record, and starts the transcript job.
    pub(super) async fn end(self: &Rc<Self>, finish: FinishRecording) -> Result<Response> {
        let Some(mut recorder) = self.load().await? else {
            return not_found();
        };
        if recorder.phase == RecorderPhase::Recording {
            self.finish(&mut recorder, Some(finish), now()).await?;
        }
        no_content()
    }

    /// Ends the recording. `finish` is the room's roster and consent record, which replace what
    /// the `/track`s carried; without it (the room's `/end` never came) those stay.
    async fn finish(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        finish: Option<FinishRecording>,
        now: Millis,
    ) -> Result<()> {
        let closed = recorder.close_adapters(|_| true);
        if let Some(finish) = finish {
            for entry in finish.roster {
                recorder.name_speaker(entry);
            }
            recorder.consents = finish.consents;
        }
        recorder.phase = RecorderPhase::Ended {
            at: now,
            job: Job::Transcribing {
                retry_at: None,
                waiting_since: None,
            },
        };
        self.save(recorder).await?;
        self.ended.set(true);
        self.end_streams(&closed);
        self.finish_all_captures();
        self.close_sfu_adapters(&closed).await;
        console_log!(
            "recorder {}: meeting ended, transcript job starts",
            self.id()
        );
        self.arm(now).await
    }

    /// While the meeting runs: closes the chunks of streams that went quiet, recovers chunks an
    /// eviction left behind, and ends a recording nobody ended.
    pub(super) async fn look_after(
        self: &Rc<Self>,
        recorder: &mut Recorder,
        now: Millis,
    ) -> Result<Option<Millis>> {
        self.finish_idle_captures(now, IDLE_STREAM_MS);
        self.recover(now);
        let heard = self.store.audio_end()?.unwrap_or(recorder.started_at);
        if self.live.borrow().streams.is_empty() && now.0.saturating_sub(heard.0) > ABANDONED_MS {
            console_warn!(
                "recorder {}: no audio and no end for hours; ending",
                self.id()
            );
            self.finish(recorder, None, now).await?;
            return Ok(Some(now));
        }
        Ok(Some(now.after(HOUSEKEEPING_MS)))
    }
}
