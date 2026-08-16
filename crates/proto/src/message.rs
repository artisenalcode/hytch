use bytes::Bytes;

/// The client→daemon control protocol.
///
/// Every variant here is *control* traffic — the daemon→client direction
/// carries raw pty bytes with no framing at all, on purpose. See the crate
/// doc comment for why that asymmetry matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Bytes to write to the child process's stdin (keyboard input, `push`).
    /// Unlike the C protocol this replaces, there is no 8-byte cap — a
    /// paste or `push` of a large blob is one frame, not thousands.
    Push(Bytes),
    /// Client wants to attach. `skip_ring` is set when the client already
    /// replayed the on-disk log itself, so the daemon skips replaying its
    /// in-memory ring buffer too (avoids showing history twice).
    Attach { skip_ring: bool },
    /// Client is detaching (or suspending) without disconnecting the socket.
    Detach,
    /// Terminal window size changed.
    Winch { rows: u16, cols: u16 },
    /// Client wants the daemon to force a redraw, with the client's current
    /// window size attached (mirrors how the C client piggybacks a resize on
    /// its post-attach redraw request).
    Redraw {
        method: RedrawMethod,
        rows: u16,
        cols: u16,
    },
    /// Send a signal to the child process.
    Kill { signal: u8 },
}

/// How the daemon should force a redraw after (re)attaching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedrawMethod {
    /// Caller didn't specify — daemon falls back to its own default.
    Unspecified,
    /// Don't force a redraw at all.
    None,
    /// Send a `^L` (only when the pty is in raw, no-echo, char-at-a-time mode).
    CtrlL,
    /// Send `SIGWINCH` to the child.
    Winch,
}

impl RedrawMethod {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            RedrawMethod::Unspecified => 0,
            RedrawMethod::None => 1,
            RedrawMethod::CtrlL => 2,
            RedrawMethod::Winch => 3,
        }
    }

    pub(crate) fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(RedrawMethod::Unspecified),
            1 => Some(RedrawMethod::None),
            2 => Some(RedrawMethod::CtrlL),
            3 => Some(RedrawMethod::Winch),
            _ => None,
        }
    }
}
