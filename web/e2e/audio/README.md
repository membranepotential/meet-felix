# Test speech

Short snippets of real speech for the end-to-end tests, cut from the owner's
test meeting of 2026-10-10 (laptop microphone and an iPhone on mobile data,
both through the Realtime SFU and the recorder). WAV, 48 kHz, mono, 16-bit:
what Chromium's `--use-file-for-fake-audio-capture` plays and what the
recorder e2e streams as an adapter would (upmixed to stereo).

`snippets.json` lists each file with the speaker's device, the words said and
the `keywords` a transcript of it must contain. `../protocol/recorder.spec.ts`
(`pnpm test:e2e:recorder`) streams all four in a row through the mock SFU's
adapter into the recorder (read by `../fixtures/speech.ts`) and checks the
keywords against the Whisper transcript.
