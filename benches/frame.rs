use std::hint::black_box;

use monoio_ws::{Frame, Opcode};
use rand::{Rng, SeedableRng, rngs::SmallRng};

const FRAME: Frame = Frame {
    fin: true,
    opcode: Opcode::Binary,
};

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 16384, sample_size = 100, args = [0, 1, 31, 32, 33, 125])]
fn encode_control_slice(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut data = vec![0; len + Frame::CONTROL_HEADER_LEN];
    for (i, item) in data.iter_mut().enumerate().take(len) {
        *item = ((i + 1) % usize::from(u8::MAX)) as u8;
    }
    bencher.bench_local(move || {
        let mut data = data.clone();
        let mask = rng.random::<u32>().to_ne_bytes();
        FRAME.encode_control_slice(&mut data, mask);
        black_box(data);
    });
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 125, 126, 65535, 65536])]
fn encode_vec(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut data = Vec::with_capacity(len + 14);
    for i in 0..len {
        data.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    bencher.bench_local(move || {
        let mut data = data.clone();
        let mask = rng.random::<u32>().to_ne_bytes();
        FRAME.encode_vec(&mut data, mask);
        black_box(data);
    });
}
