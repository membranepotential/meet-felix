# Spike 2: the first call through the real Cloudflare Realtime SFU (phase 1 acceptance)

*2026-10-09. Run locally: the Worker under `wrangler dev` with the real SFU credentials from `.dev.vars`, the web app under Vite, three headless Chromium contexts driven by Playwright. Nothing was deployed.*

## Result

**Not accepted yet.** Audio and video flow through the real SFU, but only one way per pair: a participant receives the tracks that already existed when they joined, and never (or only after a retry) the tracks published after that. In a two-person call the host sees the guest as initials for the whole meeting while the guest sees and hears the host. The cause is in the room's SFU handling, not in the browser code: the room pulls a freshly published track into the other participants' subscribe sessions in the same step that returns the publisher's answer, before the publisher's peer connection is connected. The SFU rejects every track of that pull (`not_found_track_error`), but still returns an SDP offer that the `sfu-api` client drops. A subscribe session whose first pull fails this way then waits for an answer the room never sends, and every retry on it fails with HTTP 406 and then HTTP 410 until the meeting ends. See "Diagnosis".

Everything else in the walking skeleton works: create with the secret, pre-join with consent, Welcome and State, publish with three H.264 simulcast layers, ICE to the SFU (srflx → host, 9–70 ms RTT), subscribe and play remote video and audio in Chromium, camera off and on with a new publish offer, lock, lobby, knock, admit, end for everyone with the transcript link, and the "This meeting does not exist" page on a later join.

Three runs (24/31, 23/31 and 11/19 checks; the first run had two script bugs) agreed on every finding. Script, screenshots, WebSocket frame logs, `getStats()` dumps and the wrangler log are in the session scratchpad (`…/scratchpad/pw/call.mjs`, `shots/`, `logs/`); the script is not in the repo.

## Versions

| Component | Version |
|---|---|
| `wrangler` / bundled `workerd` | 4.147.0 / 1.20261001.1 |
| `worker` crate | 0.8.7 |
| Rust / `wasm-pack` / Node | 1.95.0 / 0.15.0 / 26.6.0 |
| Svelte / SvelteKit / Vite | 5.57.1 / 3.0.0 / 8.3.2 |
| Chromium (system, headless) | 153.0.8010.52 |
| Playwright (library only, with the system Chromium) | 1.63.0 |
| Realtime SFU API | `https://rtc.live.cloudflare.com/v1`, ICE-lite, media from 141.101.90.0:1473 |

## How to run

1. Copy `.dev.vars` (with `CREATE_SECRET`, `CALLS_APP_ID`, `CALLS_APP_SECRET`) into the checkout. `pnpm install`, then `pnpm --filter web build:core` (≈4 min cold; needs `~/.cargo/bin` on `PATH` for `worker-build`).
2. `mkdir -p web/build` (or `pnpm --filter web build`): `wrangler dev` refuses to start when `assets.directory` is missing (`ERROR The directory specified by the "assets.directory" field in your configuration file does not exist`). A fresh checkout has no `web/build`.
3. `pnpm exec wrangler dev --port 8787 --var ROOM_JURISDICTION:` and wait for `Ready on http://localhost:8787`. `GET /api/ice` answers `{"iceServers":[{"urls":["stun:stun.cloudflare.com:3478"]}]}`: STUN only, no TURN.
4. `pnpm --filter web dev --port 5173`. Vite moves to 5174 if 5173 is taken (it was, by an unrelated container); the proxy to 8787 still works.
5. Playwright: `chromium.launch({ executablePath: '/usr/bin/chromium', args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required'] })`, one `browser.newContext()` per participant (separate `localStorage`, so only the creator holds the host token). An `addInitScript` wraps `window.RTCPeerConnection` to collect the app's connections in `window.__pcs`, since the app does not expose them; `page.on('websocket')` logs every signaling frame.
6. The script: host creates a meeting with recording "fromStart" (enters the create secret once), joins as "Host" through "Join and accept recording"; guest joins the link as "Guest"; both are checked for two tiles, `getStats()` twice 5 s apart, the remote `<video>` state and screenshots; host toggles the camera; host locks; a third context joins and must see the lobby; host admits; all three are checked; host ends; a fourth context opens the link.

