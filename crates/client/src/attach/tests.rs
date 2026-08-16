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
    // SessionEnded trigger and would race ahead of the daemon's response
    // below, making the test nondeterministic. Keep it alive until the
    // function ends -- the client's own conn EOF (from the daemon side
    // closing) is what should end this test's run().

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
