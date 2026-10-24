//! Constants, `const fn`, `try_set`, cyclic fields and enums.

use core::f32::consts::PI;
use quantbits::{bitfields, BitEnum, OutOfRange};

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
struct Wide {
    a: u64,
    b: i64,
    #[bits(1)]
    c: i8,
}

#[test]
fn field_constants() {
    assert_eq!((Packet::ID_BITS, Packet::ID_OFFSET), (5, 0));
    assert_eq!((Packet::DELTA_BITS, Packet::DELTA_OFFSET), (7, 5));
    assert_eq!((Packet::VOLUME_BITS, Packet::VOLUME_OFFSET), (3, 12));
    assert_eq!((Packet::MUTED_BITS, Packet::MUTED_OFFSET), (1, 21));

    assert_eq!((Packet::ID_MIN, Packet::ID_MAX), (0, 31));
    assert_eq!((Packet::DELTA_MIN, Packet::DELTA_MAX), (-64, 63));
    assert_eq!((Packet::VOLUME_MIN, Packet::VOLUME_MAX, Packet::VOLUME_STEP), (0.0, 0.875, 0.125));
    assert_eq!((Packet::PAN_MIN, Packet::PAN_MAX, Packet::PAN_STEP), (-1.0, 1.0, 2.0 / 63.0));

    assert_eq!((Wide::A_MIN, Wide::A_MAX), (0, u64::MAX));
    assert_eq!((Wide::B_MIN, Wide::B_MAX), (i64::MIN, i64::MAX));
    assert_eq!((Wide::C_MIN, Wide::C_MAX), (-1, 0));

    // MIN and MAX are values the field can actually hold.
    let p = Packet::new()
        .with_id(Packet::ID_MAX)
        .with_delta(Packet::DELTA_MIN)
        .with_volume(Packet::VOLUME_MAX)
        .with_pan(Packet::PAN_MAX);
    assert_eq!((p.id(), p.delta(), p.volume(), p.pan()), (31, -64, 0.875, 1.0));
}

const BUILT: Packet = Packet::new().with_id(9).with_delta(-3).with_volume(0.25).with_muted(true);
const _: () = assert!(BUILT.id() == 9 && BUILT.delta() == -3 && BUILT.volume() == 0.25 && BUILT.muted());

const fn bump(mut p: Packet) -> Packet {
    p.set_id(p.id() + 1);
    match p.try_set_delta(100) {
        Ok(()) => panic!(),
        Err(_) => p,
    }
}
const BUMPED: Packet = bump(BUILT);

#[test]
fn const_fn_accessors() {
    assert_eq!(
        BUILT.to_bytes(),
        Packet::new().with_id(9).with_delta(-3).with_volume(0.25).with_muted(true).to_bytes()
    );
    assert_eq!((BUMPED.id(), BUMPED.delta()), (10, -3));
}

#[test]
fn try_set() {
    let mut p = Packet::new().with_id(4).with_delta(5);
    assert_eq!(p.try_set_id(31), Ok(()));
    assert_eq!(p.id(), 31);

    let err = p.try_set_id(32).unwrap_err();
    assert_eq!((err.field, err.bits), ("id", 5));
    assert_eq!(err.to_string(), "value for `id` does not fit in 5 bits");
    assert_eq!(p.id(), 31);

    assert!(p.try_set_delta(-64).is_ok());
    assert!(p.try_set_delta(63).is_ok());
    assert!(p.try_set_delta(64).is_err());
    assert!(p.try_set_delta(-65).is_err());
    assert_eq!(p.delta(), 63);

    let boxed: Box<dyn std::error::Error> = Box::new(err);
    assert!(boxed.downcast_ref::<OutOfRange>().is_some());
}

#[bitfields]
#[derive(Clone, Copy, Debug)]
struct Angles {
    #[bits(10, range = -PI..PI, wrap)]
    yaw: f32,
    #[bits(3, range = 0.0..8.0, wrap)]
    small: f64,
    #[bits(3, range = 0.0..8.0)]
    clamped: f64,
}

