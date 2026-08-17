use super::{AttachOptions, AttachOutcome, run};
use hytch_proto::{Message, MessageCodec, RedrawMethod};
use tokio::io::{AsyncReadExt, AsyncWriteExt, split};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;

fn opts(detach_char: Option<u8>, suspend_char: Option<u8>) -> AttachOptions {
    AttachOptions {
        detach_char,
        suspend_char,
        redraw_method: RedrawMethod::Winch,
        rows: 24,
        cols: 80,
        skip_ring: false,
    }
}

#[tokio::test]
async fn keystrokes_forwarded_and_daemon_output_written_verbatim() {
    let (mut input_writer, input_reader) = tokio::io::duplex(64);
    let (output_writer, mut output_reader) = tokio::io::duplex(64);
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);

    input_writer.write_all(b"hello").await.unwrap();
    // Deliberately NOT dropping input_writer here: doing so immediately
    // gives the client an EOF on stdin, which is itself a valid
    // InputClosed trigger and would race ahead of the daemon's response
    // below, making the test nondeterministic. Keep it alive until the
    // function ends -- the client's own conn EOF (from the daemon side
    // closing) is what should end this test's run() with SessionEnded.

    let daemon = tokio::spawn(async move {
        let (daemon_read, mut daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());

        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        match framed.next().await.unwrap().unwrap() {
            Message::Push(payload) => assert_eq!(&payload[..], b"hello"),
            other => panic!("expected Push, got {other:?}"),
        }

        daemon_write.write_all(b"echo-back").await.unwrap();
        // Dropping daemon_write (end of task) closes this half, which is
        // what gives the client's conn_read a clean EOF.
    });

    let outcome = run(
        input_reader,
        output_writer,
        client_read,
        client_write,
        None,
        opts(Some(0x1c), None),
    )
    .await
    .unwrap();

    daemon.await.unwrap();
    assert_eq!(outcome, AttachOutcome::SessionEnded);

    let mut captured = Vec::new();
    output_reader.read_to_end(&mut captured).await.unwrap();
    assert_eq!(captured, b"echo-back");
}

/// A tiny `AsyncWrite` that records how many bytes each `poll_write` call
/// received and separately counts `poll_flush` calls, so a test can assert
/// on the *shape* of the calls the attach loop makes -- not just the bytes
/// that eventually land. `tokio::io::duplex`'s buffer (used by the other
/// tests here) can't distinguish "written" from "flushed": both a real
/// `tokio::io::Stdout` (channel + background thread, see the real call
/// site in `cli::attach::attach_foreground`) and this mock only guarantee
/// visibility on an explicit flush, but a duplex pipe has no such
/// distinction to lose in the first place.
#[derive(Default, Clone)]
struct FlushCountingWriter {
    state: std::sync::Arc<std::sync::Mutex<FlushCountingWriterState>>,
}

#[derive(Default)]
struct FlushCountingWriterState {
    bytes: Vec<u8>,
    flush_count_after_each_write: Vec<usize>,
    flush_calls: usize,
}

