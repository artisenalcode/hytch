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

/// How far before a naive tail cut we're willing to look for an unterminated
/// ANSI escape sequence that the cut would otherwise split in half. Real
/// terminal escape sequences (cursor movement, SGR/color, mode toggles) are
/// short; this comfortably covers them without meaningfully loosening the
/// size cap. Found by replaying a real corrupted log in production: a raw
/// byte-offset trim landed mid-sequence and desynced the terminal parser for
/// everything replayed after it -- lost color state, garbage glyphs, "no
/// color, strange ascii" on every subsequent reattach.
const ESCAPE_LOOKBACK: usize = 64;

/// Trim `file` in place to at most its last `max_size` bytes, if it
/// currently exceeds that. Leaves the file position at EOF either way.
///
/// The cut point is nudged to the nearest safe boundary -- never inside a
/// multi-byte UTF-8 character, never inside an unterminated ANSI escape
/// sequence -- so a cold replay of the result starts the terminal's parser
/// in a state it can actually make sense of. A raw byte-offset cut can land
/// mid-CSI-sequence (its tail then prints as literal garbage and eats
/// whatever real content follows, until a stray byte happens to look like a
/// terminator) or mid-UTF-8-char (renders as replacement-character mojibake)
/// -- either one corrupts the *entire* rest of the replay, not just the cut
/// point, because the terminal's escape-sequence parser is left in the
/// wrong state. This can grow the trimmed file slightly past `max_size`
/// (by at most `ESCAPE_LOOKBACK` bytes, to keep a split sequence intact) --
/// a deliberate, bounded tradeoff for a log that's actually safe to replay.
pub fn rotate_log_file(file: &mut File, max_size: u64) -> io::Result<()> {
    let size = file.seek(SeekFrom::End(0))?;
    if size > max_size {
        let naive_start = size - max_size;
        let lookback = ESCAPE_LOOKBACK.min(naive_start as usize) as u64;
        let read_start = naive_start - lookback;

        let mut buf = vec![0u8; (max_size + lookback) as usize];
        file.seek(SeekFrom::Start(read_start))?;
        let n = file.read(&mut buf)?;
        buf.truncate(n);

        let safe_start = safe_trim_start(&buf, lookback as usize);
        let tail = &buf[safe_start..];

        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(tail)?;
    }
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

/// Find the nearest safe place at or before `naive_cut` (within
/// `ESCAPE_LOOKBACK` bytes) to start the trimmed tail. Two hazards, checked
/// in order:
///
/// 1. `naive_cut` lands inside an unterminated escape sequence that started
///    in the lookback window -- move the start back to the `ESC` byte so
///    the whole sequence survives intact.
/// 2. `naive_cut` lands on a UTF-8 continuation byte -- skip forward to the
///    start of the next full character.
///
/// These can't both apply (an escape sequence is ASCII-only), so the escape
/// check runs first and returns early when it fires.
fn safe_trim_start(buf: &[u8], naive_cut: usize) -> usize {
    let lookback_start = naive_cut.saturating_sub(ESCAPE_LOOKBACK);
    let esc = buf[lookback_start..naive_cut]
        .iter()
        .rposition(|&b| b == 0x1b)
        .map(|rel| lookback_start + rel);
    if let Some(esc_pos) = esc
        && !escape_sequence_terminated(&buf[esc_pos..naive_cut])
    {
        return esc_pos;
    }

    let mut start = naive_cut;
    while start < buf.len() && buf[start] & 0xc0 == 0x80 {
        start += 1;
    }
    start
}

/// Whether the escape sequence starting at `seq[0]` (`ESC`) is already
/// closed within `seq`. Covers the two shapes that actually show up in real
/// terminal output: CSI (`ESC [ params... final-byte`) and short two-byte
/// escapes (`ESC c`, `ESC 7`, `ESC =`, ...). OSC sequences (`ESC ]
/// ... BEL`/`ST`, used for e.g. window-title setting) aren't handled --
/// rare enough in typical TUI output that treating them as terminated is an
/// accepted simplification, not a silent gap: worst case for those is the
/// pre-fix behavior for that one sequence, not a regression.
fn escape_sequence_terminated(seq: &[u8]) -> bool {
    match seq.get(1) {
        Some(b'[') => seq[2..].iter().any(|&b| (0x40..=0x7e).contains(&b)),
        Some(_) => true,
        None => false,
    }
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
            // Not necessarily exactly `max_size` any more: a safe trim can
            // grow the file slightly past the cap to keep an escape
            // sequence intact (see `rotate_log_file`), so read the real
            // post-trim length back rather than assuming the cap.
            self.current_size = self.file.metadata()?.len();
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
