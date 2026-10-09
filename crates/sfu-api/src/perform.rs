//! Carries out the core's SFU effects and builds the replies the core expects.

use meeting_core::{Effect, SfuCall, SfuReply};

use crate::client::{SfuClient, SfuError, SfuTransport, SubscribeOffer};

/// An SFU effect whose call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// Why.
    pub error: SfuError,
    /// The [`SfuReply::Failed`] to feed back, for calls the core expects a reply to.
    pub reply: Option<SfuReply>,
}

impl<T: SfuTransport> SfuClient<T> {
    /// Makes the SFU call for an `Sfu*` effect and returns the reply the core expects, if any.
    /// `SfuCloseTracks` and `SfuUpdateLayer` expect none; effects that are not SFU calls do
    /// nothing and return `Ok(None)`.
    pub async fn perform(&self, effect: &Effect) -> Result<Option<SfuReply>, Failure> {
        let failed = |error| Failure {
            error,
            reply: failure_reply(effect),
        };
        let reply = match effect {
            Effect::SfuNewSession { who, kind } => {
                let session = self.new_session().await.map_err(failed)?;
                Some(SfuReply::SessionCreated {
                    who: *who,
                    kind: *kind,
                    session,
                })
            }
            Effect::SfuPublish {
                who,
                session,
                sdp_offer,
                tracks,
            } => {
                let sdp_answer = self
                    .publish(session, sdp_offer, tracks)
                    .await
                    .map_err(failed)?;
                Some(SfuReply::Published {
                    who: *who,
                    sdp_answer,
                })
            }
            Effect::SfuSubscribe {
                who,
                session,
                tracks,
            } => {
                let SubscribeOffer { sdp, tracks } =
                    self.subscribe(session, tracks).await.map_err(failed)?;
                Some(SfuReply::SubscribeOffer {
                    who: *who,
                    sdp_offer: sdp,
                    tracks,
                })
            }
            Effect::SfuRenegotiate {
                who,
                session,
                sdp_answer,
            } => {
                self.renegotiate(session, sdp_answer)
                    .await
                    .map_err(failed)?;
                Some(SfuReply::Renegotiated { who: *who })
            }
            Effect::SfuCloseTracks { session, mids, .. } => {
                self.close_tracks(session, mids).await.map_err(failed)?;
                None
            }
            Effect::SfuUpdateLayer {
                session,
                mid,
                track,
                rid,
                ..
            } => {
                self.update_layer(session, mid, track, *rid)
                    .await
                    .map_err(failed)?;
                None
            }
            _ => None,
        };
        Ok(reply)
    }
}

/// The [`SfuReply::Failed`] the core expects when the call for `effect` fails or cannot be
/// made; `None` for effects that expect no reply.
pub fn failure_reply(effect: &Effect) -> Option<SfuReply> {
    let (who, call) = match effect {
        Effect::SfuNewSession { who, kind } => (who, SfuCall::NewSession(*kind)),
        Effect::SfuPublish { who, .. } => (who, SfuCall::Publish),
        Effect::SfuSubscribe { who, .. } => (who, SfuCall::Subscribe),
        Effect::SfuRenegotiate { who, .. } => (who, SfuCall::Renegotiate),
        _ => return None,
    };
    Some(SfuReply::Failed { who: *who, call })
}
