//! Packed views over byte buffers: values of a [`Codec`] stored back to back.

use core::fmt;
use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::ops::Neg;

use crate::__private::{index_out_of_bounds, mask};
use crate::{Codec, OutOfRange, RandomBits, StochasticCodec};

/// The width of a codec, checked once per codec at compile time.
pub(crate) struct Width<C>(PhantomData<fn() -> C>);

impl<C: Codec> Width<C> {
    pub(crate) const BITS: u32 = {
        assert!(C::BITS >= 1 && C::BITS <= 64, "quantbits: `Codec::BITS` must be 1..=64");
        C::BITS
    };
}

/// Number of bytes for `len` values of `bits` bits, or `None` on overflow.
#[inline]
pub(crate) const fn byte_len(len: usize, bits: u32) -> Option<usize> {
    match len.checked_mul(bits as usize) {
        Some(total) => Some(total.div_ceil(8)),
        None => None,
    }
}

/// Reads `bits` (1..=64) bits starting at bit `bit`, which must lie within `bytes`.
///
/// Usually a single unaligned load: a value of at most 57 bits at an arbitrary
/// start position fits in 8 bytes, one of at most 64 bits in 16. Only near the
/// end of the buffer are the bytes gathered one by one.
#[inline]
pub(crate) fn read_at(bytes: &[u8], bit: usize, bits: u32) -> u64 {
    let first = bit / 8;
    let shift = bit % 8;
    if bits <= 57 {
        if let Some(chunk) = bytes.get(first..first + 8) {
            let word = u64::from_le_bytes(chunk.try_into().unwrap());
            return (word >> shift) & mask(bits);
        }
    } else if let Some(chunk) = bytes.get(first..first + 16) {
        let word = u128::from_le_bytes(chunk.try_into().unwrap());
        return ((word >> shift) as u64) & mask(bits);
    }
    let last = (bit + bits as usize - 1) / 8;
    let mut acc: u128 = 0;
    for (k, &b) in bytes[first..=last].iter().enumerate() {
        acc |= (b as u128) << (8 * k);
    }
    ((acc >> shift) as u64) & mask(bits)
}

/// Writes the lowest `bits` bits of `value` starting at bit `bit`, which must
/// lie within `bytes`. Other bits are left untouched.
#[inline]
pub(crate) fn write_at(bytes: &mut [u8], bit: usize, bits: u32, value: u64) {
    let first = bit / 8;
    let shift = bit % 8;
    if bits <= 57 {
        if let Some(chunk) = bytes.get_mut(first..first + 8) {
            let m = mask(bits) << shift;
            let word = u64::from_le_bytes((&*chunk).try_into().unwrap());
            let word = (word & !m) | ((value << shift) & m);
            chunk.copy_from_slice(&word.to_le_bytes());
            return;
        }
    } else if let Some(chunk) = bytes.get_mut(first..first + 16) {
        let m = (mask(bits) as u128) << shift;
        let word = u128::from_le_bytes((&*chunk).try_into().unwrap());
        let word = (word & !m) | (((value as u128) << shift) & m);
        chunk.copy_from_slice(&word.to_le_bytes());
        return;
    }
    let last = (bit + bits as usize - 1) / 8;
    let m = (mask(bits) as u128) << shift;
    let v = ((value & mask(bits)) as u128) << shift;
    for (k, b) in bytes[first..=last].iter_mut().enumerate() {
        let bm = (m >> (8 * k)) as u8;
        *b = (*b & !bm) | ((v >> (8 * k)) as u8 & bm);
    }
}

/// A read-only view of `len` values of the codec `C`, packed back to back
/// into a byte slice.
///
/// The layout is the same as for `#[bitfields]` structs: value `i` occupies
/// bits `i * C::BITS` to `(i + 1) * C::BITS - 1`, where bit `j` is in byte
/// `j / 8` at position `j % 8`. It works without an allocator, e.g. on a
/// buffer received over the network:
///
/// ```
/// use quantbits::{Int, PackedSlice};
///
/// let bytes = [0b1110_0101, 0b0000_0001];
/// let values = PackedSlice::<Int<u8, 3>>::new(&bytes, 3).unwrap();
/// assert_eq!(values.iter().collect::<Vec<_>>(), [5, 4, 7]);
/// assert_eq!(values.get(3), None);
/// ```
pub struct PackedSlice<'a, C> {
    bytes: &'a [u8],
    len: usize,
    codec: PhantomData<fn() -> C>,
}

impl<C> Clone for PackedSlice<'_, C> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for PackedSlice<'_, C> {}

