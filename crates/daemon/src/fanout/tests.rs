use super::Fanout;
use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn attach_snapshot_includes_everything_pushed_before_it() {
    let fanout = Fanout::new(64, 16);
    fanout.push(Bytes::from_static(b"A"));
    let (snapshot, _rx) = fanout.attach();
    assert_eq!(snapshot, b"A");
}

#[tokio::test]
async fn attach_receiver_gets_everything_pushed_after_it() {
    let fanout = Fanout::new(64, 16);
    fanout.push(Bytes::from_static(b"A"));
    let (snapshot, mut rx) = fanout.attach();
    fanout.push(Bytes::from_static(b"B"));

    let live = rx.recv().await.unwrap();
    let mut combined = snapshot;
    combined.extend_from_slice(&live);
    assert_eq!(combined, b"AB");
}

#[tokio::test]
async fn no_client_ever_sees_a_byte_twice_or_misses_one_in_between() {
    let fanout = Arc::new(Fanout::new(64, 1024));
    let pusher_fanout = fanout.clone();
    let pusher = tokio::spawn(async move {
        for i in 0u8..=255 {
            pusher_fanout.push(Bytes::from(vec![i]));
            tokio::task::yield_now().await;
        }
    });

    // Let the pusher get a head start so attach() lands mid-stream, not
    // trivially before the first push.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (snapshot, mut rx) = fanout.attach();

    pusher.await.unwrap();

    let mut received = Vec::new();
    while let Ok(Ok(chunk)) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
        received.extend_from_slice(&chunk);
    }

    let mut combined = snapshot;
    combined.extend_from_slice(&received);

    assert_eq!(
        combined.last().copied(),
        Some(255),
        "must end at the last byte pushed"
    );
    for pair in combined.windows(2) {
        assert_eq!(
            pair[1],
            pair[0].wrapping_add(1),
            "gap or duplicate somewhere in {combined:?}"
        );
    }
}
