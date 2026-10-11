//! `GET /api/transcript/:token`, the transcript's `meeting_core::http::TranscriptStatus`, and
//! `GET /api/transcript/:token/:file`, its downloads (D9 in `docs/links`, D31 in `docs/ui`): the
//! router finds the token's recorder in KV and passes the request on. Every answer keeps out of
//! referrers and search engines ([`reply::private`]), and the token is never logged, because it
//! is the link's only secret (research 11, item 7).

use meeting_core::TranscriptRef;
use meeting_core::http::ApiErrorCode;
use worker::{Request, Response, Result, RouteContext, console_error};

use crate::internal::{self, RECORDER_STATUS, RecorderId, TRANSCRIPT_JSON, TRANSCRIPT_MD};
use crate::limits::{self, Limit};
use crate::{Config, MEETINGS, RECORDERS, reply, transcript_key};

/// `GET /api/transcript/:token`.
pub async fn status(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    reply::private(pass_on(&req, &ctx, RECORDER_STATUS).await?)
}

/// `GET /api/transcript/:token/:file`, for `transcript.md` and `transcript.json`.
pub async fn file(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let response = match ctx.param("file").map(String::as_str) {
        Some(file @ (TRANSCRIPT_MD | TRANSCRIPT_JSON)) => {
            pass_on(&req, &ctx, &format!("/{file}")).await?
        }
        _ => reply::error(ApiErrorCode::NotFound)?,
    };
    reply::private(response)
}

/// Passes the request on to the token's recorder at `path`. Counts against [`Limit::Public`],
/// like a meeting lookup. A token that is not one, or that KV does not know, is not found; so
/// is whatever the recorder does not find.
async fn pass_on(req: &Request, ctx: &RouteContext<Config>, path: &str) -> Result<Response> {
    if !limits::allows(&ctx.env, Limit::Public, req).await? {
        return limits::too_many_requests();
    }
    let Some(token) = ctx.param("token").and_then(|token| parse_token(token)) else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let Some(recorder) = ctx.kv(MEETINGS)?.get(&transcript_key(&token)).text().await? else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let recorder = RecorderId::new(recorder).stub(&ctx.durable_object(RECORDERS)?)?;
    let response = internal::get(&recorder, path).await?;
    match response.status_code() {
        200..300 => Ok(response),
        404 => reply::error(ApiErrorCode::NotFound),
        status => {
            console_error!("a recorder answered {path} with {status}");
            reply::error(ApiErrorCode::Internal)
        }
    }
}

/// The token in a transcript link: 32 lowercase hex digits, as `random::transcript_ref` makes
/// them (D9). Anything else is no token, without a KV lookup.
fn parse_token(token: &str) -> Option<TranscriptRef> {
    (token.len() == 32
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| TranscriptRef::new(token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::random;

    #[test]
    fn a_token_is_32_lowercase_hex_digits() {
        let made = random::transcript_ref().unwrap();
        assert_eq!(parse_token(made.as_str()), Some(made));
        for bad in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdef0",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcdeg",
            "../../index",
        ] {
            assert_eq!(parse_token(bad), None, "{bad:?}");
        }
    }
}
