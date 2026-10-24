#[quantbits::bitfields]
struct S {
    #[bits(8, range = 0.0..=1.0, wrap)]
    a: f32,
}

fn main() {}
