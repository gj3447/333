// KG: SPAN_333_L2_Kademlia, ATOM_L2_Kademlia_Distance
// XOR distance metric over 256-bit NodeId.

use identity333::NodeId;
use std::cmp::Ordering;

/// 256-bit XOR distance between two NodeIds. Bigger bytes first = big-endian ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Distance(pub [u8; 32]);

impl Distance {
    /// XOR distance `a ⊕ b`. Symmetric: `dist(a,b) == dist(b,a)`.
    pub fn between(a: &NodeId, b: &NodeId) -> Self {
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = a.as_bytes()[i] ^ b.as_bytes()[i];
        }
        Self(out)
    }

    /// Count leading zero bits. Used as k-bucket index.
    /// `leading_zeros() == 256` iff distance is zero (same NodeId).
    pub fn leading_zeros(&self) -> u32 {
        let mut total = 0u32;
        for b in self.0.iter() {
            if *b == 0 {
                total += 8;
            } else {
                total += b.leading_zeros();
                break;
            }
        }
        total
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 32]
    }
}

impl PartialOrd for Distance {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Distance {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nid(b: u8) -> NodeId {
        NodeId::from_bytes([b; 32])
    }

    #[test]
    fn self_distance_is_zero() {
        let a = nid(7);
        assert!(Distance::between(&a, &a).is_zero());
        assert_eq!(Distance::between(&a, &a).leading_zeros(), 256);
    }

    #[test]
    fn distance_symmetric() {
        let a = nid(0x10);
        let b = nid(0x20);
        assert_eq!(Distance::between(&a, &b), Distance::between(&b, &a));
    }

    #[test]
    fn distance_triangle_ordered() {
        // a = 0x00, b = 0x01, c = 0x80
        let a = nid(0x00);
        let b = nid(0x01);
        let c = nid(0x80);
        let d_ab = Distance::between(&a, &b);
        let d_ac = Distance::between(&a, &c);
        // b is closer to a than c is (XOR 0x01 vs 0x80 first byte).
        assert!(d_ab < d_ac);
    }

    #[test]
    fn leading_zeros_bucket_index() {
        let a = nid(0x00);
        // distance to nid(0x80 in first byte, rest 0): 0x80... → leading zeros = 0
        let mut bytes = [0u8; 32];
        bytes[0] = 0x80;
        let b = NodeId::from_bytes(bytes);
        assert_eq!(Distance::between(&a, &b).leading_zeros(), 0);

        // distance to nid(0x01 in first byte): 0x01... → leading zeros = 7
        bytes[0] = 0x01;
        let c = NodeId::from_bytes(bytes);
        assert_eq!(Distance::between(&a, &c).leading_zeros(), 7);
    }
}
