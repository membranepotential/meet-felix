//! The Realtime SFU client over `worker::Fetch`.

use sfu_api::{AppId, AppSecret, HttpRequest, HttpResponse, SfuClient, SfuTransport};
use worker::{Env, Fetch, Headers, Method, Request, RequestInit};

/// Secret: the SFU application's id.
const CALLS_APP_ID: &str = "CALLS_APP_ID";
/// Secret: the SFU application's secret.
const CALLS_APP_SECRET: &str = "CALLS_APP_SECRET";
/// Optional var replacing the API's base URL, for a local mock SFU (see the spike script).
/// Unset in `wrangler.toml`, so deployed Workers talk to Cloudflare.
const SFU_API_URL: &str = "SFU_API_URL";

/// Sends the client's requests with the Workers `fetch`.
pub struct FetchTransport;

impl SfuTransport for FetchTransport {
    type Error = worker::Error;

    async fn send(&self, request: HttpRequest) -> worker::Result<HttpResponse> {
        let headers = Headers::new();
        headers.set("Authorization", &request.authorization)?;
        let mut init = RequestInit::new();
        init.with_method(match request.method {
            sfu_api::Method::Post => Method::Post,
            sfu_api::Method::Put => Method::Put,
        });
        if let Some(body) = request.body {
            headers.set("Content-Type", "application/json")?;
            init.with_body(Some(body.into()));
        }
        init.with_headers(headers);
        let mut response = Fetch::Request(Request::new_with_init(&request.url, &init)?)
            .send()
            .await?;
        Ok(HttpResponse {
            status: response.status_code(),
            body: response.text().await?,
        })
    }
}

/// The client for the configured SFU app; `None` without credentials, in which case every SFU
/// effect fails.
pub fn client(env: &Env) -> Option<SfuClient<FetchTransport>> {
    let app_id = env.secret(CALLS_APP_ID).ok()?.to_string();
    let secret = env.secret(CALLS_APP_SECRET).ok()?.to_string();
    let client = SfuClient::new(FetchTransport, AppId::new(app_id), AppSecret::new(secret));
    Some(match env.var(SFU_API_URL) {
        Ok(base) => client.with_base_url(base.to_string()),
        Err(_) => client,
    })
}
