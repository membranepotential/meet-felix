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
- [D16: Capture each speaker's audio on the server, as Ogg/Opus chunks of up to 30 seconds (2026-10-09)](#d16-capture-each-speakers-audio-on-the-server-as-oggopus-chunks-of-up-to-30-seconds-2026-10-09)
- [D17: Transcribe after the meeting with Whisper large-v3-turbo, behind a Transcriber trait (2026-10-09)](#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09)
- [D18: Keep the audio 48 hours; keep transcripts until the owner deletes them (2026-10-09)](#d18-keep-the-audio-48-hours-keep-transcripts-until-the-owner-deletes-them-2026-10-09)
- [D19: Show the transcript link to everyone at the end (2026-10-09)](#d19-show-the-transcript-link-to-everyone-at-the-end-2026-10-09)

Code: [`crates/meeting-core/src/protocol.rs`](../../crates/meeting-core/src/protocol.rs)
(`Recording`, `RecordingPolicy`, `ClientMsg::SetRecording`, `ClientMsg::SetConsent`,
`ServerMsg::ConsentRequired`), [`crates/meeting-core/src/shell.rs`](../../crates/meeting-core/src/shell.rs)
(`StartRecording`, `RecordTrack`, `StopRecordingTrack`, `StopRecording`,
`FinishRecording`), [`crates/meeting-core/src/step.rs`](../../crates/meeting-core/src/step.rs),
[`recording.rs`](../../crates/meeting-core/src/recording.rs) (roster, consent
record, speaker labels, silence gate, chunk cuts, holes) and
[`transcript.rs`](../../crates/meeting-core/src/transcript.rs) (merge,
`transcript.json`, `transcript.md`); the `RecorderObject` in
[`crates/worker/src/recorder/`](../../crates/worker/src/recorder/mod.rs)
(phase 3, built 2026-10-09); the adapter calls in
[`crates/sfu-api/src/client.rs`](../../crates/sfu-api/src/client.rs); the
transcript page in [`web/src/routes/t/[token]/+page.svelte`](../../web/src/routes/t/[token]/+page.svelte)
and [`web/src/lib/transcript.ts`](../../web/src/lib/transcript.ts).
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
  (not built on 2026-10-09). The start page shows the policy as one switch,
  "Record and transcribe", on by default (`FromStart`); off means
  `HostDecides`
  ([D25](../ui/README.md#d25-the-start-page-has-one-recording-switch-shows-the-words-on-one-line-and-ends-in-join-now-2026-10-09)).
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
  a persistent red "Recording" badge in the header, top left
  ([D28](../ui/README.md#d28-the-meeting-screen-keeps-a-labelled-control-bar-the-recording-badge-top-left-and-toasts-off-the-tiles-2026-10-09)),
  and a banner when recording starts, and plays a chime.
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
- Hosts see who has not accepted the running recording (as built
  2026-10-10). The list comes from the room view: every other participant
  with `consentRecording` false who is joined, connecting, or waiting in
  `AwaitingConsent`; knocking guests keep their knock request. The core
  computes that rule into the view's `awaitingConsent` flag on each
  participant (2026-10-11); the web app first derived it itself and is to
  read the flag. A
  host sees one notice in the pills above the control bar, "Anna and Bo have
  not accepted the recording yet", without a button, because only they can
  accept; it goes away when they accept, leave, or recording stops. The
  tiles of those already in the room carry "Not recorded" after the name
  (not on the narrow strip tiles beside a shared screen). Guests see
  neither. Why: a
  guest who was already in the call when recording started stays joined,
  muted and unrecorded, and the host could not tell. Code:
  `awaitingConsent()` in
  [`client-state.ts`](../../web/src/lib/client-state.ts), shown in
  `Toasts.svelte` and `Tile.svelte`.

Why in the core and not in the UI: the core sends every change of the public
room view to every joined client, so no client can miss a recording start,
and no shell code can record without the state saying so. The rule in
[CLAUDE.md](../../CLAUDE.md) follows from this: never add a recording path
that bypasses `RoomState`.

Why consent is not optional: in Germany, §201 StGB makes recording privately
spoken words without consent a crime.

The consent record (2026-10-09, research 11 item 5): each participant's
consent keeps the time it was first given and the version of the notice
shown (`NOTICE_VERSION` in the core, 1 for the current pre-join text; raise it
when the text changes in substance). Withdrawing consent records the time of
the withdrawal instead of forgetting the consent; giving it again clears the
withdrawal and keeps the first time. Recording requires a consent that is not
withdrawn. `transcript.json` lists everyone who ever consented under
`consents`, with `at`, `noticeVersion` and, if withdrawn, `withdrawnAt`. The
room sends each speaker's name and consent with every `/track` and the
current consent with every `/untrack`, so the recorder holds them even if the
final `/end` (`Effect::FinishRecording`, the whole roster and consent record)
never arrives; a failed `/end` is kept in the room's ended record and sent
once more by the room's alarm 30 s later. The recorder takes all three in
through one rule, `Recorder::learn` (2026-10-11, audit finding P2-009):
while recording, a `/track` or `/untrack` that the request numbers let
through replaces that speaker's name and consent; `/end` replaces both lists
whole, whenever it arrives before the transcript is written (also after the
recorder ended a meeting nobody ended); and once the meeting ended, a
per-speaker request is older than `/end` and changes nothing. Until then,
`/end` was merged into the names, overwrote the consents, and was dropped if
the recorder had ended the meeting itself, and a late `/untrack` could
overwrite a consent `/end` had set.

The recorder never records more than in-order delivery of the room's
requests would (2026-10-11, audit finding P1-002). The room carries out a
step's effects after storing the step, and each request to the recorder
awaits a fetch while other events run, so a later step's `/stop` or
`/untrack` (a stop, a consent withdrawal, a speaker leaving) could reach the
recorder before an earlier step's `/track`, which then recorded audio that
`RoomState` says is not recorded. Now:

- The room numbers every recorder request in the order its steps produce
  them (`next_recorder_request` in the room record, `seq` in the request
  body). A room starts counting from its clock (milliseconds × 1,000) when
  its record holds no number, so numbers keep rising even if older code
  dropped the counter from the record.
- The recorder keeps the newest number it applied per speaker and for
  `/stop`. It drops a `/track` older than the speaker's last request or the
  last `/stop`, and an `/untrack` older than the speaker's last request; an
  applied `/untrack` closes all of the speaker's adapters.
- A `/stop` closes every adapter, whatever its number. Every open adapter
  came from a `/track` the room sent while it recorded, and after a stop the
  room records nothing until it starts again and sends new `/track`s. So a
  wrong or reordered number can at worst leave a speaker unrecorded until
  their next track (logged as a warning), never record them after a stop.
- A request that overtakes the first `/start` finds no record yet: the
  recorder keeps an `/untrack`'s or `/stop`'s number, and a `/track` itself,
  under a separate key; `/start` takes the numbers into the new record and
  applies the kept `/track`s newest first, so one a later request superseded
  is dropped.

The pure rule is `Recorder::begin_track`, `untrack` and `stop` and
`BeforeStart` in `crates/worker/src/recorder/state.rs`. A test applies a
seven-request history in all 5,040 arrival orders: none records anything the
in-order delivery does not, and every order in which no later request
overtakes the `/stop` gives the in-order result. Why numbers and not one
queue of recorder calls per room: the rule holds whatever order the requests
arrive in, survives the room being evicted, and is tested without a Durable
Object.

**No rollback across the deploy of 2026-10-11** (the recorder record v2 and
the request numbers). Code from before it reads a recorder record only as the
bare version 1, so after `wrangler rollback` every recorder that the new code
touched fails all requests, its transcript job included. Old code also
rewrites room records without `nextRecorderRequest` and sends unnumbered
requests, which a recorder that saw numbers drops as stale, so speakers go
unrecorded. Recording stays safe in both cases (a `/stop` still closes
everything), but transcripts break. If the new code must be undone, deploy a
fix forward instead.

**No rollback across the deploy of audit wave 2 (2026-10-11) either.** The
code before it (wave 1) cannot read two stored shapes the wave-2 code writes:

- The owner's meeting index (KV key `index`) is stored in the versioned
  envelope `{"version": 2, "record": [...]}`. Wave-1 code reads only the bare
  array, so `GET /api/meetings` fails and new meetings are created but left
  out of the list. It fails before writing, so the index itself survives.
- An ended room's record no longer holds `recordingPolicy`, which wave-1 code
  requires. For a meeting that ended within the hour before the rollback,
  the lookup and a socket's upgrade fail instead of answering "ended", and
  the ended room's alarm fails, so a pending `/end` is not sent again and
  the room's storage is not deleted.

Wave-1 code reads the other wave-2 shapes, with less effect: a recorder
adapter whose creation failed (`failing`) looks like one still being
created and is not called again, the stretches it left unrecorded
(`unrecorded`) drop out of the transcript, and a transcript job waits up to
seven days for Workers AI again. A room record loses `since` of connecting
participants when wave-1 code rewrites it, so after a later deploy they
time out at the room's next alarm.

What the notice says (owner's decision, 2026-10-09): that the meeting is
recorded, that voices become a transcript everyone in the meeting gets, and
that video is not recorded. It does not list Cloudflare's subprocessors or
where the speech model runs (research 11 found no EU-residency commitment
for Workers AI); the owner judged that detail frightening for no gain in a
private meeting app with a known host.

## D16: Capture each speaker's audio on the server, as Ogg/Opus chunks of up to 30 seconds (2026-10-09)

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
   (a small FIR low-pass and 3:1 decimation; no resampling crate needed for
   an integer ratio).
4. It encodes every 20 ms frame to Opus as it arrives (`opus-rs`, pure Rust,
   voip mode, 16 kHz mono, about 16–24 kbps), so the encoding cost is spread
   over the WebSocket messages: about 0.5 ms per frame in wasm
   ([spike 3](../spikes/03-whisper-input-and-opus.md)).
5. It cuts the stream into chunks: at the first silence after 25 seconds, or
   hard at 30 seconds, and at a gap in the stream. A chunk is an Ogg/Opus
   file of about 40–90 KB.
6. It writes each chunk with one R2 `put` to
   `audio/{recorder}/{speaker}/{generation}-{n}.ogg` (changed 2026-10-09 from
   `rec/…`, so audio and transcripts have separate prefixes), with the
   chunk's wall-clock start and a silent flag in the object's custom
   metadata. There is no multipart upload during the meeting; the open
   chunk's packets are spooled to SQLite, so an eviction loses at most 1.5 s.
7. It records chunks that are entirely silent but flags them, so the
   transcript job skips them.

Changed on 2026-10-09 (the user asked for compressed storage; spike 3): the
plan had 16 kHz WAV chunks of about 1 MB plus a compressed copy made after
the meeting. Ogg/Opus from the start makes the copy unnecessary: Whisper on
Workers AI reads Ogg/Opus directly, and a speaker-hour shrinks from 115 MB
to roughly 7–11 MB. If the encoder ever proves too costly in production, the
fallback is WAV chunks again.

Load at 10 speakers: about 1.9 MB/s in messages of at most 32 KB, about 60
messages per second, billed 20:1 as about 3 requests per second. That is
within plan limits; the CPU work per message is one downmix, one resample and,
per 20 ms of audio, one Opus encode (about 0.5 ms in wasm, spike 3).

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

As built (2026-10-09):

- The room creates the `RecorderObject` (a SQLite Durable Object in the
  rooms' jurisdiction) at the first `StartRecording` and keeps its id in the
  room's record. Each adapter streams to
  `{RECORDER_ORIGIN}/rec/{recorder}/{secret}/{who}/{generation}`, where
  `RECORDER_ORIGIN` is the `workers.dev` host, so the media stream skips the
  Pages front door (research 11), and the secret is 128 random bits the
  recorder checks on every upgrade. A speaker's new track (a republish, or a
  session replaced under D33) closes the old adapter and opens the next
  generation. The recorder stores each adapter before and its id right after
  the SFU call, and creates one adapter per call, so a retry never duplicates
  one.
- A failed adapter call is retried (2026-10-11, audit finding P2-027). Until
  then a failure left the speaker unrecorded for the rest of the meeting, and
  the transcript did not say so. One likely cause, not verified for adapters:
  the room asks for the adapter right after the publish, and the SFU refuses
  pulls of a new track for 0.5–1 s (measured for subscriber pulls on
  2026-10-10). Now the recorder answers the room's `/track` with success
  either way, stores the failure on the adapter, and its alarm calls again
  after 1 s, 2 s, 5 s, 10 s, 30 s, then every minute; after ten failed calls
  (about five minutes) it gives up until the speaker's next `/track`, which
  then asks for a new adapter even for the same track. From the first failure
  until the adapter exists or closes, the speaker's audio is missing, and the
  transcript shows that stretch as a "missing" gap if it lasts at least 1 s.
  The rule is `Recorder::settle` in `crates/worker/src/recorder/state.rs`.
  Every call has its own generation, so its own endpoint: a failed call
  closes its generation, and the next call is made for the next one. The SFU
  has no endpoint that lists or looks up adapters, and an adapter's
  connection carries no id (WebSocket adapter docs, checked 2026-10-11), so
  if a call failed only on the way back, the adapter it created cannot be
  closed; it streams to a closed generation, is refused at the upgrade, and
  closes itself after the SFU's 15 s of reconnecting. Until it does, the SFU
  pulls the speaker's track for nothing. Right before each call the recorder
  reads the record again and skips an adapter that a `/stop` or `/untrack`
  closed meanwhile (`Recorder::to_create`). Not covered: an eviction during
  the first call, which leaves the adapter asked for and never answered, as
  before.
- A message whose `sequenceNumber` is not newer than the last, by at most
  1,000, is a duplicate and dropped; a step back of more than 1,000 means the
  counter restarted (a new connection, or a 16-bit RTP counter wrapping inside
  the u32 after about 22 minutes) and counts as a hole. A forward jump in the
  sequence number, a timestamp off by more than 20 ms from the
  step the stream showed so far, or more than 1 s between messages ends the
  chunk, and the next starts with a fresh wall-clock time. Times inside a
  chunk come from its sample count.
- Silence: a 20 ms frame below −50 dBFS RMS is quiet; the stream counts as
  silent after 300 ms of quiet frames. A chunk with less than 300 ms of loud
  frames is flagged silent and skipped by the job.
- Every chunk is its own Ogg stream with its own encoder, so it decodes
  without its predecessor; pre-skip 312 (the encoder's 6.5 ms lookahead, at
  48 kHz), 960 granule units per packet. `opus-rs` 0.1.37 takes its SILK
  bitrate from the output buffer's size, not from `bitrate_bps`, so the
  recorder caps packets at 61 bytes: 17.9 kbps on spike 3's speech clip.
- Encoded packets are appended to SQLite at least every 1.5 s, so an
  eviction or deploy loses at most that much; a chunk an eviction left open is
  closed from its spool the next time the recorder runs. A closed chunk goes
  to R2 in the background (at most two uploads at a time), and its spool rows
  are deleted after the `put` succeeds. A failed upload is retried after 2 s,
  10 s, then 60 s; after five failures the chunk is lost and shows as missing
  audio. A chunk with nothing in its spool (evicted within 1.5 s of its start)
  is skipped, never uploaded or transcribed.
- A stream has one socket at a time (2026-10-11, audit wave 2). A second
  upgrade for the same `{who}/{generation}` replaces the first: the adapter
  reconnects after a dropped connection, so the newer socket is the live one,
  and the packets of two connections never interleave in one chunk. When the
  recorder closes an adapter (an untrack, a stop, a newer track, the end), it
  removes the stream from the socket's attachment before closing it, so a
  message that still arrives is ignored; the attachment survives hibernation
  and eviction, unlike the in-memory flag that did this for the end until
  then. At the end it lets go of every socket, so no audio after the end is
  recorded.
- R2 layout: audio under `audio/{recorder}/{who}/{generation}-{index}.ogg`
  with custom metadata `startMs`, `durationMs`, `silent`, `who`, `n`;
  transcripts under `transcripts/{recorder}/transcript.json|md`. The two
  prefixes are separate so the lifecycle rule of D18 can expire audio only.
- Gaps in the transcript are derived from the chunk list: a hole of at least
  1 s between consecutive chunks of one adapter stream, chunks that never
  reached R2 ("missing"), and chunks the speech model gave up on ("not
  transcribed"); and from the recorder record, stretches of at least 1 s in
  which a speaker had no adapter because creating it failed ("missing",
  `unrecorded` in the record). The transcript names every speaker it
  mentions, also one of whom it has only gaps.

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
- Each chunk that is not silent goes to Workers AI Whisper large-v3-turbo as
  base64 Ogg/Opus, without a `language` parameter: the model detects the
  language per chunk, and a mixed German/English chunk came back correct in
  both languages ([spike 3](../spikes/03-whisper-input-and-opus.md)).
  Whisper returns segments with word timestamps.
- The job merges all speakers' segments by wall-clock time into one
  transcript, stored in R2 as `transcript.md` and `transcript.json`.
- Later (not in the first phase-3 delivery): it joins the chunks into one
  Ogg/Opus file per speaker for the transcript page, which the chunks allow
  without re-encoding. The earlier "compressed copy" post-processing step is
  gone (see D16).
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

Why Opus at capture time and not a copy afterwards: a 16 kHz mono WAV holds
115 MB per speaker-hour, too much to stream to a phone, and the copy would
have been a second pipeline. Spike 3 showed the encoder is cheap inside the
Durable Object and that Whisper reads the result.

Target (phase 3): a recorded 4-person meeting yields a correctly attributed
German/English transcript within 15 minutes of its end.

As built (2026-10-09; the account stays on Workers Free, which allows 10 ms
of CPU per invocation): the alarm runs one small step per invocation and
re-arms itself at once while work remains. A step uploads what an eviction
left behind, transcribes up to four stored chunks in parallel, or merges up to
300 segments into paragraphs; the last step writes both files. Requests use
`vad_filter: true` and `beam_size: 2`, and no prompt: on the test meeting of
2026-10-10 a prompt naming the participants made the model echo it on quiet
chunks ("Phone, Phone, Phone, …") and invent names ("Steven"); without one the
same chunks gave a filler word or nothing. Segments with `no_speech_prob`
above 0.6, `avg_logprob` below −1.0 (real speech scored −0.26 to −0.44, a
guessed "Okay." on a muted phone −1.03) or `compression_ratio` above 2.4 (the
echoed prompt scored 37) are dropped; the last two are OpenAI's reference
thresholds. An
error whose text names a quota, the daily allowance or neurons waits until
00:05 UTC, and one that says 429, "rate limit" or "too many requests" waits an
hour; the status shows `retryAt`, and the transcript never fails for that.
The job waits only while the audio is kept, until the meeting's end plus
48 hours (D18; changed 2026-10-11, audit finding P2-028): a wait that would
end later, or a step that runs later, gives up every chunk still waiting as
"not transcribed", so the transcript is written and the audio deleted on
time. Until then the job waited up to seven days without progress, while the
R2 lifecycle rule deleted the chunks under it after three, and the deleted
chunks ended up "not transcribed" after three wasted attempts. A wait that
older code stored ends at that time too (`Job::step`). A job step that
awaits Workers AI or R2 stores its progress on the record as it is
afterwards, not as it loaded it, so a `/end` that arrived meanwhile keeps
its roster and consents. A chunk that
R2 no longer holds is now lost at once and appears as "missing". On Workers
Free, which allows about 214 Whisper minutes a day ([cost/](../cost/README.md)),
a meeting whose speech exceeds about two days' allowance, roughly 430
speaker-minutes beyond what other meetings use, is therefore partly not
transcribed. Bare words such as "limit" do
not count, since an input size limit would otherwise retry forever. Every
transcription step first lets go of any adapter stream still open. Any other
error counts against the chunk, which after three attempts appears as a gap
("not transcribed"); a failing merge or write fails the transcript after six
tries in a row. `WhisperTurbo` is the only `Transcriber`; `Nova3Http` is not
built.

Open: the maximum audio size per Workers AI speech request is undocumented;
a 30 s Opus chunk is under 100 KB, far from any plausible limit. Spike 3
checked Whisper only; Nova-3 stays an option behind the trait.

## D18: Keep the audio 48 hours; keep transcripts until the owner deletes them (2026-10-09)

- The recorder deletes a meeting's audio chunks from R2 48 hours after the
  meeting ended (its own alarm); an R2 lifecycle rule deletes any chunk older
  than 3 days as the backstop for a recorder that never gets there. Within
  those 48 hours the transcript page can offer each speaker's audio.
- Transcripts stay until the owner deletes them: the past-meetings page gets
  a delete button per meeting, which removes the transcript objects and the
  index entry.
- The recording notice names this: audio is kept for two days, the
  transcript stays until the host deletes it.

Why: German guidance on meeting transcription (GDD Kurzpapier 4, January
2026; DSK Orientierungshilfe Videokonferenzsysteme) says to delete the
recording once the transcript exists and to state a retention period
([research 11](../research/11-gotchas-on-cloudflare.md)). The owner
approved this change on 2026-10-09 and set the audio retention to 48 hours;
it replaces "keep both indefinitely"
(decision 4 of 2026-10-09), which itself had replaced the plan v2 draft of
"recordings 30 days, transcripts until deleted". A joined per-speaker audio
file for the transcript page (D17, later) is available only within those
48 hours.

As built (2026-10-09): once the transcript is written, the recorder's alarm
fires at the meeting's end plus 48 hours (at once if the transcript took
longer) and deletes everything under `audio/{recorder}/`. The transcript job
never waits past those 48 hours for Workers AI (D17, changed 2026-10-11), so
"at once if the transcript took longer" means minutes, not days, and the
audio goes on time. The backstop rule
expires `audio/` after 3 days, counted from each chunk's upload; only a
meeting that ran for more than a day could lose early chunks to it before
its job reaches them, and those show as "missing". It is set once with
`pnpm exec wrangler r2 bucket lifecycle add meet-felix-recordings audio-3-days audio/ --expire-days 3 --jurisdiction eu`
(wrangler 4.147, flags checked with `--help`). Transcripts live under
`transcripts/`, untouched by it. The link itself is a KV entry
(`transcript:<token>`) without expiry; the delete button will remove it.

Because a `RecorderObject` lives as long as its transcript, and the
transcript page reads its record, every record it ever stored must stay
readable (2026-10-11, audit finding P1-003). The record is stored in the same
`{version, record}` envelope as the room's (`RECORDER_VERSION`, now 2);
version 1, the bare record stored until then, is read as it is. Unlike a
room, which treats an older record as an ended meeting, a recorder never
gives up a version: a change of shape adds a version and a migration.
Frozen records under `crates/worker/fixtures/recorder/` pin every shape.
A field that older records lack and older code ignores keeps the version
(the procedure in `crate::record`'s fixture tests): the adapter's `failing`
and the record's `unrecorded` (P2-027) are such fields, pinned by the
`retrying` fixture. The
SQL tables carry a version too, in a `schema_version` table, because Durable
Objects refuse `PRAGMA user_version` (it is not on workerd's pragma
allowlist, checked 2026-10-11); `Store::open` runs the migrations a database
still needs, and their count is tied to `SCHEMA_VERSION` at compile time.

Context: R2 storage costs less than $0.01 per month for a recorded 4-person
meeting-hour of Opus audio ([cost/](../cost/README.md)).

## D19: Show the transcript link to everyone at the end (2026-10-09)

Accepted on 2026-10-09: the transcript token appears in the URL of the
transcript page and of its API calls, and the adapter secret appears in the
adapter endpoint URL, so both end up in Workers Logs' invocation records,
which only the owner can read. The code itself never logs them. Turning the
invocation logs off would hide the request-level view the owner asked for
after the first test; the owner already holds every transcript, so the
logs add no new reader.

In v1 the room shows the transcript link
([D9](../links/README.md#d9-transcript-links-carry-a-128-bit-random-token-2026-10-09))
to everyone who was let in when the meeting ends (`ServerMsg::Ended`), so guests
can save it. "Let in" means connecting or joined, including a participant in
the reconnect grace period
([D35](../core/README.md#d35-a-dropped-socket-gets-a-minute-before-it-counts-as-leaving-2026-10-10)).
Everyone else still there gets `Ended` without the link: a guest who knocked
on a locked room, one who waits for consent in `AwaitingConsent` (also one who
declined it), and one who waits for the host. Changed on 2026-10-11 from
"everyone in the room": until then, a stranger who knew the words could knock
on a locked room, never be admitted, and still receive the link to the whole
transcript, names and consent record included (audit finding P1-001). The owner's past-meetings page also lists it
([D26](../ui/README.md#d26-past-meetings-and-transcripts-are-on-a-separate-owner-only-page-behind-the-same-password-2026-10-09);
changed on 2026-10-09 from "the host's browser also lists it"). The
transcript page itself is described in
[D31](../ui/README.md#d31-the-transcript-page-shows-speaker-labelled-paragraphs-with-wall-clock-times-2026-10-09).

Later (a phase 5 candidate): guests enter an email address at the end and
receive the transcript by mail. That needs a mail provider (decision 5 of
2026-10-09).
