//! Carries out the core's SFU effects and builds the replies the core expects.

use meeting_core::{Effect, FailureCause, SfuCall, SfuReply};

use crate::client::{PublishAnswer, SfuClient, SfuError, SfuTransport, SubscribeOffer};

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
        let failed = |error: SfuError| Failure {
            reply: failure_reply(effect, error.cause()),
            error,
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
                negotiation,
                sdp_offer,
                tracks,
            } => {
                let PublishAnswer { sdp, rejected } = self
                    .publish(session, sdp_offer, tracks)
                    .await
                    .map_err(failed)?;
                Some(SfuReply::Published {
                    who: *who,
                    session: session.clone(),
                    negotiation: *negotiation,
                    sdp_answer: sdp,
                    rejected,
                })
            }
            Effect::SfuSubscribe {
                who,
                session,
                negotiation,
                tracks,
            } => {
                let SubscribeOffer { sdp, tracks } =
                    self.subscribe(session, tracks).await.map_err(failed)?;
                Some(SfuReply::SubscribeOffer {
                    who: *who,
                    session: session.clone(),
                    negotiation: *negotiation,
                    sdp_offer: sdp,
                    tracks,
                })
            }
            Effect::SfuRenegotiate {
                who,
                session,
                negotiation,
                sdp_answer,
            } => {
                self.renegotiate(session, sdp_answer)
                    .await
                    .map_err(failed)?;
                Some(SfuReply::Renegotiated {
                    who: *who,
                    session: session.clone(),
                    negotiation: *negotiation,
                })
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

/// The [`SfuReply::Failed`] the core expects when the call for `effect` fails for `cause` or
/// cannot be made ([`FailureCause::Transient`]); `None` for effects that expect no reply.
pub fn failure_reply(effect: &Effect, cause: FailureCause) -> Option<SfuReply> {
    let (who, call) = match effect {
        Effect::SfuNewSession { who, kind } => (who, SfuCall::NewSession(*kind)),
        Effect::SfuPublish {
            who,
            session,
            negotiation,
            ..
        } => (
            who,
            SfuCall::Publish {
                session: session.clone(),
                negotiation: *negotiation,
            },
        ),
        Effect::SfuSubscribe {
            who,
            session,
            negotiation,
            ..
        } => (
            who,
            SfuCall::Subscribe {
                session: session.clone(),
                negotiation: *negotiation,
            },
        ),
        Effect::SfuRenegotiate {
            who,
            session,
            negotiation,
            ..
        } => (
            who,
            SfuCall::Renegotiate {
                session: session.clone(),
                negotiation: *negotiation,
            },
        ),
        _ => return None,
    };
    Some(SfuReply::Failed {
        who: *who,
        call,
        cause,
    })
}
