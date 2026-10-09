//! The SFU WebSocket adapter's message: one protobuf `Packet` per binary message
//! (<https://developers.cloudflare.com/realtime/sfu/media-transport-adapters/websocket-adapter/>):
//!
//! ```proto
//! message Packet { uint32 sequenceNumber = 1; uint32 timestamp = 2; bytes payload = 5; }
//! ```
//!
//! A hand-written reader, because three fields do not justify a protobuf crate. Unknown fields
//! are skipped, as protobuf requires.

/// A decoded `Packet`; the payload borrows from the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Packet<'a> {
    pub sequence_number: u32,
    pub timestamp: u32,
    /// 48 kHz stereo signed 16-bit little-endian PCM, left and right interleaved.
    pub payload: &'a [u8],
}

/// The message is not a well-formed protobuf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Malformed;

impl<'a> Packet<'a> {
    /// Decodes a message. Missing fields keep their protobuf defaults (0 and empty).
    pub fn decode(mut bytes: &'a [u8]) -> Result<Self, Malformed> {
        let mut packet = Packet::default();
        while !bytes.is_empty() {
            let key = varint(&mut bytes)?;
            let (field, wire_type) = (key >> 3, key & 7);
            match (field, wire_type) {
                // A uint32 encoded as a wider varint keeps its low 32 bits, as in protobuf.
                (1, 0) => packet.sequence_number = varint(&mut bytes)? as u32,
                (2, 0) => packet.timestamp = varint(&mut bytes)? as u32,
                (5, 2) => packet.payload = length_delimited(&mut bytes)?,
                (_, 0) => {
                    varint(&mut bytes)?;
                }
                (_, 1) => take(&mut bytes, 8).map(drop)?,
                (_, 2) => length_delimited(&mut bytes).map(drop)?,
                (_, 5) => take(&mut bytes, 4).map(drop)?,
                _ => return Err(Malformed),
            }
        }
        Ok(packet)
    }
}

fn varint(bytes: &mut &[u8]) -> Result<u64, Malformed> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or(Malformed)?;
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(Malformed)
}

fn length_delimited<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8], Malformed> {
    let len = usize::try_from(varint(bytes)?).map_err(|_| Malformed)?;
    take(bytes, len)
}

fn take<'a>(bytes: &mut &'a [u8], len: usize) -> Result<&'a [u8], Malformed> {
    if bytes.len() < len {
        return Err(Malformed);
    }
    let (taken, rest) = bytes.split_at(len);
    *bytes = rest;
    Ok(taken)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn put_varint(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    /// Encodes a packet the way the SFU does (field order 1, 2, 5).
    pub(crate) fn encode(sequence_number: u32, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        put_varint(&mut out, 1 << 3);
        put_varint(&mut out, sequence_number.into());
        put_varint(&mut out, 2 << 3);
        put_varint(&mut out, timestamp.into());
        put_varint(&mut out, (5 << 3) | 2);
        put_varint(&mut out, payload.len() as u64);
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn decodes_the_three_fields() {
        let payload = [1u8, 2, 3, 4];
        let bytes = encode(300, u32::MAX, &payload);
        assert_eq!(
            Packet::decode(&bytes),
            Ok(Packet {
                sequence_number: 300,
                timestamp: u32::MAX,
                payload: &payload
            })
        );
    }

    #[test]
    fn skips_unknown_fields_and_keeps_defaults_for_missing_ones() {
        let mut bytes = vec![3 << 3, 0x96, 0x01]; // field 3, varint 150
        bytes.extend([(4 << 3) | 2, 2, 9, 9]); // field 4, two bytes
        bytes.extend([(6 << 3) | 5, 0, 0, 0, 0]); // field 6, fixed32
        bytes.extend([(7 << 3) | 1, 0, 0, 0, 0, 0, 0, 0, 0]); // field 7, fixed64
        bytes.extend([(5 << 3) | 2, 1, 42]);
        assert_eq!(
            Packet::decode(&bytes),
            Ok(Packet {
                sequence_number: 0,
                timestamp: 0,
                payload: &[42]
            })
        );
        assert_eq!(Packet::decode(&[]), Ok(Packet::default()));
    }

    #[test]
    fn rejects_truncated_and_invalid_messages() {
        let bytes = encode(1, 2, &[0; 10]);
        for cut in 1..bytes.len() {
            if Packet::decode(&bytes[..cut]).is_ok() {
                // A cut right after a whole field is a shorter valid message.
                assert!(matches!(cut, 2 | 4), "cut at {cut} decoded");
            }
        }
        assert_eq!(
            Packet::decode(&[(1 << 3) | 3]),
            Err(Malformed),
            "group wire type"
        );
        assert_eq!(
            Packet::decode(&[0x80; 11]),
            Err(Malformed),
            "endless varint"
        );
    }
}
