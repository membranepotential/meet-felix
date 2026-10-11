# meet-felix

A personal, self-hosted, serverless video meeting app. One owner is the only
host; everyone else joins as a guest through a link. It is made for small
meetings (2–10 people) and runs entirely on Cloudflare: the Realtime
SFU carries the media, Rust Durable Objects hold the meetings, and a Svelte
app in the browser calls a pure Rust core compiled to WASM. Nothing runs, and
nothing is billed, while no meeting is on. It needs Safari / iOS 16, Chrome 97
or Firefox 102 or newer.

**Status:** in daily use by its owner since October 2026. Phases 1–3 of the
[plan](docs/architecture/README.md#phases) are done; mobile polish and
operations (phases 4–5) are open. See [STATUS.md](STATUS.md).

## Features

- **Three-word links** such as `amber-otter-lamp`. A link lives only while
  its meeting runs; scheduled meetings keep theirs until they start.
- **One host.** The owner logs in with one password, creates and schedules
  meetings, and can mute, kick, lock and end them. Guests need only the link and a name.
- **Phones and desktops** in a plain browser tab: simulcast video with a layer
  per viewer, screen share on desktop, device switching inside the meeting,
  and reconnecting after a dropped connection, with a 60-second grace period.
- **Announced recording.** When the host records, every participant sees it
  and joining means consent. Each speaker's audio is captured on the server
  and deleted 48 hours after the meeting.
- **Transcript after the meeting** with Whisper on Workers AI, in German and
  English, labelled by speaker with wall-clock times, behind an unguessable
  link.
- **EU data location.** Meeting state and recordings are pinned to the EU
  jurisdiction.

## How it works

```
browser (Svelte + meeting-core as WASM) ──WebSocket──▶ Worker ─▶ RoomObject (Durable Object, meeting-core)
        │                                                  ├──▶ RecorderObject ─▶ R2 (audio, transcripts)
        └──────────── WebRTC media ──▶ Cloudflare Realtime SFU ──WebSocket adapter──┘      └─▶ Workers AI (Whisper)
```

| Path | What it is |
|---|---|
| `crates/meeting-core` | The pure core: `RoomState`, `Event`, `Effect`, `step()`. No I/O, clocks or randomness; all protocol types live here, and the TypeScript types are generated from them. |
| `crates/sfu-api` | A typed client for the Realtime SFU HTTPS API and the WebSocket adapter's protobuf. |
| `crates/worker` | The Worker: router, `RoomObject` and `RecorderObject` Durable Objects, R2, Workers AI. It executes the core's effects. |
| `web/` | The SvelteKit single-page app (Svelte 5, strict TypeScript). It owns the DOM, `RTCPeerConnection` and media devices. |
| `edge/` | An optional Pages project that forwards a custom domain whose DNS lives outside Cloudflare to the Worker. |
| `docs/` | Design decisions D1–D35, architecture, cost, testing, the dev log, research and spike write-ups. Start at [docs/README.md](docs/README.md). |

## Run your own

You need a Cloudflare account, Rust ≥ 1.95 with the `wasm32-unknown-unknown`
target, `wasm-pack`, `worker-build` (`cargo install worker-build`), and pnpm.

1. Install the dependencies: `pnpm install`.
2. In the Cloudflare dashboard, create a Realtime SFU app. Optionally, create
   a Realtime TURN key for networks that block UDP.
3. Copy `.env.example` to `.env`. This git-ignored file holds your
   deployment's own values; `wrangler.toml` only has placeholders for them.
4. Create the KV namespace with `pnpm exec wrangler kv namespace create
   MEETINGS`, and put the id it prints into `.env` as `MEETINGS_KV_ID`.
5. Create the EU bucket with `pnpm exec wrangler r2 bucket create
   meet-felix-recordings --jurisdiction eu`, and add the audio retention rule
   from [docs/recording/](docs/recording/README.md).
6. In `.env`, set `RECORDER_ORIGIN` to your Worker's `workers.dev` URL and
   `OWNER_NAME` to the name you join meetings with.
7. Set the secrets with `pnpm exec wrangler secret put NAME`: `CREATE_SECRET`
   (the owner password, at least 20 characters; generate it), `SESSION_KEY`
   (at least 32 random characters, e.g. `openssl rand -base64 32`; it signs
   the owner's session cookie), `CALLS_APP_ID` and `CALLS_APP_SECRET`, and
   optionally `TURN_KEY_ID` and `TURN_KEY_API_TOKEN`. For local runs, put the
   same names into the git-ignored `.dev.vars`.
8. Build and deploy with `pnpm run release`. It writes `wrangler.local.toml`
   (git-ignored) from `wrangler.toml` and `.env`, and deploys with it. The
   app is then served at the Worker's URL.

To serve the app on a custom domain whose DNS is not on Cloudflare, see
`edge/` and [research/10](docs/research/10-custom-hostname-external-dns.md).

`wrangler.toml` explains each binding and variable. What it costs per month
and per meeting-hour is in [docs/cost/](docs/cost/README.md).

## Development

| Task | Command |
|---|---|
| Rust tests | `cargo test` |
| Rust lints | `cargo clippy --all-targets -- -D warnings` |
| Core as WASM | `cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm` |
| Worker, local | `pnpm dev` (runs `wrangler dev` with `wrangler.local.toml` and without the EU jurisdiction, which local workerd lacks) |
| Web app | `pnpm --filter web dev`, `pnpm --filter web check`, `pnpm --filter web test` |
| End-to-end tests | `pnpm test:e2e` (mock SFU), `pnpm test:e2e:real` (real SFU), `pnpm test:e2e:recorder` (real Whisper) |
| Build and deploy | `pnpm run release` |

The test layers are described in [docs/TESTING.md](docs/TESTING.md). The
package manager is pnpm, with a 7-day minimum release age for dependencies.
[CLAUDE.md](CLAUDE.md) holds the conventions for working in the repo.

## License

[MIT](LICENSE).
