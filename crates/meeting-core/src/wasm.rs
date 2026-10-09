//! The browser-facing API (feature `wasm`).

use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;

use crate::ids::MeetingId;

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