## Checks (run 3, meeting `brim-tamper-dash`, 24/31)

```
PASS  host created a meeting with recording fromStart  (brim-tamper-dash)
PASS  pre-join offers "Join and accept recording"
PASS  host reaches the in-meeting view
PASS  both pages show two tiles
PASS  no consent dialog after joining with consent  (dialogs=0)
FAIL  A: inbound video bytesReceived grows over 5 s  (+0 bytes)
FAIL  A: inbound audio bytesReceived grows over 5 s  (+0 bytes)
PASS  A: outbound video has three simulcast encodings  (high 1280x720, mid 640x360, low 320x180, H264)
FAIL  A: both peer connections have a selected candidate pair  (publish srflx->host; subscribe: none)
PASS  B: inbound video bytesReceived grows over 5 s  (+187297 bytes, 300 kbit/s)
PASS  B: inbound audio bytesReceived grows over 5 s  (+15522 bytes, 25 kbit/s)
PASS  B: outbound video has three simulcast encodings
PASS  B: both peer connections have a selected candidate pair  (srflx->host, srflx->host)
FAIL  A: remote <video> plays  ([{"name":"Guest","video":null,"avatar":"G"}])
PASS  B: remote <video> plays  (640x360, !paused, readyState 4)
FAIL  host received at least one subscribe offer for the guest  (A received: 5x error:subscribeFailed, 0 subscribeOffer)
PASS  guest received at least one subscribe offer for the host
FAIL  guest shows host initials after camera off  (after 32 s still a frozen <video>; remote tracks muted=false)
PASS  guest shows host video again after camera on
PASS  a new publish offer was sent and answered for the new camera track  (publishOffer 1 -> 2, publishAnswer 1 -> 2)
PASS  room shows Locked on host and guest
PASS  third client sees the lobby (Waiting for the host)
PASS  host sees the knock
PASS  after admit, A, B and C each show three tiles
FAIL  A: two remote videos play with three in the room  (Guest and Third both initials)
PASS  B: two remote videos play with three in the room
PASS  C: two remote videos play with three in the room
PASS  A: reaches the end page
PASS  B: reaches the end page
PASS  C: reaches the end page
PASS  a fresh join attempt gets the ended/unknown page  (This meeting does not exist; GET /api/meeting -> 404 unknown meeting)
```

## Numbers

- **Publish (every participant).** Three `outbound-rtp` video entries, `rid` high/mid/low at 1280x720, 640x360, 320x180, `video/H264`, `scalabilityMode L1T1`, all active. After ≈60 s on the host: 1,373,995 / 683,967 / 310,134 bytes sent; audio `audio/opus`, 242,547 bytes.
- **ICE.** Every connected peer connection selected `local srflx (udp) → remote host 141.101.90.0:1473`, `currentRoundTripTime` 0.009–0.07 s. No relay: `/api/ice` serves STUN only.
- **Receive (guest, from the host).** Over 5 s: video +187,297 bytes (300 kbit/s), audio +15,522 bytes (25 kbit/s) in run 3; 85 and 10 kbit/s in run 2; 126 and 10 kbit/s in run 1. Video decoded at 640x360 (the `mid` layer the tile asked for) in runs 2 and 3; 1280x720 in run 1, where `setLayer` had been refused (below). The remote `<video>`: `videoWidth 640`, `paused false`, `readyState 4`, `currentTime` advancing; the screenshot shows the fake camera's rotating sector and timer on the remote tile.
- **Receive (third, from host and guest).** Two audio and two video `inbound-rtp` entries, 131,907 / 131,747 and 1,252,967 / … bytes after ≈45 s; both remote tiles play.
- **Receive (host).** Nothing: `inbound-rtp` empty, the subscribe peer connection never left `new` (no candidate pair), 0 `subscribeOffer` frames, 21 `error{subscribeFailed}` frames in 2 minutes (run 3; 5 by the time the two-person checks ran).
- **Camera off → on (host).** Click → `publishOffer` 47 ms → `publishAnswer` 254 ms → guest's `subscribeOffer` 396 ms (17:08:12.297 → .344 → .551 → .693). The new track is `cam-2` on a new mid; `getUserMedia` for the restart took 13 ms. The guest's tile shows the new video right away.
- **Signaling.** Join → Welcome 0.4–0.5 s; a guest's first `subscribeOffer` 0.2 s after its `publishOffer`; Lock → Locked on both pages < 1 s; knock 0.7 s after the third client's Join; End → `ended` on all three sockets within 0.1 s, sockets closed with 1000.
- **Retry cadence of the failing subscription.** `subscribeFailed` reached the host at +1.9, +3.6, +3.1, +3.1, +8.1, +8.1, +8.1, +8.1, +8.1 s… (`SUBSCRIBE_RETRY_MS` is 3 s; the DO alarm adds the rest), later every ≈5.2 s.

