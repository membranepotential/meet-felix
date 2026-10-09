//! The owner's credentials (D32 in `docs/architecture`): the creation secret, which is the
//! owner's password and the bearer token for scripts, and the session cookie it signs.
//!
//! The cookie is `owner=<expiry>.<tag>`: `expiry` is the end of the session in milliseconds
//! since the epoch, and `tag` the hex HMAC-SHA256 of that decimal string under a key derived
//! from the secret. Changing the secret therefore ends every session. Pure, so it is tested
//! natively.

use hmac::{Hmac, KeyInit, Mac};
use meeting_core::Millis;
use sha2::{Digest, Sha256};

use crate::random::hex;

/// The cookie's name.
const COOKIE: &str = "owner";
/// How long a login lasts, in seconds (30 days); the cookie's `Max-Age`.
const SESSION_SECS: u64 = 30 * 24 * 60 * 60;
/// Prefixed to the secret before hashing it into the session key, so the key differs from
/// any other hash of the secret.
const KEY_CONTEXT: &[u8] = b"meet-felix owner session";
/// The cookie's attributes: script-proof, HTTPS only (browsers also accept it on
/// `http://localhost`), sent on top-level navigations from other sites.
const ATTRIBUTES: &str = "HttpOnly; Secure; SameSite=Lax; Path=/";

/// `CREATE_SECRET`, known to be non-empty, so an empty password never matches.
pub struct CreateSecret(String);

impl CreateSecret {
    /// `None` for an empty secret.
    pub fn new(secret: String) -> Option<Self> {
        (!secret.is_empty()).then_some(Self(secret))
    }

    /// Whether `given` is the secret. Takes the same time wherever `given` differs.
    pub fn matches(&self, given: &str) -> bool {
        constant_time_eq(given.as_bytes(), self.0.as_bytes())
    }
}

/// Issues and checks the owner's session cookie.
pub struct OwnerSession {
    key: [u8; 32],
}

impl OwnerSession {
    pub fn new(secret: &CreateSecret) -> Self {
        let key = Sha256::new()
            .chain_update(KEY_CONTEXT)
            .chain_update(secret.0.as_bytes())
            .finalize();
        Self { key: key.into() }
    }

    /// The `Set-Cookie` value for a session that starts at `now`.
    pub fn issue(&self, now: Millis) -> String {
        let expiry = now.after(SESSION_SECS * 1000).0.to_string();
        let tag = self.tag(&expiry);
        format!("{COOKIE}={expiry}.{tag}; {ATTRIBUTES}; Max-Age={SESSION_SECS}")
    }

    /// The `Set-Cookie` value that ends the session in this browser.
    pub fn clear() -> String {
        format!("{COOKIE}=; {ATTRIBUTES}; Max-Age=0")
    }

    /// Whether the `Cookie` request header carries a session this key issued that has not
    /// expired at `now`.
    pub fn verify(&self, cookie_header: Option<&str>, now: Millis) -> bool {
        cookie_header
            .into_iter()
            .flat_map(|header| header.split(';'))
            .filter_map(|cookie| cookie.trim().strip_prefix(COOKIE)?.strip_prefix('='))
            .any(|value| self.is_valid(value, now))
    }

    fn is_valid(&self, value: &str, now: Millis) -> bool {
        let Some((expiry, tag)) = value.split_once('.') else {
            return false;
        };
        constant_time_eq(tag.as_bytes(), self.tag(expiry).as_bytes())
            && expiry.parse().is_ok_and(|expiry: u64| now.0 < expiry)
    }

    /// The hex HMAC-SHA256 of `expiry`.
    fn tag(&self, expiry: &str) -> String {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC takes keys of any length");
        mac.update(expiry.as_bytes());
        hex(&mac.finalize().into_bytes())
    }
}

/// Whether `a` equals `b`, in time independent of where they differ (the length still shows).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Millis = Millis(1_000_000);

    fn session(secret: &str) -> OwnerSession {
        OwnerSession::new(&CreateSecret::new(secret.into()).unwrap())
    }

    /// The `Cookie` header a browser sends back for `set_cookie`.
    fn cookie_header(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().to_owned()
    }

    #[test]
    fn an_issued_cookie_is_valid_until_it_expires() {
        let owner = session("s3cret");
        let set_cookie = owner.issue(T0);
        assert!(set_cookie.ends_with("; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000"));
        let header = format!("theme=dark; {}; other=1", cookie_header(&set_cookie));
        assert!(owner.verify(Some(&header), T0));
        let expiry = T0.after(SESSION_SECS * 1000);
        assert!(owner.verify(Some(&header), Millis(expiry.0 - 1)));
        assert!(!owner.verify(Some(&header), expiry), "expired");
    }

    /// A different hex digit.
    fn flipped(digit: &str) -> &'static str {
        if digit == "0" { "1" } else { "0" }
    }

    #[test]
    fn a_tampered_or_foreign_cookie_is_refused() {
        let owner = session("s3cret");
        let valid = cookie_header(&owner.issue(T0));
        let (expiry, tag) = valid
            .strip_prefix("owner=")
            .unwrap()
            .split_once('.')
            .unwrap();
        let later = expiry.parse::<u64>().unwrap() + 1;
        for header in [
            format!("owner={later}.{tag}"),
            format!("owner={expiry}.{}", &tag[1..]),
            format!("owner={expiry}.{}{}", flipped(&tag[..1]), &tag[1..]),
            format!("owner={expiry}"),
            format!("xowner={expiry}.{tag}"),
            cookie_header(&session("other").issue(T0)),
        ] {
            assert!(!owner.verify(Some(&header), T0), "{header}");
        }
    }

    #[test]
    fn a_missing_or_cleared_cookie_is_refused() {
        let owner = session("s3cret");
        assert!(!owner.verify(None, T0));
        assert!(!owner.verify(Some(""), T0));
        assert!(!owner.verify(Some(&cookie_header(&OwnerSession::clear())), T0));
    }

    #[test]
    fn the_secret_matches_exactly_and_is_never_empty() {
        assert!(CreateSecret::new(String::new()).is_none());
        let secret = CreateSecret::new("s3cret".into()).unwrap();
        assert!(secret.matches("s3cret"));
        for wrong in ["", "s3cre", "s3cret ", "S3cret"] {
            assert!(!secret.matches(wrong), "{wrong:?}");
        }
    }
}
