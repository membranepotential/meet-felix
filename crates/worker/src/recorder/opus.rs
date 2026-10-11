//! Opus encoding of 20 ms frames of 16 kHz mono speech (`opus-rs`, pure Rust, spike 3), the
//! spool format that keeps a chunk's packets in DO storage, and the Ogg/Opus file a chunk
//! becomes in R2 (RFC 7845).
//!
//! Each chunk is its own Ogg stream with its own encoder, so every chunk decodes from its first
//! packet: the decoder needs no state from an earlier chunk, and the pre-skip is just the
//! encoder's lookahead.

use std::borrow::Cow;

use ogg::{PacketWriteEndInfo, PacketWriter};
use opus_rs::{Application, OpusEncoder};

/// The encoder's sample rate.
const RATE: u32 = 16_000;
/// Samples in one 20 ms frame at [`RATE`].
pub const FRAME_SAMPLES: usize = 320;
/// Ogg/Opus granule positions count 48 kHz samples whatever the input rate (RFC 7845 §4), so a
/// 20 ms packet advances the granule by 960.
const GRANULE_PER_FRAME: u64 = 960;
/// The encoder's lookahead in 48 kHz samples, written as the pre-skip: libopus reports
/// `Fs/400 + Fs/250` for the VoIP application (2.5 ms plus the 4 ms delay compensation),
/// 104 samples at 16 kHz. `opus-rs` has no lookahead query; the test
/// `pre_skip_matches_the_encoders_delay` measures it with an impulse.
const PRE_SKIP: u16 = 312;
/// Target bitrate (D16: 16–24 kbps).
const BITRATE_BPS: i32 = 20_000;
/// The packet size limit handed to the encoder: 61 bytes per 20 ms, at most 24 kbps.
/// `opus-rs` 0.1.37 derives the SILK bitrate in VBR mode from the output buffer's length
/// instead of `bitrate_bps` (libopus uses `bitrate_bps`), so a full 1,276-byte buffer gave
/// 65 kbps on the speech clip of spike 3. With this cap the clip comes out at 17.9 kbps
/// (measured 2026-10-09).
const PACKET_CAP: usize = 61;
/// The Ogg stream's serial number; each file holds one stream, so any constant does.
const SERIAL: u32 = 0x6d66_7263;
/// Packets per Ogg page: one second.
const PACKETS_PER_PAGE: usize = 50;

/// Encodes frames for one chunk.
pub struct FrameEncoder {
    encoder: OpusEncoder,
    packet: [u8; PACKET_CAP],
}

impl FrameEncoder {
    pub fn new() -> Result<Self, &'static str> {
        let mut encoder = OpusEncoder::new(RATE as i32, 1, Application::Voip)?;
        encoder.bitrate_bps = BITRATE_BPS;
        encoder.use_cbr = false;
        Ok(Self {
            encoder,
            packet: [0; PACKET_CAP],
        })
    }

    /// Encodes one frame of [`FRAME_SAMPLES`] samples and appends the packet to `spool`.
    pub fn encode(&mut self, frame: &[i16], spool: &mut Vec<u8>) -> Result<(), &'static str> {
        let len = self
            .encoder
            .encode_i16(frame, FRAME_SAMPLES, &mut self.packet)?;
        append_packet(spool, &self.packet[..len]);
        Ok(())
    }
}

/// Appends a packet in the spool format: a little-endian `u16` length, then the bytes.
fn append_packet(spool: &mut Vec<u8>, packet: &[u8]) {
    let len = u16::try_from(packet.len()).expect("Opus packets are at most 1276 bytes");
    spool.extend_from_slice(&len.to_le_bytes());
    spool.extend_from_slice(packet);
}

/// The packets of spooled bytes. A truncated tail is dropped.
pub fn spooled_packets(mut spool: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        let (len, rest) = spool.split_first_chunk::<2>()?;
        let len = usize::from(u16::from_le_bytes(*len));
        let packet = rest.get(..len)?;
        spool = &rest[len..];
        Some(packet)
    })
}

/// An Ogg/Opus file of mono packets of 20 ms each.
pub fn ogg_opus<'a>(packets: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut writer = PacketWriter::new(Vec::new());
    let mut write = |packet: Cow<'a, [u8]>, end, granule| {
        writer
            .write_packet(packet, SERIAL, end, granule)
            .expect("writing to a Vec cannot fail");
    };
    write(opus_head().into(), PacketWriteEndInfo::EndPage, 0);
    let mut packets = packets.into_iter().peekable();
    let tags_end = if packets.peek().is_some() {
        PacketWriteEndInfo::EndPage
    } else {
        PacketWriteEndInfo::EndStream
    };
    write(opus_tags().into(), tags_end, 0);
    let mut count = 0;
    while let Some(packet) = packets.next() {
        count += 1;
        let end = if packets.peek().is_none() {
            PacketWriteEndInfo::EndStream
        } else if count % PACKETS_PER_PAGE == 0 {
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        write(packet.into(), end, count as u64 * GRANULE_PER_FRAME);
    }
    writer.into_inner()
}

