// KG: TASK_ATOM_L0_Identity_NodeId, CONTRACT_ATOM_L0_Identity_NodeId
// NodeID = 32-byte Ed25519 public key. Displayed as base58.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NodeId(pub(crate) [u8; 32]);

impl NodeId {
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_base58(&self) -> String {
        bs58::encode(&self.0).into_string()
    }

    pub fn from_base58(s: &str) -> Result<Self, String> {
        let v = bs58::decode(s).into_vec().map_err(|e| e.to_string())?;
        if v.len() != 32 {
            return Err(format!("expected 32 bytes, got {}", v.len()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&v);
        Ok(Self(arr))
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.to_base58())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base58_roundtrip() {
        let bytes = [7u8; 32];
        let nid = NodeId::from_bytes(bytes);
        let s = nid.to_base58();
        let back = NodeId::from_base58(&s).unwrap();
        assert_eq!(nid, back);
    }

    #[test]
    fn ord_consistent_with_bytes() {
        let a = NodeId::from_bytes([0u8; 32]);
        let b = NodeId::from_bytes([1u8; 32]);
        assert!(a < b);
    }

    #[test]
    fn display_matches_base58() {
        let nid = NodeId::from_bytes([42u8; 32]);
        assert_eq!(format!("{}", nid), nid.to_base58());
    }

    #[test]
    fn serde_json_roundtrip() {
        let nid = NodeId::from_bytes([9u8; 32]);
        let j = serde_json::to_string(&nid).unwrap();
        let back: NodeId = serde_json::from_str(&j).unwrap();
        assert_eq!(nid, back);
    }

    #[test]
    fn from_base58_rejects_wrong_length() {
        assert!(NodeId::from_base58("abc").is_err());
    }
}
