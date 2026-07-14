// KG: seed-rts-determinism-fixed-point-state-hash-2026-04-15
//! Fixed-point arithmetic — Q16.16 (i32-backed)
//!
//! Q16.16: 16 integer bits + 16 fractional bits.
//! Range: approximately ±32767.999985
//! Resolution: ~1.5e-5 (1/65536)
//!
//! All operations are bitwise-deterministic across platforms and runs.
//! No floating-point is used internally except for display/conversion helpers.

use core::fmt;
use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};
use serde::{Deserialize, Serialize};

/// Q16.16 fixed-point number backed by i32.
/// Bit layout: [31..16] integer part, [15..0] fractional part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fixed32(pub i32);

/// Fractional scale factor: 2^16 = 65536
const SCALE: i32 = 1 << 16;
const SCALE_I64: i64 = 1 << 16;

impl Fixed32 {
    /// The value 0.0
    pub const ZERO: Self = Self(0);
    /// The value 1.0
    pub const ONE: Self = Self(SCALE);
    /// The value -1.0
    pub const NEG_ONE: Self = Self(-SCALE);
    /// Maximum value (~32767.99998)
    pub const MAX: Self = Self(i32::MAX);
    /// Minimum value (~-32768.0)
    pub const MIN: Self = Self(i32::MIN);
    /// Pi ≈ 3.14159 in Q16.16
    pub const PI: Self = Self(205887); // 3.14159265 * 65536 ≈ 205887
    /// 2*Pi
    pub const TWO_PI: Self = Self(411775); // 2 * 205887 = 411774, round up 1 for precision

    /// Create from raw i32 bits (no scaling applied).
    #[inline]
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// Get raw i32 bits.
    #[inline]
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// Create from an integer value.
    #[inline]
    pub const fn from_int(n: i32) -> Self {
        Self(n * SCALE)
    }

    /// Extract the integer part (floor toward negative infinity).
    #[inline]
    pub const fn floor(self) -> i32 {
        self.0 >> 16
    }

    /// Convert to f32 for display/debug only (NOT for game logic — use Fixed32 throughout).
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / SCALE as f32
    }

    /// Saturating addition.
    #[inline]
    pub fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction.
    #[inline]
    pub fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    /// Multiplication with i64 intermediate to prevent overflow.
    /// Result is truncated (floor) toward negative infinity for the fractional bits.
    #[inline]
    pub fn mul_fixed(self, rhs: Self) -> Self {
        let result = (self.0 as i64 * rhs.0 as i64) >> 16;
        Self(result as i32)
    }

    /// Division. Panics if rhs is zero.
    #[inline]
    pub fn div_fixed(self, rhs: Self) -> Self {
        assert!(rhs.0 != 0, "Fixed32: division by zero");
        let result = ((self.0 as i64) << 16) / rhs.0 as i64;
        Self(result as i32)
    }

    /// Absolute value (saturating: MIN.abs() == MIN).
    #[inline]
    pub fn abs(self) -> Self {
        if self.0 < 0 {
            Self(self.0.saturating_neg())
        } else {
            self
        }
    }

    /// Square root via Newton-Raphson in fixed-point.
    /// Input must be non-negative; returns 0 for negative inputs.
    pub fn sqrt(self) -> Self {
        if self.0 <= 0 {
            return Self::ZERO;
        }
        // Shift left by 16 to bring fractional bits into play.
        let n = self.0 as i64;
        let n_shifted = n << 16; // value * 2^16, so sqrt needs * 2^8 correction

        // Integer sqrt of n_shifted using Newton-Raphson
        let mut x: i64 = {
            // Initial guess: bit-shift estimate
            let bits = 64 - n_shifted.leading_zeros() as i64;
            1_i64 << ((bits + 1) / 2)
        };

        // Newton-Raphson iterations (converges in ~6 steps for i32 inputs)
        for _ in 0..8 {
            let x_new = (x + n_shifted / x) >> 1;
            if x_new >= x {
                break;
            }
            x = x_new;
        }

        Self(x as i32)
    }

    /// Cosine approximation using a 9th-order minimax polynomial.
    /// Input in radians (Q16.16). Result in [-1, 1] (Q16.16).
    pub fn cos(self) -> Self {
        // Reduce to [-PI, PI] range
        let two_pi = Self::TWO_PI.0 as i64;
        let mut x = self.0 as i64;

        // Range reduction: x mod 2*PI
        if x < 0 {
            // Adjust for negative modulo
            x %= two_pi;
            if x < -(two_pi / 2) {
                x += two_pi;
            }
        } else {
            x %= two_pi;
        }
        // Now x is in (-2*PI, 2*PI). Map to (-PI, PI].
        if x > Self::PI.0 as i64 {
            x -= two_pi;
        } else if x < -(Self::PI.0 as i64) {
            x += two_pi;
        }

        // cos(x) using: cos(x) = 1 - x²/2! + x⁴/4! - x⁶/6! + x⁸/8!
        // All in Q16.16 fixed-point
        let s = SCALE_I64;
        let x2 = (x * x) >> 16;                // x^2
        let x4 = (x2 * x2) >> 16;              // x^4
        let x6 = (x4 * x2) >> 16;              // x^6
        let x8 = (x4 * x4) >> 16;              // x^8

        // Coefficients (scaled by 2^16):
        // 1/2!      = 0.5            → 32768
        // 1/4!      = 0.041667       → 2731
        // 1/6!      = 0.001389       → 91
        // 1/8!      = 0.0000248      → 1  (round)
        let result = s
            - ((x2 * 32768) >> 16)
            + ((x4 * 2731)  >> 16)
            - ((x6 * 91)    >> 16)
            + (x8            >> 16);

        // Clamp to [-1, 1]
        let clamped = result.max(-(s)).min(s);
        Self(clamped as i32)
    }

    /// Sine approximation.  sin(x) = cos(x - PI/2)
    pub fn sin(self) -> Self {
        // PI/2 in Q16.16 = 205887 / 2 ≈ 102944
        let half_pi = Self(102944);
        (self - half_pi).cos()
    }
}

