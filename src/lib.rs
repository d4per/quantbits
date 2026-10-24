//! # quantbits
//!
//! Packed bit fields of arbitrary width, stored in a byte array of exactly the
//! size needed. Supports unsigned and signed integers, `bool`, enums, and
//! floats quantized to a range with a given number of bits. There is no upper
//! limit on the total size; each individual field can be 1–64 bits.
//!
//! ```
//! use quantbits::bitfields;
//!
//! #[bitfields]
//! #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
//! pub struct Packet {
//!     #[bits(5)]                       pub id: u8,      // 0..=31
//!     #[bits(7)]                       pub delta: i8,   // -64..=63
//!     #[bits(3, range = 0.0..1.0)]     pub volume: f32, // 0.0, 0.125, … 0.875
//!     #[bits(6, range = -1.0..=1.0)]   pub pan: f32,    // -1.0 … 1.0, both endpoints included
//!                                      pub muted: bool, // always 1 bit
//! }
//!
//! let p = Packet::new()
//!     .with_id(17)
//!     .with_delta(-20)
//!     .with_volume(0.3)
//!     .with_pan(1.0)
//!     .with_muted(true);
//!
//! assert_eq!(p.id(), 17);
//! assert_eq!(p.delta(), -20);
//! assert_eq!(p.volume(), 0.25);   // 0.3 is rounded to the nearest level
//! assert_eq!(p.pan(), 1.0);
//! assert!(p.muted());
//!
//! assert_eq!(Packet::USED_BITS, 22);
//! assert_eq!(Packet::BYTES, 3);
//! assert_eq!(core::mem::size_of::<Packet>(), 3);
//! ```
//!
//! ## Field attributes
//!
//! | Attribute                             | Types                  | Meaning                                                   |
//! |---------------------------------------|------------------------|-----------------------------------------------------------|
//! | *(none)*                              | `u8`–`u64`, `i8`–`i64` | full width                                                |
//! | *(none)* or `#[bits(1)]`              | `bool`                 | 1 bit                                                     |
//! | `#[bits(N)]`                          | `u8`–`u64`             | unsigned integer, 0..=2^N-1                               |
//! | `#[bits(N)]`                          | `i8`–`i64`             | two's complement, -2^(N-1)..=2^(N-1)-1                    |
//! | `#[bits(N)]`                          | type with [`BitEnum`]  | e.g. an enum, see [Enums](#enums)                         |
//! | `#[bits(N, range = MIN..MAX)]`        | `f32`, `f64`           | 2^N levels, step (MAX-MIN)/2^N, MAX excluded              |
//! | `#[bits(N, range = MIN..=MAX)]`       | `f32`, `f64`           | 2^N levels, step (MAX-MIN)/(2^N-1), both endpoints included |
//! | `#[bits(N, range = MIN..MAX, wrap)]`  | `f32`, `f64`           | like `MIN..MAX` but cyclic, see [Cyclic fields](#cyclic-fields) |
//!
//! `MIN` and `MAX` may be arbitrary constant expressions, e.g.
//! `range = -PI..PI` for an angle.
//!
//! ## Generated methods and constants
//!
//! For each field `x` the following are generated, with the field's visibility:
//!
//! | Name                                  | Fields          | Meaning                                           |
//! |---------------------------------------|-----------------|---------------------------------------------------|
//! | `x()`, `set_x(v)`, `with_x(v)`        | all             | read, write, write and return `self`              |
//! | `X_BITS`, `X_OFFSET`                  | all             | the field's width and first bit                   |
//! | `try_set_x(v) -> Result`              | integers        | writes only if the value fits, see [`OutOfRange`] |
//! | `X_MIN`, `X_MAX`                      | integers, floats| smallest and largest value the field can hold     |
//! | `X_STEP`                              | floats          | distance between two adjacent levels              |
//! | `set_x_stochastic(v, rng)`            | floats          | write with stochastic rounding                    |
//! | `add_x_stochastic(d, rng)`, `sub_x_stochastic(d, rng)` | floats | see [Stochastic rounding](#stochastic-rounding) |
//!
//! Also `new()`, `from_bytes`, `to_bytes`, `as_bytes` and the constants
//! `BYTES` and `USED_BITS`. If a generated name collides with another one,
//! the macro reports a compile error pointing at the field.
//!
//! All methods except the stochastic ones and those for [`BitEnum`] fields are
//! `const fn`, so values can be built at compile time:
//!
//! ```
//! # use quantbits::bitfields;
//! # #[bitfields] #[derive(Clone, Copy)]
//! # pub struct Packet { #[bits(5)] pub id: u8, #[bits(3, range = 0.0..1.0)] pub volume: f32 }
//! const START: Packet = Packet::new().with_id(7).with_volume(0.5);
//! const _: () = assert!(START.id() == 7 && Packet::ID_MAX == 31);
//! assert_eq!(START.volume(), 0.5);
//! assert_eq!(Packet::VOLUME_MAX, 0.875);
//! ```
//!
//! ## Layout
//!
//! Fields are laid out in declaration order starting at bit 0, where bit `i`
//! is in byte `i / 8` at position `i % 8` (little-endian). The representation
//! is therefore the same on every platform and can be sent over the network or
//! written to a file directly via `to_bytes`/`from_bytes`. The type has
//! alignment 1, so a `[Packet; 1000]` takes exactly 3000 bytes.
//!
//! Unused bits in the last byte are always zero; `from_bytes` clears them.
//! Two values with the same fields are therefore always equal, even byte for
//! byte. The exception is [`BitEnum`] fields after `from_bytes`, see
//! [Enums](#enums).
//!
//! ## Derives
//!
//! `Clone`, `Copy`, `PartialEq`, `Eq` and `Hash` work as usual. Four derives
//! are intercepted by the macro:
//!
//! * `Debug` shows field values instead of raw bytes.
//! * `Default` uses `new()` (std only implements `Default` for arrays of up to
//!   32 elements).
//! * `PartialOrd` and `Ord` compare the fields in declaration order, as for an
//!   ordinary struct. Float fields are compared by their level, which gives
//!   the same order as the decoded values. [`BitEnum`] fields are compared by
//!   their bits, i.e. by discriminant for an enum.
//!
//! Put `#[bitfields]` *before* `#[derive(...)]`.
//!
//! ## Out-of-range values
//!
//! * Integers: `debug_assert!` in debug builds; in release builds the excess
//!   bits are masked off (the same philosophy as Rust's ordinary integer
//!   overflow). Use `try_set_x` to get an error instead.
//! * Floats: clamped to the range and rounded to the nearest level. `NaN`
//!   becomes `MIN`.
//! * Cyclic floats (`wrap`): wrap around, so `MAX + d` becomes `MIN + d`.
//!
//! Levels are computed in `f64` and then rounded to the field's type. With
//! more bits than the mantissa holds (24 for `f32`, 53 for `f64`), adjacent
//! levels may therefore produce the same value, and for `MIN..MAX` the top
//! level may round to `MAX`.
//!
//! ## Cyclic fields
//!
//! For quantities where `MIN` and `MAX` are the same point, such as angles,
//! `wrap` makes out-of-range values wrap around instead of being clamped.
//! Without `wrap`, a value just below `PI` lands on the top level, a whole
//! step away; with `wrap` it rounds to `-PI`, which is the same angle.
//!
//! ```
//! use core::f32::consts::PI;
//! # use quantbits::bitfields;
//! #[bitfields]
//! #[derive(Clone, Copy)]
//! struct Heading {
//!     #[bits(3, range = -PI..PI)]       clamped: f32,
//!     #[bits(3, range = -PI..PI, wrap)] wrapped: f32,
//! }
//!
//! let h = Heading::new().with_clamped(PI - 0.001).with_wrapped(PI - 0.001);
//! assert_eq!(h.clamped(), 0.75 * PI);
//! assert_eq!(h.wrapped(), -PI);
//! assert_eq!(h.with_wrapped(2.0 * PI + 0.5 * PI).wrapped(), 0.5 * PI);
//! ```
//!
//! ## Stochastic rounding
//!
//! A field with few bits cannot absorb small increments: with 8 bits over
//! `0.0..=1.0` the step is 1/255, so `x + 0.0001` always rounds back to `x`.
//! `add_x_stochastic` instead rounds up with a probability equal to the
//! fraction of a step (here about 2.55%), so that the *expected value* is
//! exactly `x + 0.0001`. Over many increments they then add up correctly by
//! the law of large numbers:
//!
//! ```
//! # use quantbits::bitfields;
//! use quantbits::SplitMix64;
//!
//! #[bitfields]
//! #[derive(Clone, Copy)]
//! struct Level {
//!     #[bits(8, range = 0.0..=1.0)] value: f32,
//! }
//!
//! let mut rng = SplitMix64::new(42);
//! let mut plain = Level::new();
//! let mut stochastic = Level::new();
//! for _ in 0..5000 {
//!     plain.set_value(plain.value() + 0.0001);
//!     stochastic.add_value_stochastic(0.0001, &mut rng);
//! }
//! assert_eq!(plain.value(), 0.0);                   // every increment is rounded away
//! assert!((stochastic.value() - 0.5).abs() < 0.1);  // ≈ 5000 · 0.0001
//! ```
//!
//! The random numbers come from [`RandomBits`], which is implemented for
//! [`SplitMix64`] and for every closure `FnMut() -> u64`, so any random number
//! generator can be used: `&mut || rng.next_u64()`.
//!
//! The expected value is exact as long as the result stays within the range;
//! when clamping at `MIN` or `MAX` it is biased towards the middle. Cyclic
//! fields do not have that problem. The probability has a resolution of
//! 2^-53, so increments smaller than about `X_STEP / 2^53` are lost.
//!
//! ## Enums
//!
//! A fieldless enum can be stored with `#[derive(BitEnum)]` and `#[bits(N)]`,
//! where `N` is at least [`BitEnum::BITS`] (the number of bits needed for the
//! largest discriminant). A larger `N` leaves room for future variants.
//!
//! Since `from_bytes` accepts arbitrary bytes, every bit pattern must decode
//! to something. If the enum does not cover all 2^BITS patterns, one variant
//! must therefore be marked `#[fallback]`; unknown patterns decode to it.
//!
//! `from_bytes` keeps such patterns as they are, so a value from a newer
//! version with more variants can be forwarded unchanged. The field reads as
//! the fallback, but `==`, `Hash` and `Ord` work on the bits and therefore
//! still see the difference. The same applies to the spare bits when `N` is
//! larger than `BITS`.
//!
//! ```
//! # use quantbits::bitfields;
//! use quantbits::BitEnum;
//!
//! #[derive(BitEnum, Clone, Copy, PartialEq, Debug)]
//! enum Stance { Standing, Crouching, Prone, Swimming } // 4 variants = every 2-bit pattern
//!
//! #[derive(BitEnum, Clone, Copy, PartialEq, Debug)]
//! enum Weapon {
//!     #[fallback] Unarmed,
//!     Sword = 3,
//!     Bow = 5,
//! }
//!
//! #[bitfields]
//! #[derive(Clone, Copy, Debug)]
//! struct Player {
//!     #[bits(2)] stance: Stance,
//!     #[bits(4)] weapon: Weapon, // needs 3, one spare bit for the future
//! }
//!
//! let p = Player::new().with_stance(Stance::Prone).with_weapon(Weapon::Bow);
//! assert_eq!((p.stance(), p.weapon()), (Stance::Prone, Weapon::Bow));
//! assert_eq!(Player::from_bytes([0b11_1111]).weapon(), Weapon::Unarmed);
//! assert_eq!(Weapon::BITS, 3);
//! ```
//!
//! [`BitEnum`] can also be implemented by hand, e.g. for a newtype.
//!
//! ## Compile errors
//!
//! Invalid declarations produce errors that point at the right place in the
//! code, e.g. for more bits than the type holds, floats without `range`,
//! `MIN >= MAX` or a non-finite range, `wrap` with a closed range, an enum
//! that does not fit in its field or lacks `#[fallback]`, and colliding
//! generated names. See `tests/ui/` for every case and the exact messages.

