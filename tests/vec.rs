//! `PackedVec`, the packed slices and the codecs.

use core::f32::consts::PI;
use quantbits::{
    bitfields, codec, BitEnum, Codec, Int, OutOfRange, PackedSlice, PackedSliceMut, PackedVec, SplitMix64,
    StochasticCodec,
};

/// A random value that fits in `bits` bits, as unsigned and as sign-extended.
fn random_bits(rng: &mut SplitMix64, bits: u32) -> (u64, i64) {
    let raw = rng.next_u64() & (u64::MAX >> (64 - bits));
    let shift = 64 - bits;
    (raw, ((raw << shift) as i64) >> shift)
}

macro_rules! check_widths {
    ($rng:ident, $($n:literal)*) => {$(
        check_width::<Int<u64, $n>, Int<i64, $n>>(&mut $rng, $n);
    )*};
}

fn check_width<U, S>(rng: &mut SplitMix64, bits: u32)
where
    U: Codec<Value = u64>,
    S: Codec<Value = i64>,
{
    let mut unsigned: PackedVec<U> = PackedVec::new();
    let mut signed: PackedVec<S> = PackedVec::new();
    let mut expected_u = Vec::new();
    let mut expected_s = Vec::new();
    for _ in 0..50 {
        let (u, s) = random_bits(rng, bits);
        unsigned.push(u);
        signed.push(s);
        expected_u.push(u);
        expected_s.push(s);
    }
    // Overwrite some values in the middle.
    for i in (0..50).step_by(7) {
        let (u, s) = random_bits(rng, bits);
        unsigned.set(i, u);
        signed.set(i, s);
        expected_u[i] = u;
        expected_s[i] = s;
    }
    assert_eq!(unsigned.iter().collect::<Vec<_>>(), expected_u, "{bits} bits");
    assert_eq!(signed.iter().collect::<Vec<_>>(), expected_s, "{bits} bits");
    assert_eq!(
        unsigned.iter().rev().collect::<Vec<_>>(),
        expected_u.iter().rev().copied().collect::<Vec<_>>()
    );
    assert_eq!(unsigned.as_bytes().len(), (50 * bits as usize).div_ceil(8));

    // Round trip through bytes.
    let copy = PackedVec::<U>::from_bytes(unsigned.as_bytes().to_vec(), 50).unwrap();
    assert_eq!(copy, unsigned);

    // Popping clears the bits, so the bytes equal those of a shorter vector.
    for _ in 0..13 {
        assert_eq!(unsigned.pop(), expected_u.pop());
    }
    let shorter: PackedVec<U> = expected_u.iter().copied().collect();
    assert_eq!(unsigned, shorter);
    assert_eq!(unsigned.as_bytes(), shorter.as_bytes());
}

#[test]
fn every_width_matches_a_plain_vec() {
    let mut rng = SplitMix64::new(3);
    check_widths!(rng, 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
                       33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64);
}

#[test]
fn ints_of_other_types() {
    let mut v: PackedVec<Int<u16, 13>> = PackedVec::new();
    v.push(8191);
    let err = v.try_push(8192).unwrap_err();
    assert_eq!((err.field, err.bits), ("element", 13));
    assert_eq!(v.len(), 1);
    let mut s: PackedVec<Int<i8, 3>> = [-4, -1, 0, 3].into_iter().collect();
    assert_eq!(s.iter().collect::<Vec<_>>(), [-4, -1, 0, 3]);
    assert!(s.try_set(0, 4).is_err());
    assert_eq!(s.get(0), Some(-4));
    assert_eq!(format!("{s:?}"), "[-4, -1, 0, 3]");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic = "does not fit"]
fn int_overflow_panics_in_debug() {
    PackedVec::<Int<u8, 3>>::new().push(8);
}

#[test]
fn truncate_resize_clear() {
    let mut v: PackedVec<Int<u8, 5>> = (0..20).collect();
    v.truncate(9);
    assert_eq!(v.len(), 9);
    assert_eq!(v, (0..9).collect());
    v.resize(12, 31);
    assert_eq!(v.iter().skip(8).collect::<Vec<_>>(), [8, 31, 31, 31]);
    v.resize(2, 0);
    assert_eq!(v.iter().collect::<Vec<_>>(), [0, 1]);
    v.clear();
    assert!(v.is_empty());
    assert_eq!(v.pop(), None);
    assert_eq!(v, PackedVec::default());
}

