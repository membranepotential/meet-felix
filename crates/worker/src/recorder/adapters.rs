//! The SFU calls for the WebSocket adapters: creating one adapter per call, retrying a failed
//! call from the alarm, and closing them.

use std::rc::Rc;

use meeting_core::{Millis, SessionId, TrackName};
use sfu_api::wire::{AdapterId, AdapterTrack, MAX_ADAPTERS_PER_REQUEST, OutputCodec};
use worker::{Result, console_error, console_warn};

use super::Inner;
use super::state::{Adapter, Recorder, Settled};
use super::streams::StreamKey;
use crate::{now, sfu};

/// Var: the origin the SFU's adapters connect to, e.g. the `workers.dev` host, which skips the
/// Pages front door (research 11). `https://` becomes `wss://`.
const RECORDER_ORIGIN: &str = "RECORDER_ORIGIN";

impl Inner {
    /// Asks the SFU for the adapter of `key`, if it is still to be created
    /// ([`Recorder::to_create`]), and stores what came of it ([`Recorder::settle`]). After a
    /// failure the alarm calls again ([`Inner::retry_adapters`]), armed for the next call
    /// unless it is due sooner. The record is read right before the call, since an earlier
    /// call may have let a `/stop` or `/untrack` in, and again after it.
    pub(super) async fn create_adapter(self: &Rc<Self>, key: StreamKey) -> Result<()> {
        let Some(recorder) = self.load().await? else {
            return Ok(());
        };
        let Some(adapter) = recorder.to_create(key, now()) else {
            return Ok(());
        };
        let created = match self.endpoint(&recorder.secret, key) {
            Ok(endpoint) => {
                self.call_create(
                    adapter.session.clone(),
                    adapter.track_name.clone(),
                    endpoint,
                )
                .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = &created {
            console_warn!("recorder: creating the adapter of {key:?} failed: {e}");
        }
        let Some(mut recorder) = self.load().await? else {
            return Ok(());
        };
        match recorder.settle(key, created.map_err(drop), now()) {
            Settled::Moot => Ok(()),
            Settled::Created => self.save(&recorder).await,
            Settled::Orphaned(adapter) => {
                self.save(&recorder).await?;
                self.close_sfu_adapters(&[adapter]).await;
                Ok(())
            }
            Settled::RetryAt(at) => {
                self.save(&recorder).await?;
                self.arm_by(at).await
            }
            Settled::GaveUp => {
                console_error!(
                    "recorder: gave up creating the adapter of {key:?}; \
                     their audio is missing until their next track"
                );
                self.save(&recorder).await
            }
        }
    }

    /// Calls the SFU again for every open adapter whose next call is due; returns when the
    /// next call after these is due.
    pub(super) async fn retry_adapters(
        self: &Rc<Self>,
        recorder: &Recorder,
        now: Millis,
    ) -> Result<Option<Millis>> {
        let due = recorder.adapters_due(now);
        if due.is_empty() {
            return Ok(recorder.next_adapter_call());
        }
        for key in due {
            self.create_adapter(key).await?;
        }
        Ok(self
            .load()
            .await?
            .and_then(|recorder| recorder.next_adapter_call()))
    }

    /// Asks the SFU for one adapter that streams the track to `endpoint`. One per call, so a
    /// call that fails leaves nothing half-created to undo.
    async fn call_create(
        &self,
        session_id: SessionId,
        track_name: TrackName,
        endpoint: String,
    ) -> Result<AdapterId> {
        let Some(client) = sfu::client(&self.env) else {
            return Err("CALLS_APP_ID or CALLS_APP_SECRET is not set".into());
        };
        let request = AdapterTrack::Remote {
            session_id,
            track_name,
            endpoint,
            output_codec: OutputCodec::Pcm,
        };
        let mut results = client
            .create_adapters(vec![request])
            .await
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
        results
            .pop()
            .ok_or_else(|| "no adapter result".into())
            .and_then(|r| r.map_err(|e| worker::Error::RustError(e.to_string())))
    }

    /// The adapter's endpoint: the stream's path under [`RECORDER_ORIGIN`].
    fn endpoint(&self, secret: &str, (who, generation): StreamKey) -> Result<String> {
        let origin = self.env.var(RECORDER_ORIGIN)?.to_string();
        let origin = origin.trim_end_matches('/');
        let ws_origin = if let Some(host) = origin.strip_prefix("https://") {
            format!("wss://{host}")
        } else if let Some(host) = origin.strip_prefix("http://") {
            format!("ws://{host}")
        } else {
            return Err(format!("{RECORDER_ORIGIN} must start with https:// or http://").into());
        };
        Ok(format!(
            "{ws_origin}/rec/{}/{secret}/{}/{generation}",
            self.id(),
            who.0
        ))
    }

    /// Closes adapters at the SFU, four per request. Failures are logged: an adapter whose
    /// socket the recorder closed stops streaming anyway, and closing is idempotent.
    pub(super) async fn close_sfu_adapters(&self, adapters: &[Adapter]) {
        let ids: Vec<AdapterId> = adapters.iter().filter_map(|a| a.id().cloned()).collect();
        if ids.is_empty() {
            return;
        }
        let Some(client) = sfu::client(&self.env) else {
            return;
        };
        for batch in ids.chunks(MAX_ADAPTERS_PER_REQUEST) {
            if let Err(e) = client.close_adapters(batch).await {
                console_warn!("recorder: closing adapters failed: {e}");
            }
        }
    }
}
