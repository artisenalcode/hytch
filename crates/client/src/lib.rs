//! Attach-side: raw terminal mode and the steady-state attach loop.

pub mod attach;
pub mod raw_mode;
pub mod resize;

pub use attach::{AttachOptions, AttachOutcome, run};
pub use raw_mode::RawModeGuard;
