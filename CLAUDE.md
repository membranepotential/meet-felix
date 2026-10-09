# meet-felix

Self-hosted, serverless video meetings on Cloudflare, written in Rust with a functional core and an imperative shell. PLAN.md is the design (decisions, cost model, architecture, phases). STATUS.md is the live dashboard; keep it true.

## Layout

- `crates/meeting-core` — pure Rust: `RoomState`, `Event`, `Effect`, `step()`. No I/O, no clocks, no RNG. Compiles natively and for `wasm32-unknown-unknown` (feature `wasm` adds tsify/wasm-bindgen exports). All protocol types live here; TypeScript types are generated from it.
- `crates/sfu-api` — typed client for the Cloudflare Realtime SFU HTTPS API and the WebSocket-adapter protobuf.
- `crates/worker` — the imperative shell on Cloudflare Workers (`worker` crate): router, `RoomObject` and `RecorderObject` Durable Objects, R2, Workers AI. Keeps state in DO storage so WebSocket hibernation works.
- `web/` — SvelteKit static SPA (Svelte 5 runes, TypeScript strict). Owns DOM, `RTCPeerConnection` and media devices; calls `meeting-core` as WASM. Use the Svelte MCP tools and skills for every `.svelte` file.
- `docs/spikes/` — one write-up per phase-0 spike.
- `research_notes/`, `reports/` — research behind PLAN.md; read-only.

## Commands

- Rust: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm`.
- Worker: `pnpm exec wrangler dev` (local; needs `worker-build` from `cargo install worker-build`), `pnpm exec wrangler deploy`.
- Web: `pnpm --filter web dev|check|build`.
- Package manager is pnpm with a 7-day minimum release age (`.npmrc`); build-script approvals live in `pnpm-workspace.yaml` (`allowBuilds`).

## Rules

- Core first: new behaviour goes into `meeting-core` as events/effects with tests; the worker and web shells only execute effects.
- Recording state is part of `RoomState` and broadcast on every change; never add a recording path that bypasses it.
- Meeting ids are three short English words from `meeting-core::words`; they are ephemeral and recycled.
- Verify Cloudflare and crate facts against docs/source; prices and limits are in PLAN.md with dates.
- Secrets (`.dev.vars`, API tokens, host tokens) never go into git.
