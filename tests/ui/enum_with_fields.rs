use quantbits::BitEnum;

#[derive(BitEnum)]
enum E {
    A(u8),
    B { x: u8 },
    C,
}

fn main() {}
