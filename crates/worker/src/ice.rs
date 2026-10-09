//! `GET /api/ice`: the ICE servers for `RTCPeerConnection`. With a Realtime TURN key configured,
//! they include short-lived TURN credentials, which phones on mobile data may need; without one,
//! or if Cloudflare does not hand out credentials in time, STUN only.

use std::fmt;
use std::pin::pin;
use std::time::Duration;

use futures_util::future::{Either, select};

use serde::{Deserialize, Serialize};
use worker::{
    AbortController, AbortSignal, Delay, Env, Fetch, Headers, Method, Request, RequestInit,
    console_error, console_warn,
};

/// Secret: the id of the Realtime TURN key (dashboard → Realtime → TURN).
const TURN_KEY_ID: &str = "TURN_KEY_ID";
/// Secret: the TURN key's API token.
const TURN_KEY_API_TOKEN: &str = "TURN_KEY_API_TOKEN";
/// How long TURN credentials stay valid: 24 hours, longer than any meeting.
const TURN_TTL_SECS: u32 = 86_400;
/// How long Cloudflare may take to generate credentials before the route serves STUN only. The
/// browser gives up on the route after 4 seconds, so this answers first.
const TURN_TIMEOUT: Duration = Duration::from_secs(3);
/// Cloudflare's free STUN server.
const STUN_URL: &str = "stun:stun.cloudflare.com:3478";

/// The body of `GET /api/ice`, and of Cloudflare's `generate-ice-servers` response
/// (<https://developers.cloudflare.com/realtime/turn/generate-credentials/>, checked 2026-10-09).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IceServers {
    ice_servers: Vec<IceServer>,
}

/// An `RTCIceServer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IceServer {
    urls: Vec<String>,
    /// Present for TURN servers only.
    #[serde(flatten)]
    credentials: Option<TurnCredentials>,
}

/// A TURN server's username and password. Its `Debug` output hides the password.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TurnCredentials {
    username: String,
    credential: String,
}

impl fmt::Debug for TurnCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TurnCredentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// A Realtime TURN key: its id and API token. No `Debug`, because it holds the token.
struct TurnKey {
    id: String,
    token: String,
}

#[derive(Serialize)]
struct GenerateRequest {
    ttl: u32,
}

impl IceServers {
    fn stun_only() -> Self {
        Self {
            ice_servers: vec![IceServer {
                urls: vec![STUN_URL.to_owned()],
                credentials: None,
            }],
        }
    }

    /// Without the URLs on port 53, which browsers block, so ICE does not wait for them to time
    /// out (Cloudflare's docs advise this without trickle ICE); servers left without a URL are
    /// dropped. `None` if no server is left.
    fn usable_in_browsers(self) -> Option<Self> {
        let ice_servers: Vec<IceServer> = self
            .ice_servers
            .into_iter()
            .map(|server| IceServer {
                urls: server
                    .urls
                    .into_iter()
                    .filter(|url| !uses_port_53(url))
                    .collect(),
                ..server
            })
            .filter(|server| !server.urls.is_empty())
            .collect();
        (!ice_servers.is_empty()).then_some(Self { ice_servers })
    }
}

/// Whether an ICE URL (`scheme:host[:port][?query]`) names port 53.
fn uses_port_53(url: &str) -> bool {
    let after_scheme = url.split_once(':').map_or(url, |(_, rest)| rest);
    let host_port = after_scheme.split('?').next().unwrap_or(after_scheme);
    host_port
        .rsplit_once(':')
        .is_some_and(|(_, port)| port == "53")
}

/// The ICE servers to hand out: STUN plus TURN with fresh credentials, or STUN only (logged) if
/// no TURN key is configured or Cloudflare does not generate credentials.
pub async fn ice_servers(env: &Env) -> IceServers {
    let Some(key) = turn_key(env) else {
        console_warn!("{TURN_KEY_ID} or {TURN_KEY_API_TOKEN} is not set; serving STUN only");
        return IceServers::stun_only();
    };
    let controller = AbortController::default();
    let signal = controller.signal();
    let generating = pin!(generate(&key, &signal));
    let timeout = pin!(Delay::from(TURN_TIMEOUT));
    match select(generating, timeout).await {
        Either::Left((Ok(servers), _)) => servers.usable_in_browsers().unwrap_or_else(|| {
            console_error!("TURN credentials came without a usable URL; serving STUN only");
            IceServers::stun_only()
        }),
        Either::Left((Err(e), _)) => {
            console_error!("generating TURN credentials failed: {e}; serving STUN only");
            IceServers::stun_only()
        }
        Either::Right(((), _)) => {
            controller.abort();
            console_error!("no TURN credentials within {TURN_TIMEOUT:?}; serving STUN only");
            IceServers::stun_only()
        }
    }
}

