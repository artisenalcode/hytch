//! Bridges `hytch_client::attach::run` (generic over any I/O types, for
//! testability) to the real terminal: real stdin/stdout, real raw-mode
//! termios on the real controlling tty, a real `SIGWINCH` watcher, and a
//! real `UnixStream` to the daemon.

use hytch_client::{AttachOptions, AttachOutcome};
use std::io;
use std::path::Path;

/// Attach to a running session over `socket_path`. Fails immediately if the
/// daemon isn't listening -- callers that want create-if-missing behavior
/// handle that themselves before calling this.
pub async fn attach_foreground(
    socket_path: &Path,
    detach_char: Option<u8>,
    quiet: bool,
) -> io::Result<AttachOutcome> {
    let stream = tokio::net::UnixStream::connect(socket_path).await?;
    let (conn_read, conn_write) = stream.into_split();

    let stdin_fd = rustix::stdio::stdin();
    let is_tty = rustix::termios::isatty(stdin_fd);

    // The guard must outlive the attach loop; keep it bound in this scope
    // rather than dropped early, and let its own Drop restore the terminal
    // on every exit path (including a panic), not just the happy path.
    let _raw_guard = if is_tty {
        Some(hytch_client::RawModeGuard::enable(stdin_fd)?)
    } else {
        None
    };

    let resize_events = if is_tty {
        hytch_client::resize::spawn_watcher(stdin_fd).ok()
    } else {
        None
    };

    let (rows, cols) = if is_tty {
        hytch_client::resize::current_size(stdin_fd).unwrap_or((24, 80))
    } else {
        (24, 80)
    };

    if !quiet {
        eprintln!();
    }

    let opts = AttachOptions {
        detach_char,
        suspend_char: None, // suspend orchestration is a follow-up pass; see step 4's notes
        redraw_method: hytch_proto::RedrawMethod::Winch,
        rows,
        cols,
        skip_ring: false,
    };

    hytch_client::run(
        tokio::io::stdin(),
        tokio::io::stdout(),
        conn_read,
        conn_write,
        resize_events,
        opts,
    )
    .await
}
