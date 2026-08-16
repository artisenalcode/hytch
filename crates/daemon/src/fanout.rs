//! Fans pty output out to attached clients.
//!
//! Combines the scrollback ring buffer (for the snapshot a newly-attaching
//! client needs) with a `broadcast` channel (for live output going forward)
//! behind one lock, so `attach()` can never observe a gap or a duplicate:
//! `push()` holds the lock across *both* the ring append and the broadcast
//! send, and `attach()` holds it across *both* the subscribe and the
//! snapshot, so the two are strictly ordered relative to each other. Whichever
//! happens first is fully visible to whichever happens second, exactly once.
//!
//! `bytes::Bytes` is used for the broadcast payload specifically so fanning
//! out to N attached clients is N cheap refcounted clones, not N copies of
//! the pty read buffer — a real throughput win over the C version's
//! per-client `write()` loop, not just an ergonomics one.
//!
//! Lag handling is deliberate, not inherited: the C version silently drops
//! the unwritten tail of a chunk when a client's socket write would block
//! (review finding #7). Here, a client that falls behind gets an explicit,
//! logged `Lagged` error from the broadcast channel and is disconnected —
//! see `daemon::handle_client`.

use crate::RingBuffer;
use bytes::Bytes;
use std::sync::Mutex;
use tokio::sync::broadcast;

pub struct Fanout {
    ring: Mutex<RingBuffer>,
    tx: broadcast::Sender<Bytes>,
}

impl Fanout {
    pub fn new(scrollback_capacity: usize, broadcast_capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(broadcast_capacity);
        Fanout {
            ring: Mutex::new(RingBuffer::new(scrollback_capacity)),
            tx,
        }
    }

    /// Record `data` in scrollback and fan it out to any live subscribers.
    pub fn push(&self, data: Bytes) {
        let mut ring = self.ring.lock().unwrap();
        ring.append(&data);
        // No receivers is not an error -- nobody's attached right now.
        let _ = self.tx.send(data);
    }

    /// Subscribe for live output and get a scrollback snapshot, atomically:
    /// nothing pushed after this call can be missing from the combination of
    /// (snapshot, receiver), and nothing in the snapshot can also arrive a
    /// second time via the receiver.
    pub fn attach(&self) -> (Vec<u8>, broadcast::Receiver<Bytes>) {
        let ring = self.ring.lock().unwrap();
        let rx = self.tx.subscribe();
        (ring.snapshot(), rx)
    }
}

#[cfg(test)]
mod tests;
