use quantbits::bitfields;

#[bitfields(columns = Table)]
struct Row {
    #[bits(3)]
    len: u8,
    #[bits(3)]
    a: u8,
    #[bits(3)]
    a_iter: u8,
}

fn main() {}
