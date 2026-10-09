# Recording and transcript

A recorded meeting yields a transcript in German and English, attributed to
each speaker, shortly after the meeting ends. Everyone knows the meeting is
recorded before joining, and nobody's audio is recorded without consent. The
server captures each speaker's audio separately from the SFU, stores it in R2
in short chunks, and transcribes the chunks after the meeting with Whisper on
Workers AI. There is no video recording and no live captioning
([non-goals](../architecture/README.md#non-goals)).

Decisions on this page:

- [D15: Recording is announced by the core, and joining means consent (2026-10-09)](#d15-recording-is-announced-by-the-core-and-joining-means-consent-2026-10-09)
- [D16: Capture each speaker's audio on the server, in 30-second chunks (2026-10-09)](#d16-capture-each-speakers-audio-on-the-server-in-30-second-chunks-2026-10-09)
- [D17: Transcribe after the meeting with Whisper large-v3-turbo, behind a Transcriber trait (2026-10-09)](#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09)
- [D18: Keep recordings and transcripts indefinitely for now (2026-10-09)](#d18-keep-recordings-and-transcripts-indefinitely-for-now-2026-10-09)
- [D19: Show the transcript link to everyone at the end (2026-10-09)](#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)

Code: [`crates/meeting-core/src/protocol.rs`](../../crates/meeting-core/src/protocol.rs)
(`Recording`, `RecordingPolicy`, `ClientMsg::SetRecording`, `ClientMsg::SetConsent`,
`ServerMsg::ConsentRequired`), [`crates/meeting-core/src/shell.rs`](../../crates/meeting-core/src/shell.rs)
(`StartRecording`, `RecordTrack`, `StopRecordingTrack`, `StopRecording`),
[`crates/meeting-core/src/step.rs`](../../crates/meeting-core/src/step.rs).
The `RecorderObject` and the `Transcriber` trait are not started (phase 3).
Research: [09 — post-meeting transcription and recording](../research/09-post-meeting-transcription-recording.md),
[02 §3–4 — adapters and Workers AI speech](../research/02-cloudflare-deep-dive.md),
[07 — live transcription](../research/07-live-transcription.md),
[00 — report](../research/00-report-serverless-rust-video-meetings.md).

## D15: Recording is announced by the core, and joining means consent (2026-10-09)

- The owner chooses at creation whether a meeting is recorded, as a
  `RecordingPolicy`: `FromStart` (the default, because the transcript is the
  point) starts recording when the first host joins; `HostDecides` starts it
  only when a host sends `SetRecording`. The policy is part of `RoomView`; the
  Worker is to serve it at `GET /api/meeting/{words}` for the pre-join screen
  (not built on 2026-10-09).
- The pre-join screen shows the recording notice. In a recorded meeting the
  join button reads **"Join and accept recording"**; pressing it is explicit
  consent (decision 2 of 2026-10-09).
- A guest cannot enter a recording meeting without consent. A guest who
  joins, is admitted by a host, or is let in by unlocking while the room
  records, but has not consented, waits in the `AwaitingConsent` state: they
  get `ConsentRequired`, and hosts see them in the room view. They enter
  when they send `SetConsent { consent: true }` or when recording stops. A
  guest knocking on a locked room also gets `ConsentRequired`, so they can
  consent while they wait for a host.
- The recording state lives in `RoomState` (the plan sketched it as
  `Option<Recording { since, by }>`; the code has a `RecordingStatus`: never
  started, on, or stopped) and is part of every state broadcast. The UI shows
  a persistent red "Recording" badge and a banner when recording starts, and
  plays a chime.
- Only hosts start or stop recording (`ClientMsg::SetRecording`).
- When recording starts during the meeting, everyone in the room without
  consent gets `ConsentRequired` (the consent dialog) and `Muted`, and the
  core asks the recorder for none of their tracks; a guest still knocking gets
  only `ConsentRequired`. Everyone else's audio is
  recorded (`Effect::RecordTrack`). A participant who later sends
  `SetConsent { consent: true }` is recorded from then on. A participant who
  withdraws consent with `SetConsent { consent: false }` while the room
  records is no longer recorded (`Effect::StopRecordingTrack`), and gets
  `ConsentRequired` and `Muted`. A host may enter a recording room without
  consent, so they can stop the recording; they are muted and not recorded
  like everyone else without consent.

Why in the core and not in the UI: the core sends every change of the public
room view to every joined client, so no client can miss a recording start,
and no shell code can record without the state saying so. The rule in
[CLAUDE.md](../../CLAUDE.md) follows from this: never add a recording path
that bypasses `RoomState`.

Why consent is not optional: in Germany, §201 StGB makes recording privately
spoken words without consent a crime.

## D16: Capture each speaker's audio on the server, in 30-second chunks (2026-10-09)

The pipeline, per meeting:

1. When a consenting participant publishes audio, the `RoomObject` asks the
   `RecorderObject` to create an SFU WebSocket adapter for that track. One
   adapter carries one track. A create call takes up to four entries, so a
   10-person meeting needs 10 adapters in 3 calls. Partial success is normal:
   the SFU answers HTTP 200 with an error per failed entry.
2. The adapter streams the track as 48 kHz stereo 16-bit PCM in protobuf
   `Packet` messages of at most 32 KB to the `RecorderObject`'s public
   `wss://` endpoint. The adapter cannot send custom headers, so a scoped
   token goes in the URL.
3. The `RecorderObject` downmixes and resamples each stream to 16 kHz mono
   (`rubato`).
4. It cuts the stream into chunks: at the first silence after 25 seconds, or
   hard at 30 seconds. A chunk is a 16 kHz mono 16-bit WAV of about 1 MB, the
   size Cloudflare's own Whisper tutorial uses.
5. It writes each chunk with one R2 `put` to
   `rec/{meeting}/{speaker}/{n}.wav`. There is no multipart upload during the
   meeting, so hibernation can lose at most the current partial chunk.
6. It records chunks that are entirely silent but flags them, so the
   transcript job skips them.

Load at 10 speakers: about 1.9 MB/s in messages of at most 32 KB, about 60
messages per second, billed 20:1 as about 3 requests per second. That is
within plan limits; the CPU work per message is one downmix and one resample.

Reconnects: after a dropped connection the adapter retries its endpoint for
15 seconds and may drop old audio frames meanwhile; after 15 seconds it
closes and must be created again. When a participant publishes audio again,
the `RoomObject` asks the `RecorderObject` for a new adapter. Chunk numbers
continue per speaker, with wall-clock offsets. The recorder tracks gaps and
marks them in the transcript. What an adapter does when its source track ends
is undocumented; spike 2 tests it.

Why on the server: a phone that goes to the background or closes the tab
would lose audio recorded in the browser. Per-speaker tracks give exact
attribution without separating speakers from a mix.

Cost: each speaker's audio leaves the SFU once more, at 0.69 GB per
speaker-hour, billed like media ([cost/](../cost/README.md)).

Rejected: R2 multipart upload during the meeting
([09 §2](../research/09-post-meeting-transcription-recording.md)). It needs
parts of at least 5 MiB and upload state that survives hibernation; one `put`
per chunk needs neither.

## D17: Transcribe after the meeting with Whisper large-v3-turbo, behind a Transcriber trait (2026-10-09)

When the meeting ends, the `RecorderObject` runs the transcript job in its
alarm loop:

- Each alarm run takes at most 15 minutes of wall time. Every step is
  idempotent and recorded in DO storage (chunk → segments), so a retried
  alarm repeats no finished work.
- Each chunk that is not silent goes to Workers AI Whisper large-v3-turbo with
  the `language` parameter set. Whisper returns segments with word
  timestamps.
- The job merges all speakers' segments by wall-clock time into one
  transcript, stored in R2 as `transcript.md` and `transcript.json`.
- It stitches the chunks into one WAV per speaker with R2 multipart, and
  optionally one mixed WAV.
- Workers AI allows 720 speech requests per minute. Four speakers × 1 hour
  are about 480 chunks, which is a few alarm runs.

The speech model sits behind a Rust trait, `Transcriber`, with two
implementations through `worker::Ai`: `WhisperTurbo` and `Nova3Http`.

Why Whisper large-v3-turbo: it costs $0.0005 per audio minute, returns
segments with word timestamps, and takes a `language` parameter. Nova-3 over
HTTP costs $0.0052 per minute and offers all Deepgram options; it stays
available behind the same trait if German quality disappoints.

Why an alarm loop: Cloudflare Workflows have no Rust API
([09 §3](../research/09-post-meeting-transcription-recording.md)).

Target (phase 3): a recorded 4-person meeting yields a correctly attributed
German/English transcript within 15 minutes of its end.

Open: the maximum audio size per Workers AI speech request is undocumented.
Spike 3 compares Whisper and Nova-3 on German and mixed German/English audio
and confirms the chunk size.

## D18: Keep recordings and transcripts indefinitely for now (2026-10-09)

Audio chunks, stitched WAVs and transcripts stay in R2 until deleted by hand.
A cleanup job comes later (phase 5). This replaces the plan v2 draft of
"recordings 30 days, transcripts until deleted" (decision 4 of 2026-10-09).

Context: R2 storage costs less than $0.01 per month for a recorded 4-person
meeting-hour and $0.02 for a 10-person one ([cost/](../cost/README.md)).

## D19: Show the transcript link to everyone at the end (2026-10-09)

In v1 the room shows the transcript link
([D9](../links/README.md#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09))
to everyone in the room when the meeting ends (`ServerMsg::Ended`), so guests
can save it. The host's browser also lists it.

Later (a phase 5 candidate): guests enter an email address at the end and
receive the transcript by mail. That needs a mail provider (decision 5 of
2026-10-09).