#[test]
fn capacity() {
    let mut v = PackedVec::<Int<u8, 3>>::with_capacity(100);
    assert!(v.capacity() >= 100);
    v.reserve(1000);
    assert!(v.capacity() >= 1000);
}

#[test]
fn from_bytes_checks_the_length_and_clears_the_tail() {
    assert!(PackedVec::<Int<u8, 3>>::from_bytes(vec![0; 2], 2).is_none()); // 6 bits fit in 1 byte
    assert!(PackedVec::<Int<u8, 3>>::from_bytes(vec![0; 2], 6).is_none());
    let v = PackedVec::<Int<u8, 3>>::from_bytes(vec![0xff, 0xff], 4).unwrap();
    assert_eq!(v.as_bytes(), [0xff, 0x0f]);
    assert_eq!(v.iter().collect::<Vec<_>>(), [7, 7, 7, 7]);
    assert_eq!(v.into_bytes(), [0xff, 0x0f]);
}

#[test]
#[should_panic = "index out of bounds: the len is 2 but the index is 2"]
fn set_out_of_bounds_panics() {
    let mut v: PackedVec<bool> = [true, false].into_iter().collect();
    v.set(2, true);
}

#[test]
fn bools() {
    let v: PackedVec<bool> = (0..20).map(|i| i % 3 == 0).collect();
    assert_eq!(v.as_bytes().len(), 3);
    assert_eq!(v.iter().filter(|&b| b).count(), 7);
}

#[test]
fn slices_over_buffers() {
    // 4 bytes hold 6 five-bit values; the bytes after them stay untouched.
    let mut buffer = [0xAAu8; 5];
    let mut s = PackedSliceMut::<Int<u8, 5>>::new(&mut buffer[..4], 6).unwrap();
    for i in 0..6 {
        s.set(i, 31 - i as u8);
    }
    assert_eq!(s.iter().collect::<Vec<_>>(), [31, 30, 29, 28, 27, 26]);
    assert_eq!(s.as_bytes().len(), 4);
    assert_eq!(buffer[4], 0xAA);
    assert_eq!(buffer[3] >> 6, 0b10); // 2 bits after the last value
    let r = PackedSlice::<Int<u8, 5>>::new(&buffer, 6).unwrap();
    assert_eq!(r.get(5), Some(26));
    assert_eq!(r.get(6), None);
    assert!(PackedSlice::<Int<u8, 5>>::new(&buffer, 9).is_none());
    assert!(PackedSlice::<Int<u8, 5>>::new(&buffer, usize::MAX).is_none());
}

#[test]
fn iterator_is_exact_and_double_ended() {
    let v: PackedVec<Int<u8, 7>> = (0..10).collect();
    let mut it = v.iter();
    assert_eq!(it.len(), 10);
    assert_eq!(it.next(), Some(0));
    assert_eq!(it.next_back(), Some(9));
    assert_eq!(it.nth(2), Some(3));
    assert_eq!(it.len(), 5);
    assert_eq!(it.nth(100), None);
    assert_eq!(it.next(), None);
    assert_eq!((&v).into_iter().sum::<u8>(), 45);

    let mut it = v.iter();
    assert_eq!(it.nth_back(2), Some(7));
    assert_eq!(it.clone().count(), 7);
    assert_eq!(it.clone().last(), Some(6));
    assert_eq!(it.nth_back(100), None);
    assert_eq!(it.next(), None);
    assert_eq!((v.first(), v.last()), (Some(0), Some(9)));
    assert_eq!((v.as_slice().first(), v.as_slice().last()), (Some(0), Some(9)));
    let empty: PackedVec<Int<u8, 7>> = PackedVec::new();
    assert_eq!((empty.first(), empty.last(), empty.iter().last()), (None, None, None));
}

