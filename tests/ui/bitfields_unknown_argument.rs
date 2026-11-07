use quantbits::bitfields;

#[bitfields(rows = Table)]
struct Row {
    #[bits(3)]
    a: u8,
}

fn main() {}
