//! Stochastic rounding: small increments should add up correctly on average.

use core::f64::consts::PI;
use quantbits::{bitfields, SplitMix64};

#[bitfields]
#[derive(Clone, Copy, Debug)]
struct Fields {
    /// The example from the documentation: 8 bits, step 1/255.
    #[bits(8, range = 0.0..=1.0)]
    level: f32,
    /// Step 1, so the position in steps equals the value.
    #[bits(3, range = 0.0..8.0)]
    unit: f64,
    #[bits(8, range = -PI..PI, wrap)]
    angle: f64,
}

fn mean(xs: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = xs.fold((0.0, 0), |(s, n), x| (s + x, n + 1));
    sum / n as f64
}

#[test]
fn many_small_adds_sum_up() {
    let mut rng = SplitMix64::new(1);
    let mut plain = Fields::new();
    let mut f = Fields::new();
    for _ in 0..5000 {
        plain.set_level(plain.level() + 0.0001);
        f.add_level_stochastic(0.0001, &mut rng);
    }
    assert_eq!(plain.level(), 0.0);
    // A single run has standard deviation ≈ 0.044.
    assert!((f.level() - 0.5).abs() < 0.15, "got {}", f.level());
}

#[test]
fn expected_value_is_exact() {
    // The mean over 400 runs has standard deviation ≈ 0.0022.
    let mut rng = SplitMix64::new(2);
    let up = mean((0..400).map(|_| {
        let mut f = Fields::new();
        for _ in 0..5000 {
            f.add_level_stochastic(0.0001, &mut rng);
        }
        f.level() as f64
    }));
    assert!((up - 0.5).abs() < 0.01, "mean {up}");

    let down = mean((0..400).map(|_| {
        let mut f = Fields::new().with_level(1.0);
        for _ in 0..5000 {
            f.sub_level_stochastic(0.0001, &mut rng);
        }
        f.level() as f64
    }));
    assert!((down - 0.5).abs() < 0.01, "mean {down}");
}

#[test]
fn set_stochastic_rounds_up_with_the_fraction_as_probability() {
    let mut rng = SplitMix64::new(3);
    let mut f = Fields::new();
    let n = 20_000;
    let mut ups = 0;
    for _ in 0..n {
        f.set_unit_stochastic(2.3, &mut rng);
        if f.unit() == 3.0 {
            ups += 1;
        } else {
            assert_eq!(f.unit(), 2.0);
        }
    }
    let p = ups as f64 / n as f64;
    assert!((p - 0.3).abs() < 0.015, "p = {p}");

    // Exactly on a level: never any movement.
    for _ in 0..1000 {
        f.set_unit_stochastic(5.0, &mut rng);
        assert_eq!(f.unit(), 5.0);
    }
}

#[test]
fn clamps_at_the_ends() {
    let mut rng = SplitMix64::new(4);
    let mut f = Fields::new().with_unit(7.0);
    for _ in 0..100 {
        f.add_unit_stochastic(0.6, &mut rng);
        assert_eq!(f.unit(), 7.0);
    }
    for _ in 0..100 {
        f.sub_unit_stochastic(100.0, &mut rng);
        assert_eq!(f.unit(), 0.0);
    }
    f.add_unit_stochastic(f64::NAN, &mut rng);
    assert_eq!(f.unit(), 0.0);
}

#[test]
fn wrap_goes_around() {
    // From near PI, 2000 increments of 0.001 rad = 2 rad, so across the boundary.
    let mut rng = SplitMix64::new(5);
    let start = Fields::new().with_angle(3.0).angle();
    let expected = start + 2.0 - 2.0 * PI;
    let end = mean((0..200).map(|_| {
        let mut f = Fields::new().with_angle(start);
        for _ in 0..2000 {
            f.add_angle_stochastic(0.001, &mut rng);
        }
        f.angle()
    }));
    assert!((end - expected).abs() < 0.06, "mean {end}, expected {expected}");
}

#[test]
fn closures_are_random_sources() {
    // The random number 0 always means up, u64::MAX never.
    let mut f = Fields::new();
    f.add_unit_stochastic(1e-9, &mut || 0u64);
    assert_eq!(f.unit(), 1.0);
    f.add_unit_stochastic(0.999, &mut || u64::MAX);
    assert_eq!(f.unit(), 1.0);

    let mut calls = 0;
    let mut counting = || {
        calls += 1;
        u64::MAX / 2
    };
    f.add_unit_stochastic(0.6, &mut counting); // 0.6 > 0.5 => up
    f.add_unit_stochastic(0.4, &mut counting); // 0.4 < 0.5 => stays
    assert_eq!(f.unit(), 2.0);
    assert_eq!(calls, 2);
}

#[test]
fn splitmix64_reference_values() {
    let mut rng = SplitMix64::new(0);
    assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
    assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
    assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
}
