# meet-felix

Self-hosted, serverless video meetings on Cloudflare, written in Rust with a functional core and an imperative shell.

## Read first

- [STATUS.md](STATUS.md) — the live dashboard: what is in flight, what just landed, what is next. Keep it true.
- [docs/README.md](docs/README.md) — the index of every doc, with each design decision linked. Cross-cutting decisions (platform, core and shells, browser stack, backend, EU location), the component map, phases and risks are in [docs/architecture/](docs/architecture/README.md); feature decisions are in [docs/links/](docs/links/README.md), [docs/media/](docs/media/README.md), [docs/recording/](docs/recording/README.md), [docs/core/](docs/core/README.md) and [docs/cost/](docs/cost/README.md).
- The decision records D1–D21, indexed in [docs/README.md](docs/README.md#design-decisions) — **binding**. A change that departs from a decision updates that decision in the same commit, with the reason. New decisions get the next free number and a row in the index.
- [docs/DEVLOG.md](docs/DEVLOG.md) — the running log. Add an entry at the top (newest first) for every non-trivial change: what, why, what's next.
- [docs/TESTING.md](docs/TESTING.md) — the test layers and what exists today.
- [docs/research/](docs/research/README.md) — the research the design rests on; cite it, don't re-argue it without new evidence. Read-only: do not rewrite the notes.
- [docs/spikes/](docs/spikes/) — one write-up per phase-0 spike.

## Layout

- `crates/meeting-core` — pure Rust: `RoomState`, `Event`, `Effect`, `step()`. No I/O, no clocks, no RNG. Compiles natively and for `wasm32-unknown-unknown` (feature `wasm` adds tsify/wasm-bindgen exports). All protocol types live here; TypeScript types are generated from it.
- `crates/sfu-api` — typed client for the Cloudflare Realtime SFU HTTPS API and the WebSocket-adapter protobuf.
- `crates/worker` — the imperative shell on Cloudflare Workers (`worker` crate): router, `RoomObject` and `RecorderObject` Durable Objects, R2, Workers AI. Keeps state in DO storage so WebSocket hibernation works.
- `web/` — SvelteKit static SPA (Svelte 5 runes, TypeScript strict). Owns DOM, `RTCPeerConnection` and media devices; calls `meeting-core` as WASM. Use the Svelte MCP tools and skills for every `.svelte` file.
- `docs/` — design decisions, cost, testing, dev log, research and spike write-ups (see above).

## Commands

- Rust: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm`.
- Worker: `pnpm exec wrangler dev` (local; needs `worker-build` from `cargo install worker-build`; add `--var ROOM_JURISDICTION:` because local workerd has no jurisdictions), `pnpm exec wrangler deploy`.
- Web: `pnpm --filter web dev|check|build`.
- Package manager is pnpm with a 7-day minimum release age (`.npmrc`); build-script approvals live in `pnpm-workspace.yaml` (`allowBuilds`).

## Rules

- Core first ([D20](docs/core/README.md#d20-core-first-2026-10-09)): new behaviour goes into `meeting-core` as events/effects with tests; the worker and web shells only execute effects.
- Recording state is part of `RoomState` and broadcast on every change; never add a recording path that bypasses it ([D15](docs/recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)).
- Meeting ids are three short English words from `meeting-core::words`; they are ephemeral and recycled ([D6](docs/links/README.md#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)).
- Verify Cloudflare and crate facts against docs/source; prices and limits are in [docs/cost/](docs/cost/README.md) and [docs/research/](docs/research/README.md), with dates.
- Secrets (`.dev.vars`, API tokens, host tokens) never go into git.