impl tokio::io::AsyncWrite for FlushCountingWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let mut state = self.state.lock().unwrap();
        state.bytes.extend_from_slice(buf);
        let flush_count = state.flush_calls;
        state.flush_count_after_each_write.push(flush_count);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.state.lock().unwrap().flush_calls += 1;
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn every_daemon_output_write_is_followed_by_a_flush() {
    // Regression test for a real, live-diagnosed bug: `tokio::io::stdout()`
    // hands writes to a channel + background thread rather than doing a
    // direct syscall, so `write_all().await` completing only means "handed
    // off," not "on screen" -- without an explicit flush after every
    // daemon-output write, echoed keystrokes (and completion output) can
    // sit invisible in that internal buffer instead of reaching the real
    // terminal promptly. Confirmed live: with the flush missing, real
    // character-by-character typing under a pty never echoed at all within
    // a multi-second window; with it restored, every keystroke echoed
    // within single-digit milliseconds.
    let (_input_writer, input_reader) = tokio::io::duplex(64);
    let output = FlushCountingWriter::default();
    let output_for_assert = output.clone();
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);

    let daemon = tokio::spawn(async move {
        let (daemon_read, mut daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        // Two separate daemon->client writes, each with no trailing
        // newline -- exactly the shape of a single echoed keystroke or an
        // in-place completion redraw, the pattern the live bug hit. A
        // short sleep between them (rather than back-to-back writes)
        // ensures `run`'s `conn_read.read()` completes on the first byte
        // before the second one is even written, so this reliably produces
        // two separate `output.write_all()` calls instead of the duplex
        // pipe coalescing both bytes into a single read.
        daemon_write.write_all(b"c").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        daemon_write.write_all(b"d").await.unwrap();
        // Held open a bit longer before dropping (which closes the
        // connection and ends `run`'s loop with `SessionEnded`) so the
        // second write has time to actually reach `run`'s conn_read side.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let outcome = run(
        input_reader,
        output,
        client_read,
        client_write,
        None,
        opts(Some(0x1c), None),
    )
    .await
    .unwrap();
    assert_eq!(outcome, AttachOutcome::SessionEnded);
    daemon.await.unwrap();

    let state = output_for_assert.state.lock().unwrap();
    assert_eq!(&state.bytes, b"cd");
    // Every recorded write must have already seen at least one more flush
    // than the write before it -- i.e. a flush happened between this write
    // and the previous one, not just once at the very end.
    assert_eq!(
        state.flush_count_after_each_write,
        vec![0, 1],
        "expected a flush() between the first and second daemon-output write"
    );
    assert!(state.flush_calls >= 2);
}

#[tokio::test]
async fn local_stdin_eof_reports_input_closed_not_session_ended() {
    // The distinction this test locks in: local stdin running dry (a
    // script piping `< file` into hytch, or just this test dropping the
    // writer) says nothing about whether the *remote* session is still
    // alive -- the daemon here never closes its end. A caller (the `cli`
    // crate) uses this to decide whether to propagate "log off" -- doing
    // that on InputClosed would be wrong, since the hosted program hasn't
    // actually exited.
    let (input_writer, input_reader) = tokio::io::duplex(64);
    let (output_writer, _output_reader) = tokio::io::duplex(64);
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);

    drop(input_writer); // immediate local EOF, before anything else happens

    let daemon = tokio::spawn(async move {
        let (daemon_read, _daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        // Deliberately not dropping conn_daemon (or _daemon_write) here --
        // if the outcome below came from the daemon side closing instead
        // of the local stdin EOF, that would defeat the point of this
        // test. Held alive until the task ends, well past run()'s return.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    let outcome = run(
        input_reader,
        output_writer,
        client_read,
        client_write,
        None,
        opts(Some(0x1c), None),
    )
    .await
    .unwrap();

    daemon.await.unwrap();
    assert_eq!(outcome, AttachOutcome::InputClosed);
}

#[tokio::test]
async fn detach_char_exits_locally_without_forwarding_anything_after_it() {
    let (mut input_writer, input_reader) = tokio::io::duplex(64);
    let (output_writer, _output_reader) = tokio::io::duplex(64);
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);

    // Detach char (^\) followed by more bytes, all in one chunk -- neither
    // the detach char itself nor anything after it should ever be sent.
    input_writer.write_all(b"ab\x1cc").await.unwrap();
    drop(input_writer);

    let daemon = tokio::spawn(async move {
        let (daemon_read, _daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        match framed.next().await.unwrap().unwrap() {
            Message::Push(payload) => assert_eq!(
                &payload[..],
                b"ab",
                "only bytes before the detach char should be forwarded"
            ),
            other => panic!("expected Push, got {other:?}"),
        }
        // No further messages should arrive; the client returned already.
    });

    let outcome = run(
        input_reader,
        output_writer,
        client_read,
        client_write,
        None,
        opts(Some(0x1c), None),
    )
    .await
    .unwrap();

    daemon.await.unwrap();
    assert_eq!(outcome, AttachOutcome::Detached);
}

#[tokio::test]
async fn suspend_char_sends_detach_message_and_returns_suspended() {
    let (mut input_writer, input_reader) = tokio::io::duplex(64);
    let (output_writer, _output_reader) = tokio::io::duplex(64);
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);

    input_writer.write_all(b"x\x1a").await.unwrap(); // ^Z
    drop(input_writer);

    let daemon = tokio::spawn(async move {
        let (daemon_read, _daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        match framed.next().await.unwrap().unwrap() {
            Message::Push(payload) => assert_eq!(&payload[..], b"x"),
            other => panic!("expected Push, got {other:?}"),
        }
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Detach
        ));
    });

    let outcome = run(
        input_reader,
        output_writer,
        client_read,
        client_write,
        None,
        opts(Some(0x1c), Some(0x1a)),
    )
    .await
    .unwrap();

    daemon.await.unwrap();
    assert_eq!(outcome, AttachOutcome::Suspended);
}

#[tokio::test]
async fn resize_event_sends_winch_to_daemon() {
    let (_input_writer, input_reader) = tokio::io::duplex(64);
    let (output_writer, _output_reader) = tokio::io::duplex(64);
    let (conn_client, conn_daemon) = tokio::io::duplex(4096);
    let (client_read, client_write) = split(conn_client);
    let (resize_tx, resize_rx) = tokio::sync::mpsc::channel(4);

    let daemon = tokio::spawn(async move {
        let (daemon_read, _daemon_write) = split(conn_daemon);
        let mut framed = FramedRead::new(daemon_read, MessageCodec::default());
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Attach { .. }
        ));
        assert!(matches!(
            framed.next().await.unwrap().unwrap(),
            Message::Redraw { .. }
        ));
        match framed.next().await.unwrap().unwrap() {
            Message::Winch { rows, cols } => {
                assert_eq!((rows, cols), (50, 120));
            }
            other => panic!("expected Winch, got {other:?}"),
        }
    });

    resize_tx.send((50, 120)).await.unwrap();

    // The attach loop runs until the daemon closes its end; do that right
    // after the assertion above by dropping conn_daemon at the end of the
    // spawned task, same pattern as the other tests.
    let run_fut = run(
        input_reader,
        output_writer,
        client_read,
        client_write,
        Some(resize_rx),
        opts(Some(0x1c), None),
    );
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(2), run_fut)
        .await
        .expect("run() should end once the daemon side closes")
        .unwrap();

    daemon.await.unwrap();
    assert_eq!(outcome, AttachOutcome::SessionEnded);
}
