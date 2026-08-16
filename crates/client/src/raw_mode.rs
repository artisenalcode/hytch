//! Raw-mode termios guard.
//!
//! An RAII type rather than atch's `atexit(restore_term)`: `Drop` restores
//! the original terminal settings even if the attach loop panics, whereas
//! an `atexit` hook only fires on a normal `exit()` path. Generic over any
//! `AsFd` so it's testable against a pty slave in-process, not just real
//! stdin.

use rustix::fd::AsFd;
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcsetattr};
use std::io;

pub struct RawModeGuard<Fd: AsFd> {
    fd: Fd,
    original: Termios,
}

impl<Fd: AsFd> RawModeGuard<Fd> {
    /// Save the current terminal settings for `fd` and switch it to raw
    /// mode (no echo, no line buffering, no signal-generating characters --
    /// everything passes through untouched, which is the whole point of
    /// this tool: the daemon on the other end never re-interprets bytes).
    pub fn enable(fd: Fd) -> io::Result<Self> {
        let original = tcgetattr(&fd).map_err(io::Error::from)?;
        let mut raw = original.clone();
        raw.make_raw();
        tcsetattr(&fd, OptionalActions::Drain, &raw).map_err(io::Error::from)?;
        Ok(RawModeGuard { fd, original })
    }

    /// The terminal settings as they were before `enable()` -- callers that
    /// need to temporarily restore them without dropping the guard (e.g.
    /// before suspending the process on `^Z`) can read this back out.
    pub fn original(&self) -> &Termios {
        &self.original
    }
}

impl<Fd: AsFd> Drop for RawModeGuard<Fd> {
    fn drop(&mut self) {
        // Best-effort: nothing useful to do with an error while unwinding
        // or exiting, but this must never panic in Drop.
        let _ = tcsetattr(&self.fd, OptionalActions::Drain, &self.original);
    }
}

#[cfg(test)]
mod tests;
