mod attach;
mod commands;
mod paths;
mod spawn;

use clap::{Parser, Subcommand};
use spawn::SpawnRequest;
use std::path::PathBuf;

/// Fast, lean, mouse-transparent terminal session persistence.
#[derive(Parser)]
#[command(name = "hytch", version, about)]
struct Cli {
    /// Detach character (accepts `^X` notation). Default: `^\`.
    #[arg(short = 'e', global = true)]
    detach_char: Option<String>,
    /// Disable the detach character entirely.
    #[arg(short = 'E', global = true)]
    no_detach: bool,
    /// Suppress informational messages.
    #[arg(short = 'q', global = true)]
    quiet: bool,
    /// On-disk log cap for a session being created: bytes, or a number with
    /// a k/K (KiB) or m/M (MiB) suffix. 0 disables logging. Default: 1m.
    #[arg(short = 'C', global = true)]
    log_cap: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Attach to a session, failing if it doesn't exist.
    #[command(visible_alias = "a")]
    Attach { session: String },

    /// Create a new session and attach to it.
    #[command(visible_alias = "n")]
    New {
        session: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },

    /// Create a new session, detached.
    #[command(visible_alias = "s")]
    Start {
        session: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },

    /// Copy stdin verbatim to a running session.
    #[command(visible_alias = "p")]
    Push { session: String },

    /// Stop a session (SIGTERM, then SIGKILL after a grace period).
    #[command(visible_alias = "k")]
    Kill {
        /// Skip the grace period, send SIGKILL immediately.
        #[arg(short, long)]
        force: bool,
        session: String,
    },

    /// Truncate a session's on-disk log.
    Clear { session: Option<String> },

    /// List sessions.
    #[command(visible_alias = "l", visible_alias = "ls")]
    List {
        /// Also show exited sessions that still have a log on disk.
        #[arg(short = 'a')]
        all: bool,
    },

    /// Remove a stale or exited session's socket and log.
    Rm {
        #[arg(short = 'a')]
        all: bool,
        session: Option<String>,
    },

    /// Print the current session name; exit 1 (silently) if not inside one.
    Current,

    /// Internal: run the daemon in the foreground. Not part of the public
    /// interface -- `start`/`new` re-exec into this, already detached.
    #[command(name = "__daemon-run", hide = true)]
    DaemonRun {
        socket_path: PathBuf,
        log_path: String, // "-" means None
        log_max_size: u64,
        scrollback_size: usize,
        rows: u16,
        cols: u16,
        program: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Bare `hytch [<session> [cmd...]]`: attach, creating if necessary.
    #[command(external_subcommand)]
    Default(Vec<String>),
}

const DEFAULT_LOG_MAX_SIZE: u64 = 1024 * 1024;
const SCROLLBACK_SIZE: usize = 128 * 1024;

fn parse_log_cap(spec: Option<&str>) -> u64 {
    let Some(spec) = spec else {
        return DEFAULT_LOG_MAX_SIZE;
    };
    let (digits, mult) = match spec.chars().last() {
        Some('k') | Some('K') => (&spec[..spec.len() - 1], 1024),
        Some('m') | Some('M') => (&spec[..spec.len() - 1], 1024 * 1024),
        _ => (spec, 1),
    };
    digits
        .parse::<u64>()
        .map(|n| n * mult)
        .unwrap_or(DEFAULT_LOG_MAX_SIZE)
}

// current_thread, not the default multi-threaded flavor: most invocations
// of this binary are one-shot commands (list/kill/push/current/...) with
// no real parallelism to exploit, and a multi-threaded runtime's worker-
// pool spin-up cost showed up directly in a head-to-head benchmark against
// atch as ~2.5x higher per-invocation overhead (see BENCHMARKS.md). The
// daemon (__daemon-run) is the one long-lived role, but its actual
// concurrency need -- a handful of attached clients on one pty -- doesn't
// require real OS-thread parallelism either; a single-threaded async
// reactor handles it the same way many single-threaded event-loop daemons
// do. spawn_blocking (used for log I/O) still gets its own thread pool
// regardless of this flavor.
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = Cli::parse();
    let quiet = cli.quiet;
    let detach_char = if cli.no_detach {
        None
    } else {
        Some(
            cli.detach_char
                .as_deref()
                .and_then(commands::parse_char_spec)
                .unwrap_or(0x1c), // ^\
        )
    };
    let log_max_size = parse_log_cap(cli.log_cap.as_deref());

