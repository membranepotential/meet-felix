//! Secrets and ids drawn from `crypto.getRandomValues` (the core has no randomness).

use meeting_core::{HostToken, MeetingId, ResumeToken, TranscriptRef};
use worker::{Error, Result};

/// Fresh meeting words; whether they are free is the caller's check.
pub fn meeting_id() -> Result<MeetingId> {
    Ok(MeetingId::from_random([u32()?, u32()?, u32()?]))
}

/// 256 random bits, hex-encoded.
pub fn host_token() -> Result<HostToken> {
    Ok(HostToken::new(hex(&bytes::<32>()?)))
}

pub fn resume_token() -> Result<ResumeToken> {
    Ok(ResumeToken::from_random(bytes()?))
}

/// 128 random bits, hex-encoded (D9 in `docs/links`).
pub fn transcript_ref() -> Result<TranscriptRef> {
    Ok(TranscriptRef::new(hex(&bytes::<16>()?)))
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_token_is_64_hex_chars() {
        let json = serde_json::to_value(host_token().unwrap()).unwrap();
        let token = json.as_str().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn meeting_ids_are_listed_words() {
        let id = meeting_id().unwrap();
        assert_eq!(MeetingId::parse(id.as_str()), Ok(id));
    }
}