/// `insert`, `remove`, `swap_remove` and `resize` against the same calls on a
/// `Vec`, for widths with and without whole bytes.
fn check_editing<C: Codec<Value = u64>>(rng: &mut SplitMix64, bits: u32) {
    let mut packed: PackedVec<C> = PackedVec::new();
    let mut plain: Vec<u64> = Vec::new();
    for _ in 0..300 {
        let value = if rng.next_u64() % 4 == 0 { 0 } else { random_bits(rng, bits).0 };
        let len = plain.len();
        let index = rng.next_u64() as usize % (len + 1);
        match rng.next_u64() % 5 {
            0 | 1 => {
                packed.insert(index, value);
                plain.insert(index, value);
            }
            2 if index < len => assert_eq!(packed.remove(index), plain.remove(index)),
            3 if index < len => assert_eq!(packed.swap_remove(index), plain.swap_remove(index)),
            _ => {
                let new_len = rng.next_u64() as usize % (len + 6);
                packed.resize(new_len, value);
                plain.resize(new_len, value);
            }
        }
        assert_eq!(packed.iter().collect::<Vec<_>>(), plain, "{bits} bits");
        // The bits after the last value stay zero, so the bytes compare equal.
        assert_eq!(packed, plain.iter().copied().collect::<PackedVec<C>>(), "{bits} bits");
    }
}

#[test]
fn editing_matches_a_plain_vec() {
    let mut rng = SplitMix64::new(11);
    check_editing::<Int<u64, 1>>(&mut rng, 1);
    check_editing::<Int<u64, 7>>(&mut rng, 7);
    check_editing::<Int<u64, 8>>(&mut rng, 8);
    check_editing::<Int<u64, 13>>(&mut rng, 13);
    check_editing::<Int<u64, 16>>(&mut rng, 16);
    check_editing::<Int<u64, 58>>(&mut rng, 58);
    check_editing::<Int<u64, 64>>(&mut rng, 64);
}

#[test]
#[should_panic(expected = "the len is 2 but the index is 3")]
fn insert_past_the_end_panics() {
    let mut v: PackedVec<Int<u8, 3>> = [1, 2].into_iter().collect();
    v.insert(3, 0);
}

#[test]
#[should_panic(expected = "the len is 2 but the index is 2")]
fn remove_out_of_bounds_panics() {
    let mut v: PackedVec<Int<u8, 3>> = [1, 2].into_iter().collect();
    v.remove(2);
}

// ---------------------------------------------------------------------------
// Codecs from `#[codec]`
// ---------------------------------------------------------------------------

/// The example from the question: 7 bits in -2.0..=3.0.
#[codec(7, range = -2.0..=3.0)]
pub type Pan = f32;

#[codec(3, range = -PI..PI, wrap)]
type Angle = f32;

#[codec(13)]
type Id = u16;

#[codec(4)]
type Small = i8;

#[derive(BitEnum, Clone, Copy, PartialEq, Debug)]
enum Weapon {
    #[fallback]
    Unarmed,
    Sword = 3,
    Bow = 5,
}

#[codec(4)]
type WideWeapon = Weapon;

#[bitfields]
#[derive(Clone, Copy, PartialEq, Debug)]
struct Same {
    #[bits(7, range = -2.0..=3.0)]
    pan: f32,
    #[bits(3, range = -PI..PI, wrap)]
    angle: f32,
}

#[test]
fn float_codec_matches_a_field() {
    assert_eq!(<Pan as Codec>::BITS, 7);
    assert_eq!((Pan::MIN, Pan::MAX, Pan::STEP), (Same::PAN_MIN, Same::PAN_MAX, Same::PAN_STEP));
    let mut v: PackedVec<Pan> = PackedVec::new();
    let mut a: PackedVec<Angle> = PackedVec::new();
    for i in -30..=40 {
        let x = i as f32 / 10.0;
        v.push(x);
        a.push(x * 2.0);
        let s = Same::new().with_pan(x).with_angle(x * 2.0);
        assert_eq!(v.get(v.len() - 1), Some(s.pan()), "{x}");
        assert_eq!(a.get(a.len() - 1), Some(s.angle()), "{x}");
    }
    assert_eq!(v.get(0), Some(-2.0)); // clamped
    assert_eq!(v.as_bytes().len(), (71 * 7usize).div_ceil(8));
}

#[test]
fn int_and_enum_codecs() {
    assert_eq!((Id::MIN, Id::MAX), (0, 8191));
    assert_eq!((Small::MIN, Small::MAX), (-8, 7));
    let err: OutOfRange = Id::try_encode(8192).unwrap_err();
    assert_eq!((err.field, err.bits), ("Id", 13));
    let mut w: PackedVec<WideWeapon> = PackedVec::new();
    w.push(Weapon::Bow);
    w.push_raw(0b1111); // a pattern from a newer version
    assert_eq!(w.iter().collect::<Vec<_>>(), [Weapon::Bow, Weapon::Unarmed]);
    assert_eq!(w.get_raw(1), Some(0b1111));
    assert_eq!(<WideWeapon as Codec>::BITS, 4);

    // The enum itself is a codec of `Weapon::BITS` bits.
    let e: PackedVec<Weapon> = [Weapon::Sword, Weapon::Bow].into_iter().collect();
    assert_eq!(<Weapon as Codec>::BITS, 3);
    assert_eq!(e.as_bytes(), [0b101_011]);
}