/// The configured TURN key, if both secrets are set and not empty.
fn turn_key(env: &Env) -> Option<TurnKey> {
    let secret = |name| {
        env.secret(name)
            .ok()
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty())
    };
    Some(TurnKey {
        id: secret(TURN_KEY_ID)?,
        token: secret(TURN_KEY_API_TOKEN)?,
    })
}

/// `POST /turn/keys/{id}/credentials/generate-ice-servers`, which answers 201 with the servers.
async fn generate(key: &TurnKey, signal: &AbortSignal) -> worker::Result<IceServers> {
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {}", key.token))?;
    headers.set("Content-Type", "application/json")?;
    let body = serde_json::to_string(&GenerateRequest { ttl: TURN_TTL_SECS })?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(body.into()));
    let url = format!(
        "{}/turn/keys/{}/credentials/generate-ice-servers",
        sfu_api::BASE_URL,
        key.id
    );
    let mut response = Fetch::Request(Request::new_with_init(&url, &init)?)
        .send_with_signal(signal)
        .await?;
    match response.status_code() {
        200..300 => response.json().await,
        status => Err(worker::Error::RustError(format!("HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The shape of the example response in Cloudflare's docs (2026-10-09), with made-up values.
    fn generated() -> serde_json::Value {
        json!({
          "iceServers": [
            { "urls": ["stun:stun.cloudflare.com:3478", "stun:stun.cloudflare.com:53"] },
            {
              "urls": [
                "turn:turn.cloudflare.com:3478?transport=udp",
                "turn:turn.cloudflare.com:53?transport=udp",
                "turn:turn.cloudflare.com:3478?transport=tcp",
                "turns:turn.cloudflare.com:5349?transport=tcp",
                "turn:turn.cloudflare.com:80?transport=tcp",
                "turns:turn.cloudflare.com:443?transport=tcp"
              ],
              "username": "user",
              "credential": "secret"
            }
          ]
        })
    }

    #[test]
    fn generated_servers_lose_port_53_and_keep_their_credentials() {
        let servers: IceServers = serde_json::from_value(generated()).expect("docs shape");
        let usable = servers.usable_in_browsers().expect("servers left");
        assert_eq!(
            serde_json::to_value(usable).expect("serializable"),
            json!({
              "iceServers": [
                { "urls": ["stun:stun.cloudflare.com:3478"] },
                {
                  "urls": [
                    "turn:turn.cloudflare.com:3478?transport=udp",
                    "turn:turn.cloudflare.com:3478?transport=tcp",
                    "turns:turn.cloudflare.com:5349?transport=tcp",
                    "turn:turn.cloudflare.com:80?transport=tcp",
                    "turns:turn.cloudflare.com:443?transport=tcp"
                  ],
                  "username": "user",
                  "credential": "secret"
                }
              ]
            })
        );
    }

    #[test]
    fn only_port_53_counts_as_port_53() {
        assert!(uses_port_53("turn:turn.cloudflare.com:53?transport=udp"));
        assert!(uses_port_53("stun:stun.cloudflare.com:53"));
        for url in [
            "stun:stun.cloudflare.com:3478",
            "stun:stun.cloudflare.com",
            "turn:turn53.example:3478",
            "turn:[2001:db8::53]",
            "turns:turn.cloudflare.com:5349?transport=tcp",
        ] {
            assert!(!uses_port_53(url), "{url}");
        }
        let only_53: IceServers =
            serde_json::from_value(json!({ "iceServers": [{ "urls": ["stun:x:53"] }] }))
                .expect("valid");
        assert_eq!(only_53.usable_in_browsers(), None);
    }

    #[test]
    fn the_stun_only_list_is_what_the_route_served_before() {
        assert_eq!(
            serde_json::to_value(IceServers::stun_only()).expect("serializable"),
            json!({ "iceServers": [{ "urls": ["stun:stun.cloudflare.com:3478"] }] })
        );
    }
}
