# meet-felix: plan (v2)

*2026-10-09. Evidence: `reports/Serverless Rust video meetings.md` and the nine note files in `research_notes/`. Prices checked on 2026-10-09. "Assumed" marks my own numbers; "unverified" marks claims without a primary source.*

## 1. Goal and constraints

A self-hosted Google-Meet replacement for small meetings (2–10 people) that:

- costs nothing while no meeting runs and less per month than Google Workspace Business Standard (13.60 € net, ≈16.18 € gross per user, which matches your ~17 €);
- runs in the browser on PC, tablet and phone, with camera, mic, screen share (desktop only, a browser limit) and short invite links;
- records each meeting with everyone's knowledge and delivers a transcript afterwards (German and English); no live captions;
- is written in Rust with a functional core and an imperative shell; one hosting platform: Cloudflare.

## 2. Decisions

| Area | Decision | Why |
|---|---|---|
| Media | **Cloudflare Realtime SFU + Cloudflare TURN** | Idles at $0; $0.05/GB after 1,000 free GB/month shared by SFU, TURN and adapters; simulcast, H.264/VP8/VP9/Opus; plain HTTPS API. |
| Backend | **Rust on Workers (`worker` 0.8.x); per meeting one Room DO and one Recorder DO** | workers-rs exposes WebSocket hibernation, so idle rooms bill no duration. |
| Functional core | **`meeting-core` crate: `step(RoomState, Event) -> (RoomState, Vec<Effect>)`, no I/O** | Runs in the DO and in the browser as WASM; property-tested without a network. Recording state lives here, so every client sees it by construction. |
| Frontend | **TypeScript/Svelte shell (DOM, `RTCPeerConnection`, devices) + `meeting-core` as WASM; types shared via `tsify`** | Accepted. web-sys lacks `contentHint` and gates several media APIs behind an unstable flag. |
| Invite links | **Three words from a fixed list, e.g. `meet.example/amber-otter-lamp`; no password; the link lives only while the meeting runs** | Memorable and sayable. Security comes from ephemerality, rate limiting and a lockable room (§4). |
| Who creates meetings | **Only you, behind Cloudflare Access on the "new meeting" route; guests need no login** | Otherwise anyone could run meetings on your SFU bill. Access is free for up to 50 users. |
| Recording | **Per-speaker audio, captured server-side from the SFU via WebSocket adapters into the Recorder DO, stored in R2 as 16 kHz mono chunks; announced to all participants** | Robust against phones backgrounding or closing tabs. Per-speaker tracks give exact attribution. No video recording (the SFU has none; out of scope). |
| Transcript | **Post-meeting job in Rust (DO alarm loop): Whisper large-v3-turbo on Workers AI per speaker chunk, merged by timestamps into one transcript** | $0.0005/min, returns segments with word timestamps and takes a `language` parameter. Nova-3 over HTTP ($0.0052/min, all Deepgram options) stays behind the same trait if German quality disappoints. |
| Codecs | H.264 preferred, VP8 fallback, no AV1 | Hardware H.264 everywhere incl. iOS; AV1 on iPhone is not smooth. |
| EU data | DOs and R2 pinned to EU (`jurisdiction("eu")`, EU bucket); SFU and Workers AI location not guaranteed | Nice-to-have, not a hard requirement. |

Dropped from v1: the Fly.io/str0m fallback, live captions, the knock queue as default (now an optional room lock).

## 3. Cost analysis against Google

Assumptions: 1 € = 1.15 $ (mid-September 2026 rate). Bitrates per the report (≈1.2 Mbit/s per received camera stream, 1.5 Mbit/s screen share), to be measured in spike 5. Recording adds adapter egress: each speaker's audio leaves the SFU once at 48 kHz stereo PCM, 0.69 GB per speaker-hour, billed like media.

### Fixed cost

| Item | $/month | €/month |
|---|---|---|
| Workers Paid plan | 5.00 | 4.35 |
| Cloudflare Access (≤50 users), R2 free tier (10 GB), domain you own | 0 | 0 |
| **Idle total** | **5.00** | **4.35** |

