use quantbits::BitEnum;

#[derive(BitEnum)]
#[repr(i8)]
enum E {
    #[fallback]
    A = -1,
    B = 1,
}

fn main() {}
