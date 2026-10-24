#[quantbits::bitfields]
struct S {
    #[bits(9)]
    a: u8,
    #[bits(0)]
    b: i16,
    c: f32,
}

fn main() {}
