//! Fixed-capacity scrollback ring buffer for warm reattach.
//!
//! `append` uses `copy_from_slice` (memcpy) for both the non-wrapping and
//! wrapping cases — never a per-byte loop. This is the fix for review
//! finding #6 (the C version copied the ring buffer one byte at a time on
//! its hottest path: every pty read).

/// A fixed-capacity byte ring buffer. Once full, the oldest bytes are
/// silently overwritten by new appends — this is scrollback, not a log.
pub struct RingBuffer {
    buf: Box<[u8]>,
    /// Physical index of the oldest valid byte.
    head: usize,
    /// Number of valid bytes currently stored, 0..=capacity.
    len: usize,
}

impl RingBuffer {
    /// `capacity` must be a power of two (so index wrapping can use a mask
    /// instead of a modulo).
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two(),
            "ring buffer capacity must be a power of two, got {capacity}"
        );
        RingBuffer {
            buf: vec![0u8; capacity].into_boxed_slice(),
            head: 0,
            len: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append `data`, overwriting the oldest bytes if the buffer fills.
    /// If `data` itself is longer than the capacity, only its tail
    /// (the most recent `capacity` bytes) is kept.
    pub fn append(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        let cap = self.capacity();
        let mask = cap - 1;
        let data = if data.len() >= cap {
            &data[data.len() - cap..]
        } else {
            data
        };

        let write_pos = (self.head + self.len) & mask;
        let first_len = (cap - write_pos).min(data.len());
        self.buf[write_pos..write_pos + first_len].copy_from_slice(&data[..first_len]);
        if first_len < data.len() {
            let rest = &data[first_len..];
            self.buf[..rest.len()].copy_from_slice(rest);
        }

        let new_total = self.len + data.len();
        if new_total > cap {
            self.head = (write_pos + data.len()) & mask;
            self.len = cap;
        } else {
            self.len = new_total;
        }
    }

    /// Copy out the full contents, oldest byte first.
    pub fn snapshot(&self) -> Vec<u8> {
        let cap = self.capacity();
        let mask = cap - 1;
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            out.push(self.buf[(self.head + i) & mask]);
        }
        out
    }
}

#[cfg(test)]
mod tests;