The Workers Free plan supports SQLite-backed DOs with hibernation. Whether the SFU and Workers AI speech models work on it is unverified (spike 1). If they do, idle cost is 0 €.

### Variable cost per meeting-hour

| Meeting hour | Media egress | Adapter egress | Inside free 1,000 GB | Beyond pool ($0.05/GB) | Whisper transcript | R2 storage per month |
|---|---|---|---|---|---|---|
| 2 people | 1.1 GB | 1.4 GB | $0 | $0.13 | $0.06 | <$0.01 |
| 4 people | 6.7 GB | 2.8 GB | $0 | $0.48 | $0.12 | <$0.01 |
| 4 people + screen share | 8.7 GB | 2.8 GB | $0 | $0.58 | $0.12 | <$0.01 |
| 10 people | 22–50 GB | 6.9 GB | $0 | $1.45–2.85 | $0.30 | $0.02 |

Whisper is billed per audio minute per speaker; skipping silent chunks (the recorder knows energy levels) cuts this further. The free pool covers roughly 100 four-person hours or 15–35 ten-person hours per month with recording on.

### Monthly scenarios

| Scenario | Egress | Transcript | Fixed | Total | vs ≈16–17 € |
|---|---|---|---|---|---|
| Light: 10 h, 4 people | 95 GB, free | $1.20 | $5 | **$6.20 ≈ 5.4 €** | cheaper |
| Medium: 30 h, 4 people | 285 GB, free | $3.60 | $5 | **$8.60 ≈ 7.5 €** | cheaper |
| Medium, Nova-3 instead of Whisper | 285 GB, free | $37 | $5 | **$42 ≈ 37 €** | more; use Nova-3 only for selected meetings |
| Heavy: 40 h, 10 people | 1,150–2,280 GB | $12 | $5 | **$25–81 ≈ 21–70 €** | more; cap the top simulcast layer, or accept |

Break-even is not a question of transcription any more: with Whisper, media egress is the only lever, and it is free up to 1,000 GB. Your realistic usage (a few four-person hours per week) lands at **5–8 € per month**, about half of Business Standard, with idle months at 4.35 € or 0 €.

**Is there a cheaper no-build option?** Google says "Take notes for me" came to Google AI Pro and Ultra subscribers on 2026-06-29 for meetings you organise, with participants notified. Its EUR price and whether it includes plain transcripts and recording are unverified; in the US AI Pro has listed at about $20, so it is unlikely to beat Business Standard. Check your invoice for plan name, net/gross and annual/flexible before deciding.

## 4. Architecture

```
Browser (Svelte shell + meeting-core.wasm)
   │ WebSocket (signaling)                       │ WebRTC publish + subscribe sessions
   ▼                                             ▼
Worker router ──► Room DO (Rust, hibernating sockets, RoomState in SQLite)
  /new (Access)        │ HTTPS rtc.live.cloudflare.com/v1  (sessions, tracks, renegotiate, adapters)
  /{a-b-c}             ▼
  /t/{token}     Cloudflare Realtime SFU ◄── Cloudflare TURN
                       │ one WebSocket adapter per audio track: 48 kHz stereo s16le PCM
                       ▼
                 Recorder DO (Rust, per meeting): downmix + resample 48 → 16 kHz mono (rubato),
                 energy-gated 30 s chunks cut at silence ──► R2  rec/{meeting}/{speaker}/{n}.wav
                       │ alarm loop after the meeting ends
                       ▼
                 Transcriber (same DO): Workers AI Whisper large-v3-turbo per chunk (`language`)
                 ──► merge segments by wall-clock ──► transcript.md + transcript.json in R2
                 ──► stitch chunks into one WAV per speaker (optional mixed WAV)
```

### Meeting lifecycle and links

