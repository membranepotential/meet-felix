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

Code: [`crates/meeting-core/src/shell.rs`](../../crates/meeting-core/src/shell.rs)
(the SFU effects and replies), [`crates/meeting-core/src/protocol.rs`](../../crates/meeting-core/src/protocol.rs)
(`Rid`, `TrackKind`), [`web/src/lib/sfu.ts`](../../web/src/lib/sfu.ts),
[`web/src/lib/layers.ts`](../../web/src/lib/layers.ts),
[`web/src/lib/signaling.ts`](../../web/src/lib/signaling.ts),
[`web/src/lib/media/`](../../web/src/lib/media/); `crates/sfu-api` (not started).
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
  name already in use, and the protocol has no unpublish message, so a
  re-published kind needs a new name; the core then replaces the old track
  on every subscriber. Subscribers notice a turned-off track because it
  stops delivering frames (the browser reports `mute`), and show the
  participant's initials instead.
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
core's `Rid` names `low`, `mid` and `high` (`scaleResolutionDownBy` 4, 2
and 1). Each received camera tile reports its rendered width in CSS pixels;
below 240 px it asks for `low`, below 640 px for `mid`, otherwise `high`.
The browser sends `SetLayer` only after the track has arrived and only when
the layer changes, 400 ms after the last size change. A new subscription
starts at the SFU default, `high`.

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