#![no_std]

// Makes `::quantbits::...` in generated code work inside this crate as well.
extern crate self as quantbits;

pub use quantbits_derive::{bitfields, BitEnum};

/// The distance between two adjacent levels of a float field.
///
/// Every field also has an `X_STEP` constant with the same value.
///
/// ```
/// assert_eq!(quantbits::step(0.0, 1.0, 3, false), 0.125);
/// assert_eq!(quantbits::step(0.0, 7.0, 3, true), 1.0);
/// ```
#[inline]
pub const fn step(min: f64, max: f64, bits: u32, inclusive: bool) -> f64 {
    let top = __private::mask(bits) as f64; // 2^bits - 1
    if inclusive {
        (max - min) / top
    } else {
        (max - min) / (top + 1.0)
    }
}

/// A type that can be stored in a bit field, normally an enum with
/// `#[derive(BitEnum)]`.
///
/// A manual implementation must return a value from `into_bits` that fits in
/// `BITS` bits, and `from_bits` must handle every value that fits in the
/// field, since `from_bytes` can produce arbitrary bits.
pub trait BitEnum: Sized {
    /// The smallest number of bits that holds every value.
    const BITS: u32;
    /// The value as bits.
    fn into_bits(self) -> u64;
    /// The value from bits. Must not panic.
    fn from_bits(bits: u64) -> Self;
}

