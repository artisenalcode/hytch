use super::{DaemonConfig, ShutdownReason, run};
use hytch_proto::{Message, MessageCodec};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio_util::codec::Encoder;

fn test_config(dir: &tempfile::TempDir, program: &str, args: &[&str], log: bool) -> DaemonConfig {
    DaemonConfig {
        socket_path: dir.path().join("session.sock"),
        log_path: log.then(|| dir.path().join("session.log")),
        log_max_size: 1024 * 1024,
        scrollback_size: 4096,
        program: program.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
        initial_rows: 24,
        initial_cols: 80,
    }
}

async fn connect_with_retries(path: &std::path::Path) -> UnixStream {
    for _ in 0..50 {
        if let Ok(stream) = UnixStream::connect(path).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("could not connect to {path:?} after retrying");
}

async fn send(stream: &mut UnixStream, msg: Message) {
    let mut codec = MessageCodec::default();
    let mut buf = bytes::BytesMut::new();
    codec.encode(msg, &mut buf).unwrap();
    stream.write_all(&buf).await.unwrap();
}

async fn read_for(stream: &mut UnixStream, duration: Duration) -> Vec<u8> {
    let mut collected = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + duration;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => collected.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
        }
    }
    collected
}

#[tokio::test]
async fn two_attached_clients_see_byte_identical_fanout() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(&dir, "cat", &[], false);
    let socket_path = config.socket_path.clone();
    tokio::spawn(run(config));

    let mut a = connect_with_retries(&socket_path).await;
    send(&mut a, Message::Attach { skip_ring: false }).await;
    let mut b = connect_with_retries(&socket_path).await;
    send(&mut b, Message::Attach { skip_ring: false }).await;

    send(&mut a, Message::Push(bytes::Bytes::from_static(b"ping\n"))).await;

    let (out_a, out_b) = tokio::join!(
        read_for(&mut a, Duration::from_millis(400)),
        read_for(&mut b, Duration::from_millis(400)),
    );

    assert!(!out_a.is_empty(), "client A should have received output");
    assert_eq!(
        out_a, out_b,
        "both attached clients must see identical bytes"
    );
}

#[tokio::test]
async fn reattaching_after_disconnect_replays_ring_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(&dir, "cat", &[], false);
    let socket_path = config.socket_path.clone();
    tokio::spawn(run(config));

    {
        let mut a = connect_with_retries(&socket_path).await;
        send(&mut a, Message::Attach { skip_ring: false }).await;
        send(
            &mut a,
            Message::Push(bytes::Bytes::from_static(b"marker\n")),
        )
        .await;
        // Give the daemon's pty-read loop time to append to the ring before
        // we disconnect out from under it.
        let _ = read_for(&mut a, Duration::from_millis(200)).await;
    } // `a` dropped here -- simulates an accidental disconnect.

    let mut b = connect_with_retries(&socket_path).await;
    send(&mut b, Message::Attach { skip_ring: false }).await;
    let replay = read_for(&mut b, Duration::from_millis(300)).await;

    assert!(
        replay.windows(6).any(|w| w == b"marker"),
        "reattaching client should see 'marker' replayed from scrollback, got {replay:?}"
    );
}

#[tokio::test]
async fn kill_message_terminates_child_and_daemon_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(&dir, "sleep", &["30"], true);
    let socket_path = config.socket_path.clone();
    let log_path = config.log_path.clone().unwrap();
    let handle = tokio::spawn(run(config));

    let mut client = connect_with_retries(&socket_path).await;
    send(&mut client, Message::Kill { signal: 9 }).await; // SIGKILL

    let reason = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("daemon task did not finish in time")
        .expect("daemon task panicked")
        .expect("daemon returned an error");

    assert!(matches!(reason, ShutdownReason::ChildExited(_)));
    assert!(
        !socket_path.exists(),
        "socket file must be removed on shutdown"
    );
    assert!(log_path.exists(), "log file should still exist on disk");
}
