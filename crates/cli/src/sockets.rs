//! Connecting to a session's `AF_UNIX` socket, working around the
//! ~107-byte `sun_path` length limit the same way the daemon's `bind_listener`
//! does (see its doc comment for why this isn't a hypothetical) — `chdir`
//! into the parent directory and connect using the short relative name.

use std::io;
use std::path::Path;

/// Async connect, for the attach/push/kill control-message paths.
pub async fn connect(path: &Path) -> io::Result<tokio::net::UnixStream> {
    match hytch_session::shorten_for_unix_socket(path) {
        None => tokio::net::UnixStream::connect(path).await,
        Some((dir, short_name)) => {
            let original_cwd = std::env::current_dir()?;
            std::env::set_current_dir(&dir)?;
            let result = tokio::net::UnixStream::connect(&short_name).await;
            std::env::set_current_dir(&original_cwd)?;
            result
        }
    }
}

/// Sync connect, for `list`/`rm`'s "is anything actually listening" probes
/// (deliberately not async — these are quick, one-shot checks alongside
/// otherwise-sync directory scanning code).
pub fn connect_probe(path: &Path) -> io::Result<std::os::unix::net::UnixStream> {
    match hytch_session::shorten_for_unix_socket(path) {
        None => std::os::unix::net::UnixStream::connect(path),
        Some((dir, short_name)) => {
            let original_cwd = std::env::current_dir()?;
            std::env::set_current_dir(&dir)?;
            let result = std::os::unix::net::UnixStream::connect(&short_name);
            std::env::set_current_dir(&original_cwd)?;
            result
        }
    }
}
