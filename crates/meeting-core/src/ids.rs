//! Identifiers and opaque values that cross the core's boundary.

use core::fmt;

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use tsify::Tsify;

use crate::words::WORDS;

/// A point in time in milliseconds since the Unix epoch, read from the shell's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct Millis(pub u64);

impl Millis {
    /// The instant `ms` milliseconds after `self`.
    #[must_use]
    pub const fn after(self, ms: u64) -> Self {
        Self(self.0.saturating_add(ms))
    }
}

/// A participant, identified by the shell (one id per WebSocket; never reused within a room).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct ParticipantId(pub u32);

/// Defines a newtype over a string the core never inspects.
macro_rules! opaque_string {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "wasm", derive(Tsify))]
        pub struct $name(String);

        impl $name {
            /// Wraps a value produced outside the core.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The wrapped value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

opaque_string!(
    /// A Cloudflare Realtime SFU session id.
    SessionId
);
opaque_string!(
    /// A track name within one SFU session, chosen by the publishing client.
    TrackName
);
opaque_string!(
    /// The SDP media id (`a=mid`) of a transceiver.
    Mid
);
opaque_string!(
    /// An SDP offer or answer.
    Sdp
);
opaque_string!(
    /// The opaque token in a transcript link (`/t/{token}`).
    TranscriptRef
);

/// Counts the SFU sessions of one kind a participant has had: 0 for the session created when
/// they joined, one more for each replacement (D33 in `docs/media`). The client names the
/// session whose peer connection failed by its epoch, so a stale report cannot replace a new
/// session.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct SessionEpoch(pub u32);

impl SessionEpoch {
    /// The epoch of the session that replaces this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Names one negotiation on an SFU session: a publish offer, or a subscribe negotiation from the
/// room's request for tracks to the SFU's confirmation of the client's answer. Each session
/// counts its negotiations from 1, and the SFU effects and replies carry the id, so a reply that
/// arrives after its negotiation timed out is not applied to a newer one on the same session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NegotiationId(pub u32);

impl NegotiationId {
    /// The id of the session's next negotiation.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// The secret that lets a client whose socket dropped take its place in the room again. The
/// room hands it out in [`crate::ServerMsg::Welcome`]; the client sends it back in
/// [`crate::ClientMsg::Join`]. Its `Debug` output hides the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
pub struct ResumeToken(String);

impl ResumeToken {
    /// Encodes random bytes from the shell (the core has no randomness) as lowercase hex.
    #[must_use]
    pub fn from_random(bytes: [u8; 16]) -> Self {
        Self(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
}

impl fmt::Debug for ResumeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResumeToken(..)")
    }
}

/// Three words joined by `-`, such as `amber-otter-lamp`.
///
/// [`MeetingId::parse`] is for user input and accepts only words from [`WORDS`].
/// Deserialization accepts any well-formed id (three non-empty runs of lowercase ASCII letters
/// joined by `-`) without consulting the word list, so the stored state of a running room stays
/// readable after a word is removed from the list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(try_from = "String")]
pub struct MeetingId(String);

/// The string is not three words from the word list joined by `-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMeetingId;

impl fmt::Display for InvalidMeetingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a meeting id is three listed words joined by '-'")
    }
}

impl core::error::Error for InvalidMeetingId {}

impl MeetingId {
    /// Number of words in a meeting id.
    pub const WORD_COUNT: usize = 3;

    /// Picks one word per random value. The modulo bias is below 10⁻⁶ for 32-bit inputs.
    #[must_use]
    pub fn from_random(random: [u32; Self::WORD_COUNT]) -> Self {
        let words = random.map(|r| WORDS[r as usize % WORDS.len()]);
        Self(words.join("-"))
    }

    /// Validates user input `s`; accepts exactly what [`MeetingId::from_random`] can produce
    /// with the current word list.
    pub fn parse(s: &str) -> Result<Self, InvalidMeetingId> {
        let words = Self::words(s).ok_or(InvalidMeetingId)?;
        if words.iter().all(|w| WORDS.binary_search(w).is_ok()) {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidMeetingId)
        }
    }

    /// The words of a well-formed id: three non-empty runs of lowercase ASCII letters joined by
    /// `-`. Does not consult the word list.
    fn words(s: &str) -> Option<[&str; Self::WORD_COUNT]> {
        let words: [&str; Self::WORD_COUNT] = s.split('-').collect::<Vec<_>>().try_into().ok()?;
        words
            .iter()
            .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
            .then_some(words)
    }

    /// The id as it appears in the link.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MeetingId {
    type Error = InvalidMeetingId;

    /// Accepts any well-formed id, listed words or not; see [`MeetingId`].
    fn try_from(s: String) -> Result<Self, Self::Error> {
        match Self::words(&s) {
            Some(_) => Ok(Self(s)),
            None => Err(InvalidMeetingId),
        }
    }
}

