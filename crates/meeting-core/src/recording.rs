//! The rules of the recorder (D16 in `docs/recording`): who is on the roster and how speakers
//! are labelled, when a stream of 20 ms audio frames counts as silent, where chunks are cut, and
//! when the adapter stream has a hole. Pure, so the `RecorderObject` only carries them out.

use serde::{Deserialize, Serialize};
#[cfg(feature = "wasm")]
use tsify::Tsify;

use crate::ids::{DisplayName, Millis, ParticipantId};
use crate::protocol::Role;

/// A participant as the transcript names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RosterEntry {
    /// Who.
    pub id: ParticipantId,
    /// Their display name.
    pub name: DisplayName,
    /// Host or guest.
    pub role: Role,
}

/// One participant's consent to recording, for the transcript's consent record (research 11,
/// item 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct ConsentRecord {
    /// Who.
    pub who: ParticipantId,
    /// Their display name.
    pub name: DisplayName,
    /// When they gave consent.
    pub at: Millis,
    /// The version of the recording notice they saw ([`crate::NOTICE_VERSION`]).
    pub notice_version: u32,
    /// When they withdrew it, if they did and have not given it again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "wasm", tsify(optional))]
    pub withdrawn_at: Option<Millis>,
}

/// A speaker with the label the transcript shows for them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "wasm", derive(Tsify))]
#[serde(rename_all = "camelCase")]
pub struct Speaker {
    /// Who.
    pub who: ParticipantId,
    /// The label, unique among the speakers.
    pub label: String,
    /// Host or guest.
    pub role: Role,
}

/// Labels speakers so that every label names one person (D31 in `docs/ui`): names that are
/// equal, ignoring case, get numbers in participant order ("Anna", "Anna 2"), and hosts are
/// marked ("Robin (host)"). The result is ordered by participant id.
pub fn label_speakers(speakers: &[RosterEntry]) -> Vec<Speaker> {
    let mut sorted: Vec<&RosterEntry> = speakers.iter().collect();
    sorted.sort_by_key(|entry| entry.id);
    sorted
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let key = entry.name.as_str().to_lowercase();
            let earlier = sorted[..i]
                .iter()
                .filter(|other| other.name.as_str().to_lowercase() == key)
                .count();
            let mut label = entry.name.as_str().to_owned();
            if earlier > 0 {
                label.push_str(&format!(" {}", earlier + 1));
            }
            if entry.role == Role::Host {
                label.push_str(" (host)");
            }
            Speaker {
                who: entry.id,
                label,
                role: entry.role,
            }
        })
        .collect()
}

/// Length of one audio frame the recorder encodes, in milliseconds.
pub const FRAME_MS: u64 = 20;
/// A frame whose RMS level lies below this is quiet.
pub const SILENCE_DBFS: f32 = -50.0;
/// Frames of quiet after which the stream counts as silent (300 ms).
pub const SILENCE_FRAMES: u32 = 15;
/// A chunk ends at its first silent frame after this many frames (25 s) ...
pub const SOFT_CHUNK_FRAMES: u32 = 1_250;
/// ... and at this many frames (30 s) at the latest.
pub const HARD_CHUNK_FRAMES: u32 = 1_500;
/// A chunk with fewer loud frames than this (300 ms) is silent; the transcript job skips it.
pub const MIN_VOICED_FRAMES: u32 = 15;
/// More time than this between two adapter messages is a hole in the stream.
pub const MAX_ARRIVAL_GAP_MS: u64 = 1_000;
/// A sequence number at most this far behind the last one is a late duplicate; further back,
/// the counter restarted.
pub const MAX_REORDER: i32 = 1_000;

/// The silence gate, with hysteresis: a loud frame opens it at once, and it closes only after
/// [`SILENCE_FRAMES`] quiet frames in a row. It starts closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SilenceGate {
    quiet_frames: u32,
    open: bool,
}

