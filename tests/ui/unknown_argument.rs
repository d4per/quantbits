#[quantbits::bitfields]
struct S {
    #[bits(3, rnage = 0.0..1.0)]
    a: f32,
}

fn main() {}
