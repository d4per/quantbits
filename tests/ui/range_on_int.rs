#[quantbits::bitfields]
struct S {
    #[bits(8, range = 0.0..1.0)]
    a: u8,
}

fn main() {}
