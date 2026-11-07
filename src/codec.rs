//! Conversion between values and raw bits, for packed containers.

use core::marker::PhantomData;

use crate::OutOfRange;

// Separate from the import above: rustfmt versions disagree on how to sort the two.
use crate::__private::{fits_signed, fits_unsigned, sign_extend};

/// How a value is stored in a fixed number of bits, for the packed containers
/// such as [`PackedSlice`](crate::PackedSlice) and `PackedVec`.
///
/// Implemented for `bool`, [`Int`], every enum with `#[derive(BitEnum)]`,
/// every `#[bitfields]` struct of at most 64 bits, and the types declared with
/// [`#[codec]`](crate::codec). For a type with a hand-written [`BitEnum`](crate::BitEnum)
/// implementation, declare a codec with `#[codec(N)] type Name = MyType;`.
///
/// ```
/// use quantbits::{Codec, Int};
///
/// assert_eq!(<Int<i8, 4> as Codec>::BITS, 4);
/// assert_eq!(<Int<i8, 4> as Codec>::encode(-1) & 0b1111, 0b1111);
/// assert_eq!(<Int<i8, 4> as Codec>::decode(0b1111), -1);
/// ```
pub trait Codec {
    /// The type of the stored values.
    type Value;
    /// Number of bits per value, 1..=64.
    const BITS: u32;
    /// The value as raw bits. Only the lowest `BITS` bits are stored.
    ///
    /// Values that do not fit are handled as for a field of the same kind:
    /// integers `debug_assert!` and are masked in release builds, floats are
    /// clamped (or wrapped) and rounded to the nearest level.
    fn encode(value: Self::Value) -> u64;
    /// The value from raw bits. Gets every pattern that fits in `BITS` bits
    /// and must not panic.
    fn decode(raw: u64) -> Self::Value;
    /// Like [`encode`](Self::encode), but an error if the value does not fit.
    /// The default implementation never fails.
    #[inline]
    fn try_encode(value: Self::Value) -> Result<u64, OutOfRange> {
        Ok(Self::encode(value))
    }
}

/// A [`Codec`] for floats, with stochastic rounding, see
/// [Stochastic rounding](crate#stochastic-rounding).
///
/// Implemented for the float types declared with [`#[codec]`](crate::codec).
pub trait StochasticCodec: Codec {
    /// The value as a level with stochastic rounding. `random` must be 64
    /// uniformly distributed bits.
    fn encode_stochastic(value: Self::Value, random: u64) -> u64;
    /// The level `raw` plus `delta`, with stochastic rounding. `random` must be
    /// 64 uniformly distributed bits.
    fn add_stochastic(raw: u64, delta: Self::Value, random: u64) -> u64;
}

/// A [`Codec`] for an `N`-bit integer stored as `T`, which is one of
/// `u8`–`u64` or `i8`–`i64`.
///
/// Unsigned integers hold 0..=2^N-1, signed ones -2^(N-1)..=2^(N-1)-1 in two's
/// complement, as for a `#[bits(N)]` field. `N` must be between 1 and the
/// width of `T`, otherwise using the codec is a compile error.
///
/// ```
/// use quantbits::{Codec, Int};
///
/// assert_eq!(<Int<u16, 13> as Codec>::BITS, 13);
/// assert!(<Int<u16, 13> as Codec>::try_encode(8191).is_ok());
/// assert!(<Int<u16, 13> as Codec>::try_encode(8192).is_err());
/// ```
///
/// ```compile_fail
/// // error: `Int<u8, N>` requires N in 1..=8
/// let values = quantbits::PackedSlice::<quantbits::Int<u8, 9>>::new(&[0; 4], 2).unwrap();
/// let _ = values.get(0);
/// ```
pub struct Int<T, const N: u32>(PhantomData<fn() -> T>);

macro_rules! int_codecs {
    ($($signed:ident $t:ident $width:literal),* $(,)?) => {$(
        impl<const N: u32> Codec for Int<$t, N> {
            type Value = $t;
            const BITS: u32 = {
                ::core::assert!(
                    N >= 1 && N <= $width,
                    concat!("quantbits: `Int<", stringify!($t), ", N>` requires N in 1..=", $width)
                );
                N
            };

            #[inline]
            fn encode(value: $t) -> u64 {
                ::core::debug_assert!(
                    int_codecs!(@fits $signed value, Self::BITS),
                    "quantbits: value does not fit in the `Int` codec"
                );
                int_codecs!(@raw $signed value)
            }

            #[inline]
            fn decode(raw: u64) -> $t {
                int_codecs!(@decode $signed raw, Self::BITS) as $t
            }

            #[inline]
            fn try_encode(value: $t) -> Result<u64, OutOfRange> {
                if int_codecs!(@fits $signed value, Self::BITS) {
                    Ok(int_codecs!(@raw $signed value))
                } else {
                    Err(OutOfRange { field: "element", bits: Self::BITS })
                }
            }
        }
    )*};
    (@fits unsigned $v:ident, $bits:expr) => { fits_unsigned($v as u64, $bits) };
    (@fits signed $v:ident, $bits:expr) => { fits_signed($v as i64, $bits) };
    (@raw unsigned $v:ident) => { $v as u64 };
    (@raw signed $v:ident) => { $v as i64 as u64 };
    (@decode unsigned $raw:ident, $bits:expr) => { $raw };
    (@decode signed $raw:ident, $bits:expr) => { sign_extend($raw, $bits) };
}

int_codecs! {
    unsigned u8 8, unsigned u16 16, unsigned u32 32, unsigned u64 64,
    signed i8 8, signed i16 16, signed i32 32, signed i64 64,
}

impl Codec for bool {
    type Value = bool;
    const BITS: u32 = 1;

    #[inline]
    fn encode(value: bool) -> u64 {
        value as u64
    }

    #[inline]
    fn decode(raw: u64) -> bool {
        raw != 0
    }
}
