//! The attach-side steady-state loop: forward terminal input to the daemon
//! as framed [`Message`]s, write the daemon's raw output stream straight to
//! the terminal untouched.
//!
//! Generic over the input/output/connection types specifically so this is
//! testable with in-process pipes and a real pty slave instead of requiring
//! an actual attached terminal -- the local-special-character detection
//! (detach/suspend) and the byte-forwarding logic are pure enough not to
//! need one.
//!
//! Deliberately does **not** orchestrate the OS-level suspend cycle
//! (raising `SIGTSTP` on `^Z`, waiting for `SIGCONT`, re-entering raw mode)
//! itself -- that belongs to the process-level orchestration in the `cli`
//! crate. This loop's job ends at reporting *that* a suspend was requested
//! ([`AttachOutcome::Suspended`]), after already telling the daemon so via
//! `Detach`, the same way atch's `process_kbd()` does before calling
//! `kill(getpid(), SIGTSTP)`.

use bytes::{Bytes, BytesMut};
use hytch_proto::{Message, MessageCodec, RedrawMethod};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::codec::Encoder;

pub struct AttachOptions {
    /// Byte that ends the session locally without touching the daemon.
    /// `None` disables detaching entirely (`-E`).
    pub detach_char: Option<u8>,
    /// Byte that requests a local suspend. `None` disables it (`-z`).
    pub suspend_char: Option<u8>,
    pub redraw_method: RedrawMethod,
    pub rows: u16,
    pub cols: u16,
    /// Set when the caller already replayed the on-disk log itself, so the
    /// daemon should skip replaying its in-memory ring on top of it.
    pub skip_ring: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The user pressed the detach character.
    Detached,
    /// The user pressed the suspend character; the daemon has already been
    /// told (`Detach`). Caller should suspend the process and, on resume,
    /// re-enter raw mode and call `run` again.
    Suspended,
    /// The daemon closed the connection (child exited, or the daemon itself
    /// went away) or stdin hit EOF.
    SessionEnded,
}

/// Drive one attach session: send the initial `Attach`+`Redraw` handshake,
/// then loop forwarding `input` -> daemon as `Push` frames and the daemon's
/// raw output -> `output`, until detach, suspend, or the connection ends.
///
/// `resize_events`, when given, is a channel of `(rows, cols)` window-size
/// changes -- fed by a separate `SIGWINCH` watcher (see [`crate::resize`]),
/// not read directly by this function, so the loop's resize handling is
/// testable with synthetic events instead of requiring a real terminal
/// resize signal.
pub async fn run<In, Out, ConnRead, ConnWrite>(
    mut input: In,
    mut output: Out,
    mut conn_read: ConnRead,
    mut conn_write: ConnWrite,
    mut resize_events: Option<mpsc::Receiver<(u16, u16)>>,
    opts: AttachOptions,
) -> io::Result<AttachOutcome>
where
    In: AsyncRead + Unpin,
    Out: AsyncWrite + Unpin,
    ConnRead: AsyncRead + Unpin,
    ConnWrite: AsyncWrite + Unpin,
{
    let mut codec = MessageCodec::default();
    send(
        &mut conn_write,
        &mut codec,
        Message::Attach {
            skip_ring: opts.skip_ring,
        },
    )
    .await?;
    send(
        &mut conn_write,
        &mut codec,
        Message::Redraw {
            method: opts.redraw_method,
            rows: opts.rows,
            cols: opts.cols,
        },
    )
    .await?;

    let mut in_buf = [0u8; 4096];
    let mut out_buf = [0u8; 4096];
    loop {
        tokio::select! {
            result = input.read(&mut in_buf) => {
                let n = result?;
                if n == 0 {
                    return Ok(AttachOutcome::SessionEnded);
                }
                let chunk = &in_buf[..n];
                if let Some((pos, outcome)) = find_special_char(chunk, opts.detach_char, opts.suspend_char) {
                    if pos > 0 {
                        send(&mut conn_write, &mut codec, Message::Push(Bytes::copy_from_slice(&chunk[..pos]))).await?;
                    }
                    if outcome == AttachOutcome::Suspended {
                        send(&mut conn_write, &mut codec, Message::Detach).await?;
                    }
                    return Ok(outcome);
                }
                send(&mut conn_write, &mut codec, Message::Push(Bytes::copy_from_slice(chunk))).await?;
            }
            result = conn_read.read(&mut out_buf) => {
                let n = result?;
                if n == 0 {
                    return Ok(AttachOutcome::SessionEnded);
                }
                output.write_all(&out_buf[..n]).await?;
            }
            resized = recv_resize(&mut resize_events) => {
                if let Some((rows, cols)) = resized {
                    send(&mut conn_write, &mut codec, Message::Winch { rows, cols }).await?;
                }
                // `None` means the sender was dropped; just stop selecting
                // on it going forward (recv_next on a `None` option pends
                // forever, so this branch simply won't fire again).
            }
        }
    }
}

async fn recv_resize(rx: &mut Option<mpsc::Receiver<(u16, u16)>>) -> Option<(u16, u16)> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Earliest occurrence of either special character in `chunk`, and which
/// one it was. Suspend and detach are checked together (not detach-then-
/// suspend-in-separate-passes) so whichever comes first in the byte stream
/// wins, matching a real keystroke stream where only one applies per byte.
fn find_special_char(
    chunk: &[u8],
    detach_char: Option<u8>,
    suspend_char: Option<u8>,
) -> Option<(usize, AttachOutcome)> {
    chunk.iter().enumerate().find_map(|(i, &b)| {
        if Some(b) == suspend_char {
            Some((i, AttachOutcome::Suspended))
        } else if Some(b) == detach_char {
            Some((i, AttachOutcome::Detached))
        } else {
            None
        }
    })
}

async fn send<W: AsyncWrite + Unpin>(
    w: &mut W,
    codec: &mut MessageCodec,
    msg: Message,
) -> io::Result<()> {
    let mut buf = BytesMut::new();
    codec.encode(msg, &mut buf)?;
    w.write_all(&buf).await
}

#[cfg(test)]
mod tests;
