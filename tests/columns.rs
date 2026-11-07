//! `#[bitfields(columns = Name)]`: a struct of arrays.

use core::f64::consts::PI;
use quantbits::{bitfields, BitEnum, Int, OutOfRange, PackedSlice, SplitMix64};

#[derive(BitEnum, Clone, Copy, PartialEq, Debug)]
pub enum Stance {
    Standing,
    Crouching,
    Prone,
    Swimming,
}

#[bitfields(columns = Players)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Player {
    #[bits(20)]
    pub id: u32,
    #[bits(7)]
    pub delta: i8,
    #[bits(8, range = 0.0..=1.0)]
    pub health: f32,
    #[bits(6, range = -PI..PI, wrap)]
    pub heading: f64,
    #[bits(3)]
    pub stance: Stance,
    pub alive: bool,
}

fn player(i: u32) -> Player {
    Player::new()
        .with_id(i * 1000)
        .with_delta(i as i8 - 30)
        .with_health(i as f32 / 60.0)
        .with_heading(i as f64 * 0.3)
        .with_stance(Stance::from_bits(i as u64 % 4))
        .with_alive(i % 3 != 0)
}

#[test]
fn rows_round_trip() {
    let rows: Vec<Player> = (0..60).map(player).collect();
    let mut table: Players = rows.iter().copied().collect();
    assert_eq!(table.len(), 60);
    assert_eq!(table.iter().collect::<Vec<_>>(), rows);
    assert_eq!(table.iter().next_back(), Some(rows[59]));
    assert_eq!(table.get(7), Some(rows[7]));
    assert_eq!(table.get(60), None);

    table.set(7, rows[8]);
    assert_eq!(table.get(7), Some(rows[8]));

    assert_eq!(table.pop(), Some(rows[59]));
    table.truncate(10);
    assert_eq!(table.len(), 10);
    table.extend(rows[10..12].iter().copied());
    assert_eq!(table.get(11), Some(rows[11]));
    table.clear();
    assert!(table.is_empty());
    assert_eq!(table, Players::new());
    assert_eq!(table, Players::default());
}

#[test]
fn field_accessors_match_the_struct() {
    let rows: Vec<Player> = (0..60).map(player).collect();
    let table: Players = rows.iter().copied().collect();
    for (i, p) in rows.iter().enumerate() {
        assert_eq!(table.id(i), p.id());
        assert_eq!(table.delta(i), p.delta());
        assert_eq!(table.health(i), p.health());
        assert_eq!(table.heading(i), p.heading());
        assert_eq!(table.stance(i), p.stance());
        assert_eq!(table.alive(i), p.alive());
    }
    assert_eq!(table.health_iter().collect::<Vec<_>>(), rows.iter().map(|p| p.health()).collect::<Vec<_>>());
    assert_eq!(table.alive_iter().filter(|&a| a).count(), 40);
    assert_eq!(table.stance_iter().len(), 60);
}

#[test]
fn field_setters() {
    let mut table: Players = (0..5).map(player).collect();
    table.set_id(2, 999_999);
    table.set_delta(2, -64);
    table.set_health(2, 2.0); // clamped
    table.set_heading(2, PI); // wraps to -PI
    table.set_stance(2, Stance::Swimming);
    table.set_alive(2, false);
    let p = table.get(2).unwrap();
    assert_eq!(
        (p.id(), p.delta(), p.health(), p.heading(), p.stance(), p.alive()),
        (999_999, -64, 1.0, -PI, Stance::Swimming, false)
    );
    // The other rows are unchanged.
    assert_eq!(table.get(1), Some(player(1)));
    assert_eq!(table.get(3), Some(player(3)));

    let err: OutOfRange = table.try_set_delta(0, 64).unwrap_err();
    assert_eq!((err.field, err.bits), ("delta", 7));
    assert_eq!(table.delta(0), -30);
    assert!(table.try_set_id(0, 1 << 20).is_err());
    assert_eq!(table.try_set_id(0, 5), Ok(()));
    assert_eq!(table.id(0), 5);
}

#[test]
#[should_panic = "index out of bounds: the len is 5 but the index is 5"]
fn field_getter_out_of_bounds_panics() {
    let table: Players = (0..5).map(player).collect();
    table.health(5);
}

#[test]
#[should_panic = "index out of bounds: the len is 0 but the index is 0"]
fn row_setter_out_of_bounds_panics() {
    Players::new().set(0, player(0));
}

#[test]
fn stochastic() {
    let mut rng = SplitMix64::new(9);
    let mut table: Players = (0..100).map(|_| Player::new()).collect();
    for _ in 0..5000 {
        for i in 0..100 {
            table.add_health_stochastic(i, 0.0001, &mut rng);
        }
    }
    let mean = table.health_iter().map(|h| h as f64).sum::<f64>() / 100.0;
    assert!((mean - 0.5).abs() < 0.02, "{mean}");
    table.sub_health_stochastic(0, 1.0, &mut rng);
    assert_eq!(table.health(0), 0.0);
    table.set_health_stochastic(0, 1.0, &mut rng);
    assert_eq!(table.health(0), 1.0);
}

#[test]
fn columns_are_packed() {
    let table: Players = (0..60).map(player).collect();
    let ids: PackedSlice<'_, Int<u64, 20>> = table.id_column();
    assert_eq!(ids.as_bytes().len(), 60 * 20 / 8);
    assert_eq!(ids.get(3), Some(3000));
    assert_eq!(table.alive_column().as_bytes().len(), 8);
    // Signed fields are stored as two's complement.
    assert_eq!(table.delta_column().get(0), Some(0b1100010)); // -30 in 7 bits
}

#[test]
fn debug_lists_the_rows() {
    let table: Players = (0..2).map(player).collect();
    assert_eq!(format!("{table:?}"), format!("[{:?}, {:?}]", player(0), player(1)));
}

mod private_types {
    // A private enum in a private field works as for the struct itself.
    #[derive(quantbits::BitEnum, Clone, Copy, PartialEq, Debug)]
    enum Mode {
        A,
        B,
    }

    #[quantbits::bitfields(columns = Items)]
    #[derive(Clone, Copy)]
    pub struct Item {
        #[bits(1)]
        mode: Mode,
        #[bits(4)]
        pub count: u8,
    }

    #[test]
    fn works() {
        let mut items = Items::with_capacity(4);
        items.push(Item::new().with_mode(Mode::B).with_count(9));
        assert_eq!(items.mode(0), Mode::B);
        assert_eq!(items.count(0), 9);
        assert_eq!(Mode::A, items.mode_iter().map(|_| Mode::A).next().unwrap());
    }
}
