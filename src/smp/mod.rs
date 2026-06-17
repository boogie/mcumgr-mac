//! The Simple Management Protocol (SMP) used by MCUmgr devices.
//!
//! An SMP message is an 8-byte header followed by a CBOR-encoded payload.
//! This module provides the framing primitives ([`Header`], [`FrameAssembler`])
//! and the group / command constants and return codes in [`groups`].

pub mod groups;
pub mod messages;

pub use groups::MgmtError;

use crate::error::{Error, Result};

/// Length of the fixed SMP header, in bytes.
pub const HEADER_LEN: usize = 8;

/// SMP operation codes (header byte 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Request to read state.
    Read = 0,
    /// Response to a read request.
    ReadRsp = 1,
    /// Request to write / modify state.
    Write = 2,
    /// Response to a write request.
    WriteRsp = 3,
}

impl Op {
    /// Convert a raw byte into an [`Op`], if it is a recognised code.
    pub fn from_u8(value: u8) -> Option<Op> {
        match value {
            0 => Some(Op::Read),
            1 => Some(Op::ReadRsp),
            2 => Some(Op::Write),
            3 => Some(Op::WriteRsp),
            _ => None,
        }
    }
}

/// A parsed SMP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Operation code.
    pub op: Op,
    /// Flags (reserved, currently always 0).
    pub flags: u8,
    /// Length of the CBOR payload that follows the header.
    pub len: u16,
    /// Management group identifier.
    pub group: u16,
    /// Sequence number used to correlate responses with requests.
    pub seq: u8,
    /// Command identifier within the group.
    pub id: u8,
}

impl Header {
    /// Serialise the header into its 8 wire bytes (length and group big-endian).
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let [len_hi, len_lo] = self.len.to_be_bytes();
        let [group_hi, group_lo] = self.group.to_be_bytes();
        [
            self.op as u8,
            self.flags,
            len_hi,
            len_lo,
            group_hi,
            group_lo,
            self.seq,
            self.id,
        ]
    }

    /// Parse the first 8 bytes of `bytes` into a [`Header`].
    ///
    /// Returns [`Error::MalformedFrame`] if fewer than 8 bytes are provided or
    /// the op code is unrecognised.
    pub fn decode(bytes: &[u8]) -> Result<Header> {
        if bytes.len() < HEADER_LEN {
            return Err(Error::MalformedFrame(format!(
                "need {HEADER_LEN} header bytes, got {}",
                bytes.len()
            )));
        }
        let op = Op::from_u8(bytes[0])
            .ok_or_else(|| Error::MalformedFrame(format!("unknown op code {}", bytes[0])))?;
        Ok(Header {
            op,
            flags: bytes[1],
            len: u16::from_be_bytes([bytes[2], bytes[3]]),
            group: u16::from_be_bytes([bytes[4], bytes[5]]),
            seq: bytes[6],
            id: bytes[7],
        })
    }
}

/// Build a complete request frame: an 8-byte header followed by `payload`.
///
/// The header's length field is set to the payload length.
pub fn encode_frame(op: Op, group: u16, seq: u8, id: u8, payload: &[u8]) -> Vec<u8> {
    let header = Header {
        op,
        flags: 0,
        len: payload.len() as u16,
        group,
        seq,
        id,
    };
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&header.encode());
    frame.extend_from_slice(payload);
    frame
}

/// Reassembles BLE notification fragments into complete SMP frames.
///
/// Notifications may split a single frame across multiple packets, or deliver
/// several frames at once. Feed raw bytes with [`push`](Self::push) and drain
/// complete frames with [`next_frame`](Self::next_frame).
#[derive(Debug, Default)]
pub struct FrameAssembler {
    buf: Vec<u8>,
}

impl FrameAssembler {
    /// Create an empty assembler.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append freshly received bytes to the internal buffer.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Remove and return the next complete frame (header + payload), if the
    /// buffer holds one. Returns `None` while the buffer holds only a partial
    /// frame.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let payload_len = u16::from_be_bytes([self.buf[2], self.buf[3]]) as usize;
        let frame_len = HEADER_LEN + payload_len;
        if self.buf.len() < frame_len {
            return None;
        }
        let rest = self.buf.split_off(frame_len);
        let frame = std::mem::replace(&mut self.buf, rest);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_roundtrips_through_u8() {
        for op in [Op::Read, Op::ReadRsp, Op::Write, Op::WriteRsp] {
            assert_eq!(Op::from_u8(op as u8), Some(op));
        }
        assert_eq!(Op::from_u8(7), None);
    }

    #[test]
    fn encodes_header_to_expected_bytes() {
        let header = Header {
            op: Op::Write,
            flags: 0,
            len: 0x0102,
            group: 0x0304,
            seq: 0x05,
            id: 0x06,
        };
        // op, flags, len_hi, len_lo, group_hi, group_lo, seq, id
        assert_eq!(header.encode(), [2, 0, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
    }

    #[test]
    fn decodes_header_roundtrip() {
        let header = Header {
            op: Op::ReadRsp,
            flags: 0,
            len: 300,
            group: 1,
            seq: 42,
            id: 5,
        };
        assert_eq!(Header::decode(&header.encode()).unwrap(), header);
    }

    #[test]
    fn decode_rejects_short_input() {
        assert!(Header::decode(&[0, 0, 0]).is_err());
    }

    #[test]
    fn encode_frame_prefixes_header_with_payload_length() {
        let frame = encode_frame(Op::Write, 1, 9, 5, &[0xaa, 0xbb, 0xcc]);
        assert_eq!(frame.len(), HEADER_LEN + 3);
        let header = Header::decode(&frame).unwrap();
        assert_eq!(header.len, 3);
        assert_eq!(header.group, 1);
        assert_eq!(header.seq, 9);
        assert_eq!(header.id, 5);
        assert_eq!(&frame[HEADER_LEN..], &[0xaa, 0xbb, 0xcc]);
    }

    fn frame_with_payload(payload: &[u8]) -> Vec<u8> {
        encode_frame(Op::WriteRsp, 1, 0, 1, payload)
    }

    #[test]
    fn assembler_yields_a_single_complete_frame() {
        let frame = frame_with_payload(&[1, 2, 3]);
        let mut asm = FrameAssembler::new();
        asm.push(&frame);
        assert_eq!(asm.next_frame(), Some(frame));
        assert_eq!(asm.next_frame(), None);
    }

    #[test]
    fn assembler_reassembles_fragments() {
        let frame = frame_with_payload(&[1, 2, 3, 4, 5]);
        let (a, b) = frame.split_at(5);
        let mut asm = FrameAssembler::new();
        asm.push(a);
        assert_eq!(asm.next_frame(), None, "incomplete frame should not yield");
        asm.push(b);
        assert_eq!(asm.next_frame(), Some(frame));
    }

    #[test]
    fn assembler_splits_two_concatenated_frames() {
        let first = frame_with_payload(&[1, 2]);
        let second = frame_with_payload(&[9, 9, 9]);
        let mut combined = first.clone();
        combined.extend_from_slice(&second);

        let mut asm = FrameAssembler::new();
        asm.push(&combined);
        assert_eq!(asm.next_frame(), Some(first));
        assert_eq!(asm.next_frame(), Some(second));
        assert_eq!(asm.next_frame(), None);
    }
}