1. You open `/new` (Cloudflare Access login). The Worker picks three words not in use, creates the Room DO with `jurisdiction("eu")`, stores `words → DO id` in KV with a TTL, and returns the link plus a host token kept in your browser.
2. Guests open the link. The pre-join screen shows name, device check, and the recording notice. If the room is recorded, the join button reads "Join, I accept recording and transcription" (explicit consent; §201 StGB makes recording private speech without consent a crime in Germany, so the notice is not optional).
3. The room ends when the host ends it or it has been empty for 2 minutes (DO alarm). The words are released, the Recorder DO closes its adapters and starts the transcript job.
4. Results live at `/t/{128-bit random token}`, shown to the host at the end and listed in the host's browser; participants get the link in the room if the host enables it. Retention: recordings 30 days, transcripts until deleted (open decision).

Word list: about 2,000 short, common, easy-to-spell words (2,000³ ≈ 8 × 10⁹ combinations, ≈33 bits). Only a handful of rooms are ever active, join attempts are rate-limited per IP in the Worker, and the host can lock the room (late joiners then knock). That is enough for ephemeral rooms; it is not enough for long-lived links, which is why words are recycled.

### Recording announcement (in the core, not the UI)

- `RoomState.recording: Option<Recording { since, by }>` is part of every state broadcast. The UI renders a persistent red "Recording" badge and a banner on start, and plays a chime.
- Recording is chosen at creation (default on, since the transcript is the point) and shown on the pre-join screen. Starting it mid-meeting re-prompts everyone with a consent dialog; anyone who declines is muted and unrecorded until they leave or accept. (Open decision 2.)

### Crates and packages

| Path | Kind | Contents |
|---|---|---|
| `crates/meeting-core` | pure Rust lib (`wasm32` + native) | `RoomState`, `Participant`, `Track`, `Role`, `Recording`, `ClientMsg`/`ServerMsg` (serde + tsify), `Event`, `Effect`, `step()`, simulcast layer policy, chunk-cutting and silence-gating state machine, transcript merge |
| `crates/sfu-api` | Rust lib | Typed client for the SFU HTTPS API and the adapter protobuf (`prost`) |
| `crates/worker` | `worker` crate | Router (Access check on `/new`, word → DO lookup, static assets); `RoomObject`; `RecorderObject` (adapter ingress, resample, R2 `put` per chunk, alarm-driven transcription, R2 multipart stitch); `Transcriber` trait with `WhisperTurbo` and `Nova3Http` impls via `worker::Ai` |
| `web/` | SvelteKit static site | New-meeting page, pre-join with consent, meeting grid, device picker, end-of-meeting page, transcript page; WASM via `wasm-pack` |
| `crates/meeting-core/tests` | proptest | Join/leave/lock/knock/renegotiate/recording sequences; invariants (no orphan SFU tracks, one offerer per session, recording never on without a broadcast, host never locked out) |

### Protocol sketch

- `ClientMsg`: `Join { name, consent: bool, host_token? }`, `PublishOffer`, `SubscribeAnswer`, `Ice`, `SetLayer { track, rid }`, `Lock`, `Admit`, `Kick`, `Mute`, `SetRecording { on }`, `End`, `Leave`.
- `ServerMsg`: `Welcome { state }`, `State { state }`, `PublishAnswer`, `SubscribeOffer`, `Knock { who }`, `Ended { transcript_url? }`, `Error`.
- Separate publish and subscribe SFU sessions per participant: client offers on publish, DO offers on subscribe, so no glare.
- SFU rules to design around: tracks idle for 30 s are garbage-collected; an adapter carries one track, retries its endpoint for 15 s on loss, then must be recreated; create calls take up to four adapters each.

### Recorder details

