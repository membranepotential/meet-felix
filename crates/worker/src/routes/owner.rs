//! The owner's routes, `POST /api/login`, `POST /api/logout` and `GET /api/me`, and how any
//! request proves that it is the owner's (D7 in `docs/links`, D32 in `docs/architecture`): with
//! the session cookie, or with the password as bearer token. `crate::owner` holds the
//! credentials themselves.

use meeting_core::DisplayName;
use meeting_core::http::{ApiErrorCode, LoginRequest, Owner};
use worker::{Request, Response, Result, RouteContext, console_error};

use crate::limits::{self, Limit};
use crate::owner::{self as credentials, Initiator, OwnerAuth, OwnerAuthError, OwnerSession};
use crate::{Config, now, reply};

/// The owner's password, which `POST /api/login` checks and owner-only routes also accept as a
/// bearer token (D7 in `docs/links`). At least [`credentials::MIN_SECRET_CHARS`] characters.
const CREATE_SECRET: &str = "CREATE_SECRET";
/// The random key that signs the owner's session cookies (D24 in `docs/ui`). At least
/// [`credentials::MIN_SESSION_KEY_CHARS`] characters.
const SESSION_KEY: &str = "SESSION_KEY";
/// Var with the owner's display name, which the app prefills when the owner joins (D32 in
/// `docs/architecture`). An unrendered config leaves the placeholder `${OWNER_NAME}`, which is a
/// valid name and shows as such.
const OWNER_NAME: &str = "OWNER_NAME";

/// `POST /api/login`: sets the owner's session cookie if the password is [`CREATE_SECRET`], and
/// answers like `GET /api/me`, so the app learns the owner's name without asking again.
/// Every attempt counts against [`Limit::OwnerAuth`], and one over the limit gets 429 even with
/// the right password.
pub async fn login(mut req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Some(auth) = owner_auth(&ctx) else {
        return reply::error(ApiErrorCode::NotConfigured);
    };
    if !limits::allows(&ctx.env, Limit::OwnerAuth, &req).await? {
        return limits::too_many_requests();
    }
    // The parser's message stays out of the answer and the log: it may quote the body.
    let Ok(request) = serde_json::from_str::<LoginRequest>(&req.text().await?) else {
        return reply::error(ApiErrorCode::BadRequest);
    };
    if !auth.secret.matches(&request.password) {
        return reply::error(ApiErrorCode::Unauthorized);
    }
    with_cookie(owner(&ctx)?, &auth.session.issue(now()))
}

/// `POST /api/logout`: clears the session cookie in this browser.
pub fn logout(req: Request, _ctx: RouteContext<Config>) -> Result<Response> {
    if initiator_of(&req)? == Initiator::Foreign {
        return reply::error(ApiErrorCode::CrossOrigin);
    }
    with_cookie(Response::empty()?.with_status(204), &OwnerSession::clear())
}

/// `GET /api/me`: the [`Owner`] for a valid session cookie.
pub fn me(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let Some(auth) = owner_auth(&ctx) else {
        return reply::error(ApiErrorCode::NotConfigured);
    };
    if is_owner_session(&req, &auth.session)? {
        owner(&ctx)
    } else {
        reply::error(ApiErrorCode::Unauthorized)
    }
}

/// How a request proved that it is the owner's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerProof {
    /// The creation secret as bearer token.
    Bearer,
    /// A valid session cookie.
    Cookie,
}

/// How `req` proves that it is the owner's, or the answer that refuses it. A bearer token
/// counts against [`Limit::OwnerAuth`] before it is compared, like a login.
pub async fn owner_proof(
    req: &Request,
    ctx: &RouteContext<Config>,
) -> Result<std::result::Result<OwnerProof, Response>> {
    let Some(auth) = owner_auth(ctx) else {
        return reply::error(ApiErrorCode::NotConfigured).map(Err);
    };
    if let Some(token) = bearer_token(req.headers().get("Authorization")?.as_deref()) {
        if !limits::allows(&ctx.env, Limit::OwnerAuth, req).await? {
            return limits::too_many_requests().map(Err);
        }
        if auth.secret.matches(token) {
            return Ok(Ok(OwnerProof::Bearer));
        }
    }
    if is_owner_session(req, &auth.session)? {
        Ok(Ok(OwnerProof::Cookie))
    } else {
        reply::error(ApiErrorCode::Unauthorized).map(Err)
    }
}

