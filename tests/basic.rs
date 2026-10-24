use core::f32::consts::PI;
use quantbits::bitfields;

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Packet {
    /// The sender's id
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

/// More than 128 bits: 3 + 64 + 64 + 64 + 10 + 1 = 206 bits = 26 bytes.
#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Big {
    #[bits(3)]
    tag: u8,
    a: u64, // no #[bits] = full width, starts at bit 3 and spans 9 bytes
    b: i64,
    #[bits(64, range = -1.0e6..=1.0e6)]
    precise: f64,
    #[bits(10, range = -PI..PI)]
    angle: f32,
    flag: bool,
}

/// Well over 32 bytes, so that a custom Default is really needed.
#[bitfields]
#[derive(Clone, PartialEq, Debug, Default)]
struct Huge {
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    #[bits(1)]
    last: bool,
}

#[test]
fn layout() {
    assert_eq!(Packet::USED_BITS, 22);
    assert_eq!(Packet::BYTES, 3);
    assert_eq!(core::mem::size_of::<Packet>(), 3);
    assert_eq!(core::mem::align_of::<Packet>(), 1);

    assert_eq!(Big::USED_BITS, 206);
    assert_eq!(Big::BYTES, 26);
    assert_eq!(core::mem::size_of::<Big>(), 26);

    assert_eq!(Huge::BYTES, 41);
    assert_eq!(Huge::default(), Huge::new());
}

#[test]
fn little_endian_bit_order() {
    let p = Packet::new().with_id(0b10101).with_muted(true);
    // id in bits 0–4, muted in bit 21 => byte 2, bit 5
    assert_eq!(p.to_bytes(), [0b0001_0101, 0, 0b0010_0000]);
    assert_eq!(Packet::from_bytes(p.to_bytes()), p);
}

#[test]
fn ints_roundtrip_and_neighbours_untouched() {
    let mut p = Packet::new();
    for id in 0..32u8 {
        p.set_id(id);
        assert_eq!(p.id(), id);
    }
    for d in -64..=63i8 {
        p.set_delta(d);
        assert_eq!(p.delta(), d);
        assert_eq!(p.id(), 31);
    }

    let mut big = Big::new().with_tag(7).with_flag(true);
    for &a in &[0u64, 1, u64::MAX, 0x8000_0000_0000_0001] {
        for &b in &[i64::MIN, -1, 0, 1, i64::MAX] {
            big.set_a(a);
            big.set_b(b);
            assert_eq!((big.a(), big.b()), (a, b));
            assert_eq!(big.tag(), 7);
            assert!(big.flag());
        }
    }
}

#[test]
fn half_open_float() {
    let mut p = Packet::new();
    for i in 0..8 {
        let v = i as f32 * 0.125;
        p.set_volume(v);
        assert_eq!(p.volume(), v);
    }
    p.set_volume(0.3);
    assert_eq!(p.volume(), 0.25);
    p.set_volume(1.0);
    assert_eq!(p.volume(), 0.875);
    p.set_volume(-5.0);
    assert_eq!(p.volume(), 0.0);
    p.set_volume(f32::NAN);
    assert_eq!(p.volume(), 0.0);
}

#[test]
fn inclusive_float_and_const_expr_range() {
    let mut p = Packet::new();
    p.set_pan(-1.0);
    assert_eq!(p.pan(), -1.0);
    p.set_pan(1.0);
    assert_eq!(p.pan(), 1.0);

    let mut b = Big::new();
    b.set_precise(123.456789);
    assert!((b.precise() - 123.456789).abs() < 1e-9);
    b.set_precise(1.0e6);
    assert_eq!(b.precise(), 1.0e6);

    // 10 bits over [-PI, PI): step 2PI/1024
    let step = quantbits::step(-PI as f64, PI as f64, 10, false) as f32;
    for i in -100..100 {
        let a = i as f32 * 0.031;
        b.set_angle(a);
        assert!((b.angle() - a).abs() <= step / 2.0 + 1e-6, "a={a} got {}", b.angle());
    }
}

