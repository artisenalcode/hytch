//! The daemon event loop: owns the `Pty` exclusively (all pty I/O — read,
//! write, resize, signal — is serialized through one task via a command
//! channel, the async analogue of atch's single-threaded `select()` loop
//! serializing everything through one thread), accepts client connections,
//! and fans pty output out through a [`Fanout`].

use crate::age::format_age;
use crate::fanout::Fanout;
use crate::pty::Pty;
use crate::session_log::SessionLog;
use bytes::Bytes;
use hytch_proto::{Message, MessageCodec, RedrawMethod};
use rustix::process::Signal;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt;
use tokio_util::codec::FramedRead;

pub struct DaemonConfig {
    pub socket_path: PathBuf,
    /// `None` disables the on-disk log entirely (matches `-C 0`).
    pub log_path: Option<PathBuf>,
    pub log_max_size: u64,
    /// Must be a power of two.
    pub scrollback_size: usize,
    pub program: String,
    pub args: Vec<String>,
    pub initial_rows: u16,
    pub initial_cols: u16,
}

/// Why the daemon loop stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum ShutdownReason {
    /// The child process exited on its own.
    ChildExited(Option<i32>),
    /// The daemon process itself was asked to stop (SIGTERM/SIGINT). Mirrors
    /// atch's `master_die()`: the daemon exits and cleans up its own socket,
    /// but does not kill the child — that's what `atch kill` (a `Kill`
    /// control message) is for, not a signal aimed at the daemon itself.
    DaemonSignaled,
}

enum PtyCommand {
    Push(Bytes),
    Resize(u16, u16),
    Signal(Signal),
}

/// Run the daemon until the child exits or the daemon itself is signaled.
/// Cleans up (end-of-session log marker, socket unlink) before returning.
///
/// Structured (`tracing`) logs throughout, gated by `RUST_LOG` the normal
/// way (`RUST_LOG=hytch=debug hytch start ...`) -- a genuine operational
/// upgrade over atch's ad hoc `printf` status lines, not just parity: the
/// daemon runs detached with no terminal to print to in the first place, so
/// atch's approach of writing status text to a controlling terminal that
/// may not exist doesn't translate; a real log level, structured fields,
/// and `RUST_LOG` filtering do.
#[tracing::instrument(
    skip(config),
    fields(socket = %config.socket_path.display(), program = %config.program)
)]
pub async fn run(config: DaemonConfig) -> std::io::Result<ShutdownReason> {
    // Create the session directory (and its log's parent, same directory
    // in practice) if this is the first session there -- mirrors atch's
    // get_session_dir() auto-creating ~/.cache/<prog> on demand.
    if let Some(parent) = config.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let started_at = std::time::Instant::now();

    // Extend whatever ancestry chain this daemon process itself inherited
    // (set if the CLI invocation that spawned us was already running
    // inside another session -- nested sessions) with this session's own
    // socket path, so the spawned child sees the full chain via
    // HYTCH_SESSION. This is what the self-attach guard and a shell
    // auto-attach hook's recursion guard both depend on.
    let inherited_chain = std::env::var(hytch_session::SESSION_ENVVAR).ok();
    let session_chain = hytch_session::extend_ancestry(
        inherited_chain.as_deref(),
        &config.socket_path.to_string_lossy(),
    );

    let mut pty = Pty::spawn(
        &config.program,
        &config.args,
        config.initial_rows,
        config.initial_cols,
        &session_chain,
    )?;

    let fanout = Arc::new(Fanout::new(config.scrollback_size, 1024));

    let log = match &config.log_path {
        Some(path) => Some(spawn_log_writer(path.clone(), config.log_max_size)?),
        None => None,
    };

    let listener = bind_listener(&config.socket_path)?;
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCommand>(256);

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    tracing::info!(
        rows = config.initial_rows,
        cols = config.initial_cols,
        logging = config.log_path.is_some(),
        "daemon started"
    );

    let mut buf = [0u8; 4096];
    let reason = loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    tracing::debug!("client connected");
                    tokio::spawn(handle_client(stream, fanout.clone(), cmd_tx.clone()));
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(PtyCommand::Push(data)) => { let _ = pty.write_all(&data).await; }
                    Some(PtyCommand::Resize(rows, cols)) => { let _ = pty.resize(rows, cols); }
                    Some(PtyCommand::Signal(sig)) => { let _ = pty.signal(sig); }
                    None => {} // all senders dropped; harmless, keep looping
                }
            }
            read = pty.read(&mut buf) => {
                match read {
                    Ok(0) | Err(_) => { /* fall through to pty.wait() below */ }
                    Ok(n) => {
                        let chunk = Bytes::copy_from_slice(&buf[..n]);
                        fanout.push(chunk.clone());
                        if let Some((tx, _)) = &log {
                            let _ = tx.send(chunk);
                        }
                        continue;
                    }
                }
                let status = pty.wait().await.ok();
                break ShutdownReason::ChildExited(status.and_then(|s| s.code()));
            }
            _ = sigterm.recv() => break ShutdownReason::DaemonSignaled,
            _ = sigint.recv() => break ShutdownReason::DaemonSignaled,
        }
    };

    let age = format_age(started_at.elapsed().as_secs());
    tracing::info!(reason = ?reason, age = %age, "daemon shutting down");

    if let Some((tx, handle)) = log {
        let marker = format!("\r\n[hytch: session ended after {age}]\r\n");
        let _ = tx.send(Bytes::from(marker.into_bytes()));
        drop(tx); // closes the channel once the marker above is drained
        // Wait for the writer thread to actually finish, not just for the
        // channel to close -- otherwise a detached daemon process could
        // exit before its own end-marker write lands on disk.
        let _ = handle.await;
    }
    let _ = std::fs::remove_file(&config.socket_path);

    Ok(reason)
}

