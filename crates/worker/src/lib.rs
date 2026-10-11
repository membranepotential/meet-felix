//! Cloudflare Worker for meet-felix: the HTTP router plus the per-meeting `RoomObject` and
//! `RecorderObject`.
//!
//! Routes (declared in [`route`], handled in [`routes`]; "owner only" means the owner's session
//! cookie or `Authorization: Bearer <CREATE_SECRET>`, else 401). The bodies are the types of
//! `meeting_core::http`, which the web app reads through generated TypeScript:
//! - `POST /api/login` with a `LoginRequest` sets the owner's session cookie and answers like
//!   `GET /api/me`.
//! - `POST /api/logout` clears it (204).
//! - `GET /api/me` with a valid session cookie returns the `Owner`: the display name from the
//!   var `OWNER_NAME`, or `null`.
//! - `POST /api/new` (owner only, optional `NewMeetingRequest`) creates a scheduled meeting and
//!   returns a `NewMeeting` with status 201.
//! - `GET /api/meetings` (owner only) returns the owner's last meetings, newest first, as a
//!   `Listing`.
//! - `GET /api/meeting/:words` returns a `MeetingLookup`, or 404.
//! - `GET /api/ice?meeting=:words` returns `{"iceServers"}` for `RTCPeerConnection` in a known
//!   meeting, or 404: STUN, plus TURN with fresh credentials if the secrets `TURN_KEY_ID` and
//!   `TURN_KEY_API_TOKEN` are set. TURN traffic is billed, and anyone who knows an open
//!   meeting's words gets credentials, so the route is rate-limited and they expire after hours.
//! - `GET /ws/:words` upgrades to the room's signalling WebSocket (410 once the meeting ended).
//!   A socket opened with the owner's session cookie joins as host (D32 in `docs/architecture`).
//! - `GET /rec/:recorder/:secret/:who/:n` upgrades an SFU adapter's WebSocket into the meeting's
//!   `RecorderObject`, which checks the secret.
//! - `GET /api/transcript/:token` returns the `TranscriptStatus`, or 404;
//!   `GET /api/transcript/:token/transcript.md` and `.../transcript.json` download the files.
//!   These answers carry `Referrer-Policy: no-referrer` and `X-Robots-Tag: noindex`, and the
//!   token is never logged (research 11, item 7).
//!
//! The Worker answers only paths under `/api/`, `/ws/` and `/rec/`; the static web app answers
//! the rest. `run_worker_first` in `wrangler.toml` and the dev proxy in `web/vite.config.ts` list
//! the same prefixes (the browser's two), which a test checks.
//!
//! Guards on every route:
//! - Every refusal and failure the router answers is an `ErrorBody`, `{"error": code}`, with the
//!   code's status; the detail goes to the log only. The router's own 404 and 405 are such
//!   answers too, and so is an error a handler returns (500, `internal`).
//! - Every `/api/*` response carries `Cache-Control: no-store`: many carry credentials, tokens
//!   or the owner's data.
//! - Per-client rate limits ([`limits`], D6 in `docs/links`) answer 429: `/api/meeting`,
//!   `/api/ice`, `/api/transcript` and `/ws` share one limit; every password comparison (login
//!   and bearer token) counts against a stricter one.
//! - A browser request that the session cookie would authorize must come from this origin:
//!   `POST /api/new` and `POST /api/logout` answer 403 otherwise, and a WebSocket upgrade joins
//!   as a guest. `POST /api/new` from a browser must be `application/json` (415 otherwise),
//!   which an HTML form cannot send. Bearer-token callers and other non-browser clients are not
//!   checked: they cannot act with someone else's cookie.

mod ice;
mod internal;
mod jurisdiction;
mod limits;
mod meetings;
mod owner;
mod random;
mod record;
mod recorder;
mod reply;
mod room;
mod routes;
mod sfu;

use meeting_core::http::ApiErrorCode;
use meeting_core::{Millis, TranscriptRef};
use worker::{Context, Date, Env, Request, Response, Result, Router, console_error, event};

// `room.rs` reaches it here as `crate::Jurisdiction`.
use crate::jurisdiction::Jurisdiction;
use crate::routes::{create, join, list, owner as owner_routes, transcript};

