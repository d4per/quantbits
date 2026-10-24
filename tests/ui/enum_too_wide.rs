use quantbits::BitEnum;

#[derive(BitEnum)]
enum E {
    A,
    B,
    C,
    D,
}

#[quantbits::bitfields]
struct S {
    #[bits(1)]
    e: E,
}

fn main() {}