- Inbound rate at 10 speakers: ≈1.9 MB/s in ≤32 KB messages, ≈60 messages/s, billed 20:1 ≈ 3 requests/s. Within plan limits; CPU per message is a downmix and resample.
- Chunks: 16 kHz mono s16le WAV, cut at the first silence after 25 s, hard cut at 30 s (≈1 MB, matching Cloudflare's own Whisper chunking). All-silent chunks are recorded but flagged so transcription skips them. One `put` per chunk: no multipart during the meeting, nothing to lose on hibernation beyond the current partial chunk.
- Reconnects: when a participant republishes audio, the Room DO asks the Recorder DO to create a new adapter; chunk numbering continues per speaker with wall-clock offsets.
- Post-meeting job: alarm loop, each run ≤ 15 min wall time, idempotent steps recorded in DO storage (chunk → segments), up to 720 speech requests/min allowed. Four speakers × 1 h ≈ 480 chunks ≈ a few alarm runs.

### Mobile and browser handling (shell)

- Feature-detect `getDisplayMedia`; hide the share button where absent (all phone and tablet browsers).
- iOS: on `visibilitychange` and track `mute`/`ended`, reacquire with `getUserMedia` + `replaceTrack`, reconnect signaling with backoff; `playsinline`; Wake Lock where available (iOS ≥ 18.4).
- Links must work in a plain Safari tab; the installable PWA is optional.

## 5. Phases

| Phase | Scope | Done when |
|---|---|---|
| **0. Spikes** | 1. Rust DO with 10 hibernating sockets; try the Free plan for SFU + Workers AI. 2. Adapter → Recorder DO → R2 with 10 synthetic tracks: CPU, memory, loss on reconnect. 3. Whisper turbo vs Nova-3 HTTP on German and mixed German/English audio; confirm chunk size limits. 4. Real iPhone/iPad: simulcast, lock screen, app switch, phone call. 5. Measure egress for a real 4-person hour incl. adapters. | Each spike has a written result in `docs/spikes/`; §3 updated with measured numbers. |
| **1. Walking skeleton** | Cargo workspace + pnpm `web/`, `meeting-core` with `step`, Room DO, word links with KV, Access on `/new`, 2-person audio+video through the SFU, deploy with `wrangler`. | Two browsers on different networks hold a 10-minute call; DO hibernates between messages; CI runs core property tests. |
| **2. Multi-party** | Simulcast + per-tile layer selection, active-speaker detection, desktop screen share with `contentHint: "detail"`, room lock/knock, host controls (mute, kick, end), ICE restart and network-switch reconnect, responsive grid. | 8 participants incl. one phone and one shared screen for 30 min without manual intervention. |
| **3. Recording + transcript** | Recorder DO, consent flow and badge, chunking, R2 layout, transcription job, transcript page (`/t/{token}`), stitched per-speaker WAVs, retention cleanup via alarm. | A recorded 4-person meeting yields a correctly attributed German/English transcript within 15 minutes of ending; every participant saw the notice. |
| **4. Mobile polish** | iOS background handling, Wake Lock, PWA manifest, speaker selection, Picture-in-Picture. | Checklist run on iPhone, iPad, Android, desktop Chrome/Firefox/Safari. |
| **5. Operations and extras** | Cloudflare usage dashboard, monthly budget alert, join rate limits, retention policy, docs; transcript by email (guests enter an address at the end); cancel Workspace. | A month of real use stays under 10 €. |

Effort: phases 1–3 are the bulk; phase 3 is mostly a Rust port of Cloudflare's `ai-tts-stt` example plus R2.

## 6. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| workers-rs rough edges (hibernation memory PR open, `serialize_attachment` unverified) | Backend bugs or idle billing | Spike 1; keep per-socket data ≤ 16 KB |
| Whisper turbo German quality or chunk cuts mid-word | Poor transcript | Spike 3; cut chunks at silence; Nova-3 behind the trait |
| Adapter drops audio during its 15 s retry window | Gaps in recording | Recorder tracks gaps and marks them in the transcript |
| Assumed bitrates wrong by 2× | Leaves the free pool sooner | Spike 5; cap top simulcast layer above six people |
| iOS behaviour differs from old WebKit bug reports | Dropped media on phones | Spike 4; defensive reacquire logic |
| Three-word links guessed | Uninvited joiner | Ephemeral rooms, rate limits, room lock; host sees every join |

## 7. Decisions taken on 2026-10-09

1. **Price baseline:** no further analysis; build it and measure in practice.
2. **Consent:** explicit "Join and accept recording" button on the pre-join screen.
3. **Word list:** short English words.
4. **Retention:** recordings and transcripts kept indefinitely for now; a cleanup job comes later.
5. **Transcript sharing:** v1 shows the transcript link to everyone in the room at the end so guests can save it. Later: guests enter an email address and receive the transcript by mail (phase 5 candidate; needs a mail provider).
