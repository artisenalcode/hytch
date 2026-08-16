use super::Pty;
use rustix::process::Signal;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

#[tokio::test]
async fn spawns_and_captures_child_stdout() {
    let mut pty = Pty::spawn("printf", &["hello".to_string()], 24, 80).expect("spawn");

    let mut buf = [0u8; 64];
    let n = timeout(Duration::from_secs(2), pty.read(&mut buf))
        .await
        .expect("read did not time out")
        .expect("read succeeded");

    assert_eq!(&buf[..n], b"hello");
}

#[tokio::test]
async fn resize_is_visible_via_current_size() {
    let pty = Pty::spawn("sleep", &["5".to_string()], 24, 80).expect("spawn");
    assert_eq!(pty.current_size().unwrap(), (24, 80));

    pty.resize(50, 120).unwrap();
    assert_eq!(pty.current_size().unwrap(), (50, 120));

    pty.signal(Signal::Kill).unwrap();
}

#[tokio::test]
async fn signal_terminates_the_child_and_wait_reports_it() {
    let mut pty = Pty::spawn("sleep", &["30".to_string()], 24, 80).expect("spawn");

    pty.signal(Signal::Kill).unwrap();

    let status = timeout(Duration::from_secs(2), pty.wait())
        .await
        .expect("wait did not time out")
        .expect("wait succeeded");

    assert_eq!(status.signal(), Some(Signal::Kill as i32));
}

#[tokio::test]
async fn write_reaches_child_stdin_through_the_pty() {
    // "cat" is unaffected by line-discipline echo ambiguity in the way a
    // shell `read` builtin would be: whatever bytes arrive on stdin, it
    // writes straight back out. The pty's own input echo also puts a copy
    // of what we typed into the read stream, so we only assert that cat's
    // echoed line shows up somewhere in the captured output, not an exact
    // byte match against the whole stream.
    let mut pty = Pty::spawn("cat", &[], 24, 80).expect("spawn");

    pty.write_all(b"ping\n").await.unwrap();

    let mut buf = [0u8; 256];
    let mut collected = Vec::new();
    // Give the kernel echo + cat's own write a moment to both land.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while collected.windows(4).all(|w| w != b"ping") && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(n)) = timeout(Duration::from_millis(200), pty.read(&mut buf)).await {
            collected.extend_from_slice(&buf[..n]);
        }
    }

    pty.signal(Signal::Kill).unwrap();
    assert!(
        collected.windows(4).any(|w| w == b"ping"),
        "expected 'ping' somewhere in captured output, got {collected:?}"
    );
}