impl<'a, C: Codec> PackedSlice<'a, C> {
    /// A view of the first `len` values in `bytes`, or `None` if `bytes` is
    /// too short. Bytes after the last value are ignored.
    #[inline]
    pub fn new(bytes: &'a [u8], len: usize) -> Option<Self> {
        let needed = byte_len(len, Width::<C>::BITS)?;
        (bytes.len() >= needed).then_some(Self { bytes, len, codec: PhantomData })
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

    /// The value at `index`, or `None` if out of bounds.
    #[inline]
    pub fn get(&self, index: usize) -> Option<C::Value> {
        self.get_raw(index).map(C::decode)
    }

    /// The raw bits at `index`, or `None` if out of bounds. For floats this is
    /// the level.
    #[inline]
    pub fn get_raw(&self, index: usize) -> Option<u64> {
        let bits = Width::<C>::BITS;
        (index < self.len).then(|| read_at(self.bytes, index * bits as usize, bits))
    }

    /// The first value, or `None` if empty.
    #[inline]
    pub fn first(&self) -> Option<C::Value> {
        self.get(0)
    }

    /// The last value, or `None` if empty.
    #[inline]
    pub fn last(&self) -> Option<C::Value> {
        self.get(self.len.checked_sub(1)?)
    }

    /// An iterator over the values.
    #[inline]
    pub fn iter(&self) -> Iter<'a, C> {
        Iter { bytes: self.bytes, front: 0, back: self.len, codec: PhantomData }
    }

    /// The bytes holding the values, `ceil(len * C::BITS / 8)` of them. Bits
    /// after the last value in the last byte are whatever the buffer contains.
    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        &self.bytes[..byte_len(self.len, Width::<C>::BITS).unwrap()]
    }
}

impl<'a, C: Codec> IntoIterator for PackedSlice<'a, C> {
    type Item = C::Value;
    type IntoIter = Iter<'a, C>;

    #[inline]
    fn into_iter(self) -> Iter<'a, C> {
        self.iter()
    }
}

impl<C: Codec> fmt::Debug for PackedSlice<'_, C>
where
    C::Value: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

/// A mutable view of `len` values of the codec `C`, packed back to back
/// into a byte slice. See [`PackedSlice`] for the layout.
///
/// ```
/// use quantbits::{Int, PackedSliceMut};
///
/// let mut buffer = [0u8; 4];
/// let mut values = PackedSliceMut::<Int<i8, 5>>::new(&mut buffer, 6).unwrap();
/// values.set(0, -16);
/// values.set(5, 15);
/// assert_eq!(values.get(0), Some(-16));
/// assert_eq!(values.get(5), Some(15));
/// assert!(values.try_set(1, 16).is_err());
/// ```
pub struct PackedSliceMut<'a, C> {
    bytes: &'a mut [u8],
    len: usize,
    codec: PhantomData<fn() -> C>,
}

impl<'a, C: Codec> PackedSliceMut<'a, C> {
    /// A view of the first `len` values in `bytes`, or `None` if `bytes` is
    /// too short. Bytes after the last value are never changed.
    #[inline]
    pub fn new(bytes: &'a mut [u8], len: usize) -> Option<Self> {
        let needed = byte_len(len, Width::<C>::BITS)?;
        (bytes.len() >= needed).then_some(Self { bytes, len, codec: PhantomData })
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

    /// A read-only view of the same values.
    #[inline]
    pub fn as_slice(&self) -> PackedSlice<'_, C> {
        PackedSlice { bytes: self.bytes, len: self.len, codec: PhantomData }
    }

    /// The value at `index`, or `None` if out of bounds.
    #[inline]
    pub fn get(&self, index: usize) -> Option<C::Value> {
        self.as_slice().get(index)
    }

    /// The raw bits at `index`, or `None` if out of bounds.
    #[inline]
    pub fn get_raw(&self, index: usize) -> Option<u64> {
        self.as_slice().get_raw(index)
    }

    /// The first value, or `None` if empty.
    #[inline]
    pub fn first(&self) -> Option<C::Value> {
        self.as_slice().first()
    }

    /// The last value, or `None` if empty.
    #[inline]
    pub fn last(&self) -> Option<C::Value> {
        self.as_slice().last()
    }

    /// An iterator over the values.
    #[inline]
    pub fn iter(&self) -> Iter<'_, C> {
        self.as_slice().iter()
    }

    /// The bytes holding the values, see [`PackedSlice::as_bytes`].
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        self.as_slice().as_bytes()
    }

    /// Sets the value at `index`, see [`Codec::encode`] for values that do not
    /// fit. Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn set(&mut self, index: usize, value: C::Value) {
        self.set_raw(index, C::encode(value));
    }

    /// Sets the value at `index` if it fits, otherwise leaves it unchanged.
    /// Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn try_set(&mut self, index: usize, value: C::Value) -> Result<(), OutOfRange> {
        self.check(index);
        self.set_raw(index, C::try_encode(value)?);
        Ok(())
    }

    /// Sets the raw bits at `index`; only the lowest `C::BITS` bits are used.
    /// Panics if `index` is out of bounds.
    #[inline]
    #[track_caller]
    pub fn set_raw(&mut self, index: usize, raw: u64) {
        self.check(index);
        let bits = Width::<C>::BITS;
        write_at(self.bytes, index * bits as usize, bits, raw);
    }

    #[inline]
    #[track_caller]
    fn check(&self, index: usize) {
        if index >= self.len {
            index_out_of_bounds(index, self.len);
        }
    }
}

