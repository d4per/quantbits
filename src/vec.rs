//! A growable packed vector. Needs the `alloc` feature.

use alloc::vec::Vec;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::ops::Neg;

use crate::packed::{byte_len, read_at, write_at, Width};
use crate::{Codec, Iter, OutOfRange, PackedSlice, PackedSliceMut, RandomBits, StochasticCodec};

#[cold]
fn capacity_overflow() -> ! {
    panic!("quantbits: capacity overflow")
}

/// A growable vector of values of the codec `C`, each stored in exactly
/// `C::BITS` bits. Needs the `alloc` feature.
///
/// The values are packed back to back with the same layout as
/// [`PackedSlice`], so `len` values take `ceil(len * C::BITS / 8)` bytes.
///
/// ```
/// use quantbits::{Int, PackedVec};
///
/// let mut ids: PackedVec<Int<u16, 13>> = PackedVec::new();
/// ids.push(8191);
/// ids.push(42);
/// assert_eq!(ids.get(0), Some(8191));
/// assert_eq!(ids.iter().collect::<Vec<_>>(), [8191, 42]);
/// assert_eq!(ids.as_bytes().len(), 4); // 26 bits
/// assert!(ids.try_push(8192).is_err());
/// ```
///
/// Since the values are not stored as Rust values, there is no `Index`
/// implementation and no `&mut` access to elements; use [`get`](Self::get)
/// and [`set`](Self::set) instead.
pub struct PackedVec<C> {
    /// Always exactly `byte_len(len)` bytes; bits after the last value are zero.
    bytes: Vec<u8>,
    len: usize,
    codec: PhantomData<fn() -> C>,
}

impl<C: Codec> PackedVec<C> {
    /// An empty vector. Does not allocate.
    #[inline]
    pub const fn new() -> Self {
        Self { bytes: Vec::new(), len: 0, codec: PhantomData }
    }