impl SilenceGate {
    /// Feeds one frame's RMS level in dBFS; returns whether the stream is silent at this frame.
    pub fn frame(&mut self, level_dbfs: f32) -> bool {
        if level_dbfs >= SILENCE_DBFS {
            self.open = true;
            self.quiet_frames = 0;
        } else {
            self.quiet_frames = self.quiet_frames.saturating_add(1);
            if self.quiet_frames >= SILENCE_FRAMES {
                self.open = false;
            }
        }
        !self.open
    }
}

/// Counts the frames of one chunk and says where it ends: after its first silent frame once it
/// is [`SOFT_CHUNK_FRAMES`] long, and after [`HARD_CHUNK_FRAMES`] in any case. Cutting inside
/// silence keeps words whole, since the transcript job does not overlap chunks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Chunker {
    frames: u32,
    loud_frames: u32,
}

impl Chunker {
    /// Adds one frame, `loud` if its level reached [`SILENCE_DBFS`] and `silent` as the gate says;
    /// returns whether the chunk ends after this frame.
    pub fn frame(&mut self, loud: bool, silent: bool) -> bool {
        self.frames += 1;
        self.loud_frames += u32::from(loud);
        self.frames >= HARD_CHUNK_FRAMES || (self.frames >= SOFT_CHUNK_FRAMES && silent)
    }

    /// The frames so far.
    pub fn frames(self) -> u32 {
        self.frames
    }

    /// The chunk's length so far in milliseconds.
    pub fn duration_ms(self) -> u64 {
        u64::from(self.frames) * FRAME_MS
    }

    /// Whether the chunk so far has too little sound to transcribe.
    pub fn silent(self) -> bool {
        self.loud_frames < MIN_VOICED_FRAMES
    }
}

/// One message from the SFU's WebSocket adapter, as far as continuity goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    /// The packet's `sequenceNumber`.
    pub sequence: u32,
    /// The packet's `timestamp`, in an undocumented clock.
    pub timestamp: u32,
    /// The shell's clock when the message arrived.
    pub at: Millis,
    /// Audio samples per channel in the payload.
    pub samples: u32,
}

/// How a message relates to the one before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuity {
    /// The next message of an unbroken stream (or the first one).
    Next,
    /// Not newer than the last message: a duplicate after a reconnect. Drop it.
    Stale,
    /// Audio is missing before this message: end the chunk and start a new one with a fresh
    /// wall-clock start.
    Hole,
}

/// Tracks the adapter stream's sequence numbers, timestamps and arrival times to find holes
/// (research 11, "The recorder must survive holes, restarts and silence"). Delivery after a
/// reconnect is neither gapless nor exactly-once, and the timestamp's clock rate is
/// undocumented, so the rate is learned from the first two consecutive messages.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StreamClock {
    last: Option<Arrival>,
    ticks_per_sample: Option<f64>,
}

impl StreamClock {
    /// Classifies `next` and, unless it is stale, remembers it.
    pub fn observe(&mut self, next: Arrival) -> Continuity {
        let Some(last) = self.last else {
            self.last = Some(next);
            return Continuity::Next;
        };
        // Wrapping difference, so a counter that overflows stays in order.
        let step = next.sequence.wrapping_sub(last.sequence) as i32;
        if step < -MAX_REORDER {
            // The counter restarted (a new connection, or a 16-bit RTP counter wrapping inside
            // a u32): a new stream, not a duplicate.
            self.last = Some(next);
            return Continuity::Hole;
        }
        if step <= 0 {
            return Continuity::Stale;
        }
        self.last = Some(next);
        if step > 1 || next.at.0.saturating_sub(last.at.0) > MAX_ARRIVAL_GAP_MS {
            return Continuity::Hole;
        }
        let ticks = f64::from(next.timestamp.wrapping_sub(last.timestamp) as i32);
        let samples = f64::from(last.samples);
        match self.ticks_per_sample {
            None if ticks > 0.0 && samples > 0.0 => {
                self.ticks_per_sample = Some(ticks / samples);
                Continuity::Next
            }
            None => Continuity::Next,
            // More than one frame (20 ms at 48 kHz) off the expected step, or backwards.
            Some(rate) if ticks < 0.0 || (ticks - rate * samples).abs() > rate * 960.0 => {
                Continuity::Hole
            }
            Some(_) => Continuity::Next,
        }
    }
}
