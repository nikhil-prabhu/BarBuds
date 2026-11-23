//! HDLC framing as used by pigweed's `pw_hdlc`, which carries Maestro RPC packets.
//!
//! ```text
//! 0x7E | address (varint) | control | data… | CRC-32 (LE, over address..data) | 0x7E
//! ```
//! Every byte between the flags that equals 0x7E or 0x7D is sent as 0x7D
//! followed by the byte XOR 0x20. Adjacent frames may share one flag byte.

use crate::error::BarBudsError;

const FLAG: u8 = 0x7E;
const ESCAPE: u8 = 0x7D;
const ESCAPE_MASK: u8 = 0x20;
/// Unnumbered-information frame, the only kind pw_rpc uses.
pub(crate) const CONTROL_UI: u8 = 0x03;
/// Shortest unescaped frame body: 1 address byte, control byte, 4 CRC bytes.
const MIN_BODY_LEN: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub address: u32,
    pub control: u8,
    pub data: Vec<u8>,
}

/// CRC-32 (IEEE 802.3: reflected, polynomial 0xEDB88320, init and final XOR 0xFFFFFFFF).
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// HDLC's address varint: 7 bits per byte, least significant group first,
/// stored in bits 1–7. Bit 0 is set on the final byte.
fn encode_address(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let bits = ((value & 0x7F) as u8) << 1;
        value >>= 7;
        if value == 0 {
            out.push(bits | 1);
            return;
        }
        out.push(bits);
    }
}

/// Returns the address and how many bytes it used.
fn decode_address(bytes: &[u8]) -> Result<(u32, usize), BarBudsError> {
    let mut value = 0u64;
    for (i, &byte) in bytes.iter().enumerate().take(5) {
        value |= u64::from(byte >> 1) << (i * 7);
        if byte & 1 == 1 {
            return u32::try_from(value)
                .map(|value| (value, i + 1))
                .map_err(|_| BarBudsError::malformed("HDLC address overflows 32 bits"));
        }
    }
    Err(BarBudsError::malformed("unterminated HDLC address"))
}

pub(crate) fn encode(frame: &Frame) -> Vec<u8> {
    let mut body = Vec::with_capacity(frame.data.len() + 10);
    encode_address(frame.address, &mut body);
    body.push(frame.control);
    body.extend_from_slice(&frame.data);
    let crc = crc32(&body);
    body.extend_from_slice(&crc.to_le_bytes());

    let mut out = Vec::with_capacity(body.len() + 4);
    out.push(FLAG);
    for byte in body {
        if byte == FLAG || byte == ESCAPE {
            out.extend_from_slice(&[ESCAPE, byte ^ ESCAPE_MASK]);
        } else {
            out.push(byte);
        }
    }
    out.push(FLAG);
    out
}

/// Split one frame off the front of `buf`. `None` means more bytes are needed.
///
/// Otherwise returns how many bytes to drop (always > 0) and the frame, or
/// `Ok(None)` when only flag bytes were consumed. The closing flag is left in
/// the buffer because it may also open the next frame.
pub(crate) fn decode(buf: &[u8]) -> Option<(usize, Result<Option<Frame>, BarBudsError>)> {
    let start = buf.iter().take_while(|&&b| b == FLAG).count();
    if start > 0 {
        return Some((start, Ok(None)));
    }
    let end = buf.iter().position(|&b| b == FLAG)?;
    Some((end, decode_body(&buf[..end]).map(Some)))
}

