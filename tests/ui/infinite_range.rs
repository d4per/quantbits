#[quantbits::bitfields]
struct S {
    #[bits(4, range = f64::NEG_INFINITY..f64::INFINITY)]
    infinite: f64,
    #[bits(4, range = -f64::MAX..f64::MAX)]
    too_wide: f64,
    #[bits(4, range = 0.0..1e39)]
    too_large_for_f32: f32,
    #[bits(4, range = 0.0..1e39)]
    fine_for_f64: f64,
}

fn main() {}
