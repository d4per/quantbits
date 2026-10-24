#[quantbits::bitfields]
struct S {
    #[bits(8, wrap)]
    a: u8,
}

fn main() {}
