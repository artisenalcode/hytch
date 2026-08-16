//! The `hytch` session daemon: pty management, scrollback ring buffer,
//! on-disk session log, and client fanout.

pub mod daemon;
pub mod fanout;
pub mod pty;
pub mod ring_buffer;
pub mod session_log;

pub use daemon::{DaemonConfig, ShutdownReason, run};
pub use fanout::Fanout;
pub use pty::Pty;
pub use ring_buffer::RingBuffer;
pub use session_log::SessionLog;
