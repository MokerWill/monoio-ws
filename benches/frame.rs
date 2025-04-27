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
fn encode_control(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut src = Vec::with_capacity(len);
    for i in 0..len {
        src.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    let mut dst = Vec::with_capacity(src.len() + Frame::CONTROL_HEADER_LEN);
    bencher.bench_local(|| {
        let mask = rng.random::<u32>().to_ne_bytes();
        FRAME.encode_control(&src, &mut dst, mask);
    });
    black_box(dst);
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 125, 126, 65535, 65536])]
fn encode(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut src = Vec::with_capacity(len + 14);
    for i in 0..len {
        src.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    let mut dst = Vec::with_capacity(src.len() + Frame::MAX_HEADER_LEN);
    bencher.bench_local(|| {
        let mask = rng.random::<u32>().to_ne_bytes();
        FRAME.encode(&src, &mut dst, mask);
    });
    black_box(dst);
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 128, 256, 1024, 65535, 65536])]
fn validate_utf8(bencher: divan::Bencher, len: usize) {
    let mut data = Vec::with_capacity(len);
    for i in 0..len {
        let ascii_code = (i % 95) + 32;
        data.push(ascii_code as u8);
    }
    bencher.bench_local(|| {
        assert!(Frame::validate_utf8(&data).is_some());
    });
}
