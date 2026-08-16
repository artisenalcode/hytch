//! One-shot commands that don't need the attach loop: kill, list, rm,
//! current, push, clear.

use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

/// Parse a detach-character spec: `^X` notation (control character) or a
/// literal single byte. Matches atch's `-e` option.
pub fn parse_char_spec(s: &str) -> Option<u8> {
    let bytes = s.as_bytes();
    if bytes.len() == 2 && bytes[0] == b'^' {
        let c = bytes[1].to_ascii_uppercase();
        if c == b'?' {
            return Some(0x7f); // ^? is DEL
        }
        if c.is_ascii_uppercase() || c == b'@' || (b'['..=b'_').contains(&c) {
            return Some(c & 0x1f);
        }
    }
    bytes.first().copied()
}

pub async fn kill(socket_path: &Path, name: &str, force: bool, quiet: bool) -> i32 {
    let signal: u8 = if force { 9 } else { 15 }; // SIGKILL : SIGTERM
    match send_control(socket_path, hytch_proto::Message::Kill { signal }).await {
        Ok(()) => {}
        Err(e) => {
            print_connect_error(name, &e);
            return 1;
        }
    }

    let max_wait = if force {
        Duration::from_secs(2)
    } else {
        Duration::from_secs(5)
    };
    if wait_gone(socket_path, max_wait).await {
        if !quiet {
            println!("hytch: session '{name}' stopped");
        }
        return 0;
    }

    if !force {
        // Escalate to SIGKILL after the grace period, same as atch.
        let _ = send_control(socket_path, hytch_proto::Message::Kill { signal: 9 }).await;
        if wait_gone(socket_path, Duration::from_secs(2)).await {
            if !quiet {
                println!("hytch: session '{name}' killed");
            }
            return 0;
        }
    }

    println!("hytch: session '{name}' did not stop");
    1
}

async fn wait_gone(socket_path: &Path, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if !socket_path.exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    !socket_path.exists()
}

async fn send_control(socket_path: &Path, msg: hytch_proto::Message) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::Encoder;

    let mut stream = tokio::net::UnixStream::connect(socket_path).await?;
    let mut codec = hytch_proto::MessageCodec::default();
    let mut buf = bytes::BytesMut::new();
    codec.encode(msg, &mut buf)?;
    stream.write_all(&buf).await
}

/// Stays comfortably under `hytch_proto::DEFAULT_MAX_FRAME_LEN` (64 KiB) --
/// that cap exists so the daemon never has to trust a claimed frame length
/// enough to allocate for it; a large `push` has to chunk into multiple
/// frames to respect it, not treat the whole of stdin as one frame. Found
/// by an actual 2 MiB push test that got disconnected mid-write once the
/// daemon rejected the oversized single frame.
const PUSH_CHUNK_SIZE: usize = 32 * 1024;

pub async fn push(socket_path: &Path, name: &str) -> i32 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::codec::Encoder;

    let mut stream = match tokio::net::UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(e) => {
            print_connect_error(name, &e);
            return 1;
        }
    };

    let mut codec = hytch_proto::MessageCodec::default();
    let mut stdin = tokio::io::stdin();
    let mut chunk = vec![0u8; PUSH_CHUNK_SIZE];
    loop {
        let n = match stdin.read(&mut chunk).await {
            Ok(0) => return 0,
            Ok(n) => n,
            Err(e) => {
                eprintln!("hytch: {name}: {e}");
                return 1;
            }
        };
        let mut buf = bytes::BytesMut::new();
        let msg = hytch_proto::Message::Push(bytes::Bytes::copy_from_slice(&chunk[..n]));
        if codec.encode(msg, &mut buf).is_err() || stream.write_all(&buf).await.is_err() {
            eprintln!("hytch: {name}: connection to session lost mid-push");
            return 1;
        }
    }
}

pub fn clear(log_path: &Path, name: &str, quiet: bool) -> i32 {
    match std::fs::OpenOptions::new().write(true).open(log_path) {
        Ok(f) => {
            let _ = f.set_len(0);
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0, // nothing to clear
        Err(e) => {
            if !quiet {
                eprintln!("hytch: {name}: {e}");
            }
            1
        }
    }
}

pub fn rm(socket_path: &Path, log_path: &Path, name: &str, quiet: bool) -> i32 {
    // Refuse if a daemon is actually listening -- mirrors atch: "use kill
    // first". A quick non-blocking connect attempt is enough to tell.
    if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
        println!("hytch: session '{name}' is running (use 'hytch kill {name}' first)");
        return 1;
    }

    let had_socket = socket_path.exists();
    let had_log = log_path.exists();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_file(log_path);

    if !had_socket && !had_log {
        println!("hytch: session '{name}' does not exist");
        return 1;
    }
    if !quiet {
        println!("hytch: session '{name}' removed");
    }
    0
}

pub fn current() -> i32 {
    match std::env::var("HYTCH_SESSION") {
        Ok(chain) if !chain.is_empty() => {
            println!("{}", hytch_session::short_name(&chain));
            0
        }
        _ => 1,
    }
}

pub fn list(session_dir: &Path, show_all: bool, quiet: bool) -> i32 {
    let mut count = 0;
    let entries = match std::fs::read_dir(session_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if count == 0 && !quiet {
                println!("(no sessions)");
            }
            return 0;
        }
        Err(e) => {
            eprintln!("hytch: {}: {e}", session_dir.display());
            return 1;
        }
    };

    let mut names: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    names.sort_by_key(|e| e.file_name());

    for entry in &names {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name.starts_with('.') || name.ends_with(".log") {
            continue;
        }
        // A successful connect only proves the daemon is alive and
        // accepting connections -- it says nothing about whether a client
        // is actually attached right now (that would need the daemon to
        // expose its attached-client count, or the executable-bit signal
        // atch toggles on the socket file on attach/detach; neither is
        // wired up yet, so this deliberately says "running", not
        // "attached").
        let path = entry.path();
        let running = std::os::unix::net::UnixStream::connect(&path).is_ok();
        if running {
            println!("{name:<24} [running]");
        } else {
            println!("{name:<24} [stale]");
        }
        count += 1;
    }

    if show_all {
        for entry in &names {
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let Some(base) = name.strip_suffix(".log") else {
                continue;
            };
            if session_dir.join(base).exists() {
                continue; // already listed above
            }
            println!("{base:<24} [exited]");
            count += 1;
        }
    }

    if count == 0 && !quiet {
        println!("(no sessions)");
    }
    0
}

pub fn print_connect_error(name: &str, e: &std::io::Error) {
    match e.kind() {
        std::io::ErrorKind::NotFound => println!("hytch: session '{name}' does not exist"),
        std::io::ErrorKind::ConnectionRefused => {
            println!("hytch: session '{name}' is not running")
        }
        _ => println!("hytch: {name}: {e}"),
    }
}

pub fn print_started(name: &str, quiet: bool) {
    if !quiet {
        let _ = std::io::stdout().flush();
        println!("hytch: session '{name}' started");
    }
}
