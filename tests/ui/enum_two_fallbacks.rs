use quantbits::BitEnum;

#[derive(BitEnum)]
enum E {
    #[fallback]
    A,
    #[fallback]
    B,
    C,
}

fn main() {}