#[test]
fn stochastic_adds_sum_up() {
    #[codec(8, range = 0.0..=1.0)]
    type Level = f32;

    let mut rng = SplitMix64::new(42);
    let mut v: PackedVec<Level> = PackedVec::new();
    v.resize(100, 0.0);
    for _ in 0..5000 {
        for i in 0..100 {
            v.add_stochastic(i, 0.0001, &mut rng);
        }
    }
    let mean = v.iter().map(|x| x as f64).sum::<f64>() / 100.0;
    assert!((mean - 0.5).abs() < 0.02, "{mean}");
    for i in 0..100 {
        v.sub_stochastic(i, 0.25, &mut rng);
    }
    let mean = v.iter().map(|x| x as f64).sum::<f64>() / 100.0;
    assert!((mean - 0.25).abs() < 0.02, "{mean}");

    // A value between two levels is rounded up with the right probability.
    let mut w: PackedVec<Level> = PackedVec::new();
    let x = 0.3 * Level::STEP;
    for _ in 0..10000 {
        w.push_stochastic(x, &mut rng);
    }
    let ups = w.iter().filter(|&y| y > 0.0).count();
    assert!((2700..3300).contains(&ups), "{ups}");
    w.set_stochastic(0, 1.0, &mut rng);
    assert_eq!(w.get(0), Some(1.0));
    assert_eq!(Level::encode_stochastic(2.0, 0), 255);
}

// ---------------------------------------------------------------------------
// `#[bitfields]` structs as values
// ---------------------------------------------------------------------------

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Packet {
    #[bits(5)]
    pub id: u8,
    #[bits(7)]
    pub delta: i8,
    #[bits(3, range = 0.0..1.0)]
    pub volume: f32,
    #[bits(6, range = -1.0..=1.0)]
    pub pan: f32,
    pub muted: bool,
}

#[bitfields]
#[derive(Clone, Copy)]
struct Full {
    a: u64,
}

#[test]
fn structs_as_values() {
    assert_eq!(<Packet as Codec>::BITS, 22);
    assert_eq!(<Full as Codec>::BITS, 64);
    let packets: Vec<Packet> = (0..10)
        .map(|i| Packet::new().with_id(i).with_delta(-(i as i8)).with_pan(0.5).with_muted(i % 2 == 0))
        .collect();
    let v: PackedVec<Packet> = packets.iter().copied().collect();
    assert_eq!(v.iter().collect::<Vec<_>>(), packets);
    assert_eq!(v.as_bytes().len(), 28); // 220 bits instead of 30 bytes for [Packet; 10]

    let f: PackedVec<Full> = [Full::new().with_a(u64::MAX), Full::new().with_a(7)].into_iter().collect();
    assert_eq!(f.iter().map(|x| x.a()).collect::<Vec<_>>(), [u64::MAX, 7]);
}

/// A hand-written `BitEnum` on a `#[bitfields]` struct, to nest it in another
/// struct, still compiles next to the generated `Codec` implementation.
#[bitfields]
#[derive(Clone, Copy, PartialEq, Debug)]
struct Inner {
    #[bits(3)]
    a: u8,
}

impl BitEnum for Inner {
    const BITS: u32 = 3;
    fn into_bits(self) -> u64 {
        self.a() as u64
    }
    fn from_bits(bits: u64) -> Self {
        Inner::new().with_a(bits as u8)
    }
}

#[bitfields]
#[derive(Clone, Copy)]
struct Outer {
    #[bits(3)]
    inner: Inner,
}

#[test]
fn hand_written_bit_enum_on_a_bitfields_struct() {
    let o = Outer::new().with_inner(Inner::new().with_a(5));
    assert_eq!(o.inner().a(), 5);
    let v: PackedVec<Inner> = [Inner::new().with_a(6)].into_iter().collect();
    assert_eq!(v.get(0).unwrap().a(), 6);
}
