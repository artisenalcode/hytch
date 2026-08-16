//! Real-environment wrapper around `hytch_session`'s pure path-resolution
//! functions: reads the actual `$HOME`/uid instead of taking them as
//! parameters, so the pure logic in `hytch_session` stays unit-testable
//! without touching real process state.

use std::path::{Path, PathBuf};

/// The directory session sockets live in.
///
/// Deliberate simplification versus atch: only `$HOME` is consulted, not
/// the passwd-database fallback `hytch_session::resolve_session_dir` also
/// supports (and is tested for). `$HOME` is set in every login-shell/SSH
/// scenario this tool actually targets; wiring a real `getpwuid` lookup
/// would need an extra FFI dependency for a fallback path that in practice
/// only matters for unusual non-interactive setups (bare cron, minimal
/// containers). Worth revisiting if that turns out to matter.
pub fn session_dir() -> PathBuf {
    let env_home = std::env::var("HOME").ok();
    let uid = rustix::process::getuid().as_raw();
    hytch_session::resolve_session_dir(env_home.as_deref(), None, "hytch", uid)
}

pub fn socket_path(name: &str) -> PathBuf {
    hytch_session::resolve_socket_path(name, &session_dir())
}

pub fn log_path_for(socket_path: &Path) -> PathBuf {
    let mut os = socket_path.as_os_str().to_owned();
    os.push(".log");
    PathBuf::from(os)
}
