type Id = u16;

#[quantbits::bitfields]
struct S {
    #[bits(10)]
    id: Id,
}

fn main() {}
