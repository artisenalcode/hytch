//! Persistent on-disk session log: every byte written to the pty is
//! appended here, so a cold reattach (session exited, crashed, or the
//! machine rebooted) can replay full history — not just what a live
//! in-memory ring buffer happened to retain.
//!
//! Rotation is a plain synchronous trim of the file (`rotate_log_file`),
//! deliberately kept OS-call-only and pure so it's unit-testable without
//! tokio. The daemon event loop runs it inside `tokio::task::spawn_blocking`
//! rather than inline (review finding #5: the C version ran this multi-
//! megabyte read+write synchronously inside its single-threaded select()
//! loop, stalling every attached client for the duration).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Trim `file` in place to at most its last `max_size` bytes, if it
/// currently exceeds that. Leaves the file position at EOF either way.
pub fn rotate_log_file(file: &mut File, max_size: u64) -> io::Result<()> {
    let size = file.seek(SeekFrom::End(0))?;
    if size > max_size {
        let mut tail = vec![0u8; max_size as usize];
        file.seek(SeekFrom::Start(size - max_size))?;
        let n = file.read(&mut tail)?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&tail[..n])?;
    }
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

/// A session's persistent on-disk log: open, append (with rotation once the
/// cap is crossed), and an end-of-session marker.
///
/// Tracks the file's logical size in memory (`current_size`) rather than
/// batching rotation checks behind a "bytes written since last check"
/// counter the way the C version does. atch's `pty_activity()` only checks
/// whether to rotate once accumulated writes reach `log_max_size`, then
/// resets that counter regardless of whether the file was actually trimmed
/// — so the file can grow up to ~2x the cap before a real trim happens.
/// Tracking the real size costs nothing extra (no additional syscall; we
/// already know how many bytes we just wrote) and gives a tighter guarantee:
/// the log never exceeds `max_size` after any single `append`.
pub struct SessionLog {
    file: File,
    max_size: u64,
    current_size: u64,
}

impl SessionLog {
    /// Open (creating if necessary) the log at `path`. If it already
    /// exceeds `max_size`, it's trimmed immediately — mirrors atch's
    /// `open_log()` calling `rotate_log()` before the first append.
    pub fn open(path: &Path, max_size: u64) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false) // preserve existing content; see the doc comment above
            .open(path)?;
        rotate_log_file(&mut file, max_size)?;
        let current_size = file.metadata()?.len();
        Ok(SessionLog {
            file,
            max_size,
            current_size,
        })
    }

    /// Append `data`, rotating (trimming to the tail) if this write pushes
    /// the log past `max_size`.
    pub fn append(&mut self, data: &[u8]) -> io::Result<()> {
        self.file.write_all(data)?;
        self.current_size += data.len() as u64;
        if self.current_size > self.max_size {
            rotate_log_file(&mut self.file, self.max_size)?;
            self.current_size = self.max_size;
        }
        Ok(())
    }

    /// Write a human-readable marker (e.g. "session ended after 3m 2s") at
    /// the current end of the log. Does not count toward rotation — this is
    /// a one-shot write at session teardown, not steady-state pty output.
    pub fn write_end_marker(&mut self, marker: &str) -> io::Result<()> {
        self.file.write_all(marker.as_bytes())
    }
}

#[cfg(test)]
mod tests;
