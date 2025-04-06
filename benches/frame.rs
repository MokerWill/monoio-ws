use std::hint::black_box;

use monoio_ws::Frame;
use rand::{Rng, SeedableRng, rngs::SmallRng};

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 16384, args = [0, 1, 16, 64, 125])]
fn encode_control_slice(bencher: divan::Bencher, len: usize) {
    let frame = Frame {
        fin: true,
        opcode: monoio_ws::Opcode::Binary,
    };
    let mut rng = SmallRng::from_os_rng();
    let mut data = vec![0; len + Frame::CONTROL_HEADER_LEN];
    for i in 0..len {
        data[i] = ((i + 1) % usize::from(u8::MAX)) as u8;
    }
    bencher.bench_local(move || {
        let mut data = data.clone();
        let mask = rng.random::<u32>().to_ne_bytes();
        frame.encode_control_slice(&mut data, mask);
        black_box(data);
    });
}

#[divan::bench(sample_count = 8192, args = [1, 16, 125, 126, 65535, 65536])]
fn encode_vec(bencher: divan::Bencher, len: usize) {
    let frame = Frame {
        fin: true,
        opcode: monoio_ws::Opcode::Binary,
    };
    let mut rng = SmallRng::from_os_rng();
    let mut data = Vec::with_capacity(len + 14);
    for i in 0..len {
        data.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    bencher.bench_local(move || {
        let mut data = data.clone();
        let mask = rng.random::<u32>().to_ne_bytes();
        frame.encode_vec(&mut data, mask);
        black_box(data);
    });
}
