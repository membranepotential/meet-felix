//! One adapter stream's way from protobuf packets to Opus chunks: continuity check, downmix and
//! resampling, silence gate, encoding, chunk cuts (the rules are `meeting_core::recording`'s).
//! Pure: the `RecorderObject` feeds packets with its clock and stores what the [`Note`]s say,
//! so a test can run a whole stream natively.

use meeting_core::Millis;
use meeting_core::recording::{
    Arrival, Chunker, Continuity, FRAME_MS, SILENCE_DBFS, SilenceGate, StreamClock,
};

use super::opus::{FRAME_SAMPLES, FrameEncoder};
use super::packet::Packet;
use super::resample::{Downsampler, level_dbfs};

/// The longest the encoded packets of an open chunk wait in memory before they are spooled to
/// storage; an eviction loses at most this much audio (plus one message).
pub const SPOOL_INTERVAL_MS: u64 = 1_500;

/// What the shell must store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A chunk began; `start` is the wall-clock time of its first frame.
    Started { index: u32, start: Millis },
    /// Encoded packets of chunk `index` in the spool format, to append to its spool.
    Spool { index: u32, packets: Vec<u8> },
    /// Chunk `index` ended; all its packets were spooled before this note.
    Closed {
        index: u32,
        duration_ms: u64,
        silent: bool,
    },
}

struct OpenChunk {
    index: u32,
    chunker: Chunker,
    encoder: FrameEncoder,
    /// Encoded packets not yet spooled.
    pending: Vec<u8>,
    spooled_at: Millis,
}

/// The state of one adapter stream (one speaker, one adapter generation).
pub struct Capture {
    next_index: u32,
    clock: StreamClock,
    downsampler: Downsampler,
    /// 16 kHz samples that do not yet fill a frame.
    pcm: Vec<i16>,
    gate: SilenceGate,
    chunk: Option<OpenChunk>,
}

impl Capture {
    /// A stream whose next chunk gets number `next_index`.
    pub fn new(next_index: u32) -> Self {
        Self {
            next_index,
            clock: StreamClock::default(),
            downsampler: Downsampler::new(),
            pcm: Vec::with_capacity(2 * FRAME_SAMPLES),
            gate: SilenceGate::default(),
            chunk: None,
        }
    }