    let code = match cli.command {
        Some(Command::Attach { session }) => cmd_attach(&session, detach_char, quiet).await,
        Some(Command::New { session, cmd }) => {
            cmd_new(&session, cmd, log_max_size, detach_char, quiet).await
        }
        Some(Command::Start { session, cmd }) => {
            cmd_start(&session, cmd, log_max_size, quiet).await
        }
        Some(Command::Push { session }) => {
            commands::push(&paths::socket_path(&session), &session).await
        }
        Some(Command::Kill { force, session }) => {
            commands::kill(&paths::socket_path(&session), &session, force, quiet).await
        }
        Some(Command::Clear { session }) => cmd_clear(session, quiet),
        Some(Command::List { all }) => commands::list(&paths::session_dir(), all, quiet),
        Some(Command::Rm { all, session }) => cmd_rm(all, session, quiet),
        Some(Command::Current) => commands::current(),
        Some(Command::DaemonRun {
            socket_path,
            log_path,
            log_max_size,
            scrollback_size,
            rows,
            cols,
            program,
            args,
        }) => {
            let req = SpawnRequest {
                socket_path,
                log_path: (log_path != "-").then(|| PathBuf::from(log_path)),
                log_max_size,
                scrollback_size,
                rows,
                cols,
                program,
                args,
            };
            match spawn::run_daemon_foreground(req).await {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("hytch: daemon: {e}");
                    1
                }
            }
        }
        Some(Command::Default(rest)) => cmd_default(rest, log_max_size, detach_char, quiet).await,
        None => {
            eprintln!("hytch: a session name is required. Try `hytch --help`.");
            1
        }
    };

    std::process::ExitCode::from(code as u8)
}

fn build_spawn_request(session: &str, cmd: Vec<String>, log_max_size: u64) -> SpawnRequest {
    let socket_path = paths::socket_path(session);
    let (program, args) = match cmd.split_first() {
        Some((program, rest)) => (program.clone(), rest.to_vec()),
        None => (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            vec![],
        ),
    };
    let (rows, cols) = current_terminal_size();
    SpawnRequest {
        log_path: (log_max_size > 0).then(|| paths::log_path_for(&socket_path)),
        socket_path,
        log_max_size,
        scrollback_size: SCROLLBACK_SIZE,
        rows,
        cols,
        program,
        args,
    }
}

fn current_terminal_size() -> (u16, u16) {
    let fd = rustix::stdio::stdin();
    if rustix::termios::isatty(fd) {
        hytch_client::resize::current_size(fd).unwrap_or((24, 80))
    } else {
        (24, 80)
    }
}

async fn cmd_attach(session: &str, detach_char: Option<u8>, quiet: bool) -> i32 {
    let socket_path = paths::socket_path(session);
    let log_path = paths::log_path_for(&socket_path);
    let replayed = attach::replay_log_to_stdout(&log_path);
    cmd_attach_replayed(session, &socket_path, detach_char, replayed, quiet).await
}

async fn cmd_attach_replayed(
    session: &str,
    socket_path: &std::path::Path,
    detach_char: Option<u8>,
    replayed: bool,
    quiet: bool,
) -> i32 {
    match attach::attach_foreground(socket_path, detach_char, replayed, quiet).await {
        Ok(_) => 0,
        Err(e) => {
            commands::print_connect_error(session, &e);
            1
        }
    }
}

