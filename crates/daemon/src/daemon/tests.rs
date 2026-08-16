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
async fn run_creates_the_session_directory_if_missing() {
    // Regression test: the existing test_config() helper puts the socket
    // directly in the tempdir root, which always exists (TempDir creates
    // it), so it never exercised the "first session ever, ~/.cache/hytch
    // doesn't exist yet" path -- caught manually, not by the suite.
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested").join("session-dir");
    assert!(!nested.exists());

    let config = DaemonConfig {
        socket_path: nested.join("session.sock"),
        log_path: None,
        log_max_size: 1024 * 1024,
        scrollback_size: 4096,
        program: "sleep".to_string(),
        args: vec!["1".to_string()],
        initial_rows: 24,
        initial_cols: 80,
    };
    let socket_path = config.socket_path.clone();

    let handle = tokio::spawn(run(config));
    connect_with_retries(&socket_path).await; // succeeds only if bind() worked
    handle.await.unwrap().unwrap();
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

#[tokio::test]
async fn kill_stays_responsive_even_behind_a_stuck_push_backlog() {
    // Regression test for a real bug: pushing more than the pty's small
    // kernel input buffer (~11KB on Linux) to a child that never reads its
    // stdin (like `sleep`) blocks pty writes indefinitely at the OS level.
    // Kill has to travel through the same daemon loop that owns those
    // writes -- without the write timeout + control-channel priority this
    // test locks in, a kill sent while a push is stuck behind a
    // non-draining child took 7+ seconds and still failed ("did not
    // stop"), and a second bug (found while fixing the first) made it
    // busy-loop at ~90% CPU forever instead of even that, once the timeout
    // let writes fail fast after the child died but push was still
    // checked ahead of pty reads in the select.
    let dir = tempfile::tempdir().unwrap();
    let config = test_config(&dir, "sleep", &["30"], false);
    let socket_path = config.socket_path.clone();
    let handle = tokio::spawn(run(config));

    let mut pusher = connect_with_retries(&socket_path).await;
    // Comfortably more than the pty's real buffer (a handful of KB) --
    // several 60KB frames, each near the proto's own max frame size.
    for _ in 0..8 {
        send(
            &mut pusher,
            Message::Push(bytes::Bytes::from(vec![b'x'; 60_000])),
        )
        .await;
    }

    let mut killer = connect_with_retries(&socket_path).await;
    send(&mut killer, Message::Kill { signal: 9 }).await;

    let reason = tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("daemon must stay responsive to kill even behind a stuck push backlog")
        .expect("daemon task panicked")
        .expect("daemon returned an error");

    assert!(matches!(reason, ShutdownReason::ChildExited(_)));
}
