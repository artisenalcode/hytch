use crate::{DEFAULT_MAX_FRAME_LEN, Message, MessageCodec, RedrawMethod};
use bytes::{Bytes, BytesMut};
use proptest::prelude::*;
use tokio_util::codec::{Decoder, Encoder};

fn roundtrip(msg: Message) {
    let mut codec = MessageCodec::default();
    let mut buf = BytesMut::new();
    codec.encode(msg.clone(), &mut buf).expect("encode");
    let decoded = codec
        .decode(&mut buf)
        .expect("decode")
        .expect("a full frame should decode in one shot");
    assert_eq!(decoded, msg);
    assert!(buf.is_empty(), "codec must consume exactly one frame");
}

#[test]
fn roundtrip_push_empty() {
    roundtrip(Message::Push(Bytes::new()));
}

#[test]
fn roundtrip_push_typical() {
    roundtrip(Message::Push(Bytes::from_static(b"ls -la\n")));
}

#[test]
fn roundtrip_push_at_max_frame_len() {
    // The whole point of this codec existing: push payloads are NOT capped
    // at 8 bytes the way the C `winsize`-union protocol capped them.
    let payload = vec![b'x'; DEFAULT_MAX_FRAME_LEN];
    roundtrip(Message::Push(Bytes::from(payload)));
}

#[test]
fn roundtrip_attach_skip_ring_true() {
    roundtrip(Message::Attach { skip_ring: true });
}

#[test]
fn roundtrip_attach_skip_ring_false() {
    roundtrip(Message::Attach { skip_ring: false });
}

#[test]
fn roundtrip_detach() {
    roundtrip(Message::Detach);
}

#[test]
fn roundtrip_winch() {
    roundtrip(Message::Winch { rows: 24, cols: 80 });
}

#[test]
fn roundtrip_winch_max_dimensions() {
    roundtrip(Message::Winch {
        rows: u16::MAX,
        cols: u16::MAX,
    });
}

#[test]
fn roundtrip_redraw_all_methods() {
    for method in [
        RedrawMethod::Unspecified,
        RedrawMethod::None,
        RedrawMethod::CtrlL,
        RedrawMethod::Winch,
    ] {
        roundtrip(Message::Redraw {
            method,
            rows: 24,
            cols: 80,
        });
    }
}

#[test]
fn roundtrip_kill() {
    roundtrip(Message::Kill { signal: 15 }); // SIGTERM
    roundtrip(Message::Kill { signal: 9 }); // SIGKILL
}

#[test]
fn decode_returns_none_on_partial_frame() {
    // This is the fix for the C version's "one read() == one packet" bug:
    // a short read must ask for more data, not error out or misparse.
    let mut codec = MessageCodec::default();
    let mut full = BytesMut::new();
    codec
        .encode(Message::Push(Bytes::from_static(b"hello world")), &mut full)
        .unwrap();

    // Feed it one byte at a time; decode() must return Ok(None) until the
    // whole frame has arrived, then produce exactly one message.
    let mut buf = BytesMut::new();
    let mut produced = None;
    for &byte in full.iter() {
        buf.extend_from_slice(&[byte]);
        match codec.decode(&mut buf).unwrap() {
            None => continue,
            Some(msg) => {
                produced = Some(msg);
                break;
            }
        }
    }
    assert_eq!(
        produced,
        Some(Message::Push(Bytes::from_static(b"hello world")))
    );
}

#[test]
fn decode_handles_two_frames_in_one_buffer() {
    // Simulates two back-to-back sends arriving in a single read() — the
    // reassembly case the C client_activity() never handled (finding #9).
    let mut codec = MessageCodec::default();
    let mut buf = BytesMut::new();
    codec.encode(Message::Detach, &mut buf).unwrap();
    codec
        .encode(Message::Kill { signal: 15 }, &mut buf)
        .unwrap();

    let first = codec.decode(&mut buf).unwrap().unwrap();
    let second = codec.decode(&mut buf).unwrap().unwrap();
    assert_eq!(first, Message::Detach);
    assert_eq!(second, Message::Kill { signal: 15 });
    assert!(buf.is_empty());
}

#[test]
fn decode_rejects_frame_over_max_len() {
    // Bounded-memory guard against a hostile/corrupt peer: a claimed length
    // over the max must be a decode error, not an attempt to allocate it.
    let mut codec = MessageCodec::default();
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0u8]); // Push type tag
    buf.extend_from_slice(&((DEFAULT_MAX_FRAME_LEN as u32) + 1).to_be_bytes());
    let result = codec.decode(&mut buf);
    assert!(result.is_err(), "oversized frame length must be rejected");
}

#[test]
fn decode_never_panics_on_garbage() {
    // Cheap smoke test standing in for the fuzz target until cargo-fuzz +
    // nightly toolchain are provisioned (see plan doc's open item).
    let mut codec = MessageCodec::default();
    let garbage_samples: &[&[u8]] = &[
        &[],
        &[0xFF],
        &[0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        &[0x02, 0x00, 0x00, 0x00, 0x01, 0x00], // unknown type tag
        &[0x00, 0xFF, 0xFF, 0xFF, 0xFF],       // huge claimed length
    ];
    for sample in garbage_samples {
        let mut buf = BytesMut::from(*sample);
        let _ = codec.decode(&mut buf); // must not panic, Ok or Err both fine
    }
}

proptest! {
    #[test]
    fn proptest_push_roundtrips_any_length(payload in proptest::collection::vec(any::<u8>(), 0..8192)) {
        roundtrip(Message::Push(Bytes::from(payload)));
    }

    #[test]
    fn proptest_winch_roundtrips_any_dimensions(rows: u16, cols: u16) {
        roundtrip(Message::Winch { rows, cols });
    }

    #[test]
    fn proptest_kill_roundtrips_any_signal(signal: u8) {
        roundtrip(Message::Kill { signal });
    }
}