/// The identification header (RFC 7845 §5.1).
fn opus_head() -> Vec<u8> {
    let mut head = b"OpusHead".to_vec();
    head.push(1); // version
    head.push(1); // channels
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&RATE.to_le_bytes()); // the input's rate, for information
    head.extend_from_slice(&0i16.to_le_bytes()); // output gain
    head.push(0); // channel mapping family: mono/stereo
    head
}

/// The comment header (RFC 7845 §5.2) with no comments.
fn opus_tags() -> Vec<u8> {
    let vendor = b"meet-felix";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    tags
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use ogg::PacketReader;
    use opus_rs::OpusDecoder;

    use super::*;

    fn sine(frames: usize) -> Vec<i16> {
        (0..frames * FRAME_SAMPLES)
            .map(|i| (8_000.0 * (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin()) as i16)
            .collect()
    }

    fn encode(pcm: &[i16]) -> Vec<u8> {
        let mut encoder = FrameEncoder::new().unwrap();
        let mut spool = Vec::new();
        for frame in pcm.chunks_exact(FRAME_SAMPLES) {
            encoder.encode(frame, &mut spool).unwrap();
        }
        spool
    }

    #[test]
    fn the_spool_keeps_packet_boundaries() {
        let mut spool = Vec::new();
        append_packet(&mut spool, &[1, 2, 3]);
        append_packet(&mut spool, &[]);
        append_packet(&mut spool, &[4]);
        let packets: Vec<&[u8]> = spooled_packets(&spool).collect();
        assert_eq!(packets, [&[1, 2, 3][..], &[][..], &[4][..]]);
        let truncated = &spool[..spool.len() - 1];
        assert_eq!(spooled_packets(truncated).count(), 2);
    }

    #[test]
    fn the_ogg_file_reads_back_with_headers_and_48_khz_granules() {
        let spool = encode(&sine(120));
        let file = ogg_opus(spooled_packets(&spool));
        let mut reader = PacketReader::new(Cursor::new(file));
        let head = reader.read_packet().unwrap().unwrap();
        assert_eq!(&head.data[..8], b"OpusHead");
        assert_eq!(u16::from_le_bytes([head.data[10], head.data[11]]), PRE_SKIP);
        let tags = reader.read_packet().unwrap().unwrap();
        assert_eq!(&tags.data[..8], b"OpusTags");
        let mut audio = Vec::new();
        while let Some(packet) = reader.read_packet().unwrap() {
            audio.push(packet);
        }
        assert_eq!(audio.len(), 120);
        let last = audio.last().unwrap();
        assert!(last.last_in_stream());
        assert_eq!(last.absgp_page(), 120 * GRANULE_PER_FRAME);
        assert_eq!(
            audio[49].absgp_page(),
            50 * GRANULE_PER_FRAME,
            "a page per second"
        );
        // Without audio the comment header ends the stream.
        let mut empty = PacketReader::new(Cursor::new(ogg_opus(std::iter::empty())));
        empty.read_packet().unwrap().unwrap();
        assert!(empty.read_packet().unwrap().unwrap().last_in_stream());
    }

    #[test]
    fn speech_band_audio_encodes_near_the_target_bitrate() {
        let seconds = 10;
        let spool = encode(&sine(50 * seconds));
        let bytes: usize = spooled_packets(&spool).map(<[u8]>::len).sum();
        let kbps = bytes as f64 * 8.0 / seconds as f64 / 1000.0;
        assert!((4.0..=24.4).contains(&kbps), "{kbps} kbps");
    }

    #[test]
    fn pre_skip_matches_the_encoders_delay() {
        // A click in the middle of silence comes out PRE_SKIP / 3 samples late at 16 kHz.
        let mut pcm = vec![0i16; 20 * FRAME_SAMPLES];
        let at = 10 * FRAME_SAMPLES;
        pcm[at] = 20_000;
        let spool = encode(&pcm);
        let mut decoder = OpusDecoder::new(RATE as i32, 1).unwrap();
        let mut decoded = Vec::new();
        let mut frame = [0f32; FRAME_SAMPLES];
        for packet in spooled_packets(&spool) {
            let n = decoder.decode(packet, FRAME_SAMPLES, &mut frame).unwrap();
            decoded.extend_from_slice(&frame[..n]);
        }
        let peak = decoded
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, _)| i)
            .unwrap();
        let delay = peak as i64 - at as i64;
        let expected = i64::from(PRE_SKIP) / 3;
        assert!(
            (delay - expected).abs() <= 4,
            "delay {delay}, pre-skip says {expected}"
        );
    }
}
