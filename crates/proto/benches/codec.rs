//! Encode/decode throughput for the control-channel codec, at sizes
//! representative of real traffic: a single keystroke, a typical pasted
//! line, and a chunk near the max frame size (the largest a `push` chunk
//! ever gets, per commands.rs's PUSH_CHUNK_SIZE).

use bytes::{Bytes, BytesMut};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use hytch_proto::{Message, MessageCodec};
use tokio_util::codec::{Decoder, Encoder};

fn roundtrip(payload_len: usize) -> (BytesMut, usize) {
    let mut codec = MessageCodec::default();
    let mut buf = BytesMut::new();
    let payload = Bytes::from(vec![b'x'; payload_len]);
    codec.encode(Message::Push(payload), &mut buf).unwrap();
    let frame_len = buf.len();
    (buf, frame_len)
}

fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("encode_push");
    for &size in &[1usize, 64, 4096, 32 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            let payload = Bytes::from(vec![b'x'; size]);
            let mut codec = MessageCodec::default();
            b.iter(|| {
                let mut buf = BytesMut::new();
                codec
                    .encode(Message::Push(payload.clone()), &mut buf)
                    .unwrap();
                std::hint::black_box(&buf);
            });
        });
    }
    group.finish();
}

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode_push");
    for &size in &[1usize, 64, 4096, 32 * 1024] {
        let (encoded, frame_len) = roundtrip(size);
        group.throughput(Throughput::Bytes(frame_len as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &encoded, |b, encoded| {
            let mut codec = MessageCodec::default();
            b.iter(|| {
                let mut buf = encoded.clone();
                let msg = codec.decode(&mut buf).unwrap().unwrap();
                std::hint::black_box(msg);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_encode, bench_decode);
criterion_main!(benches);