/// Binds a `UnixListener` at `path`, working around `AF_UNIX`'s ~107-byte
/// `sun_path` limit the same way atch's `socket_with_chdir` does: `chdir`
/// into the parent directory and bind the short relative name instead, then
/// restore the original working directory. Found by actually hitting the
/// failure (a moderately long `$HOME` plus a descriptive session name is
/// well within reach of real use, not a contrived case) -- `bind()` with
/// the full path just fails with "path must be shorter than SUN_LEN" and
/// no workaround, which a detached daemon's null'd stderr then swallows
/// entirely, surfacing only as a confusing "exited immediately" from the
/// CLI side.
fn bind_listener(path: &std::path::Path) -> std::io::Result<UnixListener> {
    match hytch_session::shorten_for_unix_socket(path) {
        None => UnixListener::bind(path),
        Some((dir, short_name)) => {
            let original_cwd = std::env::current_dir()?;
            std::env::set_current_dir(&dir)?;
            let result = UnixListener::bind(&short_name);
            std::env::set_current_dir(&original_cwd)?;
            result
        }
    }
}

/// Bridges async `push()` calls to a dedicated blocking OS thread that owns
/// the `SessionLog`. Keeps every bit of log file I/O — including rotation's
/// multi-megabyte read+write — off the tokio reactor thread entirely
/// (review finding #5: the C version ran this inline in its single-threaded
/// event loop, stalling every attached client for the duration).
fn spawn_log_writer(
    path: PathBuf,
    max_size: u64,
) -> std::io::Result<(mpsc::UnboundedSender<Bytes>, tokio::task::JoinHandle<()>)> {
    let mut log = SessionLog::open(&path, max_size)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<Bytes>();
    let handle = tokio::task::spawn_blocking(move || {
        while let Some(chunk) = rx.blocking_recv() {
            let _ = log.append(&chunk);
        }
    });
    Ok((tx, handle))
}

async fn recv_next(
    rx: &mut Option<broadcast::Receiver<Bytes>>,
) -> Option<Result<Bytes, broadcast::error::RecvError>> {
    match rx {
        Some(r) => Some(r.recv().await),
        None => std::future::pending().await,
    }
}

async fn handle_client(
    stream: tokio::net::UnixStream,
    fanout: Arc<Fanout>,
    cmd_tx: mpsc::Sender<PtyCommand>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut framed = FramedRead::new(read_half, MessageCodec::default());
    let mut attached: Option<broadcast::Receiver<Bytes>> = None;

    loop {
        tokio::select! {
            msg = framed.next() => {
                match msg {
                    Some(Ok(Message::Attach { skip_ring })) => {
                        tracing::info!(skip_ring, "client attached");
                        let (snapshot, rx) = fanout.attach();
                        if !skip_ring && write_half.write_all(&snapshot).await.is_err() {
                            return;
                        }
                        attached = Some(rx);
                    }
                    Some(Ok(Message::Detach)) => {
                        tracing::info!("client detached");
                        attached = None;
                    }
                    Some(Ok(Message::Push(data))) => {
                        if cmd_tx.send(PtyCommand::Push(data)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Winch { rows, cols })) => {
                        if cmd_tx.send(PtyCommand::Resize(rows, cols)).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Redraw { method, rows, cols })) => {
                        if method != RedrawMethod::None
                            && cmd_tx.send(PtyCommand::Resize(rows, cols)).await.is_err()
                        {
                            return;
                        }
                        // CtrlL/Winch redraw-signal delivery to the child is
                        // deferred to the cli/client integration pass -- the
                        // resize itself (what actually matters for most
                        // programs) already happens above.
                    }
                    Some(Ok(Message::Kill { signal })) => {
                        tracing::info!(signal, "kill requested");
                        let sig = Signal::from_raw(signal as i32).unwrap_or(Signal::Term);
                        if cmd_tx.send(PtyCommand::Signal(sig)).await.is_err() {
                            return;
                        }
                    }
                    Some(Err(e)) => {
                        tracing::debug!(error = %e, "client protocol error, disconnecting");
                        return;
                    }
                    None => return,
                }
            }
            item = recv_next(&mut attached) => {
                match item {
                    Some(Ok(data)) => {
                        if write_half.write_all(&data).await.is_err() {
                            return;
                        }
                    }
                    Some(Err(broadcast::error::RecvError::Lagged(n))) => {
                        tracing::warn!(lagged = n, "client fell too far behind, disconnecting");
                        return;
                    }
                    Some(Err(broadcast::error::RecvError::Closed)) | None => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