/// A source of uniformly distributed random bits, for stochastic rounding.
///
/// Implemented for [`SplitMix64`] and for every closure `FnMut() -> u64`, so a
/// random number generator from another crate can be used as
/// `&mut || rng.next_u64()`.
pub trait RandomBits {
    /// The next 64 random bits.
    fn next_u64(&mut self) -> u64;
}

impl<F: FnMut() -> u64> RandomBits for F {
    #[inline]
    fn next_u64(&mut self) -> u64 {
        self()
    }
}

/// A small, fast, deterministic random number generator (SplitMix64).
///
/// Good enough for stochastic rounding, but not cryptographically secure. The
/// same seed always gives the same sequence. The type is deliberately not
/// `Copy`, so that a sequence is not accidentally repeated through an
/// unintended copy.
///
/// ```
/// let mut rng = quantbits::SplitMix64::new(0);
/// assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator with the seed `seed`.
    #[inline]
    pub const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 random bits.
    #[inline]
    pub const fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl RandomBits for SplitMix64 {
    #[inline]
    fn next_u64(&mut self) -> u64 {
        SplitMix64::next_u64(self)
    }
}

/// Error from `try_set_x` when an integer does not fit in the field.
///
/// ```
/// # use quantbits::bitfields;
/// #[bitfields]
/// #[derive(Clone, Copy)]
/// struct S { #[bits(4)] small: u8 }
///
/// let mut s = S::new().with_small(3);
/// let err = s.try_set_small(16).unwrap_err();
/// assert_eq!((err.field, err.bits), ("small", 4));
/// assert_eq!(s.small(), 3); // unchanged
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutOfRange {
    /// The field's name.
    pub field: &'static str,
    /// The field's width.
    pub bits: u32,
}