## Diagnosis: the host never receives the guest

Evidence, in order (run 2, `area-ditch-harsh`; runs 1 and 3 are the same):

1. Guest `publishOffer` 17:02:59.644 → `publishAnswer` 17:02:59.953. In the same `step`, `published()` (`crates/meeting-core/src/step.rs`, the loop over `subscribers` after `fx.send(who, ServerMsg::PublishAnswer …)`) adds the guest's audio and video to the host's `wanted` set and calls `request_wanted`, so the Worker sends `POST sessions/{host-subscribe}/tracks/new` with the two remote tracks at once.
2. wrangler: `SFU call failed: track: not_found_track_error: Track not found on remote peer. Make sure the publisher peer is connected and sending packets for this track`. The guest's peer connection had the answer for 0.2 s and was not yet connected (ICE + DTLS take longer than that). The host gets `error{subscribeFailed}` at 17:03:00.168.
3. A direct probe of the SFU API with two throwaway sessions (`scratchpad/pw/sfu-probe.mjs`) shows what the response looks like when every pulled track is missing: **HTTP 200** with `"requiresImmediateRenegotiation":true`, one `errorCode: not_found_track_error` per track, **and a `sessionDescription` offer** (936 chars, 1 m-line). `SfuClient::subscribe` (`crates/sfu-api/src/client.rs`) returns `Err(first_error)` when `offered.is_empty()` and never answers that offer.
4. The retry 3 s later: a second `not_found_track_error` (the publisher may still not have been "sending packets"), then wrangler: `SFU call failed: HTTP 406: invalid_session_description: The current signaling state is expecting a remote answer: renegotiation is expected before pulling or pushing tracks. API requests were possibly made out-of-order without error checking.` The SFU is waiting for the answer to the offer from step 3.
5. From then on every retry: `SFU call failed: HTTP 410: session_error: Session appears to be disconnected. Please check if the PeerConnection is connected.` (nine times in two minutes in run 2, twenty in run 3). The core keeps the same subscribe session and retries forever; the host's subscribe `RTCPeerConnection` never gets an offer, so it never connects, so the session can never become "connected".

Why the guest and the third participant are fine: their first pull asked for tracks whose publishers had been sending for seconds (`subscribeOffer` 0.2 s after their own `publishOffer`, before their own `publishAnswer`). Their session's first negotiation succeeded, so later pulls run on a connected session, and a `not_found` there is survivable: in run 3 the guest got Third's tracks 11.3 s after Third's `publishAnswer` (17:08:39.773 → 17:08:51.122), in run 1 the host's re-published camera arrived at the guest 0.4 s after the `publishAnswer`.

So there are two defects, both outside `web/`:

- **Race (core):** `published()` pulls a track into the other participants' subscribe sessions before the publisher is connected. The SFU's own error text says what it needs: "the publisher peer is connected and sending packets". The core has no event for "publisher connected"; either the pull waits for one (the client could report `connectionState === 'connected'`, or the Worker could wait and retry on `not_found_track_error` with a short delay), or a `not_found` must be treated as "retry soon" rather than as a failed negotiation.
- **Dropped offer (sfu-api/core):** a `tracks/new` whose every track failed still carries an offer that the SFU expects answered. `subscribe()` discards it. After that the session is unusable, and the core never replaces a subscribe session that keeps failing. Either answer every offer the SFU sends (even one with no usable track), or, on 406/410, close the session and create a new one (`SfuNewSession`).

Not changed here: the task was to verify, not fix.

## Other findings

- **Camera off does not reach the guest's tile.** The host's `Stop video` stops the sender's transceiver; the guest keeps a frozen frame (`currentTime` stuck at 22.964 for 32 s, screenshot `04-B-host-camera-off.png` shows the host's fake-camera timer 15 s behind the guest's own). `RTCRtpReceiver.track.muted` stayed `false` on both of the host's tracks for the whole 32 s, so `isLive()` in `web/src/lib/media/remote.ts`, which waits for a `mute` event, never flips, and the tile never shows the initials. Chromium 153 does not raise `mute` when RTP stops on a still-negotiated m-line; the design note in `remote.ts` assumes it does. The protocol has no "track off" message, so the UI has nothing else to go on. Once the camera is on again, the new track replaces the frozen one at once.
- **`setLayer` right after the subscribe answer is refused.** Run 1: the guest sent `setLayer{owner 0, video, mid}` 0.4 s after its `subscribeAnswer` and got `error{notSubscribed}`; the room was still renegotiating. `LayerControl` does not retry, so that guest decoded 1280x720 for the rest of the call. Runs 2 and 3 did not hit the window and got 640x360.
- **`web/build` is required for `wrangler dev`** even when the site is served by Vite (step 2 above). The `dev` script in `package.json` does not create it.
- **Wrangler log.** Apart from the SFU errors quoted above, the only non-info lines were `recording not implemented yet: StartRecording { … }`, `… RecordTrack { who: ParticipantId(n), track: RemoteTrack { session: …, name: TrackName("mic-1") } }` (one per joined participant) and `… StopRecording` at the end (`console_log!` in `crates/worker/src/room.rs`). No warnings, no panics. The web consoles were clean apart from Vite's HMR messages and a Chromium accessibility hint about the password field.
- **Fake media in headless Chromium** works with `--use-fake-device-for-media-stream --use-fake-ui-for-media-stream`; the pre-join `getUserMedia` took 235 ms. The end page shows `http://localhost:5174/t/<transcript id>` (a transcript id is minted on `End` although recording is not implemented).

## Open issues