impl fmt::Display for MeetingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A participant's display name: trimmed, 1 to [`DisplayName::MAX_CHARS`] characters, no
/// control characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(try_from = "String")]
pub struct DisplayName(String);

/// The name is empty after trimming, too long, or contains control characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDisplayName;

impl fmt::Display for InvalidDisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a name has 1 to {} characters and no control characters",
            DisplayName::MAX_CHARS
        )
    }
}

impl core::error::Error for InvalidDisplayName {}

impl DisplayName {
    /// Maximum length in characters.
    pub const MAX_CHARS: usize = 40;

    /// Trims and validates `s`.
    pub fn parse(s: &str) -> Result<Self, InvalidDisplayName> {
        let s = s.trim();
        let chars = s.chars().count();
        if (1..=Self::MAX_CHARS).contains(&chars) && !s.chars().any(char::is_control) {
            Ok(Self(s.to_owned()))
        } else {
            Err(InvalidDisplayName)
        }
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DisplayName {
    type Error = InvalidDisplayName;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meeting_id_accepts_generated_ids_only() {
        let id = MeetingId::from_random([0, 1, u32::MAX]);
        assert_eq!(MeetingId::parse(id.as_str()), Ok(id.clone()));
        for bad in [
            "",
            "abide",
            "abide-acid",
            "abide-acid-acorn-acre",
            "Abide-acid-acorn",
            "abide-acid-zzz",
            "abide--acid",
            "abide-acid-acorn-",
        ] {
            assert_eq!(MeetingId::parse(bad), Err(InvalidMeetingId), "{bad:?}");
        }
    }

    #[test]
    fn meeting_id_deserialization_checks_the_form_but_not_the_word_list() {
        let from_json = |s: &str| serde_json::from_str::<MeetingId>(&format!("{s:?}"));
        assert!(from_json("abide-acid-acorn").is_ok());
        // A word removed from the list, or never in it, still deserializes.
        assert_eq!(MeetingId::parse("latter-zzz-acorn"), Err(InvalidMeetingId));
        assert_eq!(
            from_json("latter-zzz-acorn")
                .ok()
                .map(|id| id.as_str().to_owned()),
            Some("latter-zzz-acorn".to_owned())
        );
        for bad in [
            "",
            "abide-acid",
            "abide-acid-acorn-acre",
            "Abide-acid-acorn",
            "abide--acid",
        ] {
            assert!(from_json(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn resume_token_is_hex_and_debug_hides_it() {
        let token = ResumeToken::from_random([0xab; 16]);
        assert_eq!(
            serde_json::to_string(&token).ok(),
            Some(format!("\"{}\"", "ab".repeat(16)))
        );
        assert_eq!(format!("{token:?}"), "ResumeToken(..)");
    }

    #[test]
    fn display_name_is_trimmed_and_bounded() {
        assert_eq!(
            DisplayName::parse("  Ada ").map(|n| n.as_str().to_owned()),
            Ok("Ada".to_owned())
        );
        assert_eq!(DisplayName::parse("   "), Err(InvalidDisplayName));
        assert_eq!(DisplayName::parse("a\u{7}b"), Err(InvalidDisplayName));
        assert!(DisplayName::parse(&"ä".repeat(DisplayName::MAX_CHARS)).is_ok());
        assert!(DisplayName::parse(&"ä".repeat(DisplayName::MAX_CHARS + 1)).is_err());
    }
}
