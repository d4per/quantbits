#[quantbits::bitfields]
struct S {
    #[bits(4, range = 1.0..0.0)]
    a: f32,
}

fn main() {}