async fn cmd_new(
    session: &str,
    cmd: Vec<String>,
    log_max_size: u64,
    detach_char: Option<u8>,
    quiet: bool,
) -> i32 {
    // A `new` invoked directly (not via the default attach-or-create
    // fallback) hasn't replayed anything yet in this process -- do it here,
    // same as atch: a name reused from a prior (now-gone) session still has
    // its old log on disk, and creating a same-named session again should
    // show it first, same "you know exactly what it did" guarantee as
    // reattaching to one that's still running.
    let socket_path = paths::socket_path(session);
    let log_path = paths::log_path_for(&socket_path);
    let replayed = attach::replay_log_to_stdout(&log_path);
    cmd_new_after_replay(session, cmd, log_max_size, detach_char, replayed, quiet).await
}

async fn cmd_new_after_replay(
    session: &str,
    cmd: Vec<String>,
    log_max_size: u64,
    detach_char: Option<u8>,
    replayed: bool,
    quiet: bool,
) -> i32 {
    let req = build_spawn_request(session, cmd, log_max_size);
    if let Err(e) = spawn::spawn_detached(&req).await {
        eprintln!("hytch: {session}: {e}");
        return 1;
    }
    if !quiet {
        eprintln!("hytch: session '{session}' created");
    }
    let socket_path = paths::socket_path(session);
    cmd_attach_replayed(session, &socket_path, detach_char, replayed, quiet).await
}

async fn cmd_start(session: &str, cmd: Vec<String>, log_max_size: u64, quiet: bool) -> i32 {
    let req = build_spawn_request(session, cmd, log_max_size);
    match spawn::spawn_detached(&req).await {
        Ok(()) => {
            commands::print_started(session, quiet);
            0
        }
        Err(e) => {
            eprintln!("hytch: {session}: {e}");
            1
        }
    }
}

async fn cmd_default(
    rest: Vec<String>,
    log_max_size: u64,
    detach_char: Option<u8>,
    quiet: bool,
) -> i32 {
    let Some((session, cmd)) = rest.split_first() else {
        eprintln!("hytch: a session name is required. Try `hytch --help`.");
        return 1;
    };
    let cmd = cmd.to_vec();

    // Replay exactly once for this whole invocation, regardless of which
    // branch below actually ends up attaching -- see attach_foreground's
    // doc comment for why doing it per-branch would show history twice.
    let socket_path = paths::socket_path(session);
    let log_path = paths::log_path_for(&socket_path);
    let replayed = attach::replay_log_to_stdout(&log_path);

    // Try a strict attach first; only spawn if nothing is listening.
    if socket_path.exists()
        && attach::attach_foreground(&socket_path, detach_char, replayed, quiet)
            .await
            .is_ok()
    {
        return 0;
    }
    cmd_new_after_replay(session, cmd, log_max_size, detach_char, replayed, quiet).await
}

fn cmd_clear(session: Option<String>, quiet: bool) -> i32 {
    let name = match session.or_else(|| {
        std::env::var("HYTCH_SESSION")
            .ok()
            .map(|c| hytch_session::short_name(&c))
    }) {
        Some(n) => n,
        None => {
            eprintln!("hytch: no session specified and not inside one");
            return 1;
        }
    };
    let log_path = paths::log_path_for(&paths::socket_path(&name));
    commands::clear(&log_path, &name, quiet)
}

fn cmd_rm(all: bool, session: Option<String>, quiet: bool) -> i32 {
    if all {
        // Full sweep intentionally deferred -- see the plan doc. Single-
        // session rm (the common "clean up after a crash" case) is covered.
        eprintln!("hytch: rm -a is not implemented yet; remove sessions by name for now");
        return 1;
    }
    let Some(name) = session else {
        eprintln!("hytch: a session name is required (or use -a)");
        return 1;
    };
    let socket_path = paths::socket_path(&name);
    let log_path = paths::log_path_for(&socket_path);
    commands::rm(&socket_path, &log_path, &name, quiet)
}
