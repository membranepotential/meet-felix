//! Secrets and ids drawn from `crypto.getRandomValues` (the core has no randomness).

use meeting_core::{MeetingId, ResumeToken, TranscriptRef};
use worker::{Error, Result};

/// Fresh meeting words; whether they are free is the caller's check.
pub fn meeting_id() -> Result<MeetingId> {
    Ok(MeetingId::from_random([u32()?, u32()?, u32()?]))
}

pub fn resume_token() -> Result<ResumeToken> {
    Ok(ResumeToken::from_random(bytes()?))
}

/// 128 random bits, hex-encoded (D9 in `docs/links`).
pub fn transcript_ref() -> Result<TranscriptRef> {
    Ok(TranscriptRef::new(secret()?))
}

/// 128 random bits as 32 lowercase hex digits.
pub fn secret() -> Result<String> {
    Ok(hex(&bytes::<16>()?))
}

fn u32() -> Result<u32> {
    getrandom::u32().map_err(random_error)
}

fn bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(random_error)?;
    Ok(bytes)
}

fn random_error(e: getrandom::Error) -> Error {
    Error::RustError(format!("getrandom: {e}"))
}

/// Lowercase hex, two digits per byte.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_ref_is_32_hex_chars() {
        let json = serde_json::to_value(transcript_ref().unwrap()).unwrap();
        let token = json.as_str().unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn meeting_ids_are_listed_words() {
        let id = meeting_id().unwrap();
        assert_eq!(MeetingId::parse(id.as_str()), Ok(id));
    }
}
