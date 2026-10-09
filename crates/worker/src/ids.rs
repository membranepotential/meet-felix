//! Identifiers handed out by the Worker: meeting words, host tokens, participant ids.

use std::fmt;

use serde::{Deserialize, Serialize, Serializer};
use worker::{Error, Result};

/// Placeholder list until `meeting-core` provides the real ~2,000-word list.
const WORDS: [&str; 20] = [
    "amber", "otter", "lamp", "cedar", "maple", "river", "stone", "cloud", "ember", "fox",
    "harbor", "iris", "juniper", "kite", "lark", "meadow", "nectar", "orbit", "pine", "quartz",
];

/// A meeting link such as `amber-otter-lamp`: three words, each from [`WORDS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingWords([&'static str; 3]);

impl MeetingWords {
    pub fn random() -> Result<Self> {
        // `% 20` on a byte is slightly biased; acceptable for the placeholder list.
        Ok(Self(
            random_bytes::<3>()?.map(|b| WORDS[usize::from(b) % WORDS.len()]),
        ))
    }

    /// Accepts exactly three known words joined by `-`.
    pub fn parse(s: &str) -> Option<Self> {
        let mut words = s
            .split('-')
            .map(|part| WORDS.iter().copied().find(|w| *w == part));
        let parsed = [words.next()??, words.next()??, words.next()??];
        words.next().is_none().then_some(Self(parsed))
    }
}

impl fmt::Display for MeetingWords {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c] = self.0;
        write!(f, "{a}-{b}-{c}")
    }
}

impl Serialize for MeetingWords {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Secret that proves its holder created the meeting; 256 random bits, hex-encoded on the wire.
pub struct HostToken([u8; 32]);

impl HostToken {
    pub fn random() -> Result<Self> {
        random_bytes().map(Self)
    }
}

impl Serialize for HostToken {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex(&self.0))
    }
}

/// Identifies one WebSocket connection to a room; stored in the socket's attachment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParticipantId(String);

impl ParticipantId {
    pub fn random() -> Result<Self> {
        random_bytes::<16>().map(|bytes| Self(hex(&bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| Error::RustError(format!("getrandom: {e}")))?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_words_round_trip_through_display_and_parse() {
        let words = MeetingWords::random().unwrap();
        assert_eq!(MeetingWords::parse(&words.to_string()), Some(words));
    }

    #[test]
    fn meeting_words_reject_unknown_words_and_wrong_counts() {
        for input in [
            "amber-otter",
            "amber-otter-lamp-fox",
            "amber-otter-zebra",
            "",
            "Amber-otter-lamp",
        ] {
            assert_eq!(MeetingWords::parse(input), None, "{input:?}");
        }
    }

    #[test]
    fn host_token_is_64_hex_chars() {
        let json = serde_json::to_value(HostToken::random().unwrap()).unwrap();
        let token = json.as_str().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
