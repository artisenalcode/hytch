//! Append throughput for the scrollback ring buffer -- the hottest path in
//! the daemon (every single pty read goes through this). The whole point
//! of using `copy_from_slice` instead of the C version's per-byte loop
//! (review finding #6) is throughput here, so this is the benchmark that
//! actually backs that claim up.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hytch_daemon::RingBuffer;

fn bench_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("ring_buffer_append");
    // 128 B: a typical small keystroke burst. 4096 B: a full pty read
    // buffer's worth (matches BUFSIZE in both the C original and this
    // daemon's own read loop).
    for &chunk_size in &[128usize, 4096] {
        let data = vec![b'x'; chunk_size];
        group.throughput(Throughput::Bytes(chunk_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(chunk_size),
            &chunk_size,
            |b, _| {
                // 128 KiB capacity, matching the daemon's default scrollback
                // size -- large enough that most iterations exercise the
                // steady-state wraparound path, not just the initial fill.
                let mut rb = RingBuffer::new(128 * 1024);
                b.iter(|| {
                    rb.append(&data);
                    std::hint::black_box(&rb);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_append);
criterion_main!(benches);