impl<C: StochasticCodec> PackedSliceMut<'_, C> {
    /// Sets the value at `index` with stochastic rounding, see
    /// [Stochastic rounding](crate#stochastic-rounding). Panics if `index` is
    /// out of bounds.
    #[inline]
    #[track_caller]
    pub fn set_stochastic(&mut self, index: usize, value: C::Value, rng: &mut impl RandomBits) {
        self.check(index);
        self.set_raw(index, C::encode_stochastic(value, rng.next_u64()));
    }

    /// Adds `delta` to the value at `index` with stochastic rounding, so that
    /// many small increments add up correctly on average. Panics if `index` is
    /// out of bounds.
    #[inline]
    #[track_caller]
    pub fn add_stochastic(&mut self, index: usize, delta: C::Value, rng: &mut impl RandomBits) {
        let Some(raw) = self.get_raw(index) else { index_out_of_bounds(index, self.len) };
        self.set_raw(index, C::add_stochastic(raw, delta, rng.next_u64()));
    }

    /// Like [`add_stochastic`](Self::add_stochastic) but subtracts `delta`.
    #[inline]
    #[track_caller]
    pub fn sub_stochastic(&mut self, index: usize, delta: C::Value, rng: &mut impl RandomBits)
    where
        C::Value: Neg<Output = C::Value>,
    {
        self.add_stochastic(index, -delta, rng);
    }
}

impl<'a, C: Codec> IntoIterator for &'a PackedSliceMut<'_, C> {
    type Item = C::Value;
    type IntoIter = Iter<'a, C>;

    #[inline]
    fn into_iter(self) -> Iter<'a, C> {
        self.iter()
    }
}

impl<C: Codec> fmt::Debug for PackedSliceMut<'_, C>
where
    C::Value: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

/// An iterator over the values of a [`PackedSlice`], [`PackedSliceMut`] or
/// `PackedVec`.
pub struct Iter<'a, C> {
    bytes: &'a [u8],
    front: usize,
    back: usize,
    codec: PhantomData<fn() -> C>,
}

impl<C> Clone for Iter<'_, C> {
    #[inline]
    fn clone(&self) -> Self {
        Self { bytes: self.bytes, front: self.front, back: self.back, codec: PhantomData }
    }
}

impl<C: Codec> Iterator for Iter<'_, C> {
    type Item = C::Value;

    #[inline]
    fn next(&mut self) -> Option<C::Value> {
        if self.front == self.back {
            return None;
        }
        let bits = Width::<C>::BITS;
        let raw = read_at(self.bytes, self.front * bits as usize, bits);
        self.front += 1;
        Some(C::decode(raw))
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<C::Value> {
        self.front = self.front.saturating_add(n).min(self.back);
        self.next()
    }

    #[inline]
    fn count(self) -> usize {
        self.back - self.front
    }

    #[inline]
    fn last(mut self) -> Option<C::Value> {
        self.next_back()
    }
}

impl<C: Codec> DoubleEndedIterator for Iter<'_, C> {
    #[inline]
    fn next_back(&mut self) -> Option<C::Value> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        let bits = Width::<C>::BITS;
        Some(C::decode(read_at(self.bytes, self.back * bits as usize, bits)))
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<C::Value> {
        self.back = self.back.saturating_sub(n).max(self.front);
        self.next_back()
    }
}

impl<C: Codec> ExactSizeIterator for Iter<'_, C> {}

impl<C: Codec> FusedIterator for Iter<'_, C> {}

impl<C: Codec> fmt::Debug for Iter<'_, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Iter").field("remaining", &(self.back - self.front)).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplitMix64;

    #[test]
    fn read_write_at_every_offset_and_width() {
        // Like the test of `read_bits`/`write_bits`, but with buffers that end
        // right after the value, so that both the fast and the slow path run.
        let mut rng = SplitMix64::new(7);
        for bits in 1..=64u32 {
            for bit in 0..80usize {
                for extra in [0usize, 1, 7, 8, 16] {
                    let n = (bit + bits as usize).div_ceil(8) + extra;
                    let mut storage = [0u8; 40];
                    for b in storage.iter_mut() {
                        *b = rng.next_u64() as u8;
                    }
                    let before = storage;
                    let buf = &mut storage[..n];
                    let value = rng.next_u64() & mask(bits);
                    write_at(buf, bit, bits, value);
                    assert_eq!(read_at(buf, bit, bits), value, "bits {bits} at {bit}, extra {extra}");
                    // Every other bit is unchanged.
                    for j in (0..n * 8).filter(|&j| j < bit || j >= bit + bits as usize) {
                        let get = |v: &[u8]| (v[j / 8] >> (j % 8)) & 1;
                        assert_eq!(get(buf), get(&before[..n]), "bit {j} changed");
                    }
                }
            }
        }
    }
}
