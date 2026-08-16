//! Wire framing for the client↔daemon control channel.
//!
//! Deliberately asymmetric with the daemon→client direction: this codec only
//! ever frames *control* traffic (push data, attach/detach, window size,
//! redraw requests, kill signals). Raw pty output goes back to attached
//! clients as an unframed byte stream, untouched — that asymmetry is what
//! keeps mouse reporting, scroll sequences, and true-color passthrough intact.
//! Never add framing to the output direction.

mod codec;
mod message;

pub use codec::{DEFAULT_MAX_FRAME_LEN, MessageCodec};
pub use message::{Message, RedrawMethod};

#[cfg(test)]
mod tests;
