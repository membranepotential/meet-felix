# Post-meeting transcription and per-speaker recording on Cloudflare

Checked 2026-10-09. "Verified" = read on developers.cloudflare.com (raw index.md) or the workers-rs / crates.io source on that date.

## Takeaway

- Record per speaker through the SFU WebSocket adapter: one adapter per track (not per meeting), so a 10-person meeting needs 10 adapters (3 create calls of up to 4 entries). Adapter egress is 48 kHz stereo PCM, so downsample to 16 kHz mono in the DO before storing.
- Store audio in R2 via multipart upload from the DO. The upload id can be persisted in DO storage and resumed after hibernation (`resume_multipart_upload` makes no network call and does not validate the id). Parts must be >= 5 MiB and equal in size, except the last.
- Whisper-large-v3-turbo is the cheap, documented batch path (segments + word timestamps + VTT, `language` param). Nova-3 HTTP is 10x the price and its documented output schema has only channels/alternatives/words; there is no documented per-request audio size limit for either model.
- Workflows are not usable from Rust code. Docs show only JS/TS and a Python SDK. Use a DO alarm loop (one alarm, 15 min wall per run, retries) or a TS Workflow that calls the Rust Worker.
- Compression: `flacenc` (pure Rust, FLAC) and `opus-rs` (pure Rust Opus encoder, no_std) are candidates, but wasm32 compile is unverified for both.

## 1. Workers AI batch speech-to-text

Sources:
- https://developers.cloudflare.com/workers-ai/models/nova-3/index.md (+ schema-input.json / schema-output.json)
- https://developers.cloudflare.com/workers-ai/models/whisper-large-v3-turbo/index.md
- https://developers.cloudflare.com/workers-ai/models/whisper/index.md
- https://developers.cloudflare.com/workers-ai/platform/limits/index.md (updated 2026-09-17)

| | nova-3 (`@cf/deepgram/nova-3`) | whisper-large-v3-turbo | whisper (`@cf/openai/whisper`) |
|---|---|---|---|
| Price | $0.0052/min HTTP, $0.0092/min WS | $0.000513/min | $0.000453/min |
| Input | `{body, contentType}` object (schema says only `body: object`, `contentType: string`) plus option flags | `audio`: base64 string OR `{body, contentType}` object | raw binary string, or `{audio: number[]}` (bytes 0-255) |
| Language | `language` = BCP-47 hint; "only certain languages are available" depending on model/endpoint (unverified that `de` is accepted); also `detect_language` | `language` string; `task` transcribe/translate; `vad_filter`, `initial_prompt`, `beam_size` etc. | no language param listed |
| Output | `results.channels[].alternatives[]{transcript, confidence, words[]{word,start,end,confidence}}`, optional summary/sentiments | `text`, `word_count`, `segments[]{start,end,text,words[]{word,start,end},...}`, `transcription_info{language,duration}`, `vtt` | `text`, `word_count`, `words[]{word,start,end}`, `vtt` |
| Input formats | `encoding` enum: linear16, flac, mulaw, amr-nb, amr-wb, opus, speex, g729 (so headerless PCM needs `encoding`+`sample_rate`; WAV/FLAC/mp3 container support not stated) | not stated on page | not stated |