#[test]
fn debug_shows_field_values() {
    let p = Packet::new().with_id(3).with_delta(-2).with_volume(0.5).with_pan(1.0);
    let s = format!("{p:?}");
    assert_eq!(s, "Packet { id: 3, delta: -2, volume: 0.5, pan: 1.0, muted: false }");
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "does not fit")]
fn unsigned_overflow_panics_in_debug() {
    Packet::new().set_id(32);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "does not fit")]
fn signed_overflow_panics_in_debug() {
    Packet::new().set_delta(-65);
}

#[test]
fn padding_bits_are_cleared() {
    // Packet uses 22 of 24 bits; the top two in the last byte are padding.
    let a = Packet::from_bytes([0x15, 0x00, 0x20]);
    let b = Packet::from_bytes([0x15, 0x00, 0xE0]);
    assert_eq!(b.to_bytes(), [0x15, 0x00, 0x20]);
    assert_eq!(a, b);

    use std::hash::{BuildHasher, RandomState};
    let s = RandomState::new();
    assert_eq!(s.hash_one(a), s.hash_one(b));
}

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Sortable {
    #[bits(4)]
    a: u8,
    #[bits(3)]
    b: i8,
    #[bits(3, range = -1.0..=1.0)]
    c: f32,
    d: bool,
}

/// Only `PartialOrd`, without `Ord`.
#[bitfields]
#[derive(Clone, Copy, PartialEq, PartialOrd)]
struct PartialOnly {
    #[bits(2)]
    lo: u8,
    #[bits(6)]
    hi: u8,
}

#[test]
fn ord_compares_fields_in_declaration_order() {
    let mut all = Vec::new();
    for a in [0u8, 1, 7, 15] {
        for b in -4..=3i8 {
            for q in 0..8 {
                for d in [false, true] {
                    let c = -1.0 + q as f32 * (2.0 / 7.0);
                    all.push(Sortable::new().with_a(a).with_b(b).with_c(c).with_d(d));
                }
            }
        }
    }
    let key = |s: &Sortable| (s.a(), s.b(), s.c(), s.d());
    for x in &all {
        for y in &all {
            let expected = key(x).partial_cmp(&key(y)).unwrap();
            assert_eq!(x.cmp(y), expected, "{x:?} vs {y:?}");
            assert_eq!(x.partial_cmp(y), Some(expected));
        }
    }

    // `lo` is in the lowest bits but is the first field, so it weighs the most.
    let x = PartialOnly::new().with_lo(1).with_hi(0);
    let y = PartialOnly::new().with_lo(0).with_hi(63);
    assert!(x > y);
}

#[bitfields]
#[derive(Clone, Copy, Debug)]
struct RawIdent {
    #[bits(3)]
    r#type: u8,
}

#[test]
fn raw_identifiers() {
    let r = RawIdent::new().with_type(5);
    assert_eq!(r.r#type(), 5);
    assert_eq!(format!("{r:?}"), "RawIdent { type: 5 }");
}

#[bitfields]
#[derive(Clone, Copy)]
struct Endpoints {
    #[bits(1, range = 0.3..=0.9)]
    a: f64,
    #[bits(4, range = 0.3..=0.9)]
    b: f64,
    #[bits(2, range = -0.1..=0.7)]
    c: f64,
    #[bits(13, range = 0.1..=0.2)]
    d: f32,
}

#[test]
fn inclusive_range_hits_both_endpoints_exactly() {
    let mut e = Endpoints::new();
    e.set_a(0.9);
    e.set_b(0.9);
    e.set_c(0.7);
    e.set_d(0.2);
    assert_eq!((e.a(), e.b(), e.c(), e.d()), (0.9, 0.9, 0.7, 0.2));
    e.set_a(0.3);
    e.set_b(0.3);
    e.set_c(-0.1);
    e.set_d(0.1);
    assert_eq!((e.a(), e.b(), e.c(), e.d()), (0.3, 0.3, -0.1, 0.1));
}
