# Live transcription (STT) options for a self-hosted serverless Rust WebRTC meeting app (as of 2026-10-09)

Method note: page content was retrieved via WebFetch (small-model summaries of the pages) and WebSearch. Official pricing pages were read for Deepgram, AssemblyAI, Cloudflare, OpenAI. Google, Azure, Speechmatics, Gladia, Modal, RunPod figures come from third-party aggregators and are labeled UNVERIFIED. Items with no URL are in Gaps.

## 1. Cloud / serverless streaming STT: price, latency, languages, EU residency

### Takeaway
All streaming APIs bill per audio duration, so idle cost is zero. List prices span about $0.15/h (AssemblyAI Universal-Streaming) to ~$1/h (Google, Azure). Cheapest credible options for German+English streaming: AssemblyAI Universal-Streaming (multilingual), Deepgram Nova-3 multilingual, Speechmatics (unverified). Per-speaker tracks mean diarization (extra $0.12-0.30/h) is not needed. Cost is per transcribed track, so an N-person meeting costs N x hourly rate unless you gate on voice activity (VAD) and only stream speech.

### Cited Findings
- Deepgram PAYG streaming: Nova-3 monolingual $0.0048/min ($0.29/h); Nova-3 multilingual $0.0058/min ($0.35/h); Flux English $0.0065/min; Flux multilingual $0.0078/min ($0.47/h). Pre-recorded Nova-3 $0.0043/min mono, $0.0052/min multi. Streaming rates are labeled limited-time promotional (struck-through higher prices shown), so they may rise. Streaming diarization add-on $0.0020/min ($0.12/h); diarization on pre-recorded is free. New accounts get $200 credit, no expiry, no card. (fetched 2026-10-09) — [Deepgram pricing](https://deepgram.com/pricing)
- AssemblyAI streaming, billed per hour of WebSocket session time (including idle time, so close the socket when silent/ended): Universal-3.6 Pro Realtime ($0.45/h base), Universal-Streaming English $0.15/h, Universal-Streaming Multilingual $0.15/h. Diarization add-on $0.12/h (`speaker_labels: true`). German: page lists German only in a Medical Mode table for Universal-3.6 Pro Realtime and Universal-Streaming Multilingual; English model is English-only. EU endpoint `streaming.eu.assemblyai.com`, same price, audio stays in EU. PAYG accounts: 100 new streams/min limit. — [AssemblyAI pricing](https://www.assemblyai.com/pricing)
- OpenAI: `gpt-transcribe` listed at $0.0045/min; supported on Realtime transcription endpoint (`v1/realtime/transcription_sessions`, WebSocket, transcribes committed turns, i.e. not word-by-word partials) and streamed file transcripts; page gives no language list and no EU residency statement. — [OpenAI gpt-transcribe](https://developers.openai.com/api/docs/models/gpt-transcribe)
- OpenAI older/other figures, UNVERIFIED (third-party aggregators): gpt-4o-transcribe ~$0.006/min; gpt-realtime-whisper ~$0.017/min for streaming. — [CostGoat](https://costgoat.com/pricing/openai-transcription), [search result: perkstack](https://perkstack.co/blog/cheapest-gpt-4o-transcribe-api)
- Cloudflare Workers AI (official): `@cf/openai/whisper` $0.0005/audio-min; `@cf/openai/whisper-large-v3-turbo` $0.0005/audio-min (46.63 neurons/min); `@cf/deepgram/nova-3` $0.0052/min (HTTP) and $0.0092/min over WebSocket; `@cf/deepgram/flux` $0.0077/min over WebSocket. Neurons $0.011 per 1,000, 10,000 free neurons/day. Whisper models are request/response (chunked batch), Nova-3/Flux have WebSocket streaming. — [Cloudflare Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/)
- Speechmatics, UNVERIFIED (trackers; official page showed no per-hour rates in search): real-time standard $0.24/h, real-time enhanced $0.43/h (cut from $0.56/h around 2026-07-06), 20% discount above 500 h/month, free tier 40-50 h/month (sources disagree). — [Speechmatics pricing](https://www.speechmatics.com/our-technology/pricing), [usagepricing price-cut note](https://usagepricing.com/blueprint/activity/speechmatics-2026-07-06-realtime-stt-price-cut), [costbench](https://costbench.com/software/ai-transcription-apis/speechmatics/)
- Gladia (official pages via search): real-time $0.75/h Self-Serve, from $0.55/h Scaling plan. Free hours apply to async only. — [Gladia pricing](https://www.gladia.io/pricing), [Gladia support](https://support.gladia.io/article/understanding-our-transcription-pricing-pv1atikh8y9c8sw7sudm3rcy)
- Google Cloud Speech-to-Text V2 / Chirp 3, UNVERIFIED: ~$0.016/min ($0.96/h) first 500k min/month, tiers down to $0.004/min at volume; billed per channel. Streaming-specific SKU not confirmed. — [Google STT pricing](https://cloud.google.com/speech-to-text/pricing), [Dasha comparison](https://dasha.ai/blog/speech-to-text-pricing)
- Azure Speech standard real-time ~$1.00/h billed per second; diarization, continuous language ID are +$0.30/h each. UNVERIFIED (official page rendered no numbers). — [Dasha comparison](https://dasha.ai/blog/speech-to-text-pricing), [Azure Speech pricing](https://azure.microsoft.com/en-us/pricing/details/cognitive-services/speech-services/)
- LiveKit Inference provides Deepgram, AssemblyAI, Cartesia, Gemini, Speechmatics via LiveKit Cloud; plugins for 30+ others (Amazon, Azure, Google, Groq, OpenAI, Soniox). — [LiveKit STT docs](https://docs.livekit.io/agents/models/stt/)

Cost per meeting-hour (arithmetic from the figures above, per participant-hour of streamed audio; 4 participants, all streamed continuously): AssemblyAI $0.60; Deepgram Nova-3 multi $1.40; Cloudflare Nova-3 WS $2.20; Gladia $3.00; Azure/Google ~$4 (unverified). With VAD gating (people speak ~30-40% of the time, my assumption) divide by ~2.5-3, except AssemblyAI which bills session time unless you close the socket.

### Inferences
- Cheapest German-capable streaming: AssemblyAI Universal-Streaming multilingual ($0.15/h) if its German quality is acceptable (language list not confirmed from page). Deepgram Nova-3 multilingual is the safer-known option with documented German.
- Cloudflare Whisper at $0.03/audio-hour is ~5-10x cheaper than any streaming API, but is chunked (no partials), so latency = chunk length + inference. Fits "final captions per utterance after VAD endpointing", not word-by-word.
- A Rust Worker or Lambda-like backend can proxy the provider WebSocket; AssemblyAI and Cloudflare bill session time/audio, so no idle cost.

### Gaps
- EU residency not confirmed for: Deepgram (EU endpoint existence not checked), OpenAI (page silent), Cloudflare (no regional pinning checked), Speechmatics (EU endpoint existence not checked), Gladia, Google (regional endpoints exist but not checked), Azure (EU regions exist but not checked). Only AssemblyAI EU confirmed.
- Streaming latency numbers (ms) from vendors were not collected.
- German WER per vendor not found.
- Official Google/Azure/Speechmatics pricing pages not read directly.

## 2. Client-side (in-browser) transcription

### Takeaway
Web Speech API is the cheapest (free) but in Chrome/Edge it sends audio to Google's cloud, Firefox is flagged/unshipped, and on-device mode (`processLocally`) is experimental. In-browser Whisper/Moonshine works on desktop with WebGPU; I found no mobile benchmarks.

### Cited Findings
- Safari supports `webkitSpeechRecognition` (guide cites macOS 14.1 / iOS 14.5; Apple's cloud/on-device behavior not verified). Firefox: behind a flag, a Nov 2025 Google/Mozilla slide deck says "shipping soon"; no confirmation it shipped by Oct 2026. Chrome/Edge stream audio to Google servers. — [TestMu browser-support guide](https://www.testmuai.com/learning-hub/speech-recognition-api-browser-support/), [Google/Mozilla slides](https://docs.google.com/presentation/d/1zRSgSCJ32FtvwtxbVaStyxt6ZERlKL8obApqwBHRWGU/htmlpresent), [AssemblyAI blog](https://www.assemblyai.com/blog/speech-recognition-javascript-web-speech-api)
- `SpeechRecognition.processLocally` (boolean): `true` forces local recognition, `false` (default) lets the user agent choose. MDN marks it experimental; compat table not retrievable. — [MDN processLocally](https://developer.mozilla.org/en-US/docs/Web/API/SpeechRecognition/processLocally)
- Whisper in browser: Transformers.js + ONNX Runtime Web; demo default whisper-base (~73M params, ~200 MB cached download); WebGPU with `q4` dtype, falls back to WASM (slower). Safari WebGPU support described as incomplete in a tutorial (dated, unverified for 2026). — [Whisper WebGPU write-up (Habr)](https://habr.com/en/articles/850284/), [HackerNoon guide](https://hackernoon.com/turn-audio-to-text-on-your-device-free-whisper-webgpu-guide)
- Moonshine: tiny models down to ~1 MB class, streaming-oriented, MIT license for English; non-English models are legacy, non-streaming, under a non-commercial community license. JS/WASM SDK (MoonshineJS, beta). A community German ONNX checkpoint exists (`moonshine-base-de-onnx`) but licensing/quality unchecked. — [Moonshine repo](https://github.com/usefulsensors/moonshine), [MoonshineJS](https://dev.moonshine.ai/moonshine-js), [HF de checkpoint](https://huggingface.co/Phreak87/moonshine-base-de-onnx)

### Inferences
- Moonshine's commercial streaming models are English-only, so it does not meet the German requirement under a permissive license.
- Web Speech API quality/privacy is outside your control (vendor cloud, varies per browser), so it is a fallback, not the primary path.

### Gaps
- Web Speech continuous-mode limits (auto-stop timeouts, Safari quirks), iOS on-device vs Apple server behavior, and caniuse numbers not retrieved.
- No phone benchmarks (latency, battery, thermal) for Whisper-in-browser. Whisper-base German WER not found.

## 3. Self-hosted Whisper-class models in Rust; scale-to-zero GPU

### Takeaway
No self-hosted option gives German streaming plus easy Rust integration without caveats: Kyutai STT is English/French only; sherpa-onnx's listed streaming models do not include German; whisper.cpp/Whisper is multilingual but chunked. Scale-to-zero GPU (Modal/RunPod) costs ~$0.6-0.8/GPU-h plus 30-120 s cold starts, which is poor for live captions. Cloudflare Whisper at $0.03/audio-h is effectively "serverless self-hosted".

### Cited Findings
- Kyutai delayed-streams-modeling: `stt-1b-en_fr` (~1B, 0.5 s delay, semantic VAD, English+French) and `stt-2.6b-en` (English, 2.5 s delay). German not offered. Rust server serves multiple streams over WebSocket; claims 400 real-time streams on one H100, 64 connections at 3x real time on an L40S. Rust backend Apache, weights CC-BY 4.0. — [Kyutai DSM repo](https://github.com/kyutai-labs/delayed-streams-modeling)
- sherpa-onnx: streaming and offline ASR; streaming models listed for Chinese, English, Korean, French etc.; offline Zipformer, Parakeet TDT 0.6B v2 (English), multilingual Whisper, Moonshine tiny (English); WASM builds; Rust API with `rust-api-examples` folder. No German ASR model listed on the README. sherpa-rs community crate not confirmed by the page. — [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
- Open ASR leaderboard English averages (vendor blog, UNVERIFIED): Parakeet TDT 0.6B v3 6.32%, Canary 1B v2 7.15%, Whisper large-v3 7.44%, large-v3-turbo 7.83%. 25-language FLEURS average: Parakeet v3 12.0%, Whisper large-v3 12.6%. Parakeet v3 and Canary-1B-v2 cover 25 European languages including German. No German-only WER found. — [whispernotes benchmark](https://whispernotes.app/de/blog/parakeet-v3-default-mac-model), [wz-it comparison](https://wz-it.com/en/blog/local-speech-to-text-gdpr/), [Open ASR leaderboard summary](https://benchmarklist.com/benchmarks/open_asr_leaderboard/)
- Modal: T4 ~$0.59/GPU-h, L4 ~$0.80/GPU-h, per-second billing with scale to zero (aggregator figures, Aug 2026; UNVERIFIED). Cold start for GPU+model load estimated 30-120 s (third-party estimate). — [Modal T4 blog](https://modal.com/blog/nvidia-t4-price-article), [computeprices L4](https://computeprices.com/providers/modal/gpus/l4)
- RunPod serverless: per-second, Flex workers scale to zero; 16 GB GPUs $0.00016/s Flex ($0.58/h), startup time billed. — [RunPod serverless pricing](https://docs.runpod.io/serverless/pricing)

### Inferences
- Break-even: one GPU-hour at ~$0.6-0.8 is 4-5x the Deepgram multilingual hourly rate and ~20x Cloudflare Whisper, so GPU self-hosting only pays when a GPU serves many concurrent streams (Kyutai-style batching) or you already run one. For one small meeting at a time, cloud APIs win.
- Cold starts of 30-120 s mean a "start captions" button would lag; only viable if the GPU is warmed when the meeting room is created.
- CPU option (whisper.cpp small/base via whisper-rs, candle-whisper): I did not retrieve RTF numbers; not evidenced here.

### Gaps
- whisper-rs, candle whisper, sherpa-rs repos not fetched: CPU/GPU RTF, German support, maintenance status unknown from my research.
- Parakeet/Canary Rust inference path (ONNX via sherpa-onnx or `ort`) not verified; Fly GPU not researched; Modal/RunPod official pages not read.

## 4. Per-speaker transcription in an SFU; audio pipeline and Rust crates

### Takeaway
Per-track transcription is the standard pattern and gives exact speaker attribution; LiveKit Agents does exactly this. Pipeline is Opus RTP -> libopus decode (48 kHz) -> resample to 16 kHz -> VAD -> STT. Crate-level details are only partly verified.

### Cited Findings
- LiveKit Agents standalone example: on `track_subscribed`, spawn a `process_track` task per participant track, each with its own STT stream fed that track's frames. Diarization recommended only when one mic carries multiple speakers (`MultiSpeakerAdapter` in Python). — [LiveKit STT docs](https://docs.livekit.io/agents/models/stt/)
- rubato: Rust sample-rate converter; fixed-ratio FFT resamplers are faster, async sinc resamplers handle drifting ratios; v5.0.0 changed API to use `audioadapter` adapters (pin the version, examples mix APIs). — [rubato on docs.rs](https://docs.rs/crate/rubato/latest)
- str0m: sans-I/O Rust WebRTC library, you own sockets and event loop, not a JS-style RTCPeerConnection API; receive-media example is `http-post`; repo active in 2026 (last push reported 2026-04-21). — [str0m docs](https://docs.rs/str0m), [str0m on docs.rs crate page](https://docs.rs/crate/str0m/latest)
- Existing crates combine `opus` (libopus binding, ^0.3), `rubato`, `symphonia` (e.g. `kaudio`). — [kaudio on docs.rs](https://docs.rs/kaudio)

### Inferences
- Use the `opus` crate (libopus binding) for decoding (needs PLC for lost packets: call decode with empty input), rubato for 48 -> 16 kHz (mono; WebRTC Opus is usually mono-capable at 48 kHz). The sans-I/O str0m exposes already-depacketized Opus payloads in media events; verify.
- A server that decodes N tracks adds CPU per participant, small for Opus; a Rust VAD (Silero via ONNX or webrtc-vad) gates cost.

### Gaps
- Not verified: whether symphonia decodes Opus (I recall it does not, unverified); audiopus status; str0m/webrtc-rs payload API; jitter-buffer handling.

## 5. Client-side vs server-side transcription trade-offs

### Takeaway
Evidence gathered is thin; the following is mostly derived reasoning. Client-side wins on cost (no audio on server) and trivial attribution; server-side wins on consistent quality, German support, and phone battery.

### Cited Findings
- Chrome's Web Speech API sends mic audio to Google; so "client-side" does not mean "private" unless `processLocally` or an in-browser model is used. — [AssemblyAI blog](https://www.assemblyai.com/blog/speech-recognition-javascript-web-speech-api), [MDN processLocally](https://developer.mozilla.org/en-US/docs/Web/API/SpeechRecognition/processLocally)
- Whisper-base in browser needs a ~200 MB model download and WebGPU for speed. — [Habr write-up](https://habr.com/en/articles/850284/)

### Inferences
- Client STT: each participant's device pays for inference (battery/thermal on phones, 200 MB+ download), quality varies per browser/device, and Web Speech may route audio to Google/Apple. Cost to operator is zero. Speaker label is free.
- Server STT: provider sees audio (GDPR/DPA needed; EU endpoint like AssemblyAI EU); quality uniform; cost ~$0.15-0.5/participant-hour; phones only upload audio they already send to the SFU.
- Hybrid: Web Speech/in-browser as free tier or fallback; cloud API for guaranteed German quality.

### Gaps
- No measured accuracy comparison of Web Speech vs Whisper vs cloud on German meeting audio; no battery measurements.

## 6. How meeting products implement live captions

### Takeaway
Pattern: server-side STT on per-participant/mixed audio, results fanned out to clients as text events (Jitsi, LiveKit). Google Meet/Zoom internals not researched.

### Cited Findings
- Jigasi (Jitsi) joins the conference as a participant and sends all participants' audio to an external STT: Google Cloud Speech, Vosk server, a Whisper flavor via Skynet (Fast Whisper), or Oracle Cloud AI Speech. Results go to participants as chat text or JSON; JSON produces subtitles in Jitsi Meet. Enabled via `config.transcription` and Jigasi properties. The Whisper path is documented mainly in community threads. — [jitsi/jigasi](https://github.com/jitsi/jigasi), [fairkom issue](https://git.fairkom.net/hosting/fairmeeting/-/issues/181)
- LiveKit Agents: per-track STT streams in an agent that joins the room (see section 4); forwarding to clients is covered on its "Text and transcriptions" page (not read). — [LiveKit STT docs](https://docs.livekit.io/agents/models/stt/)

### Gaps
- Google Meet and Zoom caption architectures not researched (no sources gathered). Deprecated products: none identified in this research; Google Speech v1 vs v2 status not checked.