fn decode_body(escaped: &[u8]) -> Result<Frame, BarBudsError> {
    let mut body = Vec::with_capacity(escaped.len());
    let mut bytes = escaped.iter();
    while let Some(&byte) = bytes.next() {
        if byte == ESCAPE {
            let &next = bytes
                .next()
                .ok_or_else(|| BarBudsError::malformed("HDLC escape byte at end of frame"))?;
            body.push(next ^ ESCAPE_MASK);
        } else {
            body.push(byte);
        }
    }

    if body.len() < MIN_BODY_LEN {
        return Err(BarBudsError::malformed(format!(
            "HDLC frame is {} byte(s), expected at least {MIN_BODY_LEN}",
            body.len()
        )));
    }
    let (content, crc) = body.split_at(body.len() - 4);
    let actual = u32::from_le_bytes(crc.try_into().expect("split leaves 4 bytes"));
    let expected = crc32(content);
    if actual != expected {
        return Err(BarBudsError::ChecksumMismatch { expected, actual });
    }

    let (address, len) = decode_address(content)?;
    let (&control, data) = content[len..]
        .split_first()
        .ok_or_else(|| BarBudsError::malformed("HDLC frame has no control byte"))?;
    Ok(Frame {
        address,
        control,
        data: data.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors from pbpctrl's libmaestro HDLC tests (MIT/Apache-2.0), an
    // independent implementation that has talked to real Pixel Buds Pro.
    const EMPTY_FRAME: [u8; 10] = [0x7E, 0x06, 0x08, 0x09, 0x03, 0x8B, 0x3B, 0xF7, 0x42, 0x7E];
    const ESCAPED_FRAME: [u8; 19] = [
        0x7E, 0x06, 0x08, 0x09, 0x03, 0x05, 0x06, 0x07, 0x7D, 0x5D, 0x7D, 0x5E, 0x7F, 0xFF, 0xE6,
        0x2D, 0x17, 0xC6, 0x7E,
    ];

    fn escaped_frame() -> Frame {
        Frame {
            address: 0x010203,
            control: CONTROL_UI,
            data: vec![0x05, 0x06, 0x07, 0x7D, 0x7E, 0x7F, 0xFF],
        }
    }

    /// Decode every frame in `buf`, as the controller would.
    fn decode_all(mut buf: &[u8]) -> Vec<Result<Frame, BarBudsError>> {
        let mut frames = Vec::new();
        while let Some((consumed, result)) = decode(buf) {
            assert!(consumed > 0);
            buf = &buf[consumed..];
            if let Some(frame) = result.transpose() {
                frames.push(frame);
            }
        }
        frames
    }

    #[test]
    fn crc32_matches_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn address_varint() {
        for (value, bytes) in [
            (0x00, &[0x01][..]),
            (0x7F, &[0xFF]),
            (0x80, &[0x00, 0x03]),
            (0x01234, &[0x68, 0x49]),
            (0x87654, &[0xA8, 0xD8, 0x43]),
            (u32::MAX, &[0xFE, 0xFE, 0xFE, 0xFE, 0x1F]),
        ] {
            let mut out = Vec::new();
            encode_address(value, &mut out);
            assert_eq!(out, bytes, "encode {value:#x}");
            assert_eq!(
                decode_address(bytes).unwrap(),
                (value, bytes.len()),
                "decode {value:#x}"
            );
        }
        assert!(
            decode_address(&[0xFE, 0xFE, 0xFE, 0xFE, 0xFF]).is_err(),
            "overflow"
        );
    }

    #[test]
    fn encodes_reference_frames() {
        let empty = Frame {
            address: 0x010203,
            control: CONTROL_UI,
            data: vec![],
        };
        assert_eq!(encode(&empty), EMPTY_FRAME);
        assert_eq!(encode(&escaped_frame()), ESCAPED_FRAME);
    }

    #[test]
    fn decodes_reference_frames_including_shared_flags() {
        // Second frame reuses the first one's closing flag as its opening flag.
        let mut buf = EMPTY_FRAME.to_vec();
        buf.extend_from_slice(&ESCAPED_FRAME[1..]);
        let frames = decode_all(&buf);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].as_ref().unwrap().data, Vec::<u8>::new());
        assert_eq!(frames[1].as_ref().unwrap(), &escaped_frame());
    }

    #[test]
    fn incomplete_frame_needs_more_bytes() {
        // Once the opening flag is consumed, nothing decodes until the closing flag.
        for cut in 1..ESCAPED_FRAME.len() - 1 {
            assert!(decode(&ESCAPED_FRAME[1..cut]).is_none(), "cut at {cut}");
        }
    }

    #[test]
    fn corrupt_frame_is_skipped_and_next_frame_decodes() {
        let mut buf = ESCAPED_FRAME.to_vec();
        buf[5] ^= 0x01;
        buf.extend_from_slice(&EMPTY_FRAME);
        let frames = decode_all(&buf);
        assert!(matches!(
            frames[0],
            Err(BarBudsError::ChecksumMismatch { .. })
        ));
        assert!(frames[1].is_ok());
    }

    #[test]
    fn garbage_without_flags_is_rejected() {
        let frames = decode_all(&[0x01, 0x02, 0x03, 0x7E]);
        assert!(matches!(
            frames[..],
            [Err(BarBudsError::MalformedFrame { .. })]
        ));
    }
}
