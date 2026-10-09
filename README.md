# meet-felix

A personal, self-hosted, serverless video meeting app: one owner is the only
host, everyone else joins as a guest through a link. It replaces Google Meet
for small meetings (2–10 people). It runs entirely on Cloudflare: Realtime SFU
carries the media, Rust Durable Objects hold the meetings, and a Svelte app
in the browser calls a pure Rust core compiled to WASM. Meetings are joined
through three-word links such as `amber-otter-lamp`, recorded with everyone's
consent, and transcribed after they end, in German and English. It costs
nothing while no meeting runs, apart from the Workers Paid plan.

**Status:** pre-alpha. Phase 1 (walking skeleton) is in progress; nothing is
deployed yet.

## Documentation

- [docs/README.md](docs/README.md): the documentation index, with every
  design decision, the architecture, the cost model, testing and the
  research.
- [STATUS.md](STATUS.md): what is in flight right now.
- [CLAUDE.md](CLAUDE.md): conventions for working in the repo.

## Commands

| Task | Command |
|---|---|
| Rust tests | `cargo test` |
| Rust lints | `cargo clippy --all-targets -- -D warnings` |
| Core as WASM | `cargo check -p meeting-core --target wasm32-unknown-unknown --features wasm` |
| Worker, local | `pnpm exec wrangler dev` (needs `worker-build`: `cargo install worker-build`) |
| Worker, deploy | `pnpm exec wrangler deploy` |
| Web app | `pnpm --filter web dev`, `pnpm --filter web check`, `pnpm --filter web build` |

The package manager is pnpm, with a 7-day minimum release age for
dependencies.

## License

MIT (see `Cargo.toml`).
