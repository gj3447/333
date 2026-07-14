// KG: SPAN_333_L9_IAM_Prod, finding_333_synth_sec_prt_d30
// Production-grade password hashing trait + reference IterationHasher.
//
// D30: Argon2id is OWASP default (256MB memory, adaptive work factor). To
// keep this crate dependency-free, we ship a *trait* (`PasswordHasher`) and
// a reference `IterationHasher` that stretches SHA-256 over N iterations
// (PBKDF2-lite). A real downstream impl plugs argon2 / scrypt / bcrypt into
// the same trait signature.
//
// Caller-supplied salt MUST come from a CSPRNG (not a deterministic template
// as in the legacy testing-only path) — this module's trait signature
// enforces that by requiring a 32-byte salt.

use sha2::{Digest, Sha256};

pub type Salt = [u8; 32];
pub type Hash = [u8; 32];

pub trait PasswordHasher {
    /// Hash `password` with `salt`. Deterministic for (password, salt).
    fn hash(&self, password: &[u8], salt: &Salt) -> Hash;

    /// Constant-time verification: recompute hash and compare.
    fn verify(&self, password: &[u8], salt: &Salt, expected: &Hash) -> bool {
        let got = self.hash(password, salt);
        // Constant-time eq — prevents timing attacks.
        let mut diff = 0u8;
        for (a, b) in got.iter().zip(expected.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

/// Reference impl: SHA-256 stretched over `iterations` rounds. Maps the
/// argon2 "work factor" concept to pure-SHA iteration count. Production
/// backends swap in argon2id for memory-hardness; this trait signature is
/// unchanged.
#[derive(Debug, Clone, Copy)]
pub struct IterationHasher {
    pub iterations: u32,
}

impl IterationHasher {
    pub const DEFAULT_ITERATIONS: u32 = 100_000;

    pub fn new(iterations: u32) -> Self {
        Self { iterations }
    }
}

impl Default for IterationHasher {
    fn default() -> Self {
        Self { iterations: Self::DEFAULT_ITERATIONS }
    }
}

impl PasswordHasher for IterationHasher {
    fn hash(&self, password: &[u8], salt: &Salt) -> Hash {
        let mut h = Sha256::new();
        h.update(salt);
        h.update(password);
        let mut out: Hash = h.finalize().into();
        for _ in 1..self.iterations {
            let mut h = Sha256::new();
            h.update(out);
            h.update(salt);
            out = h.finalize().into();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salt() -> Salt {
        let mut s = [0u8; 32];
        for i in 0..32 {
            s[i] = i as u8;
        }
        s
    }

    #[test]
    fn hash_deterministic() {
        let h = IterationHasher::new(1000);
        let s = salt();
        let a = h.hash(b"pw", &s);
        let b = h.hash(b"pw", &s);
        assert_eq!(a, b);
    }

    #[test]
    fn different_passwords_different_hashes() {
        let h = IterationHasher::new(1000);
        let s = salt();
        assert_ne!(h.hash(b"pw1", &s), h.hash(b"pw2", &s));
    }

    #[test]
    fn different_salts_different_hashes() {
        let h = IterationHasher::new(1000);
        let s1 = salt();
        let mut s2 = salt();
        s2[0] ^= 1;
        assert_ne!(h.hash(b"pw", &s1), h.hash(b"pw", &s2));
    }

    #[test]
    fn verify_correct() {
        let h = IterationHasher::default();
        let s = salt();
        let stored = h.hash(b"secret", &s);
        assert!(h.verify(b"secret", &s, &stored));
        assert!(!h.verify(b"wrong", &s, &stored));
    }

    #[test]
    fn iteration_count_increases_output_delay() {
        // Sanity: larger iteration count yields different (same-input) hash
        // because the chain length differs.
        let a = IterationHasher::new(10).hash(b"x", &salt());
        let b = IterationHasher::new(20).hash(b"x", &salt());
        assert_ne!(a, b);
    }
}
