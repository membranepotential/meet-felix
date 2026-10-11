//! Per-client rate limits (D6 in `docs/links`), with the Workers Rate Limiting binding
//! (<https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/>, checked
//! 2026-10-11): `[[ratelimits]]` in `wrangler.toml` sets each limit's count per 60 seconds.
//! Counters are kept per Cloudflare location and are eventually consistent, so a limit is a
//! bound on abuse, not an exact count.
//!
//! A client is its IP address from `CF-Connecting-IP`, which Cloudflare's edge sets on every
//! request from the internet; the custom-domain front door (`edge/`) forwards the request with
//! its headers unchanged.

use std::net::{IpAddr, Ipv6Addr};

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
/// The key of requests without a usable `CF-Connecting-IP`; they share one counter.
const UNKNOWN_CLIENT: &str = "unknown";

/// The client a request is counted for: its IPv4 address, or the /64 network of its IPv6
/// address, because one IPv6 subscriber usually holds a whole /64. Requests without a usable
/// address share [`UNKNOWN_CLIENT`].
pub fn client_key(connecting_ip: Option<&str>) -> String {
    match connecting_ip.and_then(|ip| ip.trim().parse::<IpAddr>().ok()) {
        Some(IpAddr::V4(ip)) => ip.to_string(),
        Some(IpAddr::V6(ip)) => match ip.to_ipv4_mapped() {
            Some(ip) => ip.to_string(),
            None => {
                let [a, b, c, d, ..] = ip.segments();
                format!("{}/64", Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
            }
        },
        None => UNKNOWN_CLIENT.to_owned(),
    }
}

/// Counts `req` under `limit` and answers whether it may go on. A missing or failing binding
/// lets the request through (logged): the limits bound abuse, and an outage of the counter
/// must not stop every meeting.
pub async fn allows(env: &Env, limit: Limit, req: &Request) -> Result<bool> {
    let header = req.headers().get("CF-Connecting-IP")?;
    if header.is_none() {
        console_error!("no CF-Connecting-IP; such requests share one {limit:?} limit");
    }
    let key = client_key(header.as_deref());
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
            (Some("203.0.113.7"), "203.0.113.7"),
            (Some(" 203.0.113.7 "), "203.0.113.7"),
            (Some("::ffff:203.0.113.7"), "203.0.113.7"),
            (Some("2001:db8:1:2:3:4:5:6"), "2001:db8:1:2::/64"),
            (Some("2001:db8:1:2::9"), "2001:db8:1:2::/64"),
            (Some("::1"), "::/64"),
            (None, UNKNOWN_CLIENT),
            (Some(""), UNKNOWN_CLIENT),
            (Some("203.0.113.7, 198.51.100.1"), UNKNOWN_CLIENT),
        ] {
            assert_eq!(client_key(header), key, "{header:?}");
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
