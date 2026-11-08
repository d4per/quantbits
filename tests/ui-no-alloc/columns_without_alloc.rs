use quantbits::bitfields;

#[bitfields(columns = Particles)]
#[derive(Clone, Copy)]
struct Particle {
    #[bits(4)]
    kind: u8,
}

fn main() {}
