//! Luci frame codec — a 10-byte header followed by a UTF-8 payload.
//!
//! ```text
//! byte 0..1  remoteID (u16, always 0)
//! byte 2     commandType (1 = READ, 2 = WRITE)
//! byte 3..4  command
//! byte 5     status
//! byte 6..7  CRC (0 on send)
//! byte 8..9  dataLen (payload length)
//! ```
//!
//! The one trap: **requests write `command` and `dataLen` little-endian**
//! (the SDK's `LuciPacketConstructor`), but **responses carry them big-endian**
//! (`processLuciData` reads `payload[3] << 8 | payload[4]`). [`encode`] and
//! [`decode_frame`] account for that asymmetry.
//!
//! A single TLS read can contain several concatenated frames, so decoding works
//! off a running buffer: a frame is only consumed once ≥ `10 + dataLen` bytes
//! are present.

/// Fixed header length preceding every payload.
pub const HEADER_LEN: usize = 10;

/// A decoded Luci frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Command id (`MessageBox`), read big-endian from a response.
    pub command: u16,
    /// 1 = READ, 2 = WRITE (byte 2, endian-agnostic).
    pub command_type: u8,
    /// Response status; `1` = OK.
    pub status: u8,
    /// UTF-8 payload (lossy — a malformed byte never fails the whole read).
    pub payload: String,
}

/// Encode a **request** frame: `command` and `dataLen` little-endian, status and
/// CRC zeroed.
pub fn encode(command: u16, command_type: u8, payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let len = bytes.len() as u16;
    let mut out = Vec::with_capacity(HEADER_LEN + bytes.len());
    out.extend_from_slice(&[0, 0]); // remoteID
    out.push(command_type);
    out.push((command & 0xff) as u8); // command LE
    out.push((command >> 8) as u8);
    out.push(0); // status
    out.extend_from_slice(&[0, 0]); // CRC
    out.push((len & 0xff) as u8); // dataLen LE
    out.push((len >> 8) as u8);
    out.extend_from_slice(bytes);
    out
}

/// Decode one **response** frame from the front of `buf`: `command` and `dataLen`
/// big-endian. Returns the frame and the number of bytes consumed, or `None`
/// when `buf` does not yet hold a full frame.
pub fn decode_frame(buf: &[u8]) -> Option<(Frame, usize)> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let command = ((buf[3] as u16) << 8) | buf[4] as u16;
    let command_type = buf[2];
    let status = buf[5];
    let len = (((buf[8] as u16) << 8) | buf[9] as u16) as usize;
    let total = HEADER_LEN + len;
    if buf.len() < total {
        return None;
    }
    let payload = String::from_utf8_lossy(&buf[HEADER_LEN..total]).into_owned();
    Some((
        Frame { command, command_type, status, payload },
        total,
    ))
}

/// Drain every complete frame from the front of `buf`, leaving any trailing
/// partial frame in place for the next read.
pub fn drain_frames(buf: &mut Vec<u8>) -> Vec<Frame> {
    let mut frames = Vec::new();
    while let Some((frame, consumed)) = decode_frame(buf) {
        frames.push(frame);
        buf.drain(..consumed);
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a **response**-shaped frame (big-endian command/len) for tests —
    /// the device's wire format, the mirror image of [`encode`].
    fn encode_response(command: u16, command_type: u8, status: u8, payload: &str) -> Vec<u8> {
        let bytes = payload.as_bytes();
        let len = bytes.len() as u16;
        let mut out = vec![0, 0, command_type, (command >> 8) as u8, (command & 0xff) as u8, status, 0, 0, (len >> 8) as u8, (len & 0xff) as u8];
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn request_header_is_little_endian() {
        // VOLUME = 64 (0x40), READ, empty payload.
        let bytes = encode(64, 1, "");
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(bytes[2], 1, "commandType");
        assert_eq!(bytes[3], 0x40, "command low byte");
        assert_eq!(bytes[4], 0x00, "command high byte");
        assert_eq!(&bytes[8..10], &[0, 0], "dataLen 0");

        // DevInfo = 92 (0x5C) with a 3-byte payload.
        let bytes = encode(92, 2, "abc");
        assert_eq!(bytes[3], 0x5C);
        assert_eq!(bytes[4], 0x00);
        assert_eq!(&bytes[8..10], &[3, 0], "dataLen 3 LE");
        assert_eq!(&bytes[HEADER_LEN..], b"abc");
    }

    #[test]
    fn decodes_a_big_endian_response() {
        let wire = encode_response(64, 1, 1, "30");
        let (frame, consumed) = decode_frame(&wire).expect("full frame");
        assert_eq!(consumed, wire.len());
        assert_eq!(frame.command, 64);
        assert_eq!(frame.status, 1);
        assert_eq!(frame.payload, "30");
    }

    #[test]
    fn decodes_the_documented_devinfo_response() {
        // The live DevInfo(92) JSON from docs/firmware-notes.md §B.
        let json = r#"{"macaddress":{"bt":"F4:AB:5C:FC:A8:2F","eth0":"00:E0:3A:00:0A:8A","wlan0":"D8:F7:10:71:86:28"},"serialnumber":{"device_serialnumber":"RKARYLLP102625004937"},"versioninfo":{"devicefwversion":"AR241CE_9243.16.2","mcuversion":"16"}}"#;
        let wire = encode_response(92, 1, 1, json);
        let (frame, consumed) = decode_frame(&wire).expect("full frame");
        assert_eq!(consumed, wire.len());
        assert_eq!(frame.command, 92);
        assert_eq!(frame.status, 1);
        assert_eq!(frame.payload, json);
    }

    #[test]
    fn waits_for_a_partial_frame() {
        let wire = encode_response(92, 1, 1, "hello");
        // One byte short of the payload → not yet a frame.
        assert!(decode_frame(&wire[..wire.len() - 1]).is_none());
        // Fewer than a header → not a frame.
        assert!(decode_frame(&wire[..5]).is_none());
    }

    #[test]
    fn drains_multiple_concatenated_frames() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&encode_response(64, 1, 1, "30"));
        buf.extend_from_slice(&encode_response(51, 1, 1, "1"));
        // A trailing partial frame stays buffered.
        let partial = encode_response(50, 1, 1, "10");
        buf.extend_from_slice(&partial[..partial.len() - 1]);

        let frames = drain_frames(&mut buf);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].command, 64);
        assert_eq!(frames[0].payload, "30");
        assert_eq!(frames[1].command, 51);
        assert_eq!(frames[1].payload, "1");
        assert_eq!(buf.len(), partial.len() - 1, "partial frame retained");
    }

    #[test]
    fn round_trips_an_empty_payload_response() {
        let wire = encode_response(3, 2, 1, "");
        let (frame, consumed) = decode_frame(&wire).expect("full frame");
        assert_eq!(consumed, HEADER_LEN);
        assert_eq!(frame.command, 3);
        assert!(frame.payload.is_empty());
    }
}