#[test]
fn wrap_goes_around_instead_of_clamping() {
    let mut a = Angles::new();
    a.set_yaw(PI - 0.0001);
    assert_eq!(a.yaw(), -PI);
    a.set_yaw(PI);
    assert_eq!(a.yaw(), -PI);

    let step = Angles::YAW_STEP;
    for i in -3000..3000 {
        let v = i as f32 * 0.0137;
        a.set_yaw(v);
        // The distance around the circle should be at most half a step.
        let d = (a.yaw() - v).rem_euclid(2.0 * PI);
        let d = d.min(2.0 * PI - d);
        assert!(d <= step / 2.0 + 1e-4, "v={v} got {}", a.yaw());
    }

    for (v, wrapped, clamped) in [(7.6, 0.0, 7.0), (9.2, 1.0, 7.0), (-0.6, 7.0, 0.0), (-17.0, 7.0, 0.0)] {
        a.set_small(v);
        a.set_clamped(v);
        assert_eq!((a.small(), a.clamped()), (wrapped, clamped), "v={v}");
    }
    assert_eq!(Angles::SMALL_MAX, 7.0);
}

#[derive(BitEnum, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Stance {
    Standing,
    Crouching,
    Prone,
    Swimming,
}

#[derive(BitEnum, Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum Weapon {
    Sword = 3,
    #[fallback]
    Unarmed = 0,
    Bow = 5,
}

/// A hand-written [`BitEnum`]: percent 0..=100 in 7 bits, larger values are clamped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Percent(u8);

impl BitEnum for Percent {
    const BITS: u32 = 7;
    fn into_bits(self) -> u64 {
        self.0 as u64
    }
    fn from_bits(bits: u64) -> Self {
        Percent(bits.min(100) as u8)
    }
}

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
struct Player {
    #[bits(2)]
    stance: Stance,
    #[bits(4)]
    weapon: Weapon,
    #[bits(3)]
    spare_stance: Stance,
    #[bits(7)]
    health: Percent,
}

#[test]
fn enum_bits() {
    assert_eq!(Stance::BITS, 2);
    assert_eq!(Weapon::BITS, 3);
    assert_eq!(Player::USED_BITS, 16);

    assert_eq!(Weapon::Bow.into_bits(), 5);
    assert_eq!(Weapon::from_bits(3), Weapon::Sword);
    assert_eq!(Weapon::from_bits(4), Weapon::Unarmed);
    assert_eq!(Weapon::from_bits(u64::MAX), Weapon::Unarmed);
}

#[test]
fn enum_fields_roundtrip() {
    let mut p = Player::new();
    for s in [Stance::Standing, Stance::Crouching, Stance::Prone, Stance::Swimming] {
        for w in [Weapon::Sword, Weapon::Unarmed, Weapon::Bow] {
            p.set_stance(s);
            p.set_weapon(w);
            p.set_spare_stance(s);
            p.set_health(Percent(77));
            assert_eq!((p.stance(), p.weapon(), p.spare_stance(), p.health()), (s, w, s, Percent(77)));
        }
    }
    assert_eq!(
        format!("{:?}", Player::new().with_weapon(Weapon::Bow)),
        "Player { stance: Standing, weapon: Bow, spare_stance: Standing, health: Percent(0) }"
    );
}

#[test]
fn enum_fields_from_arbitrary_bytes() {
    // All bits set: weapon = 15 (unknown -> fallback), spare_stance = 7 (wider than
    // the enum -> the upper bits are ignored), health = 127 (clamped by from_bits).
    let p = Player::from_bytes([0xFF, 0xFF]);
    assert_eq!(p.stance(), Stance::Swimming);
    assert_eq!(p.weapon(), Weapon::Unarmed);
    assert_eq!(p.spare_stance(), Stance::Swimming);
    assert_eq!(p.health(), Percent(100));

    // The raw bits are kept: the fields read the same as a value built from
    // them, but comparisons see the original bits.
    assert_eq!(p.to_bytes(), [0xFF, 0xFF]);
    let rebuilt = Player::new()
        .with_stance(p.stance())
        .with_weapon(p.weapon())
        .with_spare_stance(p.spare_stance())
        .with_health(p.health());
    assert_eq!(format!("{p:?}"), format!("{rebuilt:?}"));
    assert_ne!(p, rebuilt);
    assert!(p > rebuilt);
}

#[test]
fn enum_fields_order_by_discriminant() {
    let a = Player::new().with_stance(Stance::Crouching).with_weapon(Weapon::Bow);
    let b = Player::new().with_stance(Stance::Prone).with_weapon(Weapon::Unarmed);
    let c = Player::new().with_stance(Stance::Prone).with_weapon(Weapon::Sword);
    assert!(a < b && b < c);
}
