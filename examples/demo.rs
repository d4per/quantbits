use core::f32::consts::PI;
use quantbits::{bitfields, SplitMix64};

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sample {
    #[bits(3, range = 0.0..1.0)]
    pub half_open: f32,
    #[bits(3, range = 0.0..=1.0)]
    pub closed: f32,
    #[bits(4)]
    pub level: i8,
    pub flag: bool,
}

/// A slowly charging meter with only 8 bits.
#[rustfmt::skip]
#[bitfields]
#[derive(Clone, Copy, Debug)]
pub struct Charge {
    #[bits(8, range = 0.0..=1.0)] pub level: f32,
}

/// A network update for a game entity, 151 bits = 19 bytes.
#[rustfmt::skip]
#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct EntityUpdate {
    #[bits(20)]                          pub entity: u32,
    #[bits(18, range = -2048.0..2048.0)] pub x: f32,
    #[bits(18, range = -2048.0..2048.0)] pub y: f32,
    #[bits(14, range = 0.0..512.0)]      pub z: f32,
    #[bits(10, range = -PI..PI, wrap)]   pub yaw: f32,
    #[bits(8, range = -PI..PI)]          pub pitch: f32,
    #[bits(7, range = 0.0..=1.0)]        pub health: f32,
    #[bits(12)]                          pub velocity_x: i16,
    #[bits(12)]                          pub velocity_y: i16,
    #[bits(31)]                          pub tick: u32,
                                         pub crouching: bool,
}

fn main() {
    println!("Sample: {} bits in {} bytes\n", Sample::USED_BITS, Sample::BYTES);

    let mut s = Sample::new();
    print!("range = 0.0..1.0,  3 bits: ");
    for q in 0..8 {
        s.set_half_open(q as f32 / 8.0);
        print!("{:.4} ", s.half_open());
    }
    print!("\nrange = 0.0..=1.0, 3 bits: ");
    for q in 0..8 {
        s.set_closed(q as f32 / 7.0);
        print!("{:.4} ", s.closed());
    }
    println!("\n");

    let u = EntityUpdate::new()
        .with_entity(123_456)
        .with_x(-311.27)
        .with_y(1024.5)
        .with_z(42.0)
        .with_yaw(1.2)
        .with_pitch(-0.3)
        .with_health(0.85)
        .with_velocity_x(-700)
        .with_velocity_y(55)
        .with_tick(9_000_000)
        .with_crouching(true);

    println!(
        "EntityUpdate: {} bits in {} bytes (size_of = {})",
        EntityUpdate::USED_BITS,
        EntityUpdate::BYTES,
        core::mem::size_of::<EntityUpdate>()
    );
    println!("{u:#?}");
    println!("Raw bytes: {:02x?}", u.to_bytes());

    println!("\nCharge: step {:.5}, 5000 increments of 0.0001", Charge::LEVEL_STEP);
    let mut rng = SplitMix64::new(2026);
    let mut plain = Charge::new();
    let mut stochastic = Charge::new();
    for i in 1..=5000 {
        plain.set_level(plain.level() + 0.0001);
        stochastic.add_level_stochastic(0.0001, &mut rng);
        if i % 1000 == 0 {
            println!(
                "  after {i:4}: plain {:.4}, stochastic {:.4} (exact {:.4})",
                plain.level(),
                stochastic.level(),
                i as f32 * 0.0001
            );
        }
    }
}