1. Fix the race and the dropped offer (Diagnosis) and rerun the script: the acceptance criterion is "A: inbound video and audio grow" and "A: remote `<video>` plays" passing for the host, and three-way video after admit.
2. Decide how a tile learns that a remote camera is off (a protocol message from the core's `tracks` list, which already drops the kind, or an RTP-inactivity check with `getStats()` on the client), since Chromium does not raise `mute`.
3. Make `setLayer` safe during renegotiation (queue it in the core or retry in `LayerControl`).
4. Why the guest's pull of Third's tracks ran only 11 s after Third's `publishAnswer` in run 3 (no SFU error was logged for it): the alarm cadence, or a negotiation still marked running? Unverified.
5. Put the Playwright script into the repo (for example `web/e2e/`) with the `playwright` dependency, so phase 1 CI can run the call against a mock or the real SFU. It currently lives in the session scratchpad only.
6. Have the `dev` script create `web/build`, or document step 2.
7. Still untested: TURN (no credentials in `/api/ice`), screen share, reconnect/resume from the browser, more than three participants, and anything on a deployed Worker.

## Run 4 (after fixes)

*2026-10-09, main checkout at `d4fe54d` (after 94f1e7a "SFU offers kept" and d4fe54d "remote tiles follow the room view; empty offers; setLayer retry"). Same setup as above: `wrangler dev --port 8787` with `.dev.vars`, Vite on 5174, three headless Chromium contexts with fake media. The "Result" and "Checks" sections above describe runs 1–3; this section supersedes them. Script `…/scratchpad/pw/call4.mjs`, outputs in `…/scratchpad/run4/` (`logs/run4.log`, `logs/ws-*.log`, `logs/stats-*.json`, `logs/hold-samples.json`, `logs/wrangler.log`, `shots/`).*

**Accepted locally.** Meeting `rug-duke-edgy`, 43/44 checks; the one FAIL is a false alarm of the script (below). Every participant now receives audio and video from every other one; the host's subscribe session is never stuck; a camera turned off reaches the other tiles in under 100 ms; the three-party call held for 3 minutes with no reconnect and no SFU error beyond two transient `not_found_track_error`s that the retry resolved.

### Checks

```
PASS  host created a meeting with recording fromStart  (rug-duke-edgy)
PASS  pre-join offers "Join and accept recording"
PASS  host reaches the in-meeting view  (1020 ms after Join)
PASS  both pages show two tiles
PASS  no consent dialog after joining with consent  (dialogs=0)
PASS  B: first remote frame from the host  (4846 ms after the guest clicked Join)
PASS  A: first remote frame from the guest  (5134 ms after the guest clicked Join)
PASS  A: inbound video bytesReceived grows over 5 s  (+140610 bytes, 225 kbit/s)
PASS  A: inbound audio bytesReceived grows over 5 s  (+16172 bytes, 26 kbit/s)
PASS  A: outbound video has three simulcast encodings (f, h, q)  (f 1280x720, h 640x360, q 320x180, H264)
PASS  A: both peer connections have a selected candidate pair  (srflx->host, srflx->host)
PASS  B: inbound video bytesReceived grows over 5 s  (+141204 bytes, 226 kbit/s)
PASS  B: inbound audio bytesReceived grows over 5 s  (+15162 bytes, 24 kbit/s)
PASS  B: outbound video has three simulcast encodings (f, h, q)
PASS  B: both peer connections have a selected candidate pair  (srflx->host, srflx->host)
PASS  A: remote <video> plays  (Guest 640x360, !paused, readyState 4)
PASS  B: remote <video> plays  (Host 640x360, !paused, readyState 4)
PASS  host received at least one subscribe offer for the guest  (A received: subscribeOffer 2, error:subscribeFailed 0 at this point)
PASS  guest received at least one subscribe offer for the host
PASS  guest shows host initials after camera off within 1 s  (after 68 ms; receiver tracks still muted=false)
PASS  guest shows host video again after camera on  (after 1343 ms; first frames 320x180, later 640x360)
PASS  a new publish offer was sent and answered for the new camera track  (publishOffer 1 -> 2, publishAnswer 1 -> 2)
PASS  room shows Locked on host and guest
PASS  third client sees the lobby (Waiting for the host)
PASS  host sees the knock  (344 ms after Third clicked Join)
PASS  after admit, A, B and C each show three tiles
PASS  A: two remote videos play with three in the room  (Guest 640x360, Third 640x360)
PASS  B: two remote videos play with three in the room  (Host 640x360, Third 320x180 at first)
PASS  C: two remote videos play with three in the room  (Host 640x360, Guest 640x360)
FAIL  three-party call held 180 s: every inbound stream kept growing in every 30 s window  (6 samples, 6 "stalled" windows: all B's mid 1, see below)
PASS  no reconnect during the hold (no new socket, no new Join, no "Connection lost" banner, all 6 peer connections "connected" in all 6 samples)
PASS  A: two remote videos still play after the hold  (currentTime 202 s and 181 s)
PASS  B: two remote videos still play after the hold  (192 s and 181 s)
PASS  C: two remote videos still play after the hold  (185 s and 185 s)
PASS  A: subscribeFailed count 1, each followed by a subscribeOffer within 5 s  (3153 ms)
PASS  B: subscribeFailed count 1, each followed by a subscribeOffer within 5 s  (3383 ms)
PASS  C: subscribeFailed count 0
PASS  A: reaches the end page
PASS  B: reaches the end page
PASS  C: reaches the end page
PASS  a fresh join attempt gets the ended/unknown page  (This meeting does not exist; GET /api/meeting -> 404)
PASS  A: no console errors
PASS  B: no console errors
PASS  C: no console errors
```

The FAIL: after the host's camera off, the guest's subscribe session keeps the m-line of the closed `cam-1` track (mid 1; the core's `unsubscribe_everyone` closes the track on every subscriber's session, the SFU keeps the m-line, and the re-published camera arrives on a new mid 2). Chromium keeps an `inbound-rtp` entry for mid 1 with no new bytes, and the script counted that as a stall. Every live stream grew in every window. The check should ignore m-lines whose track the room no longer lists.

### Checklist against the task

1. **Host receives the guest and the third participant: yes.** Host `inbound-rtp`: audio +16,172 and video +140,610 bytes over 5 s with the guest alone; four inbound streams (two audio, two video) during the hold at 420–455 kbit/s video and 48–50 kbit/s audio per page; remote `<video>` elements at 640x360, playing, `currentTime` advancing to 202 s. Screenshots `03-A-two-tiles.png`, `08-A-three.png`, `08b-A-after-hold.png` (the guests' fake-camera timers match the host's own to the frame).
2. **Host's subscribe session never stuck: yes.** `error{subscribeFailed}` frames: A 1, B 1, C 0, all at 17:30:32.305, 0.3 s after Third's `publishAnswer` (17:30:31.996): the pull of Third's tracks into the host's and guest's sessions ran before Third's peer connection sent packets. The SFU answered both with `not_found_track_error` and no offer (nothing to renegotiate on an already connected session), so `subscribe()` returned the error, the core kept the tracks wanted and retried after `SUBSCRIBE_RETRY_MS`: `subscribeOffer` 3.15 s (A) and 3.38 s (B) after the error, answered within 0.1 s, first frame of Third 4.8 s (A) and 5.2 s (B) after the admit. The wrangler log has exactly two `SFU call failed: track: not_found_track_error …` lines and **no 406, no 410**. The host's first pull of the guest (17:30:10.996, 0.2 s after the guest's `publishAnswer`) returned an offer with one m-line (audio found, video not yet); the core retried the video at 17:30:14.133; such a partial pull logs no error.
3. **Camera off reaches the guest's tile in < 1 s: yes, 68 ms** (host `unpublish` 17:30:22.242 → guest's `State` 17:30:22.322 → tile shows "H"). The receiver tracks on the guest were still `muted=false`, so the room view is what drives it, as intended. Camera on: `Start video` click → `publishOffer` +120 ms → `publishAnswer` +371 ms → guest `subscribeOffer` +550 ms → `subscribeAnswer` +615 ms → guest sees video after 1343 ms (first frames at 320x180: 1.3 s after the re-publish the host's encoder had produced 22 frames on `q` and none on `f` and `h`; during the hold the guest decodes 640x360). `getUserMedia` for the restart took 23 ms.
4. **`setLayer` refusals retried: not exercised.** No `error{notSubscribed}` occurred in this run (0 on A, B and C; 7 `setLayer` frames in all); every subscriber decoded 640x360 (`h`) by the end, so every `setLayer` took effect. The retry is covered by `web/src/lib/layers.test.ts` only.
5. **No console errors: yes.** A, B and C logged nothing but Vite's HMR debug lines, the `[probe]` timings and Chromium's password-field accessibility hint.

### Timings

| Step | Time |
|---|---|
| Host: Join click → in-meeting view | 1.02 s (Welcome 0.76 s after `join`) |
| Guest: Join click → Welcome → first `subscribeOffer` → answer | 0.54 s → +0.19 s → +0.06 s |
| Guest: Join click → first frame of the host | 4.85 s (4.1 s after the subscribe answer; unexplained, A and C decode within 0.6 s of theirs) |
| Guest: Join click → host's first frame of the guest | 5.13 s (host's second `subscribeOffer`, with video, 3.1 s after the first) |
| Third: Join click → knock on the host | 0.34 s |
| Admit → Third sees host and guest | 1.30 s (`subscribeOffer` 0.16 s after Welcome, before Third's own `publishAnswer`) |
| Admit → host / guest see Third | 4.80 s / 5.16 s (one `subscribeFailed` each, retry after 3 s) |
| Camera off → initials on the guest | 68 ms |
| Camera on → video on the guest | 1.34 s |
| End → `ended` on all three sockets | 0.18 s, sockets closed with 1000 |

### Hold (three participants, 180 s, sampled every 30 s)

Per page and window: video 418–455 kbit/s (two streams), audio 47–50 kbit/s (two streams); `currentRoundTripTime` 9–29 ms on all six peer connections; `connectionState` `connected` in every sample; `packetsLost` 0 at the 5 s sample; one app WebSocket per page for the whole meeting (the second `OPEN` in the logs is Vite's HMR socket), no `join` after the first, no "Connection lost" banner. Meeting duration from the host's join to End: 3 min 32 s; three-party from admit to End: 3 min 07 s. `logs/hold-samples.json` has the numbers.

### Wrangler log

Only these non-info lines: `recording not implemented yet: StartRecording`, three `RecordTrack` (one per participant, `mic-1`), `StopRecording`, and the two `not_found_track_error` lines quoted under 2. No warnings, no panics.

Unexpected, and outside the run: **stale local DO records.** On the first start, right after `Ready on`, wrangler printed twelve ``Uncaught Error: JSON serialization error … unknown variant `status`, expected `open` or `ended` `` before any request: Room DOs left in `.wrangler/state/v3/do/` by an earlier record schema fired their alarms, and `RoomObject` cannot deserialize them. The same twelve lines are in the previous session's log. For this run `.wrangler/state/v3` was moved into the scratchpad (`run4/stale-wrangler-state-v3`) and wrangler restarted clean. Open issue: an undeserializable record should be deleted (with its alarm) instead of throwing on every alarm.

Also environmental: the first attempt of the script died on the home page with `net::ERR_NETWORK_CHANGED` in Chromium (a host interface changed at that moment); the rerun is the run reported here.

### Verdict on the phase-1 acceptance

The criterion "two browsers on different networks hold a 10-minute call" cannot be checked locally: all three browsers run on one machine behind one NAT, reach the SFU through the same srflx candidate, and `/api/ice` serves STUN only, so neither a second network nor TURN is exercised. As the local proxy, three headless browsers held a call through the real SFU for 3 minutes (3 min 32 s in all) with audio and video flowing in both directions for every pair, no reconnect, no stuck negotiation, and the SFU log clean apart from two transient `not_found` errors that the retry resolved. The run-3 failures (host receives nothing; 406/410 loop; frozen frame after camera off) are fixed. Phase 1 is accepted locally; the different-networks, 10-minute check remains for the deployed Worker.

### Open issues after run 4

1. Stale DO record: delete a record that fails to deserialize instead of throwing on every alarm (see "Wrangler log").
2. The guest's first frame of the host came 4.1 s after its subscribe answer, against 0.5 s for the host and the third participant. Unexplained; check with `getStats()` sampled every 200 ms after the answer (`framesReceived`, `pliCount`, `keyFramesDecoded`).
3. A `not_found_track_error` on a connected session costs 3 s (`SUBSCRIBE_RETRY_MS`) before the publisher's tracks arrive. A publisher-connected signal, or a shorter first retry, would bring "admit → seen by everyone" from ≈5 s to ≈1.5 s.
4. Items 2–7 of "Open issues" above still stand (camera-off signalling is now solved by the room view; `setLayer` retry exists but was not exercised live; script not in the repo; `web/build` prerequisite; TURN, screen share, resume, >3 participants, deployed Worker untested).
