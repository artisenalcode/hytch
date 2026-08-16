//! Bridges `hytch_client::attach::run` (generic over any I/O types, for
//! testability) to the real terminal: real stdin/stdout, real raw-mode
//! termios on the real controlling tty, a real `SIGWINCH` watcher, and a
//! real `UnixStream` to the daemon.
//!
//! Also owns cold log replay: the on-disk log is the primary source of
//! "what happened," always shown first when present, whether the session
//! is still running, exited cleanly, or crashed. This is the actual
//! feature behind "resume exactly where you left off... whether the
//! session is still running, crashed, or you rebooted the machine" from
//! the original design brief -- without it, a crashed daemon (socket file
//! left behind, `ECONNREFUSED` on connect) would just report an error and
//! show nothing, even though the full history is sitting right there on
//! disk.

use hytch_client::{AttachOptions, AttachOutcome};
use std::io::{self, Write as _};
use std::path::Path;

/// Attach to a running session over `socket_path`. `replayed` records
/// whether the caller already dumped the on-disk log to stdout (via
/// [`replay_log_to_stdout`]) before calling this -- callers are responsible
/// for doing that replay exactly once, since a caller like the default
/// attach-or-create flow may try a connect, fall back to spawning a fresh
/// session, and attach again, and the log must not be shown twice across
/// that whole sequence (mirrors atch's `log_already_replayed` guard).
/// When `replayed` is true, the daemon's own in-memory ring-buffer replay
/// is skipped too, for the same reason.
pub async fn attach_foreground(
    socket_path: &Path,
    detach_char: Option<u8>,
    replayed: bool,
    quiet: bool,
) -> io::Result<AttachOutcome> {
    if let Some(e) = self_attach_error(socket_path) {
        return Err(e);
    }

    let stream = crate::sockets::connect(socket_path).await?;
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

    if !quiet && !replayed {
        eprintln!();
    }

    let opts = AttachOptions {
        detach_char,
        suspend_char: None, // suspend orchestration is a follow-up pass; see step 4's notes
        redraw_method: hytch_proto::RedrawMethod::Winch,
        rows,
        cols,
        // The on-disk log already showed the tail; skip the daemon's
        // in-memory ring-buffer replay so history isn't shown twice.
        skip_ring: replayed,
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

/// Refuses to attach if `socket_path` appears anywhere in this process's
/// own `HYTCH_SESSION` ancestry chain -- catches both direct self-attach
/// (typing the current session's own name) and indirect loops (A attaches
/// through B back to A). Without this, a shell auto-attach hook with its
/// own naive recursion guard would still be exposed to the indirect case,
/// and a direct `hytch main` typed by hand while already inside `main`
/// would just hang against the daemon's own accept loop indefinitely
/// rather than failing cleanly.
fn self_attach_error(socket_path: &Path) -> Option<io::Error> {
    let chain = std::env::var(hytch_session::SESSION_ENVVAR).ok();
    let target = socket_path.to_string_lossy();
    if hytch_session::ancestry_contains(chain.as_deref(), &target) {
        Some(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot attach to session from within itself",
        ))
    } else {
        None
    }
}

/// Soft terminal reset written immediately before a log replay: DECSTR
/// (resets scroll margins, origin mode, and other modes a stale replay
/// could leave set), SGR reset (clears any color/attribute state left over
/// from *before* this attach), exit alternate screen, and show the cursor.
/// Never a full RIS (`ESC c`) -- that also nukes the terminal's own
/// scrollback, which would be actively unfriendly right before dumping a
/// tool whose entire point is showing you scrollback history.
///
/// Exists because the log being replayed can itself be a rotation-trimmed
/// tail (see `session_log::rotate_log_file`): even with a boundary-safe
/// trim, the *state* a mid-stream cut point depends on (current color, an
/// open alternate-screen, a moved scroll region) was set by bytes that are
/// simply gone. Starting the replay from a known-clean state means a
/// stale/trimmed log renders as at-worst-incomplete, not corrupted.
const PRE_REPLAY_RESET: &[u8] = b"\x1b[!p\x1b[0m\x1b[?1049l\x1b[?25h";

/// Dump `log_path`'s contents to stdout, preceded by a soft terminal reset
/// (see [`PRE_REPLAY_RESET`]). Returns whether anything was replayed (a
/// missing or empty log is not an error -- it just means this session has
/// never produced output yet).
pub fn replay_log_to_stdout(log_path: &Path) -> bool {
    match std::fs::read(log_path) {
        Ok(bytes) if !bytes.is_empty() => {
            let mut stdout = io::stdout();
            let _ = stdout.write_all(PRE_REPLAY_RESET);
            let _ = stdout.write_all(&bytes);
            let _ = stdout.flush();
            true
        }
        _ => false,
    }
}
