//! End-to-end dispatch latency: time from `Fanout::push()` to a subscribed
//! receiver actually getting the bytes. This is the internal analogue of
//! "push -> echo" round-trip latency -- it excludes the pty/kernel hop
//! (covered qualitatively by the real 2 MiB push CLI test) and isolates
//! just this crate's own dispatch overhead: the `bytes::Bytes` clone-per-
//! subscriber fanout this crate uses instead of the C version's per-client
//! `write()` loop.

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use hytch_daemon::Fanout;

fn bench_push_to_recv_latency(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("fanout_push_to_recv_single_subscriber", |b| {
        b.to_async(&rt).iter_batched(
            || {
                let fanout = Fanout::new(4096, 16);
                let (_snapshot, rx) = fanout.attach();
                (fanout, rx)
            },
            |(fanout, mut rx)| async move {
                fanout.push(Bytes::from_static(b"typical pty output chunk\r\n"));
                let received = rx.recv().await.unwrap();
                std::hint::black_box(received);
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_push_to_recv_latency);
criterion_main!(benches);
