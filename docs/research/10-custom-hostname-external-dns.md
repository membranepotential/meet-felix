# Serve the Worker as `meet.example.com` while DNS stays at the external provider (checked 2026-10-09)

Source note: Cloudflare docs were fetched on 2026-10-09 as their `index.md` raw variants (with `curl` or WebFetch); the "Last updated" date of each page is given next to it. WebFetch summarises pages, so short quotes from it may be paraphrased. Hetzner prices come from the Hetzner Cloud API (public pricing API) on 2026-10-09. "Tested" marks something I ran myself on 2026-10-09. "Inference" marks my own reasoning. "[U]" marks a claim I could not confirm from a primary source.

## Takeaway

- **A bare CNAME `meet.example.com -> <worker>.<account>.workers.dev` does not work.** Tested: Cloudflare aborts the TLS handshake (`ssl/tls alert handshake failure`) because it has no certificate for a hostname that is not in any Cloudflare account. Over plain HTTP the same request gets `530`, `error code: 1016`.
- **Workers Custom Domains and Workers Routes still need the zone's nameservers on Cloudflare.** The Pages migration guide (updated 2026-09-22) says so directly: "Unlike Pages, Workers does not support any domain whose nameservers are not managed by Cloudflare." I found no 2025–2026 change to this.
- **Recommended: a Cloudflare Pages project on `meet.example.com` (CNAME at the DNS provider to `<project>.pages.dev`) whose `_worker.js` forwards requests to the Rust Worker through a service binding.** It costs nothing, keeps DNS at an external provider, shows the real URL, and keeps all traffic inside Cloudflare. Pages custom domains on external DNS are still documented (2026-04-21). Pages Functions can bind to a separately deployed Worker and to its Durable Objects. Tested locally in `workerd`: a WebSocket upgrade passes through a service binding into a Durable Object, and the `Host` header is preserved. A production test on Cloudflare is still open. The risk is that Cloudflare tells new projects to use Workers instead of Pages, but it has announced no end date.
- **Second choice: Cloudflare for SaaS with a cheap domain as the "SaaS zone".** This is Cloudflare's official product for hostnames on foreign DNS. The first 100 custom hostnames are free, but it needs a second domain whose nameservers are on Cloudflare (about US$5–11 per year) and a payment method on file.
- **A relay VM works but costs more and adds a hop.** The cheapest Hetzner Cloud server that can run Caddy costs EUR 5.99 net / 7.13 gross per month (CX23 + IPv4), or EUR 6.49 / 7.72 (CPX02 + IPv4) if CX23 cannot be ordered. No Hetzner managed product can relay or redirect without a VM.
- **A path-preserving redirect to `workers.dev` (GitHub Pages, free) is the last resort**, because the address bar then shows `workers.dev`.

## 1. Why a bare CNAME to `workers.dev` fails

### Findings
- Tested (2026-10-09, curl 8.x from a residential line, against the public Worker `github-oauth-login.gr2m.workers.dev`, IP 104.21.17.181). I used `meet-test.debian.org` as the foreign hostname because `debian.org` is not on Cloudflare DNS:
  | Request | Result |
  |---|---|
  | `https://<worker>.workers.dev/` (normal) | `302` from the Worker |
  | HTTPS with SNI and `Host` = foreign name, sent to the Worker's IP (exactly what a CNAME produces) | TLS alert `handshake failure`, no HTTP response at all |
  | Plain HTTP port 80 with `Host` = foreign name | `530`, body `error code: 1016` |
  | HTTPS with SNI = `<worker>.workers.dev` but `Host` = foreign name (a relay that forgets to rewrite `Host`) | `403 Forbidden` from `cloudflare` |
  | HTTPS with SNI and `Host` = `<worker>.workers.dev` plus `X-Forwarded-Host: <foreign>` (a correct relay) | `302` from the Worker |