    /// An empty vector with room for at least `capacity` values.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        let bytes = byte_len(capacity, Width::<C>::BITS).unwrap_or_else(|| capacity_overflow());
        Self { bytes: Vec::with_capacity(bytes), len: 0, codec: PhantomData }
    }

    /// Creates a vector from the bytes of `len` values, e.g. from
    /// [`into_bytes`](Self::into_bytes). Returns `None` unless `bytes` has
    /// exactly `ceil(len * C::BITS / 8)` bytes. Bits after the last value are
    /// cleared.
    pub fn from_bytes(mut bytes: Vec<u8>, len: usize) -> Option<Self> {
        if byte_len(len, Width::<C>::BITS)? != bytes.len() {
            return None;
        }
        clear_tail(&mut bytes, len * Width::<C>::BITS as usize);
        Some(Self { bytes, len, codec: PhantomData })
    }

    /// Number of values.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no values.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of values the vector can hold without reallocating.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.bytes.capacity().saturating_mul(8) / Width::<C>::BITS as usize
    }

    /// Reserves room for at least `additional` more values.
    pub fn reserve(&mut self, additional: usize) {
        let total = self.len.checked_add(additional).unwrap_or_else(|| capacity_overflow());
        let bytes = byte_len(total, Width::<C>::BITS).unwrap_or_else(|| capacity_overflow());
        self.bytes.reserve(bytes - self.bytes.len());
    }

    /// Removes all values.
    #[inline]
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.len = 0;
    }

    /// Shortens the vector to `len` values. Has no effect if it is not longer.
    pub fn truncate(&mut self, len: usize) {
        if len < self.len {
            let bits = len * Width::<C>::BITS as usize;
            self.bytes.truncate(bits.div_ceil(8));
            clear_tail(&mut self.bytes, bits);
            self.len = len;
        }
    }

    /// Appends a value, see [`Codec::encode`] for values that do not fit.
    #[inline]
    pub fn push(&mut self, value: C::Value) {
        self.push_raw(C::encode(value));
    }

    /// Appends a value if it fits, otherwise leaves the vector unchanged.
    #[inline]
    pub fn try_push(&mut self, value: C::Value) -> Result<(), OutOfRange> {
        self.push_raw(C::try_encode(value)?);
        Ok(())
    }

    /// Appends raw bits; only the lowest `C::BITS` bits are used.
    #[inline]
    pub fn push_raw(&mut self, raw: u64) {
        let bits = Width::<C>::BITS;
        let new_len = self.len.checked_add(1).unwrap_or_else(|| capacity_overflow());
        let bytes = byte_len(new_len, bits).unwrap_or_else(|| capacity_overflow());
        self.bytes.resize(bytes, 0);
        write_at(&mut self.bytes, self.len * bits as usize, bits, raw);
        self.len = new_len;
    }

    /// Removes and returns the last value, or `None` if empty.
    #[inline]
    pub fn pop(&mut self) -> Option<C::Value> {
        let last = self.len.checked_sub(1)?;
        let raw = read_at(&self.bytes, last * Width::<C>::BITS as usize, Width::<C>::BITS);
        self.truncate(last);
        Some(C::decode(raw))
    }

    /// Resizes to `len` values, appending copies of `value` if it grows.
    pub fn resize(&mut self, len: usize, value: C::Value) {
        if len <= self.len {
            self.truncate(len);
        } else {
            let raw = C::encode(value);
            self.reserve(len - self.len);
            while self.len < len {
                self.push_raw(raw);
            }
        }
    }

    /// The value at `index`, or `None` if out of bounds.
    #[inline]
    pub fn get(&self, index: usize) -> Option<C::Value> {
        self.as_slice().get(index)
    }

    /// The raw bits at `index`, or `None` if out of bounds. For floats this is
    /// the level.
    #[inline]
    pub fn get_raw(&self, index: usize) -> Option<u64> {
        self.as_slice().get_raw(index)
    }

    /// Sets the value at `index`, see [`Codec::encode`] for values that do not
    /// fit. Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn set(&mut self, index: usize, value: C::Value) {
        self.as_mut_slice().set(index, value);
    }

    /// Sets the value at `index` if it fits, otherwise leaves it unchanged.
    /// Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn try_set(&mut self, index: usize, value: C::Value) -> Result<(), OutOfRange> {
        self.as_mut_slice().try_set(index, value)
    }

    /// Sets the raw bits at `index`; only the lowest `C::BITS` bits are used.
    /// Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn set_raw(&mut self, index: usize, raw: u64) {
        self.as_mut_slice().set_raw(index, raw);
    }

    /// An iterator over the values.
    #[inline]
    pub fn iter(&self) -> Iter<'_, C> {
        self.as_slice().iter()
    }

    /// A read-only view of the values.
    #[inline]
    pub fn as_slice(&self) -> PackedSlice<'_, C> {
        PackedSlice::new(&self.bytes, self.len).unwrap()
    }

    /// A mutable view of the values, without the methods that change the length.
    #[inline]
    pub fn as_mut_slice(&mut self) -> PackedSliceMut<'_, C> {
        PackedSliceMut::new(&mut self.bytes, self.len).unwrap()
    }

    /// The packed bytes, `ceil(len * C::BITS / 8)` of them. Bits after the
    /// last value are zero.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The packed bytes, see [`as_bytes`](Self::as_bytes).
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl<C: StochasticCodec> PackedVec<C> {
    /// Appends a value with stochastic rounding, see
    /// [Stochastic rounding](crate#stochastic-rounding).
    #[inline]
    pub fn push_stochastic(&mut self, value: C::Value, rng: &mut impl RandomBits) {
        self.push_raw(C::encode_stochastic(value, rng.next_u64()));
    }

    /// Sets the value at `index` with stochastic rounding. Panics if `index`
    /// is out of bounds.
    #[inline]
    #[track_caller]
    pub fn set_stochastic(&mut self, index: usize, value: C::Value, rng: &mut impl RandomBits) {
        self.as_mut_slice().set_stochastic(index, value, rng);
    }

    /// Adds `delta` to the value at `index` with stochastic rounding, so that
    /// many small increments add up correctly on average. Panics if `index` is
    /// out of bounds.
    #[inline]
    #[track_caller]
    pub fn add_stochastic(&mut self, index: usize, delta: C::Value, rng: &mut impl RandomBits) {
        self.as_mut_slice().add_stochastic(index, delta, rng);
    }

    /// Like [`add_stochastic`](Self::add_stochastic) but subtracts `delta`.
    #[inline]
    #[track_caller]
    pub fn sub_stochastic(&mut self, index: usize, delta: C::Value, rng: &mut impl RandomBits)
    where
        C::Value: Neg<Output = C::Value>,
    {
        self.as_mut_slice().sub_stochastic(index, delta, rng);
    }
}

/// Clears the bits from `bits` to the end of the last byte.
fn clear_tail(bytes: &mut [u8], bits: usize) {
    let used = bits % 8;
    if used != 0 {
        if let Some(last) = bytes.last_mut() {
            *last &= ((1u16 << used) - 1) as u8;
        }
    }
}

impl<C> Clone for PackedVec<C> {
    #[inline]
    fn clone(&self) -> Self {
        Self { bytes: self.bytes.clone(), len: self.len, codec: PhantomData }
    }
}

impl<C: Codec> Default for PackedVec<C> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Compares the stored bits, like the `PartialEq` of a `#[bitfields]` struct.
impl<C> PartialEq for PackedVec<C> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.bytes == other.bytes
    }
}

impl<C> Eq for PackedVec<C> {}

impl<C> Hash for PackedVec<C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        self.bytes.hash(state);
    }
}

impl<C: Codec> fmt::Debug for PackedVec<C>
where
    C::Value: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<C: Codec> Extend<C::Value> for PackedVec<C> {
    fn extend<I: IntoIterator<Item = C::Value>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        self.reserve(iter.size_hint().0);
        for value in iter {
            self.push(value);
        }
    }
}

impl<C: Codec> FromIterator<C::Value> for PackedVec<C> {
    fn from_iter<I: IntoIterator<Item = C::Value>>(iter: I) -> Self {
        let mut v = Self::new();
        v.extend(iter);
        v
    }
}

impl<'a, C: Codec> IntoIterator for &'a PackedVec<C> {
    type Item = C::Value;
    type IntoIter = Iter<'a, C>;

    #[inline]
    fn into_iter(self) -> Iter<'a, C> {
        self.iter()
    }
}
