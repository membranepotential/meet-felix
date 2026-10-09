# Media: SFU and browser

Camera, microphone and screen media flow between the browsers and Cloudflare
Realtime SFU. The `RoomObject` never touches media: it negotiates sessions and
tracks with the SFU on the clients' behalf. This page records how sessions are
negotiated, how simulcast layers are chosen, which SFU rules the design must
respect, which codecs are used, what each browser can do, and how the shell
defends against iOS.

Decisions on this page:

- [D10: Each participant has separate publish and subscribe sessions (2026-10-09)](#d10-each-participant-has-separate-publish-and-subscribe-sessions-2026-10-09)
- [D11: Cameras send simulcast, and each subscriber picks a layer (2026-10-09)](#d11-cameras-send-simulcast-and-each-subscriber-picks-a-layer-2026-10-09)
- [D12: Codecs are H.264 first, VP8 as fallback, no AV1 (2026-10-09)](#d12-codecs-are-h264-first-vp8-as-fallback-no-av1-2026-10-09)
- [D13: Detect features, not browsers; screen share is desktop-only (2026-10-09)](#d13-detect-features-not-browsers-screen-share-is-desktop-only-2026-10-09)
- [D14: Calls work in a plain browser tab, and iOS gets defensive handling (2026-10-09)](#d14-calls-work-in-a-plain-browser-tab-and-ios-gets-defensive-handling-2026-10-09)
- [D33: A dead SFU session is replaced, not retried (2026-10-09)](#d33-a-dead-sfu-session-is-replaced-not-retried-2026-10-09)

Code: [`crates/meeting-core/src/shell.rs`](../../crates/meeting-core/src/shell.rs)
(the SFU effects and replies), [`crates/meeting-core/src/protocol.rs`](../../crates/meeting-core/src/protocol.rs)
(`Rid`, `TrackKind`), [`web/src/lib/sfu.ts`](../../web/src/lib/sfu.ts),
[`web/src/lib/layers.ts`](../../web/src/lib/layers.ts),
[`web/src/lib/signaling.ts`](../../web/src/lib/signaling.ts),
[`web/src/lib/media/`](../../web/src/lib/media/), [`crates/sfu-api`](../../crates/sfu-api/).
Research: [01 §1 — Realtime SFU and TURN](../research/01-hosting-and-media-platforms.md),
[02 §2 — SFU features and limits](../research/02-cloudflare-deep-dive.md),
[05 §4–6 — browser constraints, PWA, codecs](../research/05-frontend-and-browser.md),
[06 — existing projects](../research/06-existing-projects-lessons.md).

## The SFU in short

Facts checked on 2026-10-09 ([01 §1](../research/01-hosting-and-media-platforms.md),
[02 §2](../research/02-cloudflare-deep-dive.md)):

- Cloudflare Realtime SFU forwards media tracks and DataChannels. It knows no
  rooms or participants; the `RoomObject` tracks which sessions and tracks
  each participant publishes.
- The control API is HTTPS at `https://rtc.live.cloudflare.com/v1`, with the
  app secret as a bearer token. Only the server holds the secret. Endpoints:
  `sessions/new`, `tracks/new`, `renegotiate`, `tracks/update`,
  `tracks/close`, and `adapters/websocket/new|close` for recording.
- Limits: 50 API requests per second per session; up to 64 tracks per
  request; no fixed limit on tracks per session. A track that sends no
  packets for 30 seconds is garbage-collected. A session that loses
  connectivity can be reused for 30 seconds.
- Price: $0.05/GB from Cloudflare to clients; publishing is free. The first
  1,000 GB per month are free, shared by SFU, TURN and adapters.
- TURN: TURN over TLS on port 443, credentials valid for up to 48 hours, no
  charge for traffic between TURN and the SFU, IPv4 relay addresses only, no
  TCP relaying. STUN at `stun.cloudflare.com` is free.
- TURN is used since 2026-10-09: `GET /api/ice?meeting=<words>` asks
  Cloudflare for credentials valid for 24 hours (`POST
  /v1/turn/keys/{TURN_KEY_ID}/credentials/generate-ice-servers` with the
  secret `TURN_KEY_API_TOKEN`) and hands the servers to the browser, minus
  the URLs on port 53, which browsers block
  ([Generate credentials](https://developers.cloudflare.com/realtime/turn/generate-credentials/),
  checked 2026-10-09). TURN traffic is billed, so the route answers only for
  a meeting that exists (404 otherwise). Without the two secrets, if the
  call fails, or if Cloudflare takes longer than 3 seconds, it serves STUN
  only; the browser gives up after 4 seconds and uses STUN. A phone on
  mobile data may need TURN to reach the SFU.
- No SFU region can be chosen
  ([D5](../architecture/README.md#d5-pin-data-to-the-eu-where-cloudflare-allows-it-2026-10-09)).

## D10: Each participant has separate publish and subscribe sessions (2026-10-09)

Every participant holds two SFU sessions, each with its own
`RTCPeerConnection`:

- **Publish session**: carries the participant's own tracks. The client
  sends the offer (`ClientMsg::PublishOffer`); the `RoomObject` adds the
  tracks through `tracks/new` and returns the SFU's answer.
- **Subscribe session**: carries the other participants' tracks. The SFU
  creates the offer, the `RoomObject` forwards it (`ServerMsg::SubscribeOffer`),
  and the client answers (`ClientMsg::SubscribeAnswer`). One negotiation runs
  at a time per subscribe session.

Why: each side always offers on its own connection, so the two sides never
send offers at the same time. That situation, called glare, needs rollback
logic in WebRTC; this design removes it. The SFU supports both one
bidirectional session and separate sessions
([01 §1](../research/01-hosting-and-media-platforms.md)). The core models the
two kinds as `SessionKind::Publish` and `SessionKind::Subscribe`.

Rules that follow from the SFU limits:

- A track idle for 30 seconds disappears. A turned-off camera must either
  keep sending packets or be published again when it is turned back on.
  Which of the two each target browser does is to be tested (spike 4).
  The web shell (2026-10-09) does not rely on either browser behaviour: it
  stops the transceiver when the camera or screen share is turned off, and
  publishes a new track with a new name when it is turned back on. A muted
  microphone stays published with `enabled = false`, and a device switch
  swaps the track on the existing sender with `replaceTrack`, without a new
  offer.
- Track names (2026-10-09): `mic-N`, `cam-N` and `screen-N`, where `N`
  counts the tracks published in this publish session. The core rejects a
  name already in use. Publishing a kind again replaces the old track on
  every subscriber. A tile shows video only while the room view lists
  that kind in the participant's `tracks` and the subscriber holds a live
  track for it; otherwise it shows the participant's initials. The browser's
  `mute` event is not used: Chromium 153 does not raise it when RTP stops
  (2026-10-09).
- Unpublishing (2026-10-09): `ClientMsg::Unpublish { kind }` stops a track
  at once, so a turned-off camera does not stay listed until the SFU drops it
  after 30 idle seconds. The core closes the track on the publish session
  and on every subscriber (`Effect::SfuCloseTracks`), stops recording it if
  it was recorded, and removes the kind from the participant's `tracks`.
- When a participant leaves, the core closes their tracks on every
  subscriber (`Effect::SfuCloseTracks`), so no subscriber keeps a dead track.

## D11: Cameras send simulcast, and each subscriber picks a layer (2026-10-09)

Each camera track is published as simulcast: the browser sends several
encodings of the same video, each named by a `rid`. The research suggests
three layers (high, mid, low with `scaleResolutionDownBy` 1, 2 and 4;
[05 §6](../research/05-frontend-and-browser.md)). Each subscriber asks for a
layer per track, with `preferredRid` on `tracks/new`, and switches with
`tracks/update`. In the core, `ClientMsg::SetLayer` leads to
`Effect::SfuUpdateLayer`. The browser picks the layer from the tile size.

Layer choice in the web shell (2026-10-09): the encodings' `rid`s are the
core's `Rid` names `q` (quarter), `h` (half) and `f` (full), with
`scaleResolutionDownBy` 4, 2 and 1. Each received camera tile reports its
rendered width in CSS pixels; below 240 px it asks for `q`, below 640 px for
`h`, otherwise `f`. The browser sends `SetLayer` only after the track has
arrived and only when the layer changes, 400 ms after the last size change.
A new subscription starts at `f`.

Why these names (2026-10-09): when the preferred layer is unavailable, the
SFU switches to the next `rid` in alphabetical order (`ridNotAvailable:
"asciibetical"`), so the names must sort from the best layer to the worst.
`f` < `h` < `q` falls back one step at a time; the earlier names `low`, `mid`
and `high` sort as `high` < `low` < `mid`, so a missing `high` fell back to
`low` and skipped `mid`. Cloudflare's docs use the same names
([Simulcast](https://developers.cloudflare.com/realtime/sfu/simulcast/),
checked 2026-10-09).

A screen share uses one or two layers, a low maximum frame rate, and
`contentHint = "detail"`, which keeps text sharp.

Why: a large tile needs the high layer, a thumbnail does not, and SFU egress
is the main cost ([cost/](../cost/README.md)). Above six participants the plan
caps the top layer if egress turns out higher than assumed.

Open: simulcast is described as limited in Safari. Test three `rid` layers
from an iPhone before relying on them (spike 4).

## D12: Codecs are H.264 first, VP8 as fallback, no AV1 (2026-10-09)

Clients negotiate H.264 first and VP8 as the fallback, and do not offer AV1.

Why: the SFU forwards packets without transcoding, so every subscriber must
decode what a publisher sends. H.264 is the only codec with hardware support
on every platform, including all iOS devices; VP8 is the universal software
fallback. AV1 is reported "supported" but neither smooth nor power-efficient
on an iPhone 15 Pro with iOS 26.4 ([WebKit bug 311593](https://bugs.webkit.org/show_bug.cgi?id=311593)).
Safari has a working WebRTC VP9, but the plan does not need it
([05 §6](../research/05-frontend-and-browser.md)).

## D13: Detect features, not browsers; screen share is desktop-only (2026-10-09)

The shell checks whether `navigator.mediaDevices.getDisplayMedia` exists and
hides the share button where it does not. It never reads the user agent to
decide what a browser can do.

Why: `getDisplayMedia` is unsupported on iOS Safari, Chrome Android, Firefox
Android and Samsung Internet. All iOS browsers use WebKit, so Chrome on an
iPad behaves like Safari. Phones and tablets can therefore watch a screen
share but cannot send one; that is a browser limit, and a web app cannot work
around it. User-agent checks fail because Safari 26 reports a frozen iOS
version ([05 §4](../research/05-frontend-and-browser.md)).

What each browser can do (2026-10-09):

| Capability | Desktop browsers | iPhone / iPad (all browsers) | Android Chrome / Firefox |
|---|---|---|---|
| Camera and microphone | Yes | Yes; capture stops when backgrounded (older WebKit bugs, current state unverified) | Yes |
| Send a screen share | Yes | **No** | **No** |
| Screen Wake Lock | Chrome 84, Firefox 126, Safari 16.4 | iOS 18.4 (16.4–18.3 broken in home-screen apps) | Chrome 84, Firefox 126 |
| Choose speaker (`setSinkId`) | Chrome 49, Firefox 116, Safari 18.4 | iOS 18.4 | No |
| Video Picture-in-Picture | Yes; Document PiP only Chrome 116, Firefox 151 | iOS 13.4, video only | Chrome Android 105 |
| Web Push for invites | Yes | Only home-screen web apps, iOS 16.4+ | Yes |

Sources: the MDN browser-compat-data 8.1.5 set and the WebKit bugs and posts
cited in [05](../research/05-frontend-and-browser.md) and
[00](../research/00-report-serverless-rust-video-meetings.md).
`contentHint` works in Chrome and Safari, not in Firefox.

## D14: Calls work in a plain browser tab, and iOS gets defensive handling (2026-10-09)

Invite links open and work in a normal Safari tab. The installable web app
(PWA) is optional; its main use is push notifications.

The shell handles iOS defensively:

- On `visibilitychange` and on a track's `mute` or `ended` event, reacquire
  the camera and microphone with `getUserMedia` and swap the new track in
  with `replaceTrack`.
- Reconnect the signaling WebSocket with backoff.
- Set `playsinline` on every video element.
- Hold a Wake Lock where available (iOS ≥ 18.4), and request it again after
  `visibilitychange`, because the browser releases it when the page is hidden.
- Picture-in-Picture and speaker selection follow in phase 4.

Why: older WebKit bugs (2019–2023) report capture muted or ended after
backgrounding, signaling sockets dead after about 30 seconds in the
background, and microphones lost after phone calls. Behaviour on iOS 26/27 is
unverified; spike 4 tests it on real devices. iOS 26 opens every site added to
the home screen as a web app by default, and home-screen web apps have a
history of camera bugs, which is why the plain tab is the default
([05 §4–5](../research/05-frontend-and-browser.md)).

Since 2026-10-09 a stalled microphone gets a manual recovery. On iOS a
phone call mutes the mic track, and afterwards WebKit fires neither `unmute`
nor `ended`, and may not fire `visibilitychange` either; calling
`getUserMedia` again fixes it, but WebKit captures only after a user action
([WebKit 213853](https://bugs.webkit.org/show_bug.cgi?id=213853),
[204681](https://bugs.webkit.org/show_bug.cgi?id=204681),
[240651](https://bugs.webkit.org/show_bug.cgi?id=240651);
[research 11](../research/11-gotchas-on-cloudflare.md), item 4). So the
event-driven reacquire above stays, and in addition `LocalMedia.micStalled`
turns true when the mic is on but its track has been `muted` for 2 seconds
or has ended. The mute button then becomes "Re-enable microphone", and a
notice says once that the microphone was interrupted. The tap captures the
mic again and swaps the new track onto the publisher's sender, the same
path a device switch takes. These bugs predate 2024; spike 4 checks on a
real iPhone whether they still apply.

## D33: A dead SFU session is replaced, not retried (2026-10-09)

When the SFU no longer serves one of a participant's sessions, or the
browser's peer connection for it has failed, the room drops that session and
creates a new one; the browser then opens a new peer connection for it. The
room never retries calls on a session the SFU declared gone.

Why: in the second real test (2026-10-09, Firefox on a PC, a phone on mobile
data), the phone's audio and video never reached the PC. For four minutes the
Worker log showed, every few seconds, `HTTP 410: session_error: Session
appears to be disconnected. Please check if the PeerConnection is
connected.` The room treated each failed subscribe as transient and retried
it against a session that could never work again, and the browser never
learned that its peer connection was unusable.

Triggers:

- The SFU answers a call on a session with HTTP 410, or with the error code
  `session_error` (also seen with 406 after an unanswered offer). `sfu-api`
  reports this as `FailureCause::SessionGone`; every other failure is
  `Transient` and is retried as before.
  - A subscribe or renegotiate call fails this way: the subscribe session is
    replaced.
  - A publish call fails this way: the publish session is replaced.
- The browser's `RTCPeerConnection.connectionState` becomes `failed`: it
  sends `ClientMsg::ResetSession { kind, epoch }`, at most once per kind
  until the room's reset arrives or 15 seconds pass, and shows a quiet
  notice. The epoch counts the sessions of that kind (0 for the first, one
  more per replacement; `Welcome` carries the current ones, each reset the
  new one). The room ignores a reset whose epoch is not current, so the late
  failure of a connection the room already replaced cannot replace the new
  session.

What the room does (`meeting-core`, [D20](../core/README.md#d20-core-first-2026-10-09)):

- Subscribe session: it forgets what the session received and any running
  negotiation, closes nothing (the SFU no longer serves the session) and
  emits `SfuNewSession`. Once the new session exists, it sends
  `ServerMsg::SubscribeReset`, and the browser replaces its subscriber peer
  connection. The new session wants every track the others publish, and the
  room requests them at once.
- Publish session: its tracks are gone for everyone, so the room does what an
  `Unpublish` of each does (closes them on every subscriber, stops recording
  them, removes them from the room view) and emits `SfuNewSession`. Once the
  new session exists, it sends `ServerMsg::PublishReset`; the browser opens a
  new publisher peer connection and publishes its current tracks again.
- While a session is being replaced, everything that works on that session
  skips it, and a second reset changes nothing. Client messages about it
  (`PublishOffer`, `Unpublish`, `SubscribeAnswer`, `SetLayer`) are dropped
  without an error; the reset makes the client redo them. SFU replies carry
  the session they are about, so a late reply about the old session is
  ignored.
- If the SFU cannot create the new session, or does not answer within
  `NEGOTIATION_TIMEOUT_MS` (15 seconds), the participant stays in the
  meeting, gets `Error { SfuUnavailable }` once, and the room asks again
  `SUBSCRIBE_RETRY_MS` (3 seconds) later through its alarm.
- A client that reconnects compares the epochs in its `Welcome` with its
  connections and replaces any whose session the room replaced while the
  socket was down.