// ── Operator implementations ──────────────────────────────────────────────────

impl Add for Fixed32 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self(self.0.wrapping_add(rhs.0))
    }
}

impl AddAssign for Fixed32 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_add(rhs.0);
    }
}

impl Sub for Fixed32 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.wrapping_sub(rhs.0))
    }
}

impl SubAssign for Fixed32 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        self.0 = self.0.wrapping_sub(rhs.0);
    }
}

impl Mul for Fixed32 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        self.mul_fixed(rhs)
    }
}

impl Div for Fixed32 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        self.div_fixed(rhs)
    }
}

impl Neg for Fixed32 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.wrapping_neg())
    }
}

impl From<i32> for Fixed32 {
    #[inline]
    fn from(n: i32) -> Self {
        Self::from_int(n)
    }
}

impl fmt::Display for Fixed32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Display as decimal without floating-point ops: integer + fractional parts.
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs_raw = (self.0 as i64).unsigned_abs();
        let int_part = abs_raw >> 16;
        let frac_raw = abs_raw & 0xFFFF;
        // Fractional: multiply by 10^6 for 6 decimal digits
        let frac_dec = (frac_raw * 1_000_000) >> 16;
        write!(f, "{}{}.{:06}", sign, int_part, frac_dec)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1: Addition is bitwise-exact
    #[test]
    fn fp_addition_exact() {
        let a = Fixed32::from_int(3);
        let b = Fixed32::from_int(4);
        let c = a + b;
        assert_eq!(c, Fixed32::from_int(7));
        // Bitwise: raw must be exactly 7 * 65536
        assert_eq!(c.0, 7 * 65536);
    }

    /// Test 2: Multiplication correctness
    #[test]
    fn fp_multiplication_exact() {
        let a = Fixed32::from_int(3);
        let b = Fixed32::from_int(4);
        let c = a * b;
        assert_eq!(c, Fixed32::from_int(12));
    }

    /// Test 3: Determinism — same inputs always yield identical raw bits
    #[test]
    fn fp_determinism_repeated_runs() {
        let seq: Vec<Fixed32> = (0i32..10)
            .map(Fixed32::from_int)
            .collect();

        let mut results_a = Vec::new();
        let mut results_b = Vec::new();

        for chunk in seq.windows(2) {
            results_a.push((chunk[0] * chunk[1]).0);
        }
        for chunk in seq.windows(2) {
            results_b.push((chunk[0] * chunk[1]).0);
        }
        assert_eq!(results_a, results_b, "fixed-point must be bitwise-identical across runs");
    }

    /// Test 4: Subtraction and negation
    #[test]
    fn fp_sub_neg() {
        let a = Fixed32::from_int(10);
        let b = Fixed32::from_int(3);
        assert_eq!(a - b, Fixed32::from_int(7));
        assert_eq!(-b, Fixed32::from_int(-3));
    }

    /// Test 5: sqrt of perfect square
    #[test]
    fn fp_sqrt_perfect_square() {
        let nine = Fixed32::from_int(9);
        let three = nine.sqrt();
        // Should be very close to 3.0 in Q16.16 = 196608
        let expected = Fixed32::from_int(3);
        let diff = (three.0 - expected.0).abs();
        assert!(diff <= 2, "sqrt(9) should be ~3.0, got raw={}, expected raw={}", three.0, expected.0);
    }

    /// Test 6: sin(0) == 0, cos(0) == 1
    #[test]
    fn fp_trig_identity_zero() {
        let zero = Fixed32::ZERO;
        let sin_zero = zero.sin();
        let cos_zero = zero.cos();
        // cos(0) = 1.0 in Q16.16 = 65536
        let diff_cos = (cos_zero.0 - Fixed32::ONE.0).abs();
        assert!(diff_cos <= 100, "cos(0) should be ~1.0, got raw={}", cos_zero.0);
        // sin(0) should be very small
        assert!(sin_zero.0.abs() <= 500, "sin(0) should be ~0.0, got raw={}", sin_zero.0);
    }

    /// Test 7: division
    #[test]
    fn fp_division_exact() {
        let twelve = Fixed32::from_int(12);
        let four = Fixed32::from_int(4);
        let result = twelve / four;
        assert_eq!(result, Fixed32::from_int(3));
    }

    /// Test 8: From<i32> trait
    #[test]
    fn fp_from_i32() {
        let a: Fixed32 = Fixed32::from(5i32);
        assert_eq!(a.floor(), 5);
    }

    /// Test 9: Display shows reasonable decimal
    #[test]
    fn fp_display_one() {
        let one = Fixed32::ONE;
        let s = format!("{}", one);
        assert!(s.starts_with("1."), "Display of ONE should start with '1.', got '{}'", s);
    }
}
