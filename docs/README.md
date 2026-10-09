# meet-felix documentation

meet-felix is a self-hosted, serverless replacement for Google Meet for small
meetings: Cloudflare Realtime SFU carries the media, Rust Durable Objects hold
the rooms, a pure Rust core decides, and a Svelte app in the browser performs
the I/O. Meetings are recorded with everyone's consent and get a transcript
afterwards. This folder holds why it is built this way, what it costs, how it
is tested, and what changed when.

**Read first:** meet-felix is a personal meeting app with one owner as the only host; everyone else is a guest ([D32](architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09)). Do not build features for other hosts.

## Start here

| Document | Read it for |
|---|---|
| [Design decisions](#design-decisions) | The decision records, D1–D33, each kept with its feature. **Binding:** a change that departs from a decision updates it in the same commit, with reasons. |
| [architecture/](architecture/README.md) | The cross-cutting decisions: platform, core and shells, browser stack, backend, EU location; plus goal, non-goals, component map, phases and risks. |
| [cost/](cost/README.md) | What it costs per month and per meeting-hour, against Google Workspace. |
| [DEVLOG.md](DEVLOG.md) | The running log, newest first: what changed, why, what's next. |
| [TESTING.md](TESTING.md) | The test layers: core unit and property tests, worker spike scripts, web checks, and the planned device and deployment checks. |
| [spikes/](spikes/) | One write-up per spike: [01 — DO hibernation](spikes/01-do-hibernation.md), [02 — first call through the SFU](spikes/02-first-call.md), [03 — Whisper input formats and a pure-Rust Opus encoder](spikes/03-whisper-input-and-opus.md). |
| [research/](#research) | The research the design rests on. Cite it; don't re-argue it without new evidence. |

Outside this folder: [STATUS.md](../STATUS.md) is the live dashboard of what
is in flight, and [CLAUDE.md](../CLAUDE.md) holds the conventions for working
in the repo.

## Design decisions

Each decision lives with the feature it shapes. The IDs are stable; cite them
as "D12" in code comments, commits and docs. All were taken on 2026-10-09.

| # | Decision | Home |
|---|---|---|
| [D1](architecture/README.md#d1-cloudflare-is-the-only-platform-2026-10-09) | Cloudflare is the only platform | [architecture/](architecture/README.md) |
| [D2](architecture/README.md#d2-a-pure-rust-core-with-imperative-shells-2026-10-09) | A pure Rust core with imperative shells | [architecture/](architecture/README.md) |
| [D3](architecture/README.md#d3-the-browser-runs-a-typescript-and-svelte-shell-over-the-core-as-wasm-2026-10-09) | The browser runs a TypeScript and Svelte shell over the core as WASM | [architecture/](architecture/README.md) |
| [D4](architecture/README.md#d4-the-backend-is-rust-on-workers-with-durable-objects-that-hibernate-2026-10-09) | The backend is Rust on Workers, with Durable Objects that hibernate | [architecture/](architecture/README.md) |
| [D5](architecture/README.md#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09) | Pin data to the EU where Cloudflare allows it | [architecture/](architecture/README.md) |
| [D6](links/README.md#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09) | Meeting ids are three short English words that live only while the meeting runs | [links/](links/README.md) |
| [D7](links/README.md#d7-only-the-owner-creates-meetings-with-a-creation-secret-2026-10-09) | Only the owner creates meetings, with a creation secret | [links/](links/README.md) |
| [D8](links/README.md#d8-a-meeting-ends-when-a-host-ends-it-or-after-two-empty-minutes-2026-10-09) | A meeting ends when a host ends it or after two empty minutes | [links/](links/README.md) |
| [D9](links/README.md#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09) | Transcript links carry a 128-bit random token | [links/](links/README.md) |
| [D10](media/README.md#d10-each-participant-has-separate-publish-and-subscribe-sessions-2026-10-09) | Each participant has separate publish and subscribe sessions | [media/](media/README.md) |
| [D11](media/README.md#d11-cameras-send-simulcast-and-each-subscriber-picks-a-layer-2026-10-09) | Cameras send simulcast, and each subscriber picks a layer | [media/](media/README.md) |
| [D12](media/README.md#d12-codecs-are-h264-first-vp8-as-fallback-no-av1-2026-10-09) | Codecs are H.264 first, VP8 as fallback, no AV1 | [media/](media/README.md) |
| [D13](media/README.md#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09) | Detect features, not browsers; screen share is desktop-only | [media/](media/README.md) |
| [D14](media/README.md#d14-calls-work-in-a-plain-browser-tab-and-ios-gets-defensive-handling-2026-10-09) | Calls work in a plain browser tab, and iOS gets defensive handling | [media/](media/README.md) |
| [D15](recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09) | Recording is announced by the core, and joining means consent | [recording/](recording/README.md) |
| [D16](recording/README.md#d16-capture-each-speakers-audio-on-the-server-as-oggopus-chunks-of-up-to-30-seconds-2026-10-09) | Capture each speaker's audio on the server, as Ogg/Opus chunks of up to 30 seconds | [recording/](recording/README.md) |
| [D17](recording/README.md#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09) | Transcribe after the meeting with Whisper large-v3-turbo, behind a Transcriber trait | [recording/](recording/README.md) |
| [D18](recording/README.md#d18-keep-the-audio-48-hours-keep-transcripts-until-the-owner-deletes-them-2026-10-09) | Keep the audio 48 hours; keep transcripts until the owner deletes them | [recording/](recording/README.md) |
| [D19](recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09) | Show the transcript link to everyone at the end | [recording/](recording/README.md) |
| [D20](core/README.md#d20-core-first-2026-10-09) | Core first | [core/](core/README.md) |
| [D21](cost/README.md#d21-build-and-measure-no-further-price-analysis-2026-10-09) | Build and measure; no further price analysis | [cost/](cost/README.md) |
| [D22](ui/README.md#d22-phones-and-tablets-are-fully-supported-and-the-start-and-pre-join-pages-are-laid-out-for-phones-first-2026-10-09) | Phones and tablets are fully supported, and the start and pre-join pages are laid out for phones first | [ui/](ui/README.md) |
| [D23](ui/README.md#d23-dark-theme-only-2026-10-09) | Dark theme only | [ui/](ui/README.md) |
| [D24](ui/README.md#d24-the-owners-password-is-never-stored-on-the-device-the-password-manager-fills-it-every-time-2026-10-09) | The owner's password is never stored on the device; the password manager fills it every time | [ui/](ui/README.md) |
| [D25](ui/README.md#d25-the-start-page-has-one-recording-switch-shows-the-words-on-one-line-and-ends-in-join-now-2026-10-09) | The start page has one recording switch, shows the words on one line, and ends in Join now | [ui/](ui/README.md) |
| [D26](ui/README.md#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09) | Past meetings and transcripts are on a separate owner-only page behind the same password | [ui/](ui/README.md) |
| [D27](ui/README.md#d27-the-guest-pre-join-remembers-the-name-and-does-not-show-who-is-in-the-room-2026-10-09) | The guest pre-join remembers the name and does not show who is in the room | [ui/](ui/README.md) |
| [D28](ui/README.md#d28-the-meeting-screen-keeps-a-labelled-control-bar-the-recording-badge-top-left-and-toasts-off-the-tiles-2026-10-09) | The meeting screen keeps a labelled control bar, the recording badge top left, and toasts off the tiles | [ui/](ui/README.md) |
| [D29](ui/README.md#d29-a-shared-screen-keeps-people-in-a-column-on-desktops-and-in-round-chips-on-phones-2026-10-09) | A shared screen keeps people in a column on desktops and in round chips on phones | [ui/](ui/README.md) |
| [D30](ui/README.md#d30-the-active-speaker-gets-a-fog-ring-and-level-bars-never-the-lamp-colour-2026-10-09) | The active speaker gets a fog ring and level bars, never the lamp colour | [ui/](ui/README.md) |
| [D31](ui/README.md#d31-the-transcript-page-shows-speaker-labelled-paragraphs-with-wall-clock-times-2026-10-09) | The transcript page shows speaker-labelled paragraphs with wall-clock times | [ui/](ui/README.md) |
| [D32](architecture/README.md#d32-one-owner-is-the-only-host-everyone-else-is-a-guest-2026-10-09) | One owner is the only host; everyone else is a guest | [architecture/](architecture/README.md) |
| [D33](media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09) | A dead SFU session is replaced, not retried | [media/](media/README.md) |

## Features with their own docs

| Folder | Feature | Decisions |
|---|---|---|
| [links/](links/README.md) | Three-word meeting ids, who creates meetings, how a meeting ends, transcript links | [D6](links/README.md#d6-meeting-ids-are-three-short-english-words-that-live-only-while-the-meeting-runs-2026-10-09)–[D9](links/README.md#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09) |
| [media/](media/README.md) | SFU sessions, simulcast, codecs, browser capabilities, iOS handling, replacing a dead session | [D10](media/README.md#d10-each-participant-has-separate-publish-and-subscribe-sessions-2026-10-09)–[D14](media/README.md#d14-calls-work-in-a-plain-browser-tab-and-ios-gets-defensive-handling-2026-10-09), [D33](media/README.md#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09) |
| [recording/](recording/README.md) | Consent, per-speaker capture, chunks in R2, the post-meeting transcript, compressed copies of the audio, retention, sharing | [D15](recording/README.md#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)–[D19](recording/README.md#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09) |
| [core/](core/README.md) | The functional core: state, events, effects, invariants | [D20](core/README.md#d20-core-first-2026-10-09) |
| [cost/](cost/README.md) | The cost model and the Google Workspace baseline | [D21](cost/README.md#d21-build-and-measure-no-further-price-analysis-2026-10-09) |
| [ui/](ui/README.md) | Phone-first pages, dark theme, the owner's password and past-meetings page, the guest pre-join, the meeting screen, the transcript page | [D22](ui/README.md#d22-phones-and-tablets-are-fully-supported-and-the-start-and-pre-join-pages-are-laid-out-for-phones-first-2026-10-09)–[D31](ui/README.md#d31-the-transcript-page-shows-speaker-labelled-paragraphs-with-wall-clock-times-2026-10-09) |

## Research

All notes are from 2026-10-09; [research/README.md](research/README.md) says
what each covers in more detail.

| Note | Covers |
|---|---|
| [00 — report](research/00-report-serverless-rust-video-meetings.md) | The synthesized report and its recommendation. |
| [01 — hosting and media platforms](research/01-hosting-and-media-platforms.md) | Realtime SFU, Workers pricing, self-run SFU hosts, managed platforms, bandwidth. |
| [02 — Cloudflare deep dive](research/02-cloudflare-deep-dive.md) | Hibernation in workers-rs, SFU limits, adapters, Workers AI STT, data residency. |
| [03 — self-hosted Fly.io path](research/03-selfhosted-fly-path.md) | The rejected str0m-on-Fly.io fallback. |
| [04 — Rust WebRTC stack](research/04-rust-webrtc-stack.md) | str0m, webrtc-rs, signaling design, testing sans-IO logic. |
| [05 — frontend and browser](research/05-frontend-and-browser.md) | Rust vs TypeScript UI, browser limits, PWA, codecs. |
| [06 — existing projects](research/06-existing-projects-lessons.md) | Lessons from Jitsi, LiveKit and other meeting apps. |
| [07 — live transcription](research/07-live-transcription.md) | Streaming STT options (background; live captions are a non-goal). |
| [08 — Google Workspace baseline](research/08-google-workspace-baseline.md) | What Google charges for Meet transcripts. |
| [09 — post-meeting transcription and recording](research/09-post-meeting-transcription-recording.md) | Whisper and Nova-3 batch STT, R2, alarms, adapter capture. |
