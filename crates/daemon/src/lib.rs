//! The `hytch` session daemon: pty management, scrollback ring buffer,
//! on-disk session log, and client fanout.

pub mod ring_buffer;
pub mod session_log;

pub use ring_buffer::RingBuffer;
pub use session_log::SessionLog;
