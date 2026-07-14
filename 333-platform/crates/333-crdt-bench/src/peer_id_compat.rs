// KG: seed-rts-conflict-loro-vs-custom-crdt-2026-04-15
//! Loro PeerID(u64) ↔ 333 BFT PeerId([u8;32]) compatibility.
//!
//! Mapping strategies:
//! A) Truncate PeerId[0..8] → u64: REJECT — adversarial collision
//! B) blake3(PeerId) → u64:       OK for attribution only; side-table required
//! C) Separate namespaces:        RECOMMENDED — Loro u64 = label, BFT [u8;32] = key
//! D) Sequential counter/session: VIABLE for ephemeral sessions

/// Convert 333 BFT PeerId to a Loro-compatible u64 (Strategy B/C hybrid).
/// NOT a cryptographic identity — CRDT authorship attribution only.
/// Caller must maintain side-table: HashMap<u64, [u8;32]>.
#[allow(dead_code)] // used in tests; bin target doesn't see cfg(test) usage
pub(crate) fn bft_peer_to_loro_u64(bft_peer: &[u8; 32]) -> u64 {
    let hash = fnv_first8(bft_peer);
    u64::from_le_bytes(hash)
}

#[allow(dead_code)] // used in tests; bin target doesn't see cfg(test) usage
pub(crate) fn loro_u64_is_reversible() -> bool { false }

/// FNV-1a over 32 bytes → 8-byte output (stand-in for blake3 — no dep needed here)
/// Production: use blake3::hash(data).as_bytes()[..8].try_into().unwrap()
fn fnv_first8(data: &[u8; 32]) -> [u8; 8] {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data.iter() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
    h.to_le_bytes()
}

pub struct CompatRisk {
    pub strategy: &'static str,
    pub collision_risk: &'static str,
    pub reversible: bool,
    pub bft_auth_preserved: bool,
    pub recommendation: &'static str,
}

pub const RISKS: &[CompatRisk] = &[
    CompatRisk {
        strategy: "Truncate(PeerId[0..8] as u64)",
        collision_risk: "HIGH for adversarial keys",
        reversible: false, bft_auth_preserved: false,
        recommendation: "REJECT — spoofable",
    },
    CompatRisk {
        strategy: "blake3(PeerId) → u64",
        collision_risk: "LOW (64-bit pre-image)",
        reversible: false, bft_auth_preserved: false,
        recommendation: "OK for attribution only; side-table required",
    },
    CompatRisk {
        strategy: "Separate namespaces (Loro u64 + BFT [u8;32])",
        collision_risk: "NONE — different layers",
        reversible: false, bft_auth_preserved: true,
        recommendation: "RECOMMENDED — dual-layer identity",
    },
    CompatRisk {
        strategy: "Sequential counter per session",
        collision_risk: "LOW within session",
        reversible: false, bft_auth_preserved: true,
        recommendation: "VIABLE for ephemeral sessions",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn deterministic() { let p = [1u8;32]; assert_eq!(bft_peer_to_loro_u64(&p), bft_peer_to_loro_u64(&p)); }
    #[test] fn different_peers() { assert_ne!(bft_peer_to_loro_u64(&[1u8;32]), bft_peer_to_loro_u64(&[2u8;32])); }
    #[test] fn not_reversible() { assert!(!loro_u64_is_reversible()); }
}
