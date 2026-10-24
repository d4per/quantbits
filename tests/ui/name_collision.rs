#[quantbits::bitfields]
struct S {
    #[bits(3)]
    a: u8,
    #[bits(3)]
    set_a: u8,
    new: bool,
}

fn main() {}
