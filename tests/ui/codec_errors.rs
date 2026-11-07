use quantbits::codec;

#[codec(5)]
struct NotAnAlias;

#[codec]
type NoBits = u8;

#[codec(5)]
type NoRange = f32;

#[codec(2)]
type TwoBitBool = bool;

#[codec(5, range = 0.0..1.0)]
type RangeOnInt = u8;

#[codec(9)]
type TooWide = u8;

#[codec(5)]
type Generic<T> = T;

#[codec(4, range = 1.0..1.0)]
type EmptyRange = f32;

fn main() {}
