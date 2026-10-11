//! `GET /api/transcript/:token`, the transcript's `meeting_core::http::TranscriptStatus`, and
//! `GET /api/transcript/:token/:file`, its downloads (D9 in `docs/links`, D31 in `docs/ui`): the
//! router finds the token's recorder in KV and passes the request on. Every answer keeps out of
//! referrers and search engines ([`reply::private`]), and the token is never logged, because it
//! is the link's only secret (research 11, item 7).

use meeting_core::TranscriptRef;
use meeting_core::http::ApiErrorCode;
use worker::{Request, Response, Result, RouteContext, console_error};

use crate::internal::{self, RECORDER_STATUS, TRANSCRIPT_JSON, TRANSCRIPT_MD};
use crate::limits::{self, Limit};
use crate::{Config, RECORDERS, kv, reply};

/// `GET /api/transcript/:token`.
pub async fn status(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    private(pass_on(&req, &ctx, RECORDER_STATUS).await)
}

/// `GET /api/transcript/:token/:file`, for `transcript.md` and `transcript.json`.
pub async fn file(req: Request, ctx: RouteContext<Config>) -> Result<Response> {
    let response = match ctx.param("file").map(String::as_str) {
        Some(file @ (TRANSCRIPT_MD | TRANSCRIPT_JSON)) => {
            pass_on(&req, &ctx, &format!("/{file}")).await
        }
        _ => reply::error(ApiErrorCode::NotFound),
    };
    private(response)
}

/// The answer with the headers of [`reply::private`], also when the route failed: then it is
/// `internal`, as the router would answer, and the error is logged here, without the token.
fn private(response: Result<Response>) -> Result<Response> {
    let response = response.or_else(|e| {
        console_error!("a transcript request failed: {e}");
        reply::error(ApiErrorCode::Internal)
    })?;
    reply::private(response)
}

/// Passes the request on to the token's recorder at `path`. Counts against [`Limit::Public`],
/// like a meeting lookup. A token that is not one, or that KV does not know, is not found; so
/// is whatever the recorder does not find.
async fn pass_on(req: &Request, ctx: &RouteContext<Config>, path: &str) -> Result<Response> {
    if !limits::allows(&ctx.env, Limit::Public, req).await? {
        return limits::too_many_requests();
    }
    let Some(Ok(token)) = ctx.param("token").map(|token| TranscriptRef::parse(token)) else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let Some(recorder) = kv::recorder_of(&kv::open(&ctx.env)?, &token).await? else {
        return reply::error(ApiErrorCode::NotFound);
    };
    let recorder = recorder.stub(&ctx.durable_object(RECORDERS)?)?;
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
