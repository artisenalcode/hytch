//! Pty allocation and child-process spawning.
//!
//! Deliberately does **not** call raw `fork()` itself. Forking a process
//! that already has a tokio runtime running (multiple threads, locks that
//! might be held by a thread that doesn't survive the fork) is exactly the
//! kind of footgun the C version's daemonizing double-fork exists to work
//! around by hand. `tokio::process::Command` already does fork+exec safely
//! internally; the only custom piece a session daemon needs is *what runs in
//! the child between fork and exec*, which is precisely what `pre_exec` is
//! for — the standard, sanctioned extension point for this exact use case.
//! Owning the syscalls here means the pty allocation and the pre-exec setup
//! (`setsid` + `TIOCSCTTY`), not the fork/exec dance itself.

use rustix::fd::OwnedFd;
use rustix::process::{Pid, Signal};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{Winsize, tcgetwinsize, tcsetwinsize};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Stdio;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, Command};

/// A running child process attached to a pty, with the pty master end
/// wired into tokio's reactor.
pub struct Pty {
    master: AsyncFd<OwnedFd>,
    child: Child,
    /// The child's pid, which — because we call `setsid()` in `pre_exec` —
    /// is also its process group id. Used to signal the whole group, not
    /// just the immediate child, the same way atch's `killpty()` does.
    pgid: Pid,
}

impl Pty {
    /// Allocate a pty, spawn `program` with `args` attached to its slave
    /// side as controlling terminal, and set the initial window size.
    ///
    /// `session_chain` is the value to set `HYTCH_SESSION` to inside the
    /// child — the colon-separated ancestry chain from
    /// `hytch_session::extend_ancestry`. This is what makes the self-attach
    /// guard (and a shell auto-attach hook's own recursion guard) work at
    /// all: without it, a shell running inside a session has no way to know
    /// it's inside one.
    pub fn spawn(
        program: &str,
        args: &[String],
        rows: u16,
        cols: u16,
        session_chain: &str,
    ) -> io::Result<Self> {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(io::Error::from)?;
        grantpt(&master).map_err(io::Error::from)?;
        unlockpt(&master).map_err(io::Error::from)?;
        rustix::io::ioctl_fionbio(&master, true).map_err(io::Error::from)?;

        let winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        tcsetwinsize(&master, winsize).map_err(io::Error::from)?;

        let slave_name = ptsname(&master, Vec::new()).map_err(io::Error::from)?;
        let slave_path: std::path::PathBuf = slave_name.to_string_lossy().into_owned().into();
        // O_NOCTTY here is load-bearing, not decorative: this daemon process
        // is (by the time `start`/`new` re-exec into it) a session leader
        // with no controlling terminal. Opening a tty device without
        // O_NOCTTY under those conditions makes the kernel auto-assign it
        // as *this process's* controlling terminal -- stealing it out from
        // under the child we're about to spawn, whose own pre_exec below
        // claims it deliberately via setsid()+TIOCSCTTY. Without this flag
        // the daemon would acquire (and later lose, triggering a SIGHUP to
        // itself) a controlling terminal it never meant to have.
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NOCTTY.bits() as i32)
            .open(&slave_path)?;

        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd.env(hytch_session::SESSION_ENVVAR, session_chain);
        cmd.stdin(dup_stdio(&slave)?);
        cmd.stdout(dup_stdio(&slave)?);
        cmd.stderr(dup_stdio(&slave)?);
        // The child inherits the master fd unless we tell it not to; it has
        // no business holding it open (it only needs the slave side).
        drop(slave);

        // SAFETY: pre_exec runs in the forked child, after fork and before
        // exec, while the child is still single-threaded — the one window
        // where calling raw libc/syscalls directly (rather than through
        // tokio) is sound. setsid() detaches from any inherited controlling
        // terminal and makes the child a new session/process-group leader;
        // TIOCSCTTY then makes the pty slave (fd 0, just dup'd from stdin)
        // its controlling terminal, mirroring what atch's forkpty() does.
        unsafe {
            cmd.pre_exec(|| {
                rustix::process::setsid().map_err(io::Error::from)?;
                let stdin = rustix::fd::BorrowedFd::borrow_raw(0);
                rustix::process::ioctl_tiocsctty(stdin).map_err(io::Error::from)?;
                Ok(())
            });
        }

        let child = cmd.spawn()?;
        let raw_pid = child
            .id()
            .ok_or_else(|| io::Error::other("spawned child has no pid (already reaped?)"))?;
        let pgid = Pid::from_raw(raw_pid as i32)
            .ok_or_else(|| io::Error::other("spawned child reported a non-positive pid"))?;

        Ok(Pty {
            master: AsyncFd::new(master)?,
            child,
            pgid,
        })
    }

    /// Update the pty's window size (e.g. on a client's `Winch`/`Redraw`).
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        let winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        tcsetwinsize(self.master.get_ref(), winsize).map_err(io::Error::from)
    }

    pub fn current_size(&self) -> io::Result<(u16, u16)> {
        let ws = tcgetwinsize(self.master.get_ref()).map_err(io::Error::from)?;
        Ok((ws.ws_row, ws.ws_col))
    }

    /// Send a signal to the child's whole process group (matches atch's
    /// `killpty()` semantics: signal the group, not just the direct child,
    /// so grandchildren spawned by an interactive shell get it too).
    pub fn signal(&self, sig: Signal) -> io::Result<()> {
        rustix::process::kill_process_group(self.pgid, sig).map_err(io::Error::from)
    }

    /// Wait for the child to exit.
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }
}

impl AsyncRead for Pty {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            let mut guard = match this.master.poll_read_ready(cx) {
                std::task::Poll::Ready(Ok(g)) => g,
                std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            };
            match guard.try_io(|fd| {
                let n = rustix::io::read(fd.get_ref(), buf.initialize_unfilled())
                    .map_err(io::Error::from)?;
                buf.advance(n);
                Ok(())
            }) {
                Ok(result) => return std::task::Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for Pty {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        data: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        let this = self.get_mut();
        loop {
            let mut guard = match this.master.poll_write_ready(cx) {
                std::task::Poll::Ready(Ok(g)) => g,
                std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            };
            match guard.try_io(|fd| rustix::io::write(fd.get_ref(), data).map_err(io::Error::from))
            {
                Ok(result) => return std::task::Poll::Ready(result),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

fn dup_stdio(f: &std::fs::File) -> io::Result<Stdio> {
    Ok(Stdio::from(f.try_clone()?))
}

#[cfg(test)]
mod tests;