    /// Takes one adapter message that arrived at `now`. A duplicate is dropped; a hole before
    /// it ends the open chunk, so the next chunk starts with a fresh wall-clock time.
    pub fn push(
        &mut self,
        packet: Packet<'_>,
        now: Millis,
        notes: &mut Vec<Note>,
    ) -> Result<(), &'static str> {
        let arrival = Arrival {
            sequence: packet.sequence_number,
            timestamp: packet.timestamp,
            at: now,
            samples: (packet.payload.len() / 4) as u32,
        };
        match self.clock.observe(arrival) {
            Continuity::Stale => return Ok(()),
            Continuity::Hole => self.restart(notes),
            Continuity::Next => {}
        }
        self.downsampler.push(packet.payload, &mut self.pcm);
        let pcm = std::mem::take(&mut self.pcm);
        let mut frames = pcm.chunks_exact(FRAME_SAMPLES);
        let result = frames
            .by_ref()
            .try_for_each(|frame| self.frame(frame, now, notes));
        let rest = frames.remainder().len();
        self.pcm = pcm;
        self.pcm.drain(..self.pcm.len() - rest);
        result?;
        if let Some(chunk) = &mut self.chunk
            && now.0.saturating_sub(chunk.spooled_at.0) >= SPOOL_INTERVAL_MS
        {
            chunk.spooled_at = now;
            notes.push(Note::Spool {
                index: chunk.index,
                packets: std::mem::take(&mut chunk.pending),
            });
        }
        Ok(())
    }

    /// The stream ended: closes the open chunk.
    pub fn finish(&mut self, notes: &mut Vec<Note>) {
        self.restart(notes);
    }

    /// Ends the open chunk and forgets partial audio, as after a hole.
    fn restart(&mut self, notes: &mut Vec<Note>) {
        self.close(notes);
        self.pcm.clear();
        self.gate = SilenceGate::default();
    }

    fn frame(
        &mut self,
        frame: &[i16],
        now: Millis,
        notes: &mut Vec<Note>,
    ) -> Result<(), &'static str> {
        let chunk = match &mut self.chunk {
            Some(chunk) => chunk,
            None => {
                let index = self.next_index;
                self.next_index += 1;
                notes.push(Note::Started { index, start: now });
                self.chunk.insert(OpenChunk {
                    index,
                    chunker: Chunker::default(),
                    encoder: FrameEncoder::new()?,
                    pending: Vec::new(),
                    spooled_at: now,
                })
            }
        };
        let level = level_dbfs(frame);
        let silent = self.gate.frame(level);
        chunk.encoder.encode(frame, &mut chunk.pending)?;
        if chunk.chunker.frame(level >= SILENCE_DBFS, silent) {
            self.close(notes);
        }
        Ok(())
    }

    fn close(&mut self, notes: &mut Vec<Note>) {
        let Some(chunk) = self.chunk.take() else {
            return;
        };
        if !chunk.pending.is_empty() {
            notes.push(Note::Spool {
                index: chunk.index,
                packets: chunk.pending,
            });
        }
        notes.push(Note::Closed {
            index: chunk.index,
            duration_ms: u64::from(chunk.chunker.frames()) * FRAME_MS,
            silent: chunk.chunker.silent(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::opus::spooled_packets;
    use super::super::resample::tests::tone;
    use super::*;

    /// 20 ms adapter messages of `pcm` from sequence number `first`, arriving every 20 ms from
    /// `t0`; returns the notes.
    fn stream(capture: &mut Capture, pcm: &[u8], first: u32, t0: u64) -> Vec<Note> {
        let mut notes = Vec::new();
        for (i, payload) in pcm.chunks(3_840).enumerate() {
            let packet = Packet {
                sequence_number: first + i as u32,
                timestamp: (first + i as u32) * 960,
                payload,
            };
            let now = Millis(t0 + 20 * i as u64);
            capture.push(packet, now, &mut notes).unwrap();
        }
        notes
    }

    fn closed(notes: &[Note]) -> Vec<(u32, u64, bool)> {
        notes
            .iter()
            .filter_map(|n| match n {
                Note::Closed {
                    index,
                    duration_ms,
                    silent,
                } => Some((*index, *duration_ms, *silent)),
                _ => None,
            })
            .collect()
    }

    fn spooled_frames(notes: &[Note], of: u32) -> usize {
        notes
            .iter()
            .map(|n| match n {
                Note::Spool { index, packets } if *index == of => spooled_packets(packets).count(),
                _ => 0,
            })
            .sum()
    }

    #[test]
    fn continuous_speech_is_cut_at_30_s_and_every_frame_is_spooled() {
        let mut capture = Capture::new(0);
        let mut notes = stream(&mut capture, &tone(440.0, 8_000.0, 31.0), 0, 0);
        assert_eq!(closed(&notes), [(0, 30_000, false)]);
        assert_eq!(
            notes[0],
            Note::Started {
                index: 0,
                start: Millis(0)
            }
        );
        assert!(notes.contains(&Note::Started {
            index: 1,
            start: Millis(30_000)
        }));
        assert_eq!(spooled_frames(&notes, 0), 1_500);
        capture.finish(&mut notes);
        assert_eq!(closed(&notes)[1].0, 1);
        assert!((spooled_frames(&notes, 1) as i64 - 50).abs() <= 1);
    }

    #[test]
    fn packets_are_spooled_at_least_every_1_5_s() {
        let mut capture = Capture::new(0);
        let notes = stream(&mut capture, &tone(440.0, 8_000.0, 5.0), 0, 0);
        let spools = notes
            .iter()
            .filter(|n| matches!(n, Note::Spool { .. }))
            .count();
        assert_eq!(spools, 3, "at 1.5, 3.0 and 4.5 s");
    }

    #[test]
    fn a_hole_ends_the_chunk_and_the_next_starts_at_its_own_time() {
        let mut capture = Capture::new(7);
        let mut notes = stream(&mut capture, &tone(440.0, 8_000.0, 2.0), 0, 0);
        // Twenty messages lost, then the stream goes on.
        notes.extend(stream(&mut capture, &tone(440.0, 8_000.0, 1.0), 120, 2_400));
        assert_eq!(closed(&notes), [(7, 2_000, false)]);
        assert!(notes.contains(&Note::Started {
            index: 8,
            start: Millis(2_400)
        }));
        // A duplicate after a reconnect changes nothing.
        let mut dup = Vec::new();
        let payload = tone(440.0, 8_000.0, 0.02);
        let packet = Packet {
            sequence_number: 130,
            timestamp: 0,
            payload: &payload,
        };
        capture.push(packet, Millis(9_000), &mut dup).unwrap();
        assert_eq!(dup, []);
    }

    #[test]
    fn quiet_audio_makes_a_silent_chunk_cut_at_the_first_silence_after_25_s() {
        let mut capture = Capture::new(0);
        let mut quiet = tone(440.0, 20.0, 26.0);
        quiet.extend(tone(440.0, 8_000.0, 1.0));
        let notes = stream(&mut capture, &quiet, 0, 0);
        assert_eq!(closed(&notes), [(0, 25_000, true)]);
    }
}