/// The owner's credentials from [`CREATE_SECRET`] and [`SESSION_KEY`], or `None` (logged) if
/// either is unset or too short: then nobody is the owner.
pub fn owner_auth(ctx: &RouteContext<Config>) -> Option<OwnerAuth> {
    let secret = |name| ctx.secret(name).ok().map(|value| value.to_string());
    OwnerAuth::new(secret(CREATE_SECRET), secret(SESSION_KEY))
        .inspect_err(|e| {
            let (name, min) = match e {
                OwnerAuthError::Secret => (CREATE_SECRET, credentials::MIN_SECRET_CHARS),
                OwnerAuthError::SessionKey => (SESSION_KEY, credentials::MIN_SESSION_KEY_CHARS),
            };
            console_error!(
                "{name} is unset or shorter than {min} characters; nobody can log in, create or \
                 list meetings, or join as host"
            );
        })
        .ok()
}

/// Where the browser says `req` comes from, relative to `req`'s own origin. Through the
/// custom-domain front door the Worker sees the URL the browser asked for, because `edge/`
/// forwards the request object unchanged.
pub fn initiator_of(req: &Request) -> Result<Initiator> {
    let headers = req.headers();
    Ok(credentials::initiator(
        headers.get("Sec-Fetch-Site")?.as_deref(),
        headers.get("Origin")?.as_deref(),
        &req.url()?.origin().ascii_serialization(),
    ))
}

/// Whether `req` carries the owner's valid session cookie.
pub fn is_owner_session(req: &Request, session: &OwnerSession) -> Result<bool> {
    let cookies = req.headers().get("Cookie")?;
    Ok(session.verify(cookies.as_deref(), now()))
}

/// The owner's [`Owner`].
fn owner(ctx: &RouteContext<Config>) -> Result<Response> {
    Response::from_json(&Owner {
        name: owner_name(ctx),
    })
}

/// [`OWNER_NAME`], or `None` (logged) if it is unset or not a valid display name.
fn owner_name(ctx: &RouteContext<Config>) -> Option<DisplayName> {
    let value = ctx.var(OWNER_NAME).ok().map(|value| value.to_string());
    parse_owner_name(value.as_deref())
        .inspect_err(|e| console_error!("{e}; the app prefills no name for the owner"))
        .ok()
}

/// Why [`OWNER_NAME`] gives no name. A deployment mistake: the answer then carries `null`
/// rather than failing the route.
#[derive(Debug, PartialEq, Eq)]
enum OwnerNameError {
    Unset,
    Invalid(String),
}

impl std::fmt::Display for OwnerNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => write!(f, "{OWNER_NAME} is not set"),
            Self::Invalid(value) => write!(f, "{OWNER_NAME} {value:?} is not a display name"),
        }
    }
}

/// The display name in the value of [`OWNER_NAME`]; `value` is `None` if the var is unset.
fn parse_owner_name(value: Option<&str>) -> Result<DisplayName, OwnerNameError> {
    let value = value.ok_or(OwnerNameError::Unset)?;
    DisplayName::parse(value).map_err(|_| OwnerNameError::Invalid(value.to_owned()))
}

fn with_cookie(response: Response, set_cookie: &str) -> Result<Response> {
    response.headers().set("Set-Cookie", set_cookie)?;
    Ok(response)
}

/// The token of an `Authorization: Bearer <token>` header.
fn bearer_token(header: Option<&str>) -> Option<&str> {
    header?.strip_prefix("Bearer ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_name_is_null_unless_the_var_holds_a_display_name() {
        let name = |value| {
            serde_json::to_value(Owner {
                name: parse_owner_name(value).ok(),
            })
            .expect("the answer serializes")
        };
        let null = serde_json::json!({ "name": null });
        assert_eq!(name(None), null, "unset");
        assert_eq!(name(Some("")), null, "empty");
        assert_eq!(name(Some("  ")), null, "blank");
        assert_eq!(name(Some("Ada\nLovelace")), null, "a control character");
        let long = "x".repeat(200);
        assert_eq!(name(Some(&long)), null, "too long");
        assert_eq!(name(Some("Ada")), serde_json::json!({ "name": "Ada" }));
        assert_eq!(parse_owner_name(None), Err(OwnerNameError::Unset));
        assert_eq!(
            parse_owner_name(Some("")),
            Err(OwnerNameError::Invalid(String::new()))
        );
    }

    #[test]
    fn the_bearer_token_follows_the_scheme_verbatim() {
        assert_eq!(bearer_token(Some("Bearer s3cret")), Some("s3cret"));
        assert_eq!(bearer_token(Some("Bearer s3cret ")), Some("s3cret "));
        assert_eq!(bearer_token(Some("Bearer ")), Some(""));
        for header in [
            None,
            Some("s3cret"),
            Some("bearer s3cret"),
            Some("Basic s3cret"),
        ] {
            assert_eq!(bearer_token(header), None, "{header:?}");
        }
    }
}
