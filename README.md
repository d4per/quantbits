# quantbits

Packed bit fields of arbitrary width, stored in a byte array of exactly the
size needed. Supports unsigned and signed integers, `bool`, enums, and floats
quantized to a range with a given number of bits. There is no upper limit on
the total size; each individual field can be 1–64 bits.

```toml
[dependencies]
quantbits = "0.2"
```

```rust
use quantbits::bitfields;

#[bitfields]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Packet {
    #[bits(5)]                       pub id: u8,      // 0..=31
    #[bits(7)]                       pub delta: i8,   // -64..=63
    #[bits(3, range = 0.0..1.0)]     pub volume: f32, // 0.0, 0.125, … 0.875
    #[bits(6, range = -1.0..=1.0)]   pub pan: f32,    // -1.0 … 1.0, both endpoints included
                                     pub muted: bool, // always 1 bit
}

let p = Packet::new()
    .with_id(17)
    .with_delta(-20)
    .with_volume(0.3)
    .with_pan(1.0)
    .with_muted(true);

assert_eq!(p.id(), 17);
assert_eq!(p.volume(), 0.25);   // 0.3 is rounded to the nearest level
assert_eq!(Packet::BYTES, 3);
assert_eq!(core::mem::size_of::<Packet>(), 3);
```

Put `#[bitfields]` *before* `#[derive(...)]`.

For many values, `PackedVec` stores each in exactly the bits it needs, and
`#[bitfields(columns = Name)]` generates a struct of arrays with one packed
column per field (both need the `alloc` feature):

```rust
use quantbits::{codec, Int, PackedVec};

#[codec(7, range = -2.0..=3.0)]
pub type Pan = f32; // 7-bit floats from -2.0 to 3.0

let mut ids: PackedVec<Int<u16, 13>> = PackedVec::new();
let mut pans: PackedVec<Pan> = PackedVec::new();
for i in 0..1000 {
    ids.push(i * 8);
    pans.push(i as f32 / 200.0 - 2.0);
}
assert_eq!(ids.as_bytes().len(), 1625); // 13 bits each
assert_eq!(pans.as_bytes().len(), 875); // 7 bits each
```

## Features

- Getters, setters and `with_` builders for every field, plus constants such as
  `ID_BITS`, `VOLUME_MAX` and `VOLUME_STEP`.
- `const fn` accessors, so values can be built at compile time.
- A platform-independent little-endian layout with `to_bytes`/`from_bytes`, for
  sending over the network or writing to a file.
- Fieldless enums via `#[derive(BitEnum)]`, with a `#[fallback]` variant for
  unknown bit patterns.
- Cyclic float fields (`wrap`) for angles and other quantities that wrap around.
- Stochastic rounding, so that many small increments add up correctly even
  with few bits.
- Packed vectors and slices of N-bit values (`PackedVec`, `PackedSlice`), and
  struct-of-arrays tables generated from a `#[bitfields]` struct.
- `Debug`, `Default`, `PartialOrd` and `Ord` that work field by field.
- Clear compile errors for invalid declarations.
- `#![no_std]`, no `unsafe` and no runtime dependencies. `PackedVec` and
  the struct-of-arrays tables need the optional `alloc` feature.

See the [documentation](https://docs.rs/quantbits) for details.

## Minimum supported Rust version

Rust 1.84.

## License

Licensed under the [MIT license](LICENSE-MIT).