/// KV namespace mapping meeting words to the Room Durable Object id (hex) and transcript tokens
/// to the recorder's ([`transcript_key`]); also holds the owner's meeting list under
/// [`meetings::KEY`].
const MEETINGS: &str = "MEETINGS";
/// Durable Object namespace of `RoomObject`.
const ROOMS: &str = "ROOMS";
/// Durable Object namespace of `RecorderObject`.
const RECORDERS: &str = "RECORDERS";
/// Var naming the jurisdiction new rooms and recorders are pinned to; see [`Jurisdiction`].
const ROOM_JURISDICTION: &str = "ROOM_JURISDICTION";
/// The prefix of the paths whose answers carry `Cache-Control: no-store`.
const API_PREFIX: &str = "/api/";

/// Configuration checked before any route runs, so a wrong value fails every request.
struct Config {
    jurisdiction: Option<Jurisdiction>,
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let api = req.path().starts_with(API_PREFIX);
    let response = match route(req, env).await {
        // No handler answers 405; the router does, for a known path with another method.
        Ok(response) if response.status_code() == 405 => {
            reply::error(ApiErrorCode::MethodNotAllowed)?
        }
        Ok(response) => response,
        Err(e) => {
            // Without the path, which may hold a transcript token.
            console_error!("a request failed: {e}");
            reply::error(ApiErrorCode::Internal)?
        }
    };
    if api {
        reply::no_store(response)
    } else {
        Ok(response)
    }
}

/// Every route of the Worker.
async fn route(req: Request, env: Env) -> Result<Response> {
    let jurisdiction = match Jurisdiction::parse_var(&env.var(ROOM_JURISDICTION)?.to_string()) {
        Ok(jurisdiction) => jurisdiction,
        Err(e) => {
            console_error!("{e}");
            return reply::error(ApiErrorCode::NotConfigured);
        }
    };
    Router::with_data(Config { jurisdiction })
        .post_async("/api/login", owner_routes::login)
        .post("/api/logout", owner_routes::logout)
        .get("/api/me", owner_routes::me)
        .post_async("/api/new", create::create)
        .get_async("/api/meetings", list::list)
        .get_async("/api/meeting/:words", join::lookup)
        .get_async("/api/ice", join::ice_servers)
        .get_async("/api/transcript/:token", transcript::status)
        .get_async("/api/transcript/:token/:file", transcript::file)
        .get_async("/ws/:words", join::join)
        .get_async("/rec/:recorder/:secret/:who/:n", join::recorder_stream)
        .or_else_any_method("/*path", |_, _| reply::error(ApiErrorCode::NotFound))
        .run(req, env)
        .await
}

/// The KV key that maps a transcript token to its `RecorderObject`'s id. Meeting words never
/// contain `:`, so it cannot collide with them.
fn transcript_key(token: &TranscriptRef) -> String {
    format!("transcript:{}", token.as_str())
}

/// The Worker's clock.
fn now() -> Millis {
    Millis(Date::now().as_millis())
}

fn is_websocket_upgrade(req: &Request) -> Result<bool> {
    Ok(req
        .headers()
        .get("Upgrade")?
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket")))
}

#[cfg(test)]
mod tests {
    /// The path prefixes the Worker answers before the static app.
    const PREFIXES: [&str; 3] = ["/api/", "/ws/", "/rec/"];

    /// The path patterns in [`super::route`], read from this file.
    fn route_patterns() -> Vec<&'static str> {
        let source = include_str!("lib.rs");
        let start = source.find("Router::with_data(").expect("route declares the routes");
        let end = start + source[start..].find(".run(req, env)").expect("route runs");
        source[start..end]
            .split("(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect()
    }

    #[test]
    fn every_route_and_the_static_app_agree_on_the_worker_s_prefixes() {
        let patterns = route_patterns();
        assert!(patterns.len() > 10, "{patterns:?}");
        for pattern in patterns {
            assert!(
                pattern == "/*path" || PREFIXES.iter().any(|p| pattern.starts_with(p)),
                "{pattern} is outside {PREFIXES:?}"
            );
        }
        let worker_first = PREFIXES.map(|p| format!("\"{p}*\"")).join(", ");
        assert!(
            include_str!("../../../wrangler.toml")
                .contains(&format!("run_worker_first = [{worker_first}]")),
            "wrangler.toml's run_worker_first lists {worker_first}"
        );
        // The browser calls `/api` and `/ws`; SFU adapters reach `/rec` on the Worker itself.
        let dev_proxy = include_str!("../../../web/vite.config.ts");
        for browser in ["/api", "/ws"] {
            assert!(
                dev_proxy.contains(&format!("'{browser}': {{")),
                "web/vite.config.ts proxies {browser}"
            );
        }
        assert!(super::API_PREFIX == PREFIXES[0]);
    }
}
