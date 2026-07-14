// KG: CONTRACT_333_WireProtocol, ATOM_Web_WireProtocol
// KG: plan-333-longinus-full-coverage-2026-04-14 — src-wire-MsgType/WireHeader/WireMessage/WireError/encode/DecodeResult/decode/message_size
// Binary message protocol: version(1) + type(1) + length(2) + payload
// Contract: encode→decode roundtrip. unknown version→ignore. unknown type→skip.

/// Protocol version (for forward compatibility)
pub const WIRE_VERSION: u8 = 1;

/// Maximum payload size (64KB)
pub const MAX_PAYLOAD: usize = 65535;

/// Message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    /// CRDT state delta
    StateUpdate = 0x01,
    /// Full state sync (for new peers)
    StateFull = 0x02,
    /// Presence (cursor, status)
    Presence = 0x03,
    /// BFT consensus message
    Consensus = 0x04,
    /// Room management (join/leave)
    RoomControl = 0x05,
    /// Heartbeat / keepalive
    Heartbeat = 0x06,
    /// Application-specific message
    AppMessage = 0x10,
}

impl MsgType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::StateUpdate),
            0x02 => Some(Self::StateFull),
            0x03 => Some(Self::Presence),
            0x04 => Some(Self::Consensus),
            0x05 => Some(Self::RoomControl),
            0x06 => Some(Self::Heartbeat),
            0x10 => Some(Self::AppMessage),
            _ => None,
        }
    }
}

/// Wire message header (4 bytes)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireHeader {
    pub version: u8,
    pub msg_type: u8,
    pub length: u16,
}

/// Complete wire message
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireMessage {
    pub header: WireHeader,
    pub payload: Vec<u8>,
}

/// Encode error
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    PayloadTooLarge,
    BufferTooShort,
    UnknownVersion(u8),
    InvalidLength,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::PayloadTooLarge => write!(f, "payload exceeds 64KB"),
            Self::BufferTooShort => write!(f, "buffer too short for header"),
            Self::UnknownVersion(v) => write!(f, "unknown wire version: {}", v),
            Self::InvalidLength => write!(f, "declared length exceeds buffer"),
        }
    }
}

/// Encode a message into bytes
pub fn encode(msg_type: MsgType, payload: &[u8]) -> Result<Vec<u8>, WireError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(WireError::PayloadTooLarge);
    }

    let len = payload.len() as u16;
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.push(WIRE_VERSION);
    buf.push(msg_type as u8);
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
    Ok(buf)
}

/// Decode result
pub enum DecodeResult {
    /// Successfully decoded message
    Ok(WireMessage),
    /// Unknown version — skip this message (forward compat)
    SkipVersion(u8),
    /// Unknown message type — skip (forward compat)
    SkipType(u8),
    /// Error
    Err(WireError),
}

/// Decode bytes into a message
pub fn decode(buf: &[u8]) -> DecodeResult {
    if buf.len() < 4 {
        return DecodeResult::Err(WireError::BufferTooShort);
    }

    let version = buf[0];
    let msg_type_raw = buf[1];
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;

    // Forward compatibility: unknown version → skip
    if version != WIRE_VERSION {
        return DecodeResult::SkipVersion(version);
    }

    // Forward compatibility: unknown type → skip
    if MsgType::from_u8(msg_type_raw).is_none() {
        return DecodeResult::SkipType(msg_type_raw);
    }

    if buf.len() < 4 + length {
        return DecodeResult::Err(WireError::InvalidLength);
    }

    let payload = buf[4..4 + length].to_vec();

    DecodeResult::Ok(WireMessage {
        header: WireHeader {
            version,
            msg_type: msg_type_raw,
            length: length as u16,
        },
        payload,
    })
}

/// Calculate total message size from buffer (for framing)
pub fn message_size(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let length = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    Some(4 + length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let payload = b"hello world";
        let encoded = encode(MsgType::StateUpdate, payload).unwrap();
        match decode(&encoded) {
            DecodeResult::Ok(msg) => {
                assert_eq!(msg.header.version, WIRE_VERSION);
                assert_eq!(msg.header.msg_type, MsgType::StateUpdate as u8);
                assert_eq!(msg.payload, payload);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn all_message_types() {
        let types = [
            MsgType::StateUpdate,
            MsgType::StateFull,
            MsgType::Presence,
            MsgType::Consensus,
            MsgType::RoomControl,
            MsgType::Heartbeat,
            MsgType::AppMessage,
        ];
        for mt in types {
            let encoded = encode(mt, b"test").unwrap();
            match decode(&encoded) {
                DecodeResult::Ok(msg) => assert_eq!(msg.header.msg_type, mt as u8),
                _ => panic!("failed for {:?}", mt),
            }
        }
    }

    #[test]
    fn unknown_version_skipped() {
        let mut buf = encode(MsgType::StateUpdate, b"data").unwrap();
        buf[0] = 99; // unknown version
        match decode(&buf) {
            DecodeResult::SkipVersion(99) => {} // expected
            _ => panic!("expected SkipVersion"),
        }
    }

    #[test]
    fn unknown_type_skipped() {
        let mut buf = encode(MsgType::StateUpdate, b"data").unwrap();
        buf[1] = 0xFF; // unknown type
        match decode(&buf) {
            DecodeResult::SkipType(0xFF) => {} // expected
            _ => panic!("expected SkipType"),
        }
    }

    #[test]
    fn payload_too_large() {
        let big = vec![0u8; MAX_PAYLOAD + 1];
        assert_eq!(encode(MsgType::StateUpdate, &big), Err(WireError::PayloadTooLarge));
    }

    #[test]
    fn buffer_too_short() {
        match decode(&[0x01, 0x01]) {
            DecodeResult::Err(WireError::BufferTooShort) => {}
            _ => panic!("expected BufferTooShort"),
        }
    }

    #[test]
    fn empty_payload() {
        let encoded = encode(MsgType::Heartbeat, &[]).unwrap();
        assert_eq!(encoded.len(), 4); // header only
        match decode(&encoded) {
            DecodeResult::Ok(msg) => {
                assert!(msg.payload.is_empty());
                assert_eq!(msg.header.length, 0);
            }
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn message_size_calculation() {
        let encoded = encode(MsgType::Presence, b"cursor data here").unwrap();
        assert_eq!(message_size(&encoded), Some(encoded.len()));
    }
}