- Error 1001 page (updated 2026-09-10) lists as a cause: "An external domain that is not on using Cloudflare has a CNAME record to a domain active on Cloudflare", and "A web request was sent to a Cloudflare IP address for a non-existent Cloudflare domain." — [error 1001](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1001/)
- Error 1016 page (updated 2026-04-23): "Cloudflare cannot resolve the origin web server's IP address"; causes include "There is no DNS record for the hostname in the target". — [error 1016](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1016/)
- Error 1014 (CNAME Cross-User Banned) applies when the CNAME source is itself a proxied Cloudflare zone in another account; the fix Cloudflare names is Cloudflare for SaaS on the target side. Not our case, because `example.com` is not on Cloudflare. — [error 1014](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1014)
- The `workers.dev` docs (updated 2026-09-22) recommend a route or Custom Domain for production; `workers.dev` is "intended for personal or hobby projects that aren't business-critical". The page says nothing about external CNAMEs. — [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)

### Inference
Two things fail, in this order: (1) TLS, because Cloudflare only presents certificates for hostnames that one of its products (zone, Pages custom domain, SaaS custom hostname) has registered; (2) routing, because even past TLS the edge maps `Host` to a zone or Worker, and an unknown `Host` gets 1001/1016. Which error code a browser would see does not matter: HTTPS fails at the handshake.

## 2. Cloudflare Pages custom domain + proxy to the Rust Worker (recommended)

