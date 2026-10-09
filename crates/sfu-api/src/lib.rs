//! Typed client for the Cloudflare Realtime SFU HTTPS API
//! (<https://developers.cloudflare.com/realtime/sfu/api/>).
//!
//! [`SfuClient`] has one method per operation the room needs (`sessions/new`, `tracks/new` to
//! publish and to subscribe, `renegotiate`, `tracks/close`, `tracks/update`) and
//! [`SfuClient::perform`], which carries out one of `meeting-core`'s `Sfu*` effects and returns
//! the `SfuReply` to feed back into the core. The HTTP layer is the [`SfuTransport`] trait, so
//! this crate has no dependency on the Workers runtime and its tests run natively.

mod client;
mod perform;
pub mod wire;

#[cfg(test)]
mod tests;

pub use client::{
    AppId, AppSecret, BASE_URL, HttpRequest, HttpResponse, Method, SfuClient, SfuError,
    SfuTransport, SubscribeOffer,
};
pub use perform::{Failure, failure_reply};
