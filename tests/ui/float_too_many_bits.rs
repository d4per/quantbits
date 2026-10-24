#[quantbits::bitfields]
struct S {
    #[bits(65, range = 0.0..1.0)]
    a: f64,
}

fn main() {}
