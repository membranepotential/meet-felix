//! The browser-facing API (feature `wasm`).

use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

use crate::ids::MeetingId;
use crate::protocol::{AfterClose, ClientMsg, PROTOCOL_VERSION, ServerMsg};

/// Whether `s` is a valid meeting id, i.e. three listed words joined by `-`.
#[wasm_bindgen(js_name = validateMeetingId)]
pub fn validate_meeting_id(s: &str) -> bool {
    MeetingId::parse(s).is_ok()
}

/// A meeting id built from the first three values of `random_u32s`, which the caller fills
/// from `crypto.getRandomValues`. Throws if fewer than three values are given.
#[wasm_bindgen(js_name = generateMeetingId)]
pub fn generate_meeting_id(random_u32s: &[u32]) -> Result<Ts<MeetingId>, JsError> {
    let random = random_u32s
        .first_chunk::<{ MeetingId::WORD_COUNT }>()
        .ok_or_else(|| JsError::new("generateMeetingId needs three random u32 values"))?;
    Ok(MeetingId::from_random(*random).into_ts()?)
}

/// Parses and validates a whole message from the room's socket. Throws if `json` is not a
/// valid [`ServerMsg`].
#[wasm_bindgen(js_name = parseServerMsg)]
pub fn parse_server_msg(json: &str) -> Result<Ts<ServerMsg>, JsError> {
    let msg: ServerMsg = serde_json::from_str(json)?;
    Ok(msg.into_ts()?)
}

/// The protocol version this build speaks; send it in every `Join`.
#[wasm_bindgen(js_name = protocolVersion)]
pub fn protocol_version() -> u32 {
    PROTOCOL_VERSION
}

/// What to do after the room's socket closed with `code`: reconnect, stop, or reload the page.
#[wasm_bindgen(js_name = afterClose)]
pub fn after_close(code: u16) -> Result<Ts<AfterClose>, JsError> {
    Ok(crate::protocol::after_close(code).into_ts()?)
}

/// The JSON text of a message for the room's socket. Throws if `msg` is not a valid
/// [`ClientMsg`], e.g. if its display name is empty.
#[wasm_bindgen(js_name = encodeClientMsg)]
pub fn encode_client_msg(msg: Ts<ClientMsg>) -> Result<String, JsError> {
    Ok(serde_json::to_string(&msg.to_rust()?)?)
}
