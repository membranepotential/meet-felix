//! The speech model behind the `Transcriber` trait (D17): Whisper large-v3-turbo on Workers AI
//! now; Deepgram Nova-3 could take its place without touching the job.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use worker::Ai;

use super::state::{AiFailure, classify_ai_error};

/// A segment of speech within one chunk, in seconds from the chunk's start.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedText {
    pub start_s: f64,
    pub end_s: f64,
    pub text: String,
}

/// Why a chunk could not be transcribed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeError {
    pub class: AiFailure,
    /// The model's error text, for the log.
    pub message: String,
}

/// Turns one Ogg/Opus chunk into timed text.
pub trait Transcriber {
    /// Transcribes `audio`; `prompt` names the participants, so names are spelled the same in
    /// every chunk.
    #[allow(
        async_fn_in_trait,
        reason = "Workers run single-threaded, so no caller needs a `Send` future"
    )]
    async fn transcribe(
        &self,
        audio: &[u8],
        prompt: &str,
    ) -> Result<Vec<TimedText>, TranscribeError>;
}

/// `@cf/openai/whisper-large-v3-turbo`: $0.0005 per audio minute, segments with timestamps,
/// language detected per request (spike 3).
pub struct WhisperTurbo(pub Ai);

/// The model's id on Workers AI.
pub const WHISPER_TURBO: &str = "@cf/openai/whisper-large-v3-turbo";

#[derive(Serialize)]
struct WhisperInput<'a> {
    /// Base64 of the audio file.
    audio: String,
    /// Drops non-speech before decoding, against hallucinated text in silence (research 11).
    vad_filter: bool,
    /// Beam size 1 or 2 hallucinates least (research 11).
    beam_size: u32,
    initial_prompt: &'a str,
}

/// The parts of the model's reply the job reads.
#[derive(Debug, Deserialize)]
pub struct WhisperOutput {
    #[serde(default)]
    pub segments: Vec<WhisperSegment>,
}

#[derive(Debug, Deserialize)]
pub struct WhisperSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub no_speech_prob: f64,
}

/// Segments more likely silence than speech are dropped.
const MAX_NO_SPEECH_PROB: f64 = 0.6;

impl Transcriber for WhisperTurbo {
    async fn transcribe(
        &self,
        audio: &[u8],
        prompt: &str,
    ) -> Result<Vec<TimedText>, TranscribeError> {
        let input = WhisperInput {
            audio: BASE64.encode(audio),
            vad_filter: true,
            beam_size: 2,
            initial_prompt: prompt,
        };
        match self.0.run::<_, WhisperOutput>(WHISPER_TURBO, input).await {
            Ok(output) => Ok(speech(output)),
            Err(e) => {
                let message = e.to_string();
                Err(TranscribeError {
                    class: classify_ai_error(&message),
                    message,
                })
            }
        }
    }
}

/// The segments worth keeping: some text, and probably speech.
pub fn speech(output: WhisperOutput) -> Vec<TimedText> {
    output
        .segments
        .into_iter()
        .filter(|s| s.no_speech_prob <= MAX_NO_SPEECH_PROB && !s.text.trim().is_empty())
        .map(|s| TimedText {
            start_s: s.start,
            end_s: s.end,
            text: s.text.trim().to_owned(),
        })
        .collect()
}

// `Nova3Http` (`@cf/deepgram/nova-3` over HTTP, $0.0052 per minute) is the fallback D17 names if
// German quality disappoints; it is not built, since nothing calls for it yet.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_speech_and_drops_silence_and_empty_text() {
        let reply = serde_json::json!({
            "text": "Hallo zusammen. Thank you.",
            "segments": [
                { "start": 0.0, "end": 1.5, "text": " Hallo zusammen.", "no_speech_prob": 0.01,
                  "avg_logprob": -0.2, "compression_ratio": 1.1, "words": [] },
                { "start": 20.0, "end": 21.0, "text": " Thank you.", "no_speech_prob": 0.9 },
                { "start": 22.0, "end": 23.0, "text": "  ", "no_speech_prob": 0.0 }
            ],
            "transcription_info": { "language": "de", "language_probability": 0.9,
                "duration": 30.0, "duration_after_vad": 2.0 },
            "word_count": 4
        });
        let output: WhisperOutput = serde_json::from_value(reply).unwrap();
        assert_eq!(
            speech(output),
            [TimedText {
                start_s: 0.0,
                end_s: 1.5,
                text: "Hallo zusammen.".into()
            }]
        );
        let empty: WhisperOutput = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(speech(empty), []);
    }

    #[test]
    fn the_request_carries_base64_audio_and_the_anti_hallucination_settings() {
        let input = WhisperInput {
            audio: BASE64.encode(b"OggS"),
            vad_filter: true,
            beam_size: 2,
            initial_prompt: "Participants: Anna.",
        };
        assert_eq!(
            serde_json::to_value(input).unwrap(),
            serde_json::json!({ "audio": "T2dnUw==", "vad_filter": true, "beam_size": 2,
                "initial_prompt": "Participants: Anna." })
        );
    }
}
