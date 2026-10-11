//! The owner's credentials (D32 in `docs/architecture`): the creation secret, which is the
//! owner's password and the bearer token for scripts, and the session cookie that proves a
//! login.
//!
//! The cookie is `owner=<expiry>.<tag>`: `expiry` is the end of the session in milliseconds
//! since the epoch, and `tag` the hex HMAC-SHA256 of that decimal string. The HMAC key is
//! derived from the random Worker secret `SESSION_KEY` and the password, so a leaked cookie
//! gives nothing to test password guesses against offline, and changing either secret ends
//! every session (D24 in `docs/ui`). Pure, so it is tested natively.

use hmac::{Hmac, KeyInit, Mac};
use meeting_core::Millis;
use sha2::Sha256;

use crate::constant_time;
use crate::random::hex;

/// The cookie's name.
const COOKIE: &str = "owner";
/// How long a login lasts, in seconds (30 days); the cookie's `Max-Age`.
const SESSION_SECS: u64 = 30 * 24 * 60 * 60;
/// Prefixed to the password when deriving the session key, so the key differs from any other
/// MAC of the password.
const KEY_CONTEXT: &[u8] = b"meet-felix owner session";
/// The cookie's attributes: script-proof, HTTPS only (browsers also accept it on
/// `http://localhost`), sent on top-level navigations from other sites.
const ATTRIBUTES: &str = "HttpOnly; Secure; SameSite=Lax; Path=/";
/// The shortest `CREATE_SECRET` accepted. The password is guessable online only through the
/// throttled login and bearer paths, so 20 machine-generated characters (about 119 bits from
/// base64) put guessing out of reach; a short, human-chosen password does not.
pub const MIN_SECRET_CHARS: usize = 20;
/// The shortest `SESSION_KEY` accepted: 32 characters, such as the 44 of
/// `openssl rand -base64 32`.
pub const MIN_SESSION_KEY_CHARS: usize = 32;

/// `CREATE_SECRET`, known to be at least [`MIN_SECRET_CHARS`] long, so an empty or short
/// password never matches.
pub struct CreateSecret(String);

impl CreateSecret {
    /// `None` for a secret shorter than [`MIN_SECRET_CHARS`].
    pub fn new(secret: String) -> Option<Self> {
        (secret.chars().count() >= MIN_SECRET_CHARS).then_some(Self(secret))
    }

    /// Whether `given` is the secret. Takes the same time wherever `given` differs.
    pub fn matches(&self, given: &str) -> bool {
        constant_time::eq(given.as_bytes(), self.0.as_bytes())
    }
}

/// `SESSION_KEY`: a random secret used only to sign session cookies, at least
/// [`MIN_SESSION_KEY_CHARS`] long.
pub struct SessionKey(String);

impl SessionKey {
    /// `None` for a key shorter than [`MIN_SESSION_KEY_CHARS`].
    pub fn new(key: String) -> Option<Self> {
        (key.chars().count() >= MIN_SESSION_KEY_CHARS).then_some(Self(key))
    }
}

/// Issues and checks the owner's session cookie.
pub struct OwnerSession {
    key: [u8; 32],
}

impl OwnerSession {
    /// The session under `key` for the password `secret`: HMAC-SHA256 keyed with `key` over the
    /// context and the password.
    pub fn new(key: &SessionKey, secret: &CreateSecret) -> Self {
        let mut mac = Hmac::<Sha256>::new_from_slice(key.0.as_bytes())
            .expect("HMAC takes keys of any length");
        mac.update(KEY_CONTEXT);
        mac.update(secret.0.as_bytes());
        Self {
            key: mac.finalize().into_bytes().into(),
        }
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
        constant_time::eq(tag.as_bytes(), self.tag(expiry).as_bytes())
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

/// The owner's configured credentials: both secrets are needed for any owner route, so a
/// missing or short one locks the owner out instead of weakening the check.
pub struct OwnerAuth {
    pub secret: CreateSecret,
    pub session: OwnerSession,
}

/// What is wrong with the owner's secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerAuthError {
    /// `CREATE_SECRET` is unset or shorter than [`MIN_SECRET_CHARS`].
    Secret,
    /// `SESSION_KEY` is unset or shorter than [`MIN_SESSION_KEY_CHARS`].
    SessionKey,
}

impl OwnerAuth {
    /// From the two secrets' values, `None` standing for an unset one.
    pub fn new(
        secret: Option<String>,
        session_key: Option<String>,
    ) -> Result<Self, OwnerAuthError> {
        let secret = secret
            .and_then(CreateSecret::new)
            .ok_or(OwnerAuthError::Secret)?;
        let key = session_key
            .and_then(SessionKey::new)
            .ok_or(OwnerAuthError::SessionKey)?;
        Ok(Self {
            session: OwnerSession::new(&key, &secret),
            secret,
        })
    }
}

/// Where a browser says a request comes from, relative to the origin it is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Initiator {
    /// A page of this origin, such as the app itself.
    SameOrigin,
    /// Another origin, including sibling subdomains, which `SameSite=Lax` does not stop.
    Foreign,
    /// Neither `Sec-Fetch-Site` nor `Origin`: not a browser, such as a script, which cannot
    /// act with someone else's cookie.
    Unknown,
}

