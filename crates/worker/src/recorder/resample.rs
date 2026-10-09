//! 48 kHz stereo PCM from the adapter to 16 kHz mono for the speech encoder: downmix, low-pass,
//! keep every third sample (D16). The low-pass runs before the decimation, or everything above
//! 8 kHz would fold into the speech band (research 11), and only every third output is computed.
//! The filter history lives in the [`Downsampler`], so frames join without clicks.

use std::f32::consts::PI;

/// Input samples per output sample.
pub const FACTOR: usize = 3;
/// Filter length: odd and symmetric, so the delay is a whole number of samples.
const TAPS: usize = 63;
/// The pass band ends here, below the 8 kHz Nyquist frequency of the output.
const CUTOFF_HZ: f32 = 7_000.0;
const INPUT_HZ: f32 = 48_000.0;

/// Downmixes, filters and decimates a stream, keeping state across calls.
#[derive(Debug, Clone)]
pub struct Downsampler {
    coefficients: [f32; TAPS],
    /// The last `TAPS - 1` mono input samples, then the ones not yet consumed.
    input: Vec<f32>,
    /// The index in `input` of the newest sample of the next output's window.
    next: usize,
}

impl Default for Downsampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Downsampler {
    pub fn new() -> Self {
        // A windowed sinc (Blackman window), normalised to unit gain at 0 Hz.
        let cutoff = CUTOFF_HZ / INPUT_HZ;
        let middle = (TAPS - 1) as f32 / 2.0;
        let mut coefficients = [0.0; TAPS];
        for (n, c) in coefficients.iter_mut().enumerate() {
            let x = n as f32 - middle;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * PI * cutoff * x).sin() / (PI * x)
            };
            let phase = 2.0 * PI * n as f32 / (TAPS - 1) as f32;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            *c = sinc * window;
        }
        let sum: f32 = coefficients.iter().sum();
        coefficients.iter_mut().for_each(|c| *c /= sum);
        Self {
            coefficients,
            input: vec![0.0; TAPS - 1],
            next: TAPS - 1,
        }
    }

    /// Appends the 16 kHz mono samples for `pcm` (48 kHz stereo s16le, interleaved) to `out`.
    /// A trailing partial stereo frame is ignored.
    pub fn push(&mut self, pcm: &[u8], out: &mut Vec<i16>) {
        self.input.extend(pcm.chunks_exact(4).map(|frame| {
            let left = i16::from_le_bytes([frame[0], frame[1]]);
            let right = i16::from_le_bytes([frame[2], frame[3]]);
            (f32::from(left) + f32::from(right)) / 2.0
        }));
        while self.next < self.input.len() {
            let window = &self.input[self.next + 1 - TAPS..=self.next];
            let y: f32 = window
                .iter()
                .zip(self.coefficients.iter().rev())
                .map(|(x, c)| x * c)
                .sum();
            out.push(y.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16);
            self.next += FACTOR;
        }
        let consumed = self.input.len() - (TAPS - 1);
        self.input.drain(..consumed);
        self.next -= consumed;
    }
}

/// The RMS level of `frame` in dBFS (0 for a full-scale square wave; −∞ for silence).
pub fn level_dbfs(frame: &[i16]) -> f32 {
    if frame.is_empty() {
        return f32::NEG_INFINITY;
    }
    let power = frame
        .iter()
        .map(|&s| {
            let x = f64::from(s) / 32_768.0;
            x * x
        })
        .sum::<f64>()
        / frame.len() as f64;
    (10.0 * power.log10()) as f32
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One second of a stereo sine at `hz` with the given peak, as s16le bytes.
    pub(crate) fn tone(hz: f32, peak: f32, seconds: f32) -> Vec<u8> {
        let n = (INPUT_HZ * seconds) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (peak * (2.0 * PI * hz * i as f32 / INPUT_HZ).sin()) as i16;
                let b = s.to_le_bytes();
                [b[0], b[1], b[0], b[1]]
            })
            .collect()
    }

    /// Runs `pcm` through in 20 ms messages and returns the output after the filter settled.
    fn through(pcm: &[u8]) -> Vec<i16> {
        let mut down = Downsampler::new();
        let mut out = Vec::new();
        for message in pcm.chunks(3_840) {
            down.push(message, &mut out);
        }
        out.split_off(TAPS)
    }

    fn peak(samples: &[i16]) -> f32 {
        samples
            .iter()
            .map(|s| f32::from(s.unsigned_abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn one_input_second_gives_one_output_second() {
        let mut down = Downsampler::new();
        let mut out = Vec::new();
        for message in tone(440.0, 1_000.0, 1.0).chunks(3_840) {
            down.push(message, &mut out);
        }
        assert_eq!(out.len(), 16_000);
    }

    #[test]
    fn speech_band_tones_pass_and_high_tones_are_attenuated_by_more_than_40_db() {
        let passed = peak(&through(&tone(1_000.0, 10_000.0, 1.0)));
        assert!(
            (9_800.0..=10_200.0).contains(&passed),
            "1 kHz peak {passed}"
        );
        let stopped = peak(&through(&tone(20_000.0, 10_000.0, 1.0)));
        assert!(stopped < 100.0, "20 kHz peak {stopped} (40 dB is 100)");
        let alias = peak(&through(&tone(12_000.0, 10_000.0, 1.0)));
        assert!(alias < 100.0, "12 kHz would alias to 4 kHz; peak {alias}");
    }

    #[test]
    fn constant_input_stays_constant_and_channels_are_averaged() {
        let pcm: Vec<u8> = (0..4_800)
            .flat_map(|_| {
                let (l, r) = (1_200i16.to_le_bytes(), 800i16.to_le_bytes());
                [l[0], l[1], r[0], r[1]]
            })
            .collect();
        assert!(through(&pcm).iter().all(|&s| s == 1_000));
    }

    #[test]
    fn levels_in_dbfs() {
        assert_eq!(level_dbfs(&[0; 320]), f32::NEG_INFINITY);
        assert!((level_dbfs(&[i16::MAX; 320]) - 0.0).abs() < 0.01);
        assert!((level_dbfs(&[328; 320]) + 40.0).abs() < 0.1);
    }
}
