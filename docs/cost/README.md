# Cost

What meet-felix costs to run, compared with Google Workspace. The fixed cost
is the Workers Paid plan; everything else scales with meeting hours, and most
of it falls inside Cloudflare's free 1,000 GB per month. Realistic use (a few
four-person hours per week) lands at 5–8 € per month, about half of Google
Workspace Business Standard. All figures are from 2026-10-09. "Assumed" marks
the plan's own numbers; "unverified" marks claims without a primary source.
Spike 5 replaces the assumed bitrates with measured ones.

Decisions on this page:

- [D21: Build and measure; no further price analysis (2026-10-09)](#d21-build-and-measure-no-further-price-analysis-2026-10-09)

Code: none.
Research: [01 §2, §6 — Workers pricing, bandwidth per meeting-hour](../research/01-hosting-and-media-platforms.md),
[08 — Google Workspace baseline](../research/08-google-workspace-baseline.md),
[09 §4–5 — adapter and audio size math](../research/09-post-meeting-transcription-recording.md),
[00 — report](../research/00-report-serverless-rust-video-meetings.md) (cost sections).

## Assumptions

- Exchange rate: 1 € = 1.15 $ (mid-September 2026 rate).
- Bitrates (assumed, from the research report): about 1.2 Mbit/s per received
  camera stream, 1.5 Mbit/s for a screen share. To be measured in spike 5.
- Recording adds adapter egress: each speaker's audio leaves the SFU once, as
  48 kHz stereo PCM, at 0.69 GB per speaker-hour, billed like media.
- SFU, TURN and adapter egress share one pool: 1,000 GB free per month, then
  $0.05/GB. TURN is in use since 2026-10-09 (`GET /api/ice`, for known meetings only); traffic between
  TURN and the SFU is free, so relayed calls add little.
- Workers Free (the plan in use since 2026-10-09) caps the recorder instead of
  billing it: Workers AI 10,000 neurons a day, about 214 Whisper minutes at
  46.63 neurons per audio minute (the job waits for the 00:00 UTC reset
  instead of failing); SQLite DOs 100,000 rows written a day (deletes count),
  of which the recorder spends two per speaker every 1.5 s (a spool row
  written, later deleted) plus a few per 30 s chunk, about 5,000 per
  speaker-hour, so some 20 recorded speaker-hours a day; 100,000 DO requests
  a day, where incoming WebSocket messages count 20:1, so 20 ms adapter frames
  (50 a second) cost 9,000 billed requests per speaker-hour, about eleven
  speaker-hours a day if nothing else used the allowance (frame size
  unmeasured);
  10 ms CPU per WebSocket message and per alarm run
  ([DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/),
  checked 2026-10-09).

## Fixed cost

| Item | $/month | €/month |
|---|---|---|
| Workers Paid plan | 5.00 | 4.35 |
| Cloudflare Access (≤50 users), R2 free tier (10 GB), domain you own | 0 | 0 |
| **Idle total** | **5.00** | **4.35** |

The Workers Free plan supports SQLite-backed DOs with hibernation. Whether the
SFU and the Workers AI speech models work on it is unverified (spike 1). If
they do, the idle cost is 0 €.

## Variable cost per meeting-hour

| Meeting hour | Media egress | Adapter egress | Inside free 1,000 GB | Beyond pool ($0.05/GB) | Whisper transcript | R2 storage per month |
|---|---|---|---|---|---|---|
| 2 people | 1.1 GB | 1.4 GB | $0 | $0.13 | $0.06 | <$0.01 |
| 4 people | 6.7 GB | 2.8 GB | $0 | $0.48 | $0.12 | <$0.01 |
| 4 people + screen share | 8.7 GB | 2.8 GB | $0 | $0.58 | $0.12 | <$0.01 |
| 10 people | 22–50 GB | 6.9 GB | $0 | $1.45–2.85 | $0.30 | $0.02 |

Whisper bills per audio minute per speaker. Skipping silent chunks (the
recorder knows energy levels) cuts this further. The free pool covers roughly
100 four-person hours or 15–35 ten-person hours per month with recording on.

## Monthly scenarios

| Scenario | Egress | Transcript | Fixed | Total | vs ≈16–17 € |
|---|---|---|---|---|---|
| Light: 10 h, 4 people | 95 GB, free | $1.20 | $5 | **$6.20 ≈ 5.4 €** | cheaper |
| Medium: 30 h, 4 people | 285 GB, free | $3.60 | $5 | **$8.60 ≈ 7.5 €** | cheaper |
| Medium, Nova-3 instead of Whisper | 285 GB, free | $37 | $5 | **$42 ≈ 37 €** | more; use Nova-3 only for selected meetings |
| Heavy: 40 h, 10 people | 1,150–2,280 GB | $12 | $5 | **$25–81 ≈ 21–70 €** | more; cap the top simulcast layer, or accept |

With Whisper, transcription no longer decides the break-even: media egress is
the only lever, and it is free up to 1,000 GB. Realistic use (a few
four-person hours per week) lands at **5–8 € per month**, about half of
Business Standard, with idle months at 4.35 € or 0 €. The phase 5 target is a
month of real use under 10 €.

## Google Workspace baseline

- **Business Standard** is the cheapest Workspace plan with plain Meet
  transcripts; Business Starter has neither recording nor transcripts. Its
  German list price is **13.60 € net** per user per month (annual option),
  **≈16.18 € gross** with 19 % VAT. That matches the owner's ~17 €; the exact
  plan, net or gross, and annual or flexible billing were not checked against
  the invoice ([08](../research/08-google-workspace-baseline.md)).
- **Google AI Pro** (a lead, unverified): Google says "Take notes for me"
  came to Google AI Pro and Ultra subscribers on 2026-06-29, for meetings you
  organise, with participants notified. Its EUR price, and whether it
  includes plain transcripts and recording, are unverified. In the US AI Pro
  has listed at about $20, so it is unlikely to beat Business Standard.

## Levers

- **Transcription model**: Whisper large-v3-turbo costs a tenth of Nova-3
  HTTP. Keep Nova-3 for selected meetings
  ([D17](../recording/README.md#d17-transcribe-after-the-meeting-with-whisper-large-v3-turbo-behind-a-transcriber-trait-2026-10-09)).
- **Silent chunks**: skip them in transcription.
- **Top simulcast layer**: cap it above six people
  ([D11](../media/README.md#d11-cameras-send-simulcast-and-each-subscriber-picks-a-layer-2026-10-09)).
- **Free pool**: staying under 1,000 GB per month makes media and adapters free.
- **Workers Free plan**: if the SFU and Workers AI work on it, idle cost
  drops from 4.35 € to 0 €.

## D21: Build and measure; no further price analysis (2026-10-09)

The plan's figures are good enough to decide; no more time goes into price
research (decision 1 of 2026-10-09). Real numbers come from use: spike 5
measures egress for a real four-person hour, and phase 5 adds the Cloudflare
usage dashboard and a monthly budget alert. When measured numbers exist, they
replace the assumed ones on this page, with their date.