/// The initiator of a request to `own_origin` (`scheme://host[:port]`), from its
/// `Sec-Fetch-Site` and `Origin` headers. `Sec-Fetch-Site` decides when present: the browser
/// computes it, so it stays right behind proxies that rewrite the host, such as Vite's in
/// development. Browsers send `Origin` on every POST and WebSocket handshake
/// (<https://fetch.spec.whatwg.org/#origin-header>), so a browser request without either
/// header does not occur in current browsers.
pub fn initiator(
    sec_fetch_site: Option<&str>,
    origin: Option<&str>,
    own_origin: &str,
) -> Initiator {
    match (sec_fetch_site, origin) {
        (Some(site), _) if site.eq_ignore_ascii_case("same-origin") => Initiator::SameOrigin,
        (Some(_), _) => Initiator::Foreign,
        (None, Some(origin)) if origin.eq_ignore_ascii_case(own_origin) => Initiator::SameOrigin,
        (None, Some(_)) => Initiator::Foreign,
        (None, None) => Initiator::Unknown,
    }
}

/// Whether a `Content-Type` header names JSON, which an HTML form cannot send.
pub fn is_json(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|value| value.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case("application/json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: Millis = Millis(1_000_000);
    const PASSWORD: &str = "a-machine-made-password-1";
    const KEY: &str = "a-random-session-key-of-32-chars";

    fn auth(password: &str, key: &str) -> OwnerAuth {
        OwnerAuth::new(Some(password.into()), Some(key.into())).unwrap()
    }

    fn session(password: &str, key: &str) -> OwnerSession {
        auth(password, key).session
    }

    /// The `Cookie` header a browser sends back for `set_cookie`.
    fn cookie_header(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().to_owned()
    }

    #[test]
    fn an_issued_cookie_is_valid_until_it_expires() {
        let owner = session(PASSWORD, KEY);
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
        let owner = session(PASSWORD, KEY);
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
        ] {
            assert!(!owner.verify(Some(&header), T0), "{header}");
        }
    }

    #[test]
    fn changing_either_secret_ends_every_session() {
        let issued = cookie_header(&session(PASSWORD, KEY).issue(T0));
        let new_password = session("another-machine-made-pass", KEY);
        let new_key = session(PASSWORD, "another-random-session-key-of-32");
        assert!(!new_password.verify(Some(&issued), T0));
        assert!(!new_key.verify(Some(&issued), T0));
    }

    #[test]
    fn a_missing_or_cleared_cookie_is_refused() {
        let owner = session(PASSWORD, KEY);
        assert!(!owner.verify(None, T0));
        assert!(!owner.verify(Some(""), T0));
        assert!(!owner.verify(Some(&cookie_header(&OwnerSession::clear())), T0));
    }

    #[test]
    fn the_secret_matches_exactly() {
        let secret = auth(PASSWORD, KEY).secret;
        assert!(secret.matches(PASSWORD));
        for wrong in ["", "a-machine-made-password-", "a-machine-made-password-1 "] {
            assert!(!secret.matches(wrong), "{wrong:?}");
        }
    }

    #[test]
    fn short_or_missing_secrets_lock_the_owner_out() {
        let at = |n: usize| "x".repeat(n);
        let new = |secret: Option<String>, key: Option<String>| OwnerAuth::new(secret, key).err();
        let key = Some(at(MIN_SESSION_KEY_CHARS));
        assert_eq!(new(None, key.clone()), Some(OwnerAuthError::Secret));
        assert_eq!(
            new(Some(at(MIN_SECRET_CHARS - 1)), key.clone()),
            Some(OwnerAuthError::Secret)
        );
        let secret = Some(at(MIN_SECRET_CHARS));
        assert_eq!(new(secret.clone(), None), Some(OwnerAuthError::SessionKey));
        assert_eq!(
            new(secret.clone(), Some(at(MIN_SESSION_KEY_CHARS - 1))),
            Some(OwnerAuthError::SessionKey)
        );
        assert_eq!(new(secret, key), None);
    }

    #[test]
    fn the_browser_headers_name_the_initiator() {
        const OWN: &str = "https://meet.example";
        use Initiator::*;
        for (site, origin, expected) in [
            (Some("same-origin"), None, SameOrigin),
            // Behind Vite's proxy the origin differs, but the browser knows better.
            (
                Some("same-origin"),
                Some("http://localhost:5173"),
                SameOrigin,
            ),
            (Some("same-site"), Some(OWN), Foreign),
            (Some("cross-site"), None, Foreign),
            (Some("none"), None, Foreign),
            (None, Some(OWN), SameOrigin),
            (None, Some("https://other.example"), Foreign),
            (None, Some("https://meet.example:8443"), Foreign),
            (None, Some("null"), Foreign),
            (None, None, Unknown),
        ] {
            assert_eq!(
                initiator(site, origin, OWN),
                expected,
                "{site:?} {origin:?}"
            );
        }
    }

    #[test]
    fn only_json_counts_as_json() {
        assert!(is_json(Some("application/json")));
        assert!(is_json(Some("Application/JSON; charset=utf-8")));
        for other in [
            None,
            Some("text/plain"),
            Some("text/plain;charset=UTF-8"),
            Some("application/x-www-form-urlencoded"),
            Some("multipart/form-data; boundary=x"),
        ] {
            assert!(!is_json(other), "{other:?}");
        }
    }
}