### Findings: Pages custom domains on external DNS still work
- "If you are deploying to a subdomain, it is not necessary for your site to be a Cloudflare zone. You will need to add a custom CNAME record … `shop.example.com` → `<YOUR_SITE>.pages.dev`." You must first add the domain in the Pages dashboard; a CNAME added without that gives a `522`. CAA records must allow Cloudflare's CAs (Let's Encrypt, Google Trust Services, SSL.com). — [Pages custom domains](https://developers.cloudflare.com/pages/configuration/custom-domains/) (updated 2026-04-21)
- The Workers migration guide's comparison matrix: "Custom domains outside Cloudflare zones" = ✅ Pages, ❌ Workers; "Unlike Pages, Workers does not support any domain whose nameservers are not managed by Cloudflare." — [Migrate from Pages](https://developers.cloudflare.com/workers/static-assets/migration-guides/migrate-from-pages/) (updated 2026-09-22)
- Pages status: the Pages overview opens with "Are you sure you want to use Pages? Workers supports most Pages use cases and offers a broader feature set … Start new projects with Workers." It still says "Available on all plans" and documents Git, Direct Upload and C3 deploys. — [Pages overview](https://developers.cloudflare.com/pages/) (updated 2026-08-25). No official end-of-life date found; a September 2026 GitHub issue and 2026 blog posts agree there is no forced migration. — [search summary, U for the third-party posts](https://cogley.jp/articles/cloudflare-pages-to-workers-migration)
- `example.com` has no CAA records (my `dig CAA example.com`, 2026-10-09), so certificate issuance is not blocked.

### Findings: can a Pages project front the Rust Worker?
- Advanced mode: a `_worker.js` (Module Worker syntax) in the output directory takes over all requests; it must call `env.ASSETS.fetch(request)` for static files. — [Advanced mode](https://developers.cloudflare.com/pages/functions/advanced-mode/) (updated 2026-04-21)
- Service bindings from Pages: "Service bindings enable you to call a Worker from within your Pages Function", example `return context.env.SERVICE.fetch(context.request);`. — [Pages bindings](https://developers.cloudflare.com/pages/functions/bindings/) (updated 2026-06-25)
- Durable Objects from Pages: "You cannot create and deploy a Durable Object within a Pages project"; you bind to a DO class in a separate Worker. "Durable Object bindings configured in a Pages project's Wrangler configuration file require the `script_name` key." — [Pages bindings](https://developers.cloudflare.com/pages/functions/bindings/), [Pages Wrangler config](https://developers.cloudflare.com/pages/functions/wrangler-configuration/) (updated 2026-06-25)
- WASM: Pages Functions can import `.wasm` modules. — [Module support](https://developers.cloudflare.com/pages/functions/module-support/) (updated 2026-04-21). Not needed in this design, because the Rust code stays in its own Worker.
- Cost: "Requests to your Functions are billed as Cloudflare Workers requests"; static requests that do not invoke a Function are free and unlimited; `_routes.json` can exclude static paths (max 100 rules). — [Pages Functions pricing](https://developers.cloudflare.com/pages/functions/pricing/) (updated 2026-09-08), [routing](https://developers.cloudflare.com/pages/functions/routing/) (updated 2026-09-18). "Requests made from your Worker to another worker via a Service Binding do not incur additional request fees." — [Workers pricing](https://developers.cloudflare.com/workers/platform/pricing/) (updated 2026-10-02)
- Service binding limits: each call counts toward the subrequest limit; max 32 Worker invocations per request. — [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) (updated 2026-10-07). Neither the service-binding page nor the HTTP-interface page mentions WebSockets. — [Service bindings HTTP](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/http/) (updated 2026-04-23)
- Forwarding a WebSocket upgrade with `stub.fetch(request)` from a Worker into a DO is the documented pattern. — [DO WebSockets](https://developers.cloudflare.com/durable-objects/best-practices/websockets/) (updated 2026-09-30)
- **Tested locally** (workerd 1.20260930.2, compatibility date 2026-09-01; files in the session scratchpad): a "front" Worker standing in for `_worker.js` with (a) a service binding `BACK` to a second Worker that forwards `/ws/*` to a hibernating DO, and (b) a cross-script DO binding (`durableObjectNamespace = (className = "Room", serviceName = "back")`). Both paths completed the WebSocket handshake and echoed a message. The back Worker saw the original `Host` header. This is the same runtime that Cloudflare runs, but not a production deployment.

### Inference: the design and its limits
```js
// _worker.js in the Pages output directory (Pages project "meet")
export default {
  fetch(request, env) {
    return env.MEET.fetch(request); // service binding to the Rust Worker; WebSocket upgrades included
  },
};
```
- The simplest variant forwards everything, including static files, to the Rust Worker, which keeps its own static assets. One Pages Function invocation per request is billed as a Workers request; the service-binding hop is free. For a personal meeting app this stays far below the free 100,000 requests/day. To save requests, copy the static files into the Pages output and exclude them with `_routes.json`; that adds a second copy of the assets to deploy.
- The Rust Worker can set `workers_dev = false`, so it is reachable only through the binding. Service bindings do not need `workers.dev`.
- The Rust Worker sees `Host: meet.example.com`, so absolute URLs and `Origin` checks work unchanged.
- Limits: two deployables (Pages project + Worker); Pages is de-emphasised by Cloudflare, so a future migration deadline is possible; Pages Functions lack some Workers-only features (Cron Triggers, gradual deployments, Workers Logs per the migration matrix), which does not matter for a 5-line proxy.
- Open: a production test that a WebSocket to `wss://meet.example.com/ws/...` survives the Pages → service binding → Rust Worker → DO chain. I expect it to pass because workerd passed, but I have not run it on Cloudflare.

### Steps
1. Deploy the Rust Worker as today (name e.g. `meet-felix`).
2. Create a Pages project (Direct Upload: `wrangler pages deploy <dir> --project-name meet`) whose output directory contains `_worker.js` above, and a Pages Wrangler config with `services = [{ binding = "MEET", service = "meet-felix" }]`. The advanced-mode page only mentions deploying through Git integration; that Direct Upload also picks up `_worker.js` is common practice but [U].
3. In the dashboard: Workers & Pages → `meet` → Custom domains → Set up a domain → `meet.example.com`.
4. At the DNS provider: a `CNAME` record `meet` with value `meet.pages.dev.` (the trailing dot makes the target absolute; the provider's behaviour without it was not checked, third-party docs say relative names are appended to the zone — [Ansible hetzner_dns_record_set](https://docs.ansible.com/ansible/latest/collections/community/dns/hetzner_dns_record_sets_module.html)).
5. Wait until the custom domain shows Active, then open `https://meet.example.com` and a WebSocket to `/ws/...`.

## 3. Workers Custom Domains or Routes on external DNS

### Findings
- Workers Custom Domains require "An active Cloudflare zone", and "cannot [be created] on a hostname with an existing CNAME DNS record". — [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) (updated 2026-09-29)
- Migration guide: "Workers does not support any domain whose nameservers are not managed by Cloudflare." (updated 2026-09-22, see section 2)
- Partial (CNAME) zone setup, which would let a zone stay on external DNS: "only available to customers on a Business or Enterprise plan". — [Partial setup](https://developers.cloudflare.com/dns/zone-setups/partial-setup/) (updated 2026-08-14). Subdomain zones are Enterprise-only (given in the task).
- 2026 changelog search: a "Domains tab" in the Workers dashboard (2026-05-14) lets you buy or add domains; nothing for external DNS. — [changelog](https://developers.cloudflare.com/changelog/post/2026-05-14-domains-tab/)
- Workers for Platforms custom hostnames use the same Cloudflare for SaaS mechanism (customer CNAME to the SaaS zone) and are a paid product. — [WfP hostname routing](https://developers.cloudflare.com/cloudflare-for-platforms/workers-for-platforms/configuration/hostname-routing/)

### Conclusion
Not possible on Free/Pro. No 2025–2026 change found.

## 4. Cloudflare for SaaS (custom hostnames) with a separate SaaS zone

### Findings
- Plans (updated 2026-08-14): Free, Pro, Business include **100** custom hostnames; max 50,000; **US$0.10** per additional hostname. — [SaaS plans](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/plans/)
- "Cloudflare for SaaS is bundled with non-Enterprise plans." — [SaaS overview](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/) (updated 2026-04-29). Enabling it on a non-Enterprise zone: "Enter payment information." — [Enable](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/start/enable/) (updated 2026-04-29)
- Getting started: "Add your zone to Cloudflare on a Free plan"; create a proxied fallback origin record; the customer creates a CNAME at their own DNS to your CNAME target; "Do not configure a custom hostname which matches the zone name"; ready when `status` and `ssl.status` are both `active`. — [Getting started](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/start/getting-started/) (updated 2026-06-19)
- Hostname validation: real-time validation "occurs automatically when your customer adds their" CNAME and "does not require any additional actions". — [Real-time validation](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/domain-support/hostname-validation/realtime-validation/) (updated 2026-04-15)
- Certificate validation: methods are TXT, Delegated DCV, and automatic HTTP. With HTTP DCV, "all your customers have to do is add a CNAME to your `$CNAME_TARGET`"; Cloudflare attempts HTTP validation once the hostname points at the target, even if TXT was selected. — [HTTP DCV](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/security/certificate-management/issue-and-validate/validate-certificates/http/) (updated 2026-09-24)
- Worker as origin (updated 2026-06-19): fallback origin as an originless record (`AAAA 100::`); a Worker route `*/*` "matches all traffic entering your zone", including custom hostname traffic; alternatively route only `vanity.customer.com`; `custom_origin_server` "is bypassed because the Worker processes the request before origin resolution occurs"; per-hostname logic via `request.headers.get("host")`. WebSockets and DOs are not mentioned. — [Workers as your fallback origin](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/start/advanced-settings/worker-as-origin/)
- "WebSockets are supported on all Cloudflare plans"; Cloudflare closes idle WebSockets and may close them during releases. — [Network WebSockets](https://developers.cloudflare.com/network/websockets/) (updated 2026-08-14)
- SaaS-zone domain cost: Cloudflare Registrar charges "at cost, without markup fees" — [Registrar](https://developers.cloudflare.com/registrar/) (updated 2026-04-24). `.de` is not on Cloudflare's TLD list — [TLD policies](https://www.cloudflare.com/tld-policies/). Prices from a third-party price tracker (updated 2026-10-09, [U]): `.uk` US$5.30/yr, `.cc` US$8.00/yr, `.com` US$10.46/yr, `.dev` US$8.20 first year then US$12.20. — [cfdomainpricing.com](https://cfdomainpricing.com/). Any domain from any registrar works as long as its nameservers point to Cloudflare (full setup on the Free plan).
- DNS provider side: the provider must accept CNAME and TXT records, so the CNAME and any TXT pre-validation records can be set from the CLI.

### Inference: steps
1. Get a cheap domain, e.g. `example-saas.uk`, on Cloudflare (Free plan, full setup).
2. In that zone: add `AAAA origin 100::` (proxied), enable Custom Hostnames (payment method), set `origin.example-saas.uk` as the fallback origin.
3. Add a Worker route for the Rust Worker: `meet.example.com/*` (or `*/*`) on that zone. Whether `wrangler.toml` accepts a custom hostname in `routes` with `zone_name = "example-saas.uk"` is [U]; the dashboard path is documented.
4. Create custom hostname `meet.example.com` (certificate method HTTP, or TXT if you want the certificate before the DNS cutover).
5. At the DNS provider: a `CNAME` record `meet` with value `origin.example-saas.uk.`. For TXT pre-validation: a `TXT` record `<name from Cloudflare>` with value `'"<value>"'`.
6. Wait for `status` and `ssl.status` = `active`.
- The Worker is invoked directly on `meet.example.com`, with no proxy code and the real `Host`. WebSockets and DOs behave as on any Workers route (inference; not stated on the SaaS pages).
- Costs: US$5–11/yr for the extra domain; US$0 for one hostname. Risk: one more domain to renew, a card on file, and more moving parts than Pages.

## 5. Other mechanisms

### Relay on a new Hetzner Cloud server (Caddy)
- Prices (Hetzner Cloud API, 2026-10-09, fsn1/nbg1/hel1 identical; VAT 19 %): CX23 (2 vCPU, 4 GB) EUR 5.49 net/month; CPX02 (1 vCPU, 1 GB, new) EUR 5.99; CAX11 EUR 5.99; CPX12 EUR 11.49. Primary IPv4 EUR 0.50 net (0.595 gross)/month. 20 TB traffic included, EUR 1.00/TB beyond. The API lists CX23 and CPX02 as available in fsn1 and nbg1, while the marketing page shows "Currently not available" near the shared-resource plans — [Hetzner Cloud](https://www.hetzner.com/cloud/), so CX23 orders may fail [U]. A third-party tracker reports CPX02 was added on 2026-10-06 at EUR 6.49 (= 5.99 + IPv4) — [agentdeals issue](https://github.com/robhunter/agentdeals/issues/2473).
  | Server | Net/month incl. IPv4 | Gross (19 %) |
  |---|---|---|
  | CX23 | EUR 5.99 | EUR 7.13 |
  | CPX02 | EUR 6.49 | EUR 7.72 |
- IPv6-only would save EUR 0.50, but "Servers that only have a Primary IPv6 do not have public IPv4 connectivity" — [Primary IPs FAQ](https://docs.hetzner.com/cloud/servers/primary-ips/faq/), so IPv4-only meeting guests could not connect. Not acceptable for meeting links.
- Caddy: "Since Caddy v2.11.0" the `Host` header is set to the upstream automatically for HTTPS upstreams (older versions: `header_up Host {upstream_hostport}`); Caddy sets `X-Forwarded-For`, `X-Forwarded-Proto`, `X-Forwarded-Host`; "The proxy also supports WebSocket connections"; by default "WebSocket connections are forcibly closed" on config reload unless `stream_close_delay` is set. — [Caddy reverse_proxy](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy)
  ```caddyfile
  meet.example.com {
      reverse_proxy https://meet-felix.<account>.workers.dev {
          header_up Host {upstream_hostport}   # automatic since Caddy 2.11, explicit for older builds
          transport http {
              tls_server_name meet-felix.<account>.workers.dev
          }
          stream_close_delay 4h                # keep WebSockets open across Caddy reloads
      }
  }
  ```
- Cloudflare side: the tested rows in section 1 show that `workers.dev` accepts a request with the right SNI and `Host` regardless of the client, and rejects a foreign `Host` with 403. I found no documented restriction or bot challenge on `workers.dev` for datacenter clients; `workers.dev` has no zone-level WAF settings to configure (inference; my test ran from a residential IP, not from Hetzner).
- Side effects (inference): the Worker sees `Host: <worker>.workers.dev` and the relay's IP in `CF-Connecting-IP`, so the app must take the public hostname from `X-Forwarded-Host` or config, and IP-based limits must read `X-Forwarded-For`. The browser's `Origin: https://meet.example.com` passes through unchanged, so an `Origin == Host` check would fail. `workers_dev` must stay enabled, so the `workers.dev` URL also stays public.
- Latency (measured from a residential line, 2026-10-09, `ping -c 5`): Cloudflare edge (nearest colo) 9.3 ms, `nbg.icmp.hetzner.com` 23.0 ms, `fsn.icmp.hetzner.com` 24.9 ms. Hetzner → Cloudflare Frankfurt was not measured; a forum post says Falkenstein reaches the internet via Nuremberg with about 3 ms between them — [LowEndTalk, U](https://lowendtalk.com/discussion/comment/3129986/). Estimate: the relay adds roughly 15–20 ms per round trip for signaling and page loads. Media is not affected: WebRTC still flows browser ↔ Cloudflare Realtime SFU directly.
- Dependency: signaling and static files then depend on one VM that must be patched and monitored; if it is down, nobody can join, although running media sessions continue until they need signaling.

### Hetzner managed products without a VM
- Load Balancer (LB11 EUR 6.99 net/month): targets are "the public or private IPv4 of a cloud server" or dedicated root servers of the same owner; no arbitrary external IPs; HTTP services talk "HTTP" to targets (HTTPS only as raw TCP); no `Host` rewrite documented. — [LB FAQ](https://docs.hetzner.com/cloud/load-balancers/faq/). Cannot relay to Cloudflare, and costs more than a server.
- Object Storage: "Custom domains for Buckets — Not supported"; "Website" is listed under notable exceptions. — [Supported actions](https://docs.hetzner.com/storage/object-storage/supported-actions/). Cannot serve `meet.example.com`, not even a redirect.
- Conclusion: **no Hetzner managed product can relay or redirect `meet.example.com` without a VM.** Hetzner Webhosting (`.htaccess` redirects) was not researched.

### Fly.io relay
- No free tier for new organisations; shared IPv4 and IPv6 free; first 10 single-hostname certificates free; shared-cpu-1x 256 MB US$2.19/month in iad (Europe slightly higher, the page shows no static EU figure); stopped Machines cost US$0.15/GB-month of rootfs; EU egress US$0.02/GB. — [Fly pricing](https://docs.fly.io/about/pricing)
- Inference: Caddy (or the Fly proxy plus a tiny Rust/Go proxy) on one auto-stopping Machine would cost about US$0.3–2.5/month and handle WebSockets. It is cheaper than Hetzner but adds a third provider, a cold start on the first request, and the same relay side effects as above.

### Deno Deploy, Vercel, Netlify
- Netlify: staff answer "We do not support proxying websockets at this time." — [Netlify forum](https://answers.netlify.com/t/does-netlify-support-websocket-proxying/11230). Rewrites time out after 26 s. **Does not work** for `/ws/*`.
- Vercel: WebSockets in Functions are a public beta (June 2026) with connections cut after about 5 minutes in one test; external WebSocket rewrites unconfirmed — [Vercel changelog](https://vercel.com/changelog/websocket-support-is-now-available-for-python-functions), [classmethod test](https://dev.classmethod.jp/en/articles/vercel-functions-websocket-public-beta-verification/). **Not suitable** for hour-long meetings.
- Deno Deploy: free plan 1M requests, 20 GiB egress, 10 h CPU, 5 custom domains — [Deno pricing](https://deno.com/deploy/pricing). One version of the acceptable-use policy lists "reverse proxy" as acceptable, another bans "Proxy or VPN" — [Deno AUP](https://docs.deno.com/deploy/acceptable_use_policy/). Would need hand-written WebSocket piping. Possible but policy-ambiguous; not recommended.

### Cloudflare Tunnel, Snippets/Rules, orange-to-orange
- Tunnel: "The `cfargotunnel.com` subdomain only proxies traffic for DNS records in the same Cloudflare account" — [Tunnel DNS](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/routing-to-tunnel/dns/) (updated 2026-04-17). It also needs an origin machine running `cloudflared`. Does not apply.
- Snippets, Redirect/Origin Rules: they run on a zone's traffic, so they need the zone on Cloudflare (inference). Do not apply.
- Orange-to-orange is the case where the customer's zone is itself on Cloudflare and CNAMEs to a SaaS provider; it does not help when `example.com` is at an external DNS provider (inference).

### Path-preserving redirect (last resort)
- GitHub Pages: a subdomain CNAME to `<user>.github.io` with "Enforce HTTPS" — [GitHub Pages custom domain](https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/managing-a-custom-domain-for-your-github-pages-site). The apex `example.com` already uses GitHub Pages; `meet` would need its own repository with a `CNAME` file. A `404.html` with `location.replace("https://meet-felix.<account>.workers.dev" + location.pathname + location.search + location.hash)` preserves the path (inference; the `#` fragment survives only because the redirect runs in JavaScript). Free, but the meeting runs on the `workers.dev` URL.

## Ranked recommendation

Ranked by the owner's priorities: DNS stays at the external provider, lowest monthly cost, no home server, real `meet.example.com` in the address bar.

| # | Option | What it needs | Cost | WebSocket / DO | Operational risk |
|---|---|---|---|---|---|
| 1 | **Pages project on `meet.example.com` + `_worker.js` forwarding via service binding to the Rust Worker** | Pages project with 5-line `_worker.js` and a service binding; CNAME `meet → <project>.pages.dev` at the DNS provider | **US$0** (Pages Function requests count toward the free 100k/day; binding hop free) | Yes: binding passes the upgrade and `Host` (tested in local workerd; production test open). DOs stay in the Rust Worker | Low–medium: Cloudflare says "Start new projects with Workers"; no EOL announced. Two deployables |
| 2 | **Cloudflare for SaaS on a cheap SaaS zone** | Second domain with Cloudflare nameservers, payment method on file, fallback origin `AAAA 100::`, Worker route, custom hostname; CNAME at the DNS provider | **US$5–11/year** (domain) + US$0 (≤100 hostnames) | Yes: Worker invoked directly on `meet.example.com` (inference; WebSockets on all plans) | Low: official, documented product. Extra domain to renew; more setup steps |
| 3 | Fly.io Machine relay (Caddy) | Fly account with card, one auto-stopping Machine, free cert; CNAME at the DNS provider | ~US$0.3–2.5/month (estimate) | Yes (Caddy proxies WebSockets) | Medium: third provider, cold start, extra hop, Worker sees relay IP and `workers.dev` host |
| 4 | Hetzner Cloud relay (Caddy) | New CX23 (or CPX02) + IPv4, Caddy, OS updates; CNAME or A/AAAA at the DNS provider | **EUR 7.13 gross/month** (CX23) or EUR 7.72 (CPX02) | Yes (Caddy proxies WebSockets; set `stream_close_delay`) | Medium: VM to patch, single point of failure for joining, +15–20 ms per signaling round trip, same host/IP side effects as #3 |
| 5 | GitHub Pages JS redirect to `workers.dev` | Separate repo with `CNAME` + `404.html` redirect | US$0 | Yes, but on `workers.dev` | Low, but fails the "real URL" priority |
| — | Bare CNAME to `workers.dev` | — | — | — | **Does not work**: TLS handshake failure (tested) |
| — | Workers Custom Domain / Route | Zone on Cloudflare nameservers | — | — | **Does not work** with external DNS |
| — | Partial (CNAME) setup / subdomain zone | Business / Enterprise plan | Business+ | — | **Out of budget** |
| — | Hetzner Load Balancer / Object Storage | — | — | — | **Does not work**: no external targets / no custom domains |
| — | Netlify proxy | — | — | No WebSocket proxying | **Does not work** |
| — | Vercel rewrite, Deno Deploy proxy | — | Free tier | Vercel: ~5 min connection cap; Deno: AUP ambiguous | **Not recommended** |
| — | Cloudflare Tunnel, Snippets, Rules | Zone on Cloudflare, origin machine | — | — | **Does not apply** |

## Gaps

1. Production test of option 1: Pages custom domain on external DNS + WebSocket through the service binding into the Rust Worker's DO. Only the local workerd run was done.
2. Whether `wrangler` accepts a custom-hostname route (`meet.example.com/*`) on the SaaS zone in `routes`; documented only for the dashboard.
3. Hetzner → Cloudflare RTT was not measured; the relay latency figure is an estimate.
4. Whether `workers.dev` challenges or rate-limits requests from Hetzner datacenter IPs; my test ran from a residential IP.
5. Whether CX23 can actually be ordered today (API says available, website suggests otherwise).
6. Cloudflare Registrar TLD prices come from a third-party tracker, not from Cloudflare.
7. Hetzner's handling of CNAME values without a trailing dot (third-party docs only).
