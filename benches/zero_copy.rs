use std::hint::black_box;

use monoio_ws::{Arena, Frame};
use rand::{Rng, SeedableRng, rngs::SmallRng};

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 16384, sample_size = 100, args = [0, 1, 31, 32, 33, 125])]
fn zero_copy_encode_control(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut src = Vec::with_capacity(len);
    for i in 0..len {
        src.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    let mut dst = Vec::with_capacity(src.len() + Frame::CONTROL_HEADER_LEN);
    bencher.bench_local(|| {
        let mask = rng.random::<u32>().to_ne_bytes();
        Frame::binary(&src).encode_control(&mut dst, mask);
    });
    black_box(dst);
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 125, 126, 65535, 65536])]
fn zero_copy_encode(bencher: divan::Bencher, len: usize) {
    let mut rng = SmallRng::from_os_rng();
    let mut src = Vec::with_capacity(len + 14);
    for i in 0..len {
        src.push(((i + 1) % usize::from(u8::MAX)) as u8);
    }
    let mut dst = Vec::with_capacity(src.len() + Frame::MAX_HEADER_LEN);
    bencher.bench_local(|| {
        let mask = rng.random::<u32>().to_ne_bytes();
        Frame::binary(&src).encode(&mut dst, mask);
    });
    black_box(dst);
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 128, 256, 1024, 65535, 65536])]
fn arena_alloc_slice_copy(bencher: divan::Bencher, len: usize) {
    let arena = Arena::with_capacity(1024 * 1024);
    let data: Vec<u8> = (0..len).map(|i| (i % 256) as u8).collect();
    
    bencher.bench_local(|| {
        let allocated = arena.alloc_slice_copy(&data);
        black_box(allocated);
    });
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 128, 256, 1024, 65535, 65536])]
fn vec_clone(bencher: divan::Bencher, len: usize) {
    let data: Vec<u8> = (0..len).map(|i| (i % 256) as u8).collect();
    
    bencher.bench_local(|| {
        let cloned = data.clone();
        black_box(cloned);
    });
}

#[divan::bench(sample_count = 4096, sample_size = 25, args = [1, 16, 128, 256, 1024, 65535, 65536])]
fn arena_reset(bencher: divan::Bencher, len: usize) {
    let arena = Arena::with_capacity(1024 * 1024);
    let data: Vec<u8> = (0..len).map(|i| (i % 256) as u8).collect();
    
    bencher.bench_local(|| {
        let _allocated = arena.alloc_slice_copy(&data);
        arena.reset();
    });
}