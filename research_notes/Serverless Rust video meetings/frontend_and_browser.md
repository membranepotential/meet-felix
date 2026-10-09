# Frontend (Rust/WASM) and browser constraints for a WebRTC meeting app, as of 2026-10-09

Method note: crate versions/dates come from the crates.io API and GitHub API (queried 2026-10-09). Browser-support data comes from the MDN browser-compat-data npm package v8.1.5 (queried 2026-10-09, "BCD" below), Apple/WebKit release notes, and web-sys IDL sources. Items marked UNVERIFIED rest on secondary or old sources.

## 1. Rust web frameworks: versions, activity, fitness for a media-heavy client-rendered SPA

### Takeaway
Leptos (0.8.22, very active) is the pragmatic pick for a client-rendered SPA; Dioxus is active but its web target is one of several renderers and 0.8 is still alpha; Yew is maintained but slower-moving and VDOM-based; Sycamore is small. None has a maintained, mainstream video-call reference app; the WebRTC code is plain web-sys in every case, so the framework mainly affects UI reactivity ergonomics, not media handling.

### Cited Findings
- Leptos: latest stable 0.8.22, published 2026-10-05; 0.9.0-beta published 2026-07-18 (0.9.0-alpha 2026-05-19); repo ~21.4k stars, last push 2026-10-06; crates.io recent downloads ~1.4M. — [crates.io API](https://crates.io/api/v1/crates/leptos), [GitHub releases](https://github.com/leptos-rs/leptos/releases), [GitHub API](https://api.github.com/repos/leptos-rs/leptos)
- Leptos 0.8.x release cadence: 0.8.16 (Feb), 0.8.17 (Mar 1), 0.8.18 (Apr 16), 0.8.19 (Jun 25), 0.8.21 (Sep 26), 0.8.22 (Oct 5), mostly bug fixes (hydration, Suspense, stores). 0.9 beta notes: signals deref to `dyn Fn()` (call syntax on stable), `lazy` opt-in feature, "some semver-breaking changes". No firm 0.9 date. — [Leptos releases](https://github.com/leptos-rs/leptos/releases)
- Leptos is fine-grained reactive (signals, no VDOM) and supports CSR, SSR, hydration and islands (from release notes mentioning hydration/islands router fixes). CSR-only apps are built with Trunk or cargo-leptos. — [Leptos releases](https://github.com/leptos-rs/leptos/releases) (CSR build tooling detail from general knowledge, UNVERIFIED here)
- `leptos-use` (utility hooks over web-sys, e.g. media/wake-lock style hooks) is at 0.19.2, updated 2026-09-01. — [crates.io](https://crates.io/api/v1/crates/leptos-use). Which hooks it exposes was not checked (UNVERIFIED).
- Dioxus: stable 0.7.10 (2026-07-30); 0.8.0-alpha.0 (2026-05-19) and 0.8.0-alpha.1 (2026-07-31) are pre-releases; ~39.3k stars; 824 open issues. 0.7.0 released 2025-09-08. — [GitHub releases](https://github.com/DioxusLabs/dioxus/releases), [crates.io](https://crates.io/api/v1/crates/dioxus), [0.7 blog](https://dioxuslabs.com/blog/release-070)
- Dioxus 0.7: subsecond hot-patching incl. web, `#[wasm_split]` route-level code splitting (separate .wasm per route), Axum-based fullstack server functions, Stores, native WGPU renderer (Blitz, "work in progress"). The post gives no bundle-size numbers. — [Dioxus 0.7 blog](https://dioxuslabs.com/blog/release-070)
- Dioxus docs claim web apps "can weigh less than 50kb" (hello-world, project claim, not independently measured). — [botmonster summary of Dioxus docs](https://botmonster.com/coding/dioxus-rust-cross-platform-app-framework/) (secondary; UNVERIFIED)
- Yew: 0.23.0 published 2026-03-10 (with yew-router 0.20.0, yew-agent 0.5.0); 0.22.1 on 2026-02-28; 0.22.0 Nov/Dec 2025 (`#[component]` replaces `#[function_component]`, MSRV 1.84). Repo ~32.8k stars, last push 2026-10-09. Still pre-1.0, breaking changes expected. Recent downloads ~436k (about a third of Leptos). — [GitHub releases](https://github.com/yewstack/yew), [Yew 0.22 blog](https://yew.rs/blog/2025/11/29/release-0-22), [crates.io](https://crates.io/api/v1/crates/yew)
- Sycamore: 0.9.4 on 2026-10-06, 0.9.3 on 2026-08-31, previously 0.9.2 in 2025-09 and 0.9.1 in 2024-11; ~3.4k stars; ~20k recent downloads (about 1/70 of Leptos). Fine-grained reactive, like Leptos. — [crates.io](https://crates.io/api/v1/crates/sycamore), [GitHub](https://api.github.com/repos/sycamore-rs/sycamore)
- All four bind to browser APIs through `web-sys`/`js-sys`/`wasm-bindgen` (current: web-sys 0.3.106 and wasm-bindgen 0.2.129, both 2026-09-25; `gloo` 0.12.0, 2026-03-25). — [crates.io web-sys](https://crates.io/api/v1/crates/web-sys), [wasm-bindgen](https://crates.io/api/v1/crates/wasm-bindgen), [gloo](https://crates.io/api/v1/crates/gloo)
- WebRTC example search on GitHub: only tiny/new Leptos projects (`hiroshimorowaka/screen-share`, 2 stars, P2P screen share; `ypcipansor/juncto`, 1 star, "self-hosted WebRTC video conferencing, written entirely in Rust", pushed 2026-10-09); Yew has `codec-abc/Yew-WebRTC-Chat` (128 stars, last push 2023-08, data/chat); Dioxus: nothing relevant found. — [GitHub search API](https://api.github.com/search/repositories?q=leptos+webrtc), [codec-abc/Yew-WebRTC-Chat](https://github.com/codec-abc/Yew-WebRTC-Chat)
- A Rust+WASM WebRTC video chat tutorial exists (plain web-sys, no UI framework). — [Medium, C. Schleich](https://charles-schleich.medium.com/webrtc-video-chat-tutorial-using-rust-wasm-fa340f7aeef9)
- `libp2p-webrtc-websys` is a Rust browser WebRTC transport over web-sys (data channels only; useful as a reference for the web-sys RtcPeerConnection pattern, not media). — [docs.rs](https://docs.rs/libp2p-webrtc-websys/)
- Safari 26.0 added an in-place Wasm interpreter (faster launch, less memory for large modules) and `WebAssembly.JSTag`; Safari 27.0 adds JSPI (JavaScript Promise Integration). Relevant because Rust WASM bundles are large. — [Safari 26.0 release notes](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md), [WebKit Features for Safari 27.0](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)
- Bundle size: no reliable independent 2026 comparison of Leptos/Dioxus/Yew found. Only generic advice (build with LTO, `opt-level=z`, `wasm-opt -Oz`, measure gzip/brotli). — search results; treat all framework size numbers as UNVERIFIED.

### Inferences
- A meeting UI has many independently updating elements (per-participant tiles, audio levels, mute state, connection quality). Fine-grained signals (Leptos, Sycamore) match that better than Yew's VDOM re-render model. Leptos has by far the largest ecosystem/activity of the two.
- Dioxus' value (one codebase for web/desktop/mobile native) is not needed for a browser app; its web path is fine but it brings a larger surface and a pre-release 0.8 line. Choose it only if a native shell is planned.
- Since WebRTC/media code is web-sys in all frameworks, keep the media/session layer in a framework-independent Rust crate (state machine + thin web-sys wrappers) and expose it to the UI as signals; that keeps the framework choice reversible.
- Recommendation: Leptos 0.8.x in CSR mode (pin 0.8.22; evaluate 0.9 only once stable).

### Gaps
- No measured bundle sizes or mobile touch/performance reports specific to Leptos/Dioxus/Yew were found. Must be measured with a spike (build target phone-class devices: iPhone and mid-range Android).
- Could not verify the contents of leptos.dev/yew.rs/dioxuslabs.com guides beyond the release posts above.

## 2. web-sys coverage of WebRTC and media APIs; ergonomics

### Takeaway
Core peer-connection, getUserMedia, getDisplayMedia, transceiver and `sendEncodings` types are stable in web-sys. Things you need for a serious SFU client are behind `--cfg=web_sys_unstable_apis` or missing entirely: `scalabilityMode`, `RTCRtpSender.transform`/`RTCRtpScriptTransform` (encoded transforms), Wake Lock, `MediaStreamTrackProcessor/Generator`, WebCodecs. Expect to use `js_sys::Reflect`/`JsValue` for gaps.

### Cited Findings
- The unstable-API gate is `RUSTFLAGS="--cfg=web_sys_unstable_apis"`; docs.rs marks gated items "This API is unstable and requires `--cfg=web_sys_unstable_apis`". — [docs.rs RtcRtpScriptTransform](https://docs.rs/web-sys/latest/web_sys/struct.RtcRtpScriptTransform.html), [wasm-bindgen guide: unstable APIs](https://rustwasm.github.io/docs/wasm-bindgen/web-sys/unstable-apis.html)
- Stable (enabled IDL) in web-sys: `RTCPeerConnection` incl. `createOffer`, `addTransceiver` with `RTCRtpTransceiverInit.sendEncodings`; `RTCRtpTransceiver` (`mid`, `direction`, `sender`, `setCodecPreferences`); `RTCRtpSender` (`setParameters`, `getParameters`, `replaceTrack`, `getStats`, static `getCapabilities`); `RTCRtpEncodingParameters` with `active`, `priority`, `maxBitrate`, `rid`, `scaleResolutionDownBy`, `ssrc`, `degradationPreference` (and legacy `rtx`/`fec`); `MediaDevices.getUserMedia` and `getDisplayMedia` (stable, `DisplayMediaStreamConstraints`); `MediaStreamTrack.getSettings`. — [webidls/enabled/RTCRtpSender.webidl](https://github.com/wasm-bindgen/wasm-bindgen/blob/main/crates/web-sys/webidls/enabled/RTCRtpSender.webidl), [RTCPeerConnection.webidl](https://raw.githubusercontent.com/wasm-bindgen/wasm-bindgen/main/crates/web-sys/webidls/enabled/RTCPeerConnection.webidl), [MediaDevices.webidl](https://raw.githubusercontent.com/wasm-bindgen/wasm-bindgen/main/crates/web-sys/webidls/enabled/MediaDevices.webidl)
- Unstable in web-sys (`webidls/unstable/`): `RTCRtpEncodingParameters.scalabilityMode` (the file `unstable/RTCRtpSender.webidl` contains only that one member), `RTCRtpReceiver.jitterBufferTarget`, `RTCRtpSender/Receiver.transform` + `SFrameTransform` + `RTCEncodedVideoFrame` metadata (`WebRTCEncodeTransform.webidl`), `Navigator.wakeLock`/`WakeLock`/`WakeLockSentinel` (`ScreenWakeLock.webidl`), `MediaStreamTrackProcessor/Generator` (`MediaCaptureTransform.webidl`), WebCodecs, `ImageCapture`, `MediaSession`, `PictureInPicture`, `AudioContext` (sink-related extension). — [webidls/unstable directory](https://github.com/wasm-bindgen/wasm-bindgen/tree/main/crates/web-sys/webidls/unstable); docs.rs confirms `RtcRtpScriptTransform`, `RtcEncodedVideoFrame`, `WakeLock` are unstable.
- `MediaStreamTrack.contentHint` was not found in the enabled `MediaStreamTrack.webidl` (grep returned nothing); `RTCRtpEncodingParameters.codec` and `getCapabilities` extras are also absent. Use `js_sys::Reflect::set` for `contentHint = "detail"`/`"text"`. — [MediaStreamTrack.webidl](https://raw.githubusercontent.com/wasm-bindgen/wasm-bindgen/main/crates/web-sys/webidls/enabled/MediaStreamTrack.webidl) (absence from a grep; confirm in a spike)
- `RTCRtpScriptTransform` needs a Worker running the transform (`TransformStream` over `readable`/`writable`), assigned to `sender.transform`/`receiver.transform`. Baseline 2025. BCD: Chrome/Chrome Android 141, Firefox 117, Safari/iOS 15.4. Chrome's legacy `createEncodedStreams` exists in Chrome 86+ only. — [MDN](https://developer.mozilla.org/en-US/docs/Web/API/RTCRtpScriptTransform), BCD 8.1.5
- Safari 26.0 WebRTC additions relevant to encoded transforms: `RTCEncodedAudio/VideoFrame` constructors and serialization, CSRC info, `RTCRtpScriptTransformer.generateKeyFrame(rid)`; removed `fec` and `rtx` from encoding parameters. Safari 26.0 also added the Speaker Selection API on iOS/iPadOS and a default system speaker device. — [Safari 26.0 release notes](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md)
- `AudioWorklet` is stable in web-sys (not gated per docs.rs check). BCD: Chrome 66, Firefox 76, Safari 14.1, iOS Safari 14.5. A worklet processor must be a separate JS module/WASM instance loaded with `audioWorklet.addModule`; it cannot be written as a web-sys closure. — docs.rs check; BCD 8.1.5
- Known ergonomic problems (general web-sys practice, UNVERIFIED by a single source here): every API returns `Result<_, JsValue>`; dictionaries are builders with setters that return `&mut` (older) or are constructed via `Object::new` + `Reflect`; event callbacks need `Closure<dyn FnMut(Event)>` kept alive or `.forget()` (leak); Promises are consumed via `wasm_bindgen_futures::JsFuture` and `spawn_local`; each used web-sys type needs a Cargo feature flag (e.g. `RtcPeerConnection`, `RtcRtpTransceiverInit`), which bloats feature lists. `gloo` provides event listeners/timers wrappers with RAII cleanup. — [wasm-bindgen guide](https://rustwasm.github.io/docs/wasm-bindgen/), [gloo crate](https://crates.io/crates/gloo)

### Inferences
- The only gate-dependent pieces for a first version: Wake Lock (alternatively a tiny JS shim), `scalabilityMode` (needed only for SVC; plain simulcast with `rid` + `scaleResolutionDownBy` + `maxBitrate` works on stable), and encoded transforms (only if E2EE/SFrame is planned). Setting the cfg flag globally in `.cargo/config.toml` is the simple fix.
- Reasonable design: a ~200 line JS shim module (via `#[wasm_bindgen(module = ...)]`) for the awkward bits (contentHint, wake lock, worker transform) is cheaper than fighting generated bindings.

### Gaps
- Did not check which methods inside stable `RtcRtpSender`/`RtcRtpTransceiver` pages carry per-member unstable markers; the struct-level docs.rs banner showed gating only for ScriptTransform, EncodedVideoFrame, WakeLock (and the transform members).
- No sample code compiled; claims about Closure ergonomics are general knowledge.

## 3. Thin TypeScript/Svelte UI plus Rust WASM core

### Takeaway
Viable and common: write the protocol codec/state machine in Rust, build with wasm-bindgen, and let TypeScript own DOM, WebRTC and media. Share types with `tsify` (at wasm boundary) or `ts-rs`/`specta` (type generation). The browser APIs gain first-class TypeScript types and no unstable-cfg problems, at the cost of two languages.

### Cited Findings
- Crate status: `ts-rs` 12.0.1 (2026-01-31, ~7.4M recent downloads); `specta` stable 1.0.5 with 2.0.0-rc.25 (2026-05-07) still a release candidate; `typeshare` 1.0.5 (2026-01-02); `tsify` 0.5.8 (2026-08-23; `tsify-next` fork 0.5.6 last updated 2025-04-20, effectively superseded); `serde-wasm-bindgen` 0.6.5 (last release 2024-02-27, still ~15.9M recent downloads). — [crates.io API](https://crates.io/api/v1/crates/ts-rs) (and `/specta`, `/typeshare`, `/tsify`, `/tsify-next`, `/serde-wasm-bindgen`)
- `tsify` generates `.d.ts` and the wasm-bindgen ABI glue for types passed through `#[wasm_bindgen]` functions; defaults to serde_json, optional serde-wasm-bindgen for serialization. — [tsify docs via search](https://crates.io/crates/tsify) (secondary summary; UNVERIFIED in detail)
- `ts-rs` is per-type derive that respects serde attributes (feature `serde-compat`); specta handles dependent types as a graph and underpins rspc/tauri-specta. — [docs.rs/specta](https://docs.rs/specta), [ts-rs](https://github.com/Aleph-Alpha/ts-rs)
- `serde-wasm-bindgen` converts via native JS values (not JSON strings); the usual guidance is to prefer it over `JsValue::from_serde` (deprecated). — [wasm-bindgen guide](https://rustwasm.github.io/docs/wasm-bindgen/reference/arbitrary-data-with-serde.html)
- Safari 26 (WebKit) confirms Wasm is optimized for large modules (in-place interpreter), and Safari 27 adds JSPI; the thin-core approach with a small module has lower startup risk. — see section 1 sources.

### Inferences
- Recommendation for a Rust server + TS client: derive types once on the server crate with `ts-rs` (stable, simple, fixed-version) or `tsify` if the same types also cross the wasm boundary; keep wire format one canonical encoding (JSON or CBOR via serde), so Rust server, Rust WASM core and TS agree. Avoid specta 2.0 rc for a new long-lived project until it is stable.
- No source in this search offered a "usual recommendation" ranking; the above is a judgment, not a cited consensus.

### Gaps
- Svelte/SvelteKit-specific integration pitfalls (WASM loading in Vite, SSR disabled for WASM) not researched.
- No cited benchmark comparing Svelte vs Leptos bundle size / runtime.

## 4. Browser constraints in 2026

### Takeaway
Phones cannot share their screen from a browser: BCD marks `getDisplayMedia` unsupported on Chrome Android, Firefox Android and iOS Safari. Camera/mic work everywhere but iOS suspends capture in the background, and home-screen PWAs on iOS have a history of camera/mic bugs. Plan UI for "viewer-only on phones" for screen share.

### Cited Findings
- `getDisplayMedia`: Chrome 72, Edge 79, Firefox 66, Safari (macOS) 13; Chrome Android, Firefox Android, Samsung Internet and iOS Safari: not supported. Chrome Android 72-88 and Firefox Android 66-79 exposed the method but it always rejected with `NotAllowedError`. Needs a user gesture and a secure context; permission is not persisted. — BCD 8.1.5 ([MDN getDisplayMedia](https://developer.mozilla.org/en-US/docs/Web/API/MediaDevices/getDisplayMedia))
- Vendors confirm: Twilio says mobile browsers do not support screen sharing (use native SDKs); SignalWire says unavailable on iOS Safari. iOS screen broadcast needs a native ReplayKit extension. — [Twilio docs](https://www.twilio.com/docs/video/screen-share-chrome), [SignalWire](https://signalwire.com/docs/browser-sdk/guides/screen-sharing), [Ant Media](https://antmedia.io/webrtc-browser-support/)
- Safari 26.x and 27.0 release notes list no `getDisplayMedia` change for iOS. — [Safari 26.0](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md), [26.4](https://developer.apple.com/documentation/safari-release-notes/safari-26_4-release-notes.md), [WebKit Safari 27.0 features](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)
- Latest Safari: 26.0 (2025-09-15), 26.4 (2026-03-24, added WebTransport, `MediaStreamTrackProcessor` respects `track.enabled=false`, iOS WebRTC network slicing, `MediaDeviceInfo` secure-context only), 26.6 exists; Safari 27.0 features post dated 2026-09-17 (adds `targetLatency`, `RTCRtpCodec`, `jitterBufferTarget`, JSPI, PiP fixes, fix for wrong frame orientation after transient device rotation). Exact 27.0 ship date not stated on the page. — [26.4 notes](https://developer.apple.com/documentation/safari-release-notes/safari-26_4-release-notes.md), [WebKit 26.6](https://webkit.org/blog/18178/webkit-features-for-safari-26-6/), [WebKit 27.0](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)
- All iOS browsers use WebKit, so Chrome/Firefox on iOS share Safari's WebRTC set (EU-only alternative-engine path exists). — [Ant Media](https://antmedia.io/webrtc-browser-support/)
- Wake Lock: Chrome/Chrome Android 84, Firefox 126 (desktop and Android), Safari 16.4; iOS Safari 18.4 for full (in 16.4-18.3 flagged partial: "does not work in standalone Home Screen web apps", WebKit bug 254545). — BCD 8.1.5, [WebKit bug 254545](https://webkit.org/b/254545)
- Picture-in-Picture (`video.requestPictureInPicture`): Chrome 69, Chrome Android 105, Firefox 153 desktop only, Safari 13.1, iOS 13.4. Document PiP (arbitrary HTML window, useful for a floating meeting window): Chrome 116 and Firefox 151 desktop only; no Safari, no mobile. Safari 26.0/27.0 only fix PiP edge cases. — BCD 8.1.5, release notes above
- Screen Orientation `lock()`: Chrome (partial), Chrome Android 38, Firefox 144 (desktop and Android); no Safari/iOS. — BCD 8.1.5
- Speaker selection: `HTMLMediaElement.setSinkId` Chrome desktop 49, Firefox 116, Safari 18.4 (incl. iOS 18.4); not on Chrome Android or Firefox Android; `selectAudioOutput` Firefox 116 only. Safari 26.0 adds Speaker Selection API on iOS/iPadOS. — BCD 8.1.5, Safari 26.0 notes
- `MediaStreamTrack.contentHint`: Chrome 70, Safari 12.1, iOS 12.2; Firefox not supported. — BCD 8.1.5
- iOS background: capture is stopped/muted when the page is backgrounded; tracks may stay muted/ended and need `getUserMedia` again; signalling sockets and peers reported dead after ~30 s in background; phone-call interruptions can leave mic muted; installed home-screen apps reported to lose mic when backgrounded or locked. Sources are WebKit bugs from 2019-2023, so current behavior UNVERIFIED. — [WebKit 232599](https://bugs.webkit.org/show_bug.cgi?id=232599), [226163](https://bugs.webkit.org/show_bug.cgi?id=226163), [247994](https://bugs.webkit.org/show_bug.cgi?id=247994), [239602](https://bugs.webkit.org/show_bug.cgi?id=239602), [173268](https://bugs.webkit.org/show_bug.cgi?id=173268)
- Safari 26.4 fixed `HTMLMediaElement` not detecting new tracks, which "caused Safari to pause video when leaving a tab". Safari 26.0 fixed enumerateDevices permission semantics and the `configurationchange` event for echo-cancellation changes. — [26.4 notes](https://developer.apple.com/documentation/safari-release-notes/safari-26_4-release-notes.md), [26.0 notes](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md)
- iOS autoplay: `playsinline` required for inline video; camera permission prompts reappear after the tab/session ends. — [Ant Media](https://antmedia.io/webrtc-browser-support/). The "autoplay needs a user gesture" rule for unmuted playback on iOS is general knowledge, not re-verified here.
- Safari 26 user agent reports a frozen OS version on iOS 26 (do not UA-sniff the iOS version). — [Safari 26.0 notes](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md)

### Inferences
- Phones: join, camera, mic, view others' shares, but hide the screen-share button (feature-detect `navigator.mediaDevices?.getDisplayMedia`). Native apps would be needed for phone screen sharing (ReplayKit/MediaProjection), which is out of scope for a PWA.
- Must handle: `visibilitychange`, track `mute`/`unmute`/`ended`, re-acquire with `getUserMedia` + `replaceTrack`, reconnect signalling. Use Wake Lock on mobile, re-request on `visibilitychange` (wake locks are released when the page is hidden; general knowledge).
- Layout should survive orientation changes without orientation lock on iOS.

### Gaps
- No 2026-specific source for iOS 26/27 background-capture behavior; test on device.
- Firefox Android WebRTC quirks not researched.
- Android Chrome background behavior (camera stops when tab hidden) is not covered by a source here.

## 5. PWA installability, push, camera/mic from home screen

### Takeaway
iOS 26 turns every home-screen site into a web app by default, and Web Push works only for home-screen web apps (iOS 16.4+). Camera/mic from a home-screen app works in principle but has recurring bug reports; a plain Safari tab is the safer default for calls.

### Cited Findings
- iOS 26/iPadOS 26: every site added to Home Screen opens as a web app by default; user can disable "Open as Web App"; a manifest is no longer required (if present, it is honored). — [WebKit Features in Safari 26.0](https://webkit.org/blog/17333/webkit-features-in-safari-26-0/), [Safari 26.0 notes](https://developer.apple.com/documentation/safari-release-notes/safari-26-release-notes.md)
- Web Push on iOS/iPadOS 16.4+: only for Home Screen web apps with manifest `display: standalone|fullscreen`; permission request must come from a direct user gesture; allow `*.push.apple.com`; no Apple Developer membership needed; Badging API (`setAppBadge`) available. — [WebKit blog: Web Push for Web Apps on iOS and iPadOS](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)
- BCD: `Notification` on iOS Safari is undefined unless the page is a Home Screen web app; notifications on Chrome Android and iOS must go through a service worker (`showNotification`). `PushManager`: Chrome 42, Firefox 44 / Android 48, Safari 16, iOS 16.4. — BCD 8.1.5
- Camera/mic from home-screen apps: historical WebKit bugs (navigator.mediaDevices undefined in standalone, bug 185448 later resolved); 2024 report of camera failing in standalone mode on some devices (WebKit bug 273938); iOS 26 developer-forum report of rotated camera output and `portrait-primary` orientation always in home-screen apps (user report, not confirmed by Apple). — [WebKit 185448](https://bugs.webkit.org/show_bug.cgi?id=185448), [273938](https://bugs.webkit.org/show_bug.cgi?id=273938), [Apple forum 801146](https://developer.apple.com/forums/thread/801146)
- Safari 27.0 fixes wrong orientation of captured frames after transient device rotation. — [WebKit Safari 27.0](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)
- Chrome Android supports installable PWAs and push (BCD shows `PushManager` Chrome Android 42); install-prompt specifics (`beforeinstallprompt`, WebAPK) not verified here.

### Inferences
- Treat the PWA as an optional wrapper for push/invites; run calls so they also work in a normal browser tab. Meeting links should open fine in Safari without installation.

### Gaps
- No source confirming current iOS 26/27 state of camera/mic in standalone mode beyond the forum report.
- Android PWA install criteria and Firefox Android PWA support not researched.
- Whether iOS web push delivers reliably as call-style invites (no CallKit-style ringing) is not sourced.

## 6. Codec support matrix and recommendations

### Takeaway
H.264 is the only codec that is reliably hardware-accelerated and available on every browser including all iOS; VP8 is the universal software fallback; VP9 and AV1 are partial on Apple. For screen share use a high-resolution, low-framerate track with `contentHint = "detail"`/`"text"`; H.264 or VP9 (Chrome) are preferred over VP8 for text clarity (preference is general knowledge, UNVERIFIED).

### Cited Findings
- Ant Media 2026 table (updated 2026-09-07; low-authority vendor page): Safari macOS/iOS: VP8 yes (software), VP9 no, H.264 yes (HW), AV1 no; Chrome/Edge: VP8, VP9, H.264 yes, AV1 limited; Firefox: VP9 partial, no AV1; the page contradicts itself (calls Safari VP9 "limited and inconsistent" elsewhere). — [Ant Media](https://antmedia.io/webrtc-browser-support/)
- WebKit STP notes: a "functional WebRTC VP9 codec" (STP 110), VP9 profile 0 enabled by default (STP 120), VP9 Profile 2 (STP 117), software-decoder fallback for VP9-SVC (STP 135); Safari 27 fixes colorspace propagation of WebRTC VP9 encoders. So VP9 exists in Safari WebRTC (contradicts Ant Media). Which stable iOS version first shipped it and on which devices it encodes were not confirmed. — [STP 110](https://webkit.org/blog/10929/release-notes-for-safari-technology-preview-110/), [STP 120](https://webkit.org/blog/11548/release-notes-for-safari-technology-preview-120/), [STP 135](https://webkit.org/blog/12040/release-notes-for-safari-technology-preview-135/), [Safari 27.0](https://webkit.org/blog/18325/webkit-features-for-safari-27-0/)
- Safari 17.0 added AV1 for WebRTC on devices with hardware decode (iPhone 15 Pro/Pro Max, M3); a 2026 WebKit bug (tested iPhone 15 Pro + iOS 26.4) says AV1 is reported "supported" but not "smooth"/"powerEfficient". Encode in Safari: not found. — [ConnectyCube on Safari 17](https://connectycube.com/2023/09/20/safari-17-0-whats-new-for-webrtc/), [WebKit bug 311593](https://bugs.webkit.org/show_bug.cgi?id=311593)
- Simulcast: "strong in Chrome and Firefox, limited in Safari" (no version detail). Safari supports `addTransceiver` with `sendEncodings` since 11/iOS 11 (BCD), but H.264 simulcast on Safari is reported working in older sources (UNVERIFIED). — [Ant Media](https://antmedia.io/webrtc-browser-support/), BCD 8.1.5
- Safari 26.0 removed `fec` and `rtx` from encoding parameters; Safari 27 adds `RTCRtpCodec` dictionaries (codec inspection/configuration). — release notes above
- `RTCRtpTransceiver.setCodecPreferences`: Chrome 76, Firefox 128, Safari 13.1, iOS 13.4. `RTCRtpEncodingParameters.scalabilityMode` and per-encoding `codec` were not in BCD lookup (MISSING in the paths queried, so unknown). — BCD 8.1.5
- Content hints: Chrome 70, Safari 12.1/iOS 12.2, Firefox none (BCD).

### Inferences
- Recommended: negotiate H.264 (Constrained Baseline/High, packetization-mode 1) + VP8 as the baseline for all, offer VP9 only for desktop Chrome/Edge/Firefox senders if the SFU supports it. Skip AV1 for mobile.
- For camera simulcast use three layers via `rid` (e.g. high/mid/low with `scaleResolutionDownBy` 1/2/4); for screen share one or two layers, `maxFramerate` low, `contentHint "detail"` or `"text"`, `degradationPreference "maintain-resolution"` (these WebRTC-spec behaviors are general knowledge).

### Gaps
- No authoritative per-version codec table (caniuse/webrtcHacks 2025-26 post) was retrieved; the VP9/H.264 simulcast status on iOS 26 needs an actual device test.
- Chrome AV1 SVC / Firefox VP9 state in 2026 not verified.

## 7. Layout/UX references for responsive meeting grids

### Takeaway
Only generic findings; no authoritative 2026 source was retrieved.

### Cited Findings
- None retrieved with citations in this session (no Google Meet/Jitsi layout documentation fetched).

### Inferences
- Patterns to implement, from general knowledge (UNVERIFIED): CSS-grid/flex tiles with a computed best-fit rows x columns by tile count and aspect ratio (16:9 on landscape, 3:4-ish on portrait phones), a pinned/speaker layout with a filmstrip, active-speaker highlighting from audio-level stats, pagination with only visible tiles subscribed (SFU simulcast layer selection based on tile size and visibility), `IntersectionObserver`/`ResizeObserver` for layer choice, `object-fit: cover` for cameras and `contain` for shared screens, `dvh` units and safe-area insets on iOS, touch targets >= 44 px, controls auto-hide on tap. Safari 26/27 notes do not change any of these.
- Because `ResizeObserver`-driven layer selection ties UI to media negotiation, keep it in the shared Rust core.

### Gaps
- Jitsi/Meet layout algorithms not researched; no library survey done.