impl core::fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let unit = if self.bits == 1 { "bit" } else { "bits" };
        write!(f, "value for `{}` does not fit in {} {unit}", self.field, self.bits)
    }
}

impl core::error::Error for OutOfRange {}

/// Helpers for the generated code. Not part of the public API.
#[doc(hidden)]
pub mod __private {
    /// A mask with `bits` ones in the lowest bits (1..=64).
    #[inline(always)]
    pub const fn mask(bits: u32) -> u64 {
        if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        }
    }

    /// Reads `bits` (1..=64) bits starting at bit `offset`.
    ///
    /// A field of at most 64 bits at an arbitrary start position spans at most
    /// 9 bytes, so they are gathered into a `u128`. `offset` and `bits` are
    /// always constants in the generated code, so the compiler unrolls the loop.
    #[inline(always)]
    pub const fn read_bits<const N: usize>(bytes: &[u8; N], offset: usize, bits: u32) -> u64 {
        let first = offset / 8;
        let shift = offset % 8;
        let last = (offset + bits as usize - 1) / 8;
        let mut acc: u128 = 0;
        let mut i = first;
        while i <= last {
            acc |= (bytes[i] as u128) << (8 * (i - first));
            i += 1;
        }
        ((acc >> shift) as u64) & mask(bits)
    }

    /// Writes the lowest `bits` bits of `value` starting at bit `offset`.
    /// Other bits are left untouched.
    #[inline(always)]
    pub const fn write_bits<const N: usize>(bytes: &mut [u8; N], offset: usize, bits: u32, value: u64) {
        let first = offset / 8;
        let shift = offset % 8;
        let last = (offset + bits as usize - 1) / 8;
        let m = (mask(bits) as u128) << shift;
        let v = ((value & mask(bits)) as u128) << shift;
        let mut i = first;
        while i <= last {
            let k = 8 * (i - first);
            let bm = (m >> k) as u8;
            bytes[i] = (bytes[i] & !bm) | ((v >> k) as u8 & bm);
            i += 1;
        }
    }

    /// Sign-extends a `bits`-bit two's complement number.
    #[inline(always)]
    pub const fn sign_extend(raw: u64, bits: u32) -> i64 {
        let shift = 64 - bits;
        ((raw << shift) as i64) >> shift
    }

    #[inline(always)]
    pub const fn fits_unsigned(value: u64, bits: u32) -> bool {
        value <= mask(bits)
    }

    #[inline(always)]
    pub const fn fits_signed(value: i64, bits: u32) -> bool {
        let half = 1i128 << (bits - 1);
        let v = value as i128;
        v >= -half && v < half
    }

    #[inline(always)]
    pub const fn out_of_range(field: &'static str, bits: u32) -> super::OutOfRange {
        super::OutOfRange { field, bits }
    }

    /// Whether `min..max` (or `min..=max`) gives finite levels: `min < max`
    /// and a finite step greater than 0. Rejects NaN, infinite endpoints and
    /// ranges too wide for `f64`, such as `-f64::MAX..f64::MAX`.
    pub const fn valid_range(min: f64, max: f64, bits: u32, inclusive: bool) -> bool {
        let step = super::step(min, max, bits, inclusive);
        min < max && min.is_finite() && max.is_finite() && step.is_finite() && step > 0.0
    }

    /// Float -> position counted in steps from `min`, without rounding.
    #[inline(always)]
    pub const fn level(value: f64, min: f64, max: f64, bits: u32, inclusive: bool) -> f64 {
        (value - min) / super::step(min, max, bits, inclusive)
    }

    /// Reduces a position to `[0, 2^bits)`, for cyclic fields.
    #[inline(always)]
    pub const fn wrap_level(x: f64, bits: u32) -> f64 {
        let n = mask(bits) as f64 + 1.0;
        let r = x % n;
        if r < 0.0 {
            r + n
        } else {
            r
        }
    }

    /// Float -> level (0..=2^bits-1). Rounds to the nearest level and clamps,
    /// or wraps around if `wrap`.
    #[inline(always)]
    pub const fn quantize(value: f64, min: f64, max: f64, bits: u32, inclusive: bool, wrap: bool) -> u64 {
        let x = level(value, min, max, bits, inclusive) + 0.5;
        if wrap {
            // The position is non-negative, so `as u64` rounds down. The mask
            // handles the case where `r + n` in wrap_level rounds up to exactly n.
            (wrap_level(x, bits) as u64) & mask(bits)
        } else {
            // `as u64` saturates: negative numbers and NaN become 0, too large ones u64::MAX.
            let q = x as u64;
            if q > mask(bits) {
                mask(bits)
            } else {
                q
            }
        }
    }

    /// Stochastic rounding of the position `x` to a level: up with a
    /// probability equal to the fractional part, so that the expected value is
    /// exactly `x`. `random` must be 64 uniformly distributed bits.
    #[inline(always)]
    pub const fn stochastic_level(x: f64, bits: u32, wrap: bool, random: u64) -> u64 {
        let top = mask(bits);
        let x = if wrap {
            wrap_level(x, bits)
        } else if x <= 0.0 {
            0.0
        } else if x >= top as f64 {
            top as f64
        } else {
            x // including NaN, which becomes level 0 below
        };
        let floor = x as u64;
        let frac = x - floor as f64;
        // The top 53 bits give a uniform number in [0, 1).
        let u = (random >> 11) as f64 * (1.0 / (1u64 << 53) as f64);
        let q = if u < frac { floor.wrapping_add(1) } else { floor };
        if wrap {
            q & top
        } else if q > top {
            top
        } else {
            q
        }
    }

    /// Level -> float.
    #[inline(always)]
    pub const fn dequantize(q: u64, min: f64, max: f64, bits: u32, inclusive: bool) -> f64 {
        // `min + top * step` is not always exactly `max` due to rounding, e.g.
        // for `0.3..=0.9`, so the top level returns `max` directly.
        if inclusive && q >= mask(bits) {
            return max;
        }
        min + q as f64 * super::step(min, max, bits, inclusive)
    }

    /// Compares the same field in two buffers, for the generated `Ord`/`PartialOrd`.
    #[inline(always)]
    pub fn cmp_bits<const N: usize>(
        a: &[u8; N],
        b: &[u8; N],
        offset: usize,
        bits: u32,
        signed: bool,
    ) -> core::cmp::Ordering {
        let (x, y) = (read_bits(a, offset, bits), read_bits(b, offset, bits));
        if signed {
            sign_extend(x, bits).cmp(&sign_extend(y, bits))
        } else {
            x.cmp(&y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::__private::*;

    #[test]
    fn read_write_every_offset_and_width() {
        // Write a pattern at every possible position and width in a 20-byte
        // buffer and check both the value and that surrounding bits are untouched.
        for bits in 1..=64u32 {
            for offset in 0..(160 - bits as usize) {
                for &fill in &[0x00u8, 0xFF] {
                    let mut buf = [fill; 20];
                    let value = 0xA5C3_9617_E28B_4D5Fu64 & mask(bits);
                    write_bits(&mut buf, offset, bits, value);
                    assert_eq!(read_bits(&buf, offset, bits), value, "bits={bits} offset={offset}");
                    for b in 0..160usize {
                        if b < offset || b >= offset + bits as usize {
                            let bit = (buf[b / 8] >> (b % 8)) & 1;
                            assert_eq!(bit, fill & 1, "bit {b} changed, bits={bits} offset={offset}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sign_extension() {
        assert_eq!(sign_extend(0b1111, 4), -1);
        assert_eq!(sign_extend(0b1000, 4), -8);
        assert_eq!(sign_extend(0b0111, 4), 7);
        assert_eq!(sign_extend(u64::MAX, 64), -1);
    }

    #[test]
    fn quantize_dequantize() {
        let ranges = [(0.0, 1.0), (0.0, 0.1), (-0.1, 0.7), (0.3, 0.9), (-3.3, 1.1), (-1.0e6, 1.0e6)];
        for &(min, max) in &ranges {
            for bits in 1..=64u32 {
                let top = mask(bits);
                assert_eq!(dequantize(0, min, max, bits, true), min);
                assert_eq!(dequantize(top, min, max, bits, true), max, "{min}..={max}, {bits} bits");
                assert_eq!(dequantize(0, min, max, bits, false), min);
            }
            // Every level should decode and encode back to itself.
            for bits in 1..=12u32 {
                for (inclusive, wrap) in [(false, false), (true, false), (false, true)] {
                    for q in 0..=mask(bits) {
                        let v = dequantize(q, min, max, bits, inclusive);
                        assert_eq!(quantize(v, min, max, bits, inclusive, wrap), q);
                    }
                }
            }
        }
    }

    #[test]
    fn wrapping() {
        // 3 bits over [0, 8): step 1, the levels are the integers 0..=7.
        let q = |v: f64| quantize(v, 0.0, 8.0, 3, false, true);
        assert_eq!([q(7.4), q(7.6), q(8.0), q(9.2), q(-0.4), q(-0.6), q(-8.0)], [7, 0, 0, 1, 0, 7, 0]);
        assert_eq!(q(f64::NAN), 0);
        assert_eq!(q(f64::INFINITY), 0);
        assert_eq!(q(1.0e300), 0);
    }

    #[test]
    fn stochastic_rounding_endpoints() {
        // u = 0 always rounds up if the fraction is > 0, u ≈ 1 never does.
        let (lo, hi) = (0u64, u64::MAX);
        assert_eq!(stochastic_level(2.25, 3, false, lo), 3);
        assert_eq!(stochastic_level(2.25, 3, false, hi), 2);
        assert_eq!(stochastic_level(2.0, 3, false, lo), 2);
        // Clamping and wrapping at the ends, respectively.
        assert_eq!(stochastic_level(7.5, 3, false, lo), 7);
        assert_eq!(stochastic_level(-0.5, 3, false, lo), 0);
        assert_eq!(stochastic_level(7.5, 3, true, lo), 0);
        assert_eq!(stochastic_level(-0.5, 3, true, hi), 7);
        assert_eq!(stochastic_level(f64::NAN, 3, false, lo), 0);
    }

    // The macro also works inside the crate thanks to `extern crate self as quantbits`.
    #[crate::bitfields]
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Internal {
        #[bits(3)]
        a: u8,
    }

    #[test]
    fn macro_inside_crate() {
        assert_eq!(Internal::new().with_a(5).a(), 5);
    }
}
