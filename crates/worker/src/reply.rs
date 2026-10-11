//! The router's answers: the error body every route shares (`meeting_core::http::ErrorBody`)
//! and the headers that keep answers out of caches, referrers and search engines.

use meeting_core::http::{ApiErrorCode, ErrorBody};
use worker::{Response, Result};

/// The answer for `code`: its status, and `{"error": code}`. The detail behind it goes to the
/// log, never to the caller.
pub fn error(code: ApiErrorCode) -> Result<Response> {
    Ok(Response::from_json(&ErrorBody { error: code })?.with_status(code.status()))
}

/// Adds `Cache-Control: no-store`, so no browser or intermediary keeps an API answer.
pub fn no_store(response: Response) -> Result<Response> {
    with_headers(response, &[("Cache-Control", "no-store")])
}

/// Adds the headers that keep a transcript out of referrers and search engines; [`no_store`]
/// keeps it out of caches, as every `/api/*` answer.
pub fn private(response: Response) -> Result<Response> {
    with_headers(
        response,
        &[
            ("Referrer-Policy", "no-referrer"),
            ("X-Robots-Tag", "noindex"),
        ],
    )
}

fn with_headers(response: Response, set: &[(&str, &str)]) -> Result<Response> {
    // A copy: the headers of a response fetched from a Durable Object are immutable.
    let headers = response.headers().clone();
    for (name, value) in set {
        headers.set(name, value)?;
    }
    Ok(response.with_headers(headers))
}
