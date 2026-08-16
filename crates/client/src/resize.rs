//! `SIGWINCH` watcher: translates the OS signal into `(rows, cols)` values
//! on a channel, which [`crate::attach::run`] consumes generically (see its
//! doc comment for why the split exists — it keeps the attach loop's resize
//! *handling* testable without needing to raise a real signal).

use rustix::fd::AsFd;
use rustix::termios::tcgetwinsize;
use std::io;
use tokio::sync::mpsc;

/// Read the current window size of `fd` (typically stdin) via `TIOCGWINSZ`.
pub fn current_size<Fd: AsFd>(fd: Fd) -> io::Result<(u16, u16)> {
    let ws = tcgetwinsize(fd).map_err(io::Error::from)?;
    Ok((ws.ws_row, ws.ws_col))
}

/// Spawn a task that listens for `SIGWINCH` and pushes the terminal's
/// current size (re-queried from `fd` on each signal, not computed from the
/// signal itself -- `SIGWINCH` carries no payload) onto the returned
/// channel.
pub fn spawn_watcher<Fd>(fd: Fd) -> io::Result<mpsc::Receiver<(u16, u16)>>
where
    Fd: AsFd + Send + 'static,
{
    let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        while signal.recv().await.is_some() {
            if let Ok(size) = current_size(&fd)
                && tx.send(size).await.is_err()
            {
                return; // receiver dropped -- attach loop ended
            }
        }
    });
    Ok(rx)
}
