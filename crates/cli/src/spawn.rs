//! Launching the daemon fully detached from the current terminal/session.
//!
//! Never calls raw `fork()` from inside this (already-running, multi-
//! threaded tokio) process. Instead it re-execs this same binary with the
//! hidden `__daemon-run` subcommand -- `tokio::process::Command::spawn()`
//! does the fork+exec internally in the well-tested standard way, and
//! `pre_exec` (running in the freshly forked, still-single-threaded child,
//! before exec) is the sanctioned place to call `setsid()` so the daemon
//! becomes its own session leader, immune to a `SIGHUP` from the terminal
//! that launched it.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub const DAEMON_SUBCOMMAND: &str = "__daemon-run";

pub struct SpawnRequest {
    pub socket_path: PathBuf,
    pub log_path: Option<PathBuf>,
    pub log_max_size: u64,
    pub scrollback_size: usize,
    pub rows: u16,
    pub cols: u16,
    pub program: String,
    pub args: Vec<String>,
}

impl SpawnRequest {
    /// Build the argv for re-execing this binary into `__daemon-run` mode,
    /// matching the positional order the `DaemonRun` clap variant expects.
    fn daemon_run_args(&self) -> Vec<String> {
        let mut args = vec![
            DAEMON_SUBCOMMAND.to_string(),
            self.socket_path.to_string_lossy().into_owned(),
            self.log_path
                .as_deref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| "-".to_string()),
            self.log_max_size.to_string(),
            self.scrollback_size.to_string(),
            self.rows.to_string(),
            self.cols.to_string(),
            self.program.clone(),
        ];
        args.extend(self.args.iter().cloned());
        args
    }
}

/// Spawn the daemon detached and wait (briefly) for its socket to appear,
/// so the caller can report success/failure the way `atch start` does.
pub async fn spawn_detached(req: &SpawnRequest) -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(req.daemon_run_args())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: pre_exec runs in the forked child, single-threaded, before
    // exec -- the same sound window pty.rs's spawn() relies on.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(io::Error::from)
        });
    }

    let mut child = cmd.spawn()?;
    tokio::select! {
        status = child.wait() => {
            let status = status?;
            Err(io::Error::other(format!(
                "daemon process exited immediately ({status}) -- check the program name/path"
            )))
        }
        () = wait_for_socket(&req.socket_path, Duration::from_secs(3)) => Ok(()),
    }
}

async fn wait_for_socket(path: &Path, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The `__daemon-run` entry point itself: build a `DaemonConfig` and run it
/// to completion in *this* process (already detached by the caller above).
pub async fn run_daemon_foreground(req: SpawnRequest) -> io::Result<i32> {
    let config = hytch_daemon::DaemonConfig {
        socket_path: req.socket_path,
        log_path: req.log_path,
        log_max_size: req.log_max_size,
        scrollback_size: req.scrollback_size,
        program: req.program,
        args: req.args,
        initial_rows: req.rows,
        initial_cols: req.cols,
    };
    let reason = hytch_daemon::run(config).await?;
    Ok(match reason {
        hytch_daemon::ShutdownReason::ChildExited(code) => code.unwrap_or(0),
        hytch_daemon::ShutdownReason::DaemonSignaled => 0,
    })
}