- Nova-3 documented input parameters (all booleans unless noted): `language`, `smart_format`, `punctuate`, `utterances`, `utt_split` (number), `paragraphs`, `diarize`, `multichannel`, `channels` (number), `encoding`, `numerals`, `filler_words`, `detect_language`, `keyterm`, `keywords`, `redact`, `search`, `replace`, `topics`, `sentiment`, `mode` (general/medical/finance), etc. WS-only: `interim_results`, `endpointing`, `vad_events`, `utterance_end_ms`. So Q2: all the pre-recorded options you named are listed as parameters. Gap: the output schema does not list `utterances` or `paragraphs` result objects, so whether they appear in the HTTP response is unverified; word timestamps are in the schema.
- Max audio size/duration per request: not documented on any model page or on the Workers AI limits page. The limits page only has rate limits: Automatic Speech Recognition = 720 requests/min. Related hard numbers: Worker request body 100 MB on Free/Pro plans (https://developers.cloudflare.com/workers/platform/limits/index.md); Workers AI Batch API payload < 10 MB (https://developers.cloudflare.com/workers-ai/features/batch-api/index.md; this is the async queue API, not the plain HTTP call). Cloudflare's own Whisper tutorial splits audio into ~1 MB chunks to avoid memory/time limits (https://developers.cloudflare.com/workers-ai/guides/tutorials/build-a-workers-ai-whisper-with-chunking/index.md). Plan on chunking (e.g. 30-60 s, or a few MB) and test the real ceiling.
- R2 URL instead of bytes: not documented for these models (none of the schemas has a URL field). Read the object in the Worker and pass bytes/base64.
- Rust: workers-rs has `worker::Ai` with `run<T: Serialize, U: DeserializeOwned>(model, input)` (source: https://github.com/cloudflare/workers-rs/blob/main/worker/src/ai.rs; listed at https://developers.cloudflare.com/workers/languages/rust/index.md). The REST API is also available (https://developers.cloudflare.com/workers-ai/get-started/rest-api/index.md). Gap: no Rust example for speech models; serializing a byte array via serde_wasm_bindgen may become a JS array of numbers, so base64 string for whisper-large-v3-turbo is the safer shape (unverified).

## 2. R2 from a DO in Rust

- workers-rs `Bucket` (https://github.com/cloudflare/workers-rs/blob/main/worker/src/r2/mod.rs): `put`, `get`, `head`, `delete`, `list`, `create_multipart_upload`, `resume_multipart_upload`; `MultipartUpload` has `upload_part(part_number: u16, ...)`, `upload_id()`, `abort()`, `complete(parts)`; `UploadedPart::new(part_number, etag)` can rebuild a part record (so persist `(part_number, etag)` in DO storage).
- Multipart rules (https://developers.cloudflare.com/r2/objects/multipart-objects/index.md and https://developers.cloudflare.com/r2/api/workers/workers-api-reference/index.md): min part 5 MiB except last; all parts except last the same size; part max 5 GiB; max 10,000 parts; incomplete uploads auto-abort after 7 days (lifecycle-configurable); `resumeMultipartUpload` does no validity check; uploads can be completed/aborted from another invocation.
- Limits (https://developers.cloudflare.com/r2/platform/limits/index.md, updated 2026-06-08): object up to 5 TiB (4.995 TiB multipart); single-part upload 5 GiB; 1 write/s per same key (429 beyond); metadata 8 KiB. Don't append by rewriting one key; use distinct keys per chunk or multipart.
- Hibernation: the docs do not say "survives hibernation"; it follows from the design (upload id is a plain string, resume is stateless). Persist upload id + part etags in DO SQLite storage. Verified for the API; the hibernation conclusion is inferred.
- DO limits (https://developers.cloudflare.com/durable-objects/platform/limits/index.md): 30 s CPU per request/WS message (configurable to 5 min), 10 GB storage per DO, 2 MB max value/row, received WS message up to 32 MiB.
- Pricing (https://developers.cloudflare.com/r2/pricing/index.md): Standard $0.015/GB-month; Class A $4.50/M; Class B $0.36/M; egress free; free tier 10 GB-month, 1 M Class A, 10 M Class B. Infrequent Access $0.01/GB-month, $9/M Class A, $0.90/M Class B, $0.01/GB retrieval, 30-day minimum. `CreateMultipartUpload`, `UploadPart`, `CompleteMultipartUpload` are Class A; `AbortMultipartUpload` and delete are free. A 5 MiB part is ~3 min of 16 kHz mono s16, i.e. ~20 Class A per speaker-hour (negligible).

## 3. Workflows vs DO alarms

- Workflows docs list a JavaScript/TypeScript API (`WorkflowEntrypoint`, `step.do`) and a Python SDK (https://developers.cloudflare.com/workflows/llms.txt, https://developers.cloudflare.com/workflows/build/workers-api/index.md, updated 2026-10-08). No Rust SDK page found; workers-rs has no Workflow binding (grep of README and lib.rs empty). Verified: no documented Rust path. Workaround (unverified feasibility): a small TS Workflow Worker triggered by REST API or binding, calling a Rust Worker via service binding; or call the Workflows REST API from Rust (https://developers.cloudflare.com/workflows/build/trigger-workflows/index.md).
- Workflows limits (https://developers.cloudflare.com/workflows/reference/limits/index.md, 2026-09-21): unlimited wall time per step, 30 s default / 5 min max CPU per step, step result 1 MiB, 10,000 steps default (25,000 max), 10,000 subrequests default, 1 GB state per instance (paid), 365-day sleep.
- DO alarms (https://developers.cloudflare.com/durable-objects/api/alarms/index.md): one alarm per DO (a second `setAlarm` overrides); at-least-once; retry with exponential backoff from 2 s, up to 6 retries; alarm handler wall time 15 min (limits page); CPU 30 s default per invocation. Pattern: store a job queue in DO SQLite, process N chunks per alarm, reschedule. Many Workers AI calls fit because AI wait time is wall time, not CPU; keep each alarm run under 15 min and make steps idempotent. Subrequest cap for the DO (50 free / 10,000 paid per invocation) is per https://developers.cloudflare.com/workers/platform/limits/index.md.
- ASR rate limit 720 req/min per account bounds fan-out.

## 4. SFU WebSocket adapter (whole-meeting recording)

Sources: https://developers.cloudflare.com/realtime/sfu/features/media-transport-adapters/websocket-adapter/index.md (updated 2026-10-06), https://developers.cloudflare.com/realtime/sfu/platform/limits/index.md (2026-10-06), https://developers.cloudflare.com/realtime/sfu/platform/pricing/index.md (2026-09-22).

- Correction to the brief: the docs say "Each adapter connects one SFU track to your service in one direction". A create/close request takes 1 to 4 `tracks` entries, and each successful entry gets its own `adapterId` (and its own `sessionId` + `trackName`). So the 1-4 limit is per request, not tracks per adapter. Entries in one request can name different sessions (each entry has `sessionId`), so a 10-person meeting = 10 adapters in 3 requests. Partial success is normal (HTTP 200 with per-item `errorCode`); do not retry the whole batch.
- No per-app/per-session adapter count limit and no max adapter duration appear in the limits page (gap: absence of a documented cap, not proof there is none).
- Reconnect (egress, `location: "remote"`): SFU retries the same endpoint for up to 15 s after a disconnect, with a short bounded backlog that can drop old audio frames; no gapless guarantee. After 15 s the adapter closes and you must create a new one. The endpoint URL token must stay valid for reconnects. A DO that hibernates and closes the WebSocket would count as a disconnect only if the socket drops; hibernation keeps the socket open.
- Track end / participant reconnect: a track with no incoming media for 30 s is garbage-collected; a session that loses WebRTC connectivity has a 30 s reuse window, after which a new session is needed. Docs do not say what the adapter does when its source track is removed (gap, likely closes; verify by test). Plan to create a new adapter whenever a participant re-publishes (new sessionId/trackName), keyed by participant in the DO.
- Close is idempotent and returns `bytesProcessed` (not authoritative for billing).
- Cost: traffic from Cloudflare to the WebSocket endpoint is Realtime egress at $0.05/GB, 1,000 GB/month shared free with SFU/TURN. 48 kHz stereo s16le = 192,000 B/s = 691 MB/h per speaker, so a 10-person hour is ~6.9 GB (~$0.35 beyond free tier, free within it). Packet size limit 32 KB (given).
- Endpoint auth: no custom headers; put a scoped token in the URL path/query.

## 5. Audio size math

- 16 kHz mono s16le = 32,000 B/s = 115.2 MB/h per speaker (matches your figure; 1 h = 109.9 MiB). The adapter delivers 48 kHz stereo (6x larger), so downmix + resample in the DO before storing. R2 at $0.015/GB-month: 10 speakers x 1 h ~ 1.15 GB ~ $0.017/month.
- Whisper cost for 10 speakers x 60 min = 600 audio-min x $0.000513 = ~$0.31 per meeting (silent speakers still cost unless you skip silence); Nova-3 HTTP 600 x $0.0052 = ~$3.12.

## 6. Pure-Rust encoders for wasm32 (all from crates.io API, 2026-10-09; wasm builds NOT tested)

| Crate | Version (date) | Notes |
|---|---|---|
| `flacenc` | 0.5.1 (2025-12-18) | "Pure rust library for embedding FLAC encoder", https://github.com/yotarok/flacenc-rs. Lossless, ~50-60% of PCM size for speech (unverified estimate). Has optional `par` feature (threads); use `default-features = false`. wasm32-unknown-unknown compile unverified. FLAC is accepted by Nova-3 (`encoding: flac`), so no need to decode before transcription. |
| `opus-rs` | 0.1.37 (2026-10-07) | README: pure Rust port of libopus 1.6, `#![no_std]`, no alloc, "runs on ... WebAssembly"; encoder API `OpusEncoder::new(16000,1,Application::Voip)`, `encode_i16`. Gives ~16 kbps = 7 MB/h per speaker (vs 115 MB). Young crate (0.1.x); README claims, unverified by me. Raw Opus packets need an Ogg container (not part of this crate as far as I saw) for tools; Nova-3 lists `opus` encoding, Whisper input format support unknown. |
| `mousiki` | 0.2.1 (2026-03-26) | "Pure Rust Opus codec"; encoder/wasm support not confirmed. |
| `opus` | 0.4.0 | Bindings to C libopus; needs C toolchain for wasm32, not pure Rust. |
| `ogg-opus` 0.1.2 (2021), `audiopus` 0.2.0 (2021) | stale, C-based | skip |
| `hound` | 3.5.1 (2023) | WAV read/write only (no compression) |

- ADPCM: I found no maintained pure-Rust ADPCM encoder crate (`adpcm-xq` does not exist on crates.io); G.711 mu-law is trivial to hand-write (2:1) and matches Nova-3's `mulaw` encoding. Unverified that quality suffices for German ASR.
- Recommendation: store 16 kHz mono as FLAC (lossless, safe for ASR, works with Nova-3) unless storage cost matters; at $0.017/month per meeting-hour-of-10, compression is optional. Plain 16-bit WAV/raw PCM in multipart parts is simplest; compression is mainly worth it for R2 part sizes and the ASR upload size.

## Gaps (not verified)

1. Maximum audio size/duration per Workers AI ASR request (not documented); accepted containers (WAV/mp3/webm) for whisper models; whether `utterances`/`paragraphs` appear in Nova-3 HTTP responses; whether `language=de` is accepted by Nova-3 on Workers AI (Deepgram upstream supports German for Nova-3, unverified here).
2. How the Rust `Ai::run` serializes binary input for ASR; test with a real call.
3. Adapter behaviour on source-track end, and any per-app adapter cap or max duration.
4. wasm32 build of `flacenc` / `opus-rs`; encode CPU time vs the DO 30 s CPU per message limit.
5. DO hibernation + multipart is inferred from the API, not documented.
6. Workflows from Rust: no documented support; check https://github.com/cloudflare/workers-rs/issues before depending on it.
