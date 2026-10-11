//! Per-client rate limits (D6 in `docs/links`), with the Workers Rate Limiting binding
//! (<https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/>, checked
//! 2026-10-11): `[[ratelimits]]` in `wrangler.toml` sets each limit's count per 60 seconds.
//! Counters are kept per Cloudflare location and are eventually consistent, so a limit is a
//! bound on abuse, not an exact count.
//!
//! A client is its IP address from `CF-Connecting-IP`, which Cloudflare's edge sets on every
//! request from the internet; the custom-domain front door (`edge/`) forwards the request with
//! its headers unchanged. Whether the header survives that service binding is unverified
//! (research 11, item 10), so a request without a usable address is not limited at all: one
//! counter for all of them would let a stranger's failed logins lock the owner out.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};

use worker::{Env, Request, Response, Result, console_error};

/// A rate limit and the binding that counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// The routes anyone may call: meeting lookups, ICE servers and WebSocket joins. Bounds
    /// how fast one client can guess meeting words or mint TURN credentials.
    Public,
    /// Every comparison with the owner's password: logins and bearer tokens. Counts attempts,
    /// not failures: the binding cannot read a counter without raising it, and refusing only
    /// after failures would still answer a right guess.
    OwnerAuth,
}

impl Limit {
    fn binding(self) -> &'static str {
        match self {
            Self::Public => "PUBLIC_LIMIT",
            Self::OwnerAuth => "OWNER_AUTH_LIMIT",
        }
    }
}

/// The window of every limit in `wrangler.toml`, sent as `Retry-After`.
const PERIOD_SECS: u32 = 60;
/// Whether this isolate has logged a request without a usable `CF-Connecting-IP`.
static LOGGED_NO_CLIENT: AtomicBool = AtomicBool::new(false);

/// The client a request is counted for: its IPv4 address, or the /64 network of its IPv6
/// address, because one IPv6 subscriber usually holds a whole /64. `None` without a usable
/// address.
pub fn client_key(connecting_ip: Option<&str>) -> Option<String> {
    Some(match connecting_ip?.trim().parse::<IpAddr>().ok()? {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => ip.to_string(),
            None => {
                let [a, b, c, d, ..] = ip.segments();
                format!("{}/64", Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
            }
        },
    })
}

/// Counts `req` under `limit` and answers whether it may go on. A request without a usable
/// `CF-Connecting-IP`, a missing binding and a failing one let the request through (logged):
/// the limits bound abuse, and neither a lost header nor an outage of the counter may stop
/// meetings or lock the owner out.
pub async fn allows(env: &Env, limit: Limit, req: &Request) -> Result<bool> {
    let Some(key) = client_key(req.headers().get("CF-Connecting-IP")?.as_deref()) else {
        if !LOGGED_NO_CLIENT.swap(true, Ordering::Relaxed) {
            console_error!(
                "no usable CF-Connecting-IP; requests without one are not rate-limited \
                 (logged once per isolate)"
            );
        }
        return Ok(true);
    };
    let outcome = match env.rate_limiter(limit.binding()) {
        Ok(limiter) => limiter.limit(key).await,
        Err(e) => Err(e),
    };
    Ok(outcome.map_or_else(
        |e| {
            console_error!("rate limit {limit:?} unavailable, not applied: {e}");
            true
        },
        |outcome| outcome.success,
    ))
}

/// The answer to a client over its limit: 429 with `Retry-After`.
pub fn too_many_requests() -> Result<Response> {
    let response = Response::error("too many requests", 429)?;
    response
        .headers()
        .set("Retry-After", &PERIOD_SECS.to_string())?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_are_ipv4_addresses_or_ipv6_networks() {
        for (header, key) in [
            (Some("203.0.113.7"), Some("203.0.113.7")),
            (Some(" 203.0.113.7 "), Some("203.0.113.7")),
            (Some("::ffff:203.0.113.7"), Some("203.0.113.7")),
            (Some("2001:db8:1:2:3:4:5:6"), Some("2001:db8:1:2::/64")),
            (Some("2001:db8:1:2::9"), Some("2001:db8:1:2::/64")),
            (Some("::1"), Some("::/64")),
            (None, None),
            (Some(""), None),
            (Some("203.0.113.7, 198.51.100.1"), None),
        ] {
            assert_eq!(client_key(header).as_deref(), key, "{header:?}");
        }
    }

    #[test]
    fn addresses_in_one_ipv6_network_share_a_key() {
        assert_eq!(
            client_key(Some("2001:db8:aa:bb::1")),
            client_key(Some("2001:db8:aa:bb:ffff:ffff:ffff:ffff"))
        );
        assert_ne!(
            client_key(Some("2001:db8:aa:bb::1")),
            client_key(Some("2001:db8:aa:bc::1"))
        );
    }
}
