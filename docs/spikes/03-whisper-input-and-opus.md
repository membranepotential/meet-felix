# Spike 3: Whisper input formats and a pure-Rust Opus encoder (2026-10-09)

Question: can the recorder store compressed audio from the start, or does the
transcript job need raw PCM? Two parts: what Workers AI Whisper accepts, and
what an Opus encoder costs inside a Durable Object.

## Whisper large-v3-turbo accepts WAV, FLAC and Ogg/Opus

Test audio: two sentences from `espeak-ng`, German then English, mixed into
one 9.7 s clip at 16 kHz mono, sent as base64 in `audio` through the Workers
AI REST API (`/ai/run/@cf/openai/whisper-large-v3-turbo`).

| Format | Bytes | Time | Detected language | Text |
|---|---|---|---|---|
| WAV s16 | 309,534 | 2.5 s | en | both sentences correct |
| FLAC | 167,061 | 2.4 s | en | both sentences correct |
| Ogg/Opus 16 kbps (`libopus`, voip) | 17,971 | 1.6 s | de | correct except "And our" for "And now" |

Each reply had `segments` with `words` and `transcription_info`. The language
is detected per request; a mixed German/English chunk was transcribed in both
languages without a `language` parameter, so the job does not set one.

## opus-rs encodes at 37× realtime inside wasm

`opus-rs` 0.1.37 (pure Rust port of libopus 1.6, verified target
`wasm32-unknown-unknown` per its README) with `ogg` 0.9 for the container:

- Native, release, 67.7 s of speech at 16 kHz mono, 20 ms frames: 1.18 s,
  57× realtime.
- wasm32 in Node 26, 30 s of synthetic signal: 0.81 s, 37× realtime, about
  0.5 ms per 20 ms frame; a 30 s chunk costs about 0.8 s of CPU spread over
  1,500 WebSocket messages.
- The `.wasm` of encoder plus Ogg writer is 480 KB before compression.
- `ffprobe` reads the written file as Opus, 1 channel, correct duration.
- Caveat: `bitrate_bps = 16000` gave 21 kbps on synthetic noise in wasm and
  65 kbps on the native run; the implementation must check the rate it gets
  on real speech and aim for 16–24 kbps.

## Decision

Store Ogg/Opus chunks directly (updates D16, D17 and D31). The 115 MB per
speaker-hour of 16 kHz WAV becomes roughly 7–11 MB, Whisper reads the
chunks as they are, and no post-processing step is needed. If the encoder
ever turns out too costly in production, the fallback is plain WAV chunks;
the user accepted that trade on 2026-10-09.

Scripts: `scratchpad/asr` (REST calls) and `scratchpad/opus-spike`
(benchmark), not kept in the repo.
