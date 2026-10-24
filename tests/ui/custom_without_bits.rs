use quantbits::BitEnum;

#[derive(BitEnum)]
enum E {
    A,
    B,
}

#[quantbits::bitfields]
struct S {
    e: E,
}

fn main() {}
