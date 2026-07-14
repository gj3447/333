// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
//! Deterministic RNG — PCG32 seeded per frame.
//!
//! Each frame gets a fully deterministic, reproducible random sequence by
//! mixing (global_seed XOR frame_n) through the PCG32 state initialization.
//! The same global_seed + frame_n pair will always produce the same sequence
//! regardless of platform, OS, or runtime.
//!
//! PCG32: O'Neill, "PCG: A Family of Simple Fast Space-Efficient Statistically
//! Good Algorithms for Random Number Generation" (2014).

use crate::determinism::fixed_point::Fixed32;

/// PCG32-based deterministic RNG, seeded per frame.
/// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
#[derive(Debug, Clone)]
pub struct DeterministicRng {
    /// PCG32 internal state
    pub(crate) state: u64,
    /// PCG32 stream (must be odd)
    pub(crate) inc: u64,
}

/// Magic constants for PCG32.
const PCG_MULTIPLIER: u64 = 6_364_136_223_846_793_005;

impl DeterministicRng {
    /// Create a new RNG with explicit state and stream.
    /// Both arguments should typically come from `seed_for_frame`.
    pub fn new(state: u64, stream: u64) -> Self {
        let inc = (stream << 1) | 1; // ensure odd
        let mut rng = Self { state: 0, inc };
        // Warm up: advance once before first use (PCG spec)
        rng.state = rng.state.wrapping_add(inc);
        rng.state = pcg32_step(rng.state, rng.inc);
        // Mix in the desired initial state
        rng.state = rng.state.wrapping_add(state);
        rng.state = pcg32_step(rng.state, rng.inc);
        rng
    }

    /// Canonical constructor: derive per-frame RNG from global seed and frame number.
    ///
    /// Uses a xorshift64 mix to spread (global_seed XOR frame_n) into distinct
    /// PCG state and stream seeds, ensuring no correlation between adjacent frames.
    ///
    /// # Example
    /// ```
    /// use triple_three::determinism::rng::DeterministicRng;
    /// let rng = DeterministicRng::seed_for_frame(0xDEADBEEF_CAFEF00D, 42);
    /// ```
    pub fn seed_for_frame(global_seed: u64, frame_n: u64) -> Self {
        // Mix to get two independent 64-bit seeds
        let mixed = xorshift64_mix(global_seed ^ frame_n.wrapping_mul(0x9E3779B97F4A7C15));
        let stream = xorshift64_mix(mixed ^ 0x6C62272E07BB0142); // arbitrary salt
        Self::new(mixed, stream)
    }

    /// Advance RNG and return next u32.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let old_state = self.state;
        self.state = pcg32_step(old_state, self.inc);
        pcg32_output(old_state)
    }

    /// Advance RNG and return next u64 (two consecutive u32 values combined).
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let lo = self.next_u32() as u64;
        let hi = self.next_u32() as u64;
        (hi << 32) | lo
    }

    /// Return a random Fixed32 in [0, 1) via bridge from u32.
    /// Deterministic: no float operations involved in the random bit generation.
    /// The u32 maps to Q16.16 by taking the upper 16 bits as the raw fractional value.
    pub fn next_fixed_unit(&mut self) -> Fixed32 {
        // Take upper 16 bits of u32 as Q16.16 fractional part (integer part = 0)
        let bits = self.next_u32();
        let frac = (bits >> 16) as i32; // 0..=65535, fractional bits of Q16.16
        Fixed32::from_raw(frac)
    }

    /// Return a random Fixed32 in [min, max).
    pub fn next_fixed_range(&mut self, min: Fixed32, max: Fixed32) -> Fixed32 {
        let unit = self.next_fixed_unit();
        let range = max - min;
        min + (range * unit)
    }

    /// Return a random u32 in [0, bound) using rejection sampling (unbiased).
    pub fn next_u32_below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            return 0;
        }
        // Rejection sampling for uniform distribution
        let threshold = bound.wrapping_neg() % bound;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % bound;
            }
        }
    }
}

/// PCG32 advance step: state = state * multiplier + inc
#[inline(always)]
fn pcg32_step(state: u64, inc: u64) -> u64 {
    state.wrapping_mul(PCG_MULTIPLIER).wrapping_add(inc)
}

/// PCG32 output function: XSH-RR (xorshift high bits, random rotate)
#[inline(always)]
fn pcg32_output(state: u64) -> u32 {
    let xorshifted = (((state >> 18) ^ state) >> 27) as u32;
    let rot = (state >> 59) as u32;
    xorshifted.rotate_right(rot)
}

/// xorshift64 mixing function for seeding.
#[inline(always)]
fn xorshift64_mix(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    // splitmix finalizer for extra diffusion
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1: Same seed + frame → same sequence (reproducibility)
    #[test]
    fn rng_seed_reproducibility() {
        let mut rng1 = DeterministicRng::seed_for_frame(0xCAFE_BABE_1234_5678, 100);
        let mut rng2 = DeterministicRng::seed_for_frame(0xCAFE_BABE_1234_5678, 100);

        for _ in 0..16 {
            assert_eq!(rng1.next_u32(), rng2.next_u32(), "same seed+frame must produce identical values");
        }
    }

    /// Test 2: Different frames → different sequences
    #[test]
    fn rng_different_frames_differ() {
        let mut rng_f0 = DeterministicRng::seed_for_frame(0xDEAD_BEEF, 0);
        let mut rng_f1 = DeterministicRng::seed_for_frame(0xDEAD_BEEF, 1);
        // First few values should differ (with overwhelming probability)
        let vals0: Vec<u32> = (0..4).map(|_| rng_f0.next_u32()).collect();
        let vals1: Vec<u32> = (0..4).map(|_| rng_f1.next_u32()).collect();
        assert_ne!(vals0, vals1, "different frames must produce different sequences");
    }

    /// Test 3: next_u64 is reproducible
    #[test]
    fn rng_u64_reproducibility() {
        let mut rng1 = DeterministicRng::seed_for_frame(42, 7);
        let mut rng2 = DeterministicRng::seed_for_frame(42, 7);
        assert_eq!(rng1.next_u64(), rng2.next_u64());
    }

    /// Test 4: next_fixed_unit stays in [0, 1)
    #[test]
    fn rng_fixed_unit_range() {
        let mut rng = DeterministicRng::seed_for_frame(0x1337, 0);
        for _ in 0..1000 {
            let v = rng.next_fixed_unit();
            assert!(v.0 >= 0, "fixed unit must be >= 0");
            assert!(v.0 < Fixed32::ONE.0, "fixed unit must be < 1.0 (raw={})", v.0);
        }
    }

    /// Test 5: next_u32_below stays in [0, bound)
    #[test]
    fn rng_u32_below_bound() {
        let mut rng = DeterministicRng::seed_for_frame(0xABCD, 99);
        for _ in 0..1000 {
            let v = rng.next_u32_below(100);
            assert!(v < 100, "value {} must be < 100", v);
        }
    }
}
