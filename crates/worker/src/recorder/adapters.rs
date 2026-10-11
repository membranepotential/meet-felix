//! The SFU calls for the WebSocket adapters: creating one adapter per call, and closing them.

use meeting_core::{SessionId, TrackName};
use sfu_api::wire::{AdapterId, AdapterTrack, MAX_ADAPTERS_PER_REQUEST, OutputCodec};
use worker::{Result, console_warn};

use super::Inner;
use super::state::Adapter;
use super::streams::StreamKey;
use crate::sfu;

/// Var: the origin the SFU's adapters connect to, e.g. the `workers.dev` host, which skips the
/// Pages front door (research 11). `https://` becomes `wss://`.
const RECORDER_ORIGIN: &str = "RECORDER_ORIGIN";

impl Inner {
    /// Asks the SFU for one adapter that streams the track to `endpoint`. One per call, so a
    /// call that fails leaves nothing half-created to undo.
    pub(super) async fn create_adapter(
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
    pub(super) fn endpoint(&self, secret: &str, (who, generation): StreamKey) -> Result<String> {
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
        let ids: Vec<AdapterId> = adapters.iter().filter_map(|a| a.id.clone()).collect();
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
