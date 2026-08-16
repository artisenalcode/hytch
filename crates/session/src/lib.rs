//! Session naming, directory/socket-path resolution, and the ancestry-chain
//! self-attach guard. Shared by `cli`, `daemon`, and `client` — this is the
//! one place that knows how a session *name* becomes a socket *path*.

use std::path::{Path, PathBuf};

pub const SESSION_ENVVAR: &str = "HYTCH_SESSION";

/// Where session sockets live by default, given `$HOME` and a passwd-database
/// fallback home directory (both already resolved by the caller — this
/// function has no OS dependency, so it's fully unit-testable).
///
/// Mirrors atch's fallback chain: `$HOME` if set and not `/`, else the passwd
/// entry's home directory under the same condition, else `/tmp/.<prog>-<uid>`.
pub fn resolve_session_dir(
    env_home: Option<&str>,
    passwd_home: Option<&str>,
    prog_basename: &str,
    uid: u32,
) -> PathBuf {
    match usable_home(env_home).or_else(|| usable_home(passwd_home)) {
        Some(home) => PathBuf::from(format!("{home}/.cache/{prog_basename}")),
        None => PathBuf::from(format!("/tmp/.{prog_basename}-{uid}")),
    }
}

/// A home directory is usable if it's set, non-empty, and not `/`.
fn usable_home(h: Option<&str>) -> Option<&str> {
    h.filter(|s| !s.is_empty() && *s != "/")
}

/// Resolve a session name to its socket path. A name containing `/` is used
/// as-is (relative or absolute); a bare name is joined under `session_dir`.
pub fn resolve_socket_path(name: &str, session_dir: &Path) -> PathBuf {
    if name.contains('/') {
        PathBuf::from(name)
    } else {
        session_dir.join(name)
    }
}

/// Rejects bare session names that are really directory-traversal
/// components in disguise. `Path::join` doesn't normalize `.`/`..`, so
/// `resolve_socket_path("..", dir)` produces a `PathBuf` that *looks* like
/// `dir/..` but the kernel resolves at bind/connect time to `dir`'s parent
/// -- an already-existing directory, not a fresh socket path. Found by
/// hitting this directly: `hytch start ..` silently "succeeded" against
/// the session directory itself. `resolve_socket_path` still accepts these
/// (it has no opinion on validity, only resolution); callers creating a
/// *new* session should check this first and reject with a clear message
/// instead of a confusing downstream bind failure.
pub fn is_valid_bare_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

/// Build the ancestry-chain env var value for a session being spawned inside
/// `current` (the parent's own chain, if any). A single, non-nested session
/// has one entry; nested sessions accumulate, outermost first.
pub fn extend_ancestry(current: Option<&str>, new_socket_path: &str) -> String {
    match current.filter(|c| !c.is_empty()) {
        Some(chain) => format!("{chain}:{new_socket_path}"),
        None => new_socket_path.to_string(),
    }
}

/// Whether `target_socket_path` appears anywhere in the ancestry chain —
/// catches both direct self-attach and indirect loops (A attaches through B
/// back to A). Compares whole `:`-delimited segments, not a substring match.
pub fn ancestry_contains(chain: Option<&str>, target_socket_path: &str) -> bool {
    match chain.filter(|c| !c.is_empty()) {
        Some(c) => c.split(':').any(|segment| segment == target_socket_path),
        None => false,
    }
}

/// `AF_UNIX`'s `sun_path` is 108 bytes on Linux, including the NUL
/// terminator, so the longest usable path is 107 bytes. A session dir
/// under a moderately long `$HOME` plus a descriptive session name blows
/// past this in practice — not a hypothetical (found by actually hitting
/// it: `UnixListener::bind` fails outright with "path must be shorter than
/// SUN_LEN").
pub const MAX_SOCKET_PATH_LEN: usize = 107;

/// If `path` is too long for a raw `AF_UNIX` bind/connect, returns
/// `(dir_to_chdir_into, short_relative_name)` so the caller can `chdir`
/// there and bind/connect using just the short name instead — mirrors
/// atch's `socket_with_chdir`. Returns `None` when `path` already fits and
/// no workaround is needed.
///
/// Pure path arithmetic only, no I/O — the actual `chdir`+bind/connect+
/// restore sequence is the caller's job (this crate deliberately does no
/// OS calls, see the module doc comment).
pub fn shorten_for_unix_socket(path: &Path) -> Option<(PathBuf, PathBuf)> {
    if path.as_os_str().len() <= MAX_SOCKET_PATH_LEN {
        return None;
    }
    let parent = path.parent()?.to_path_buf();
    let name = path.file_name()?.to_owned();
    Some((parent, PathBuf::from(name)))
}

/// Human-readable form of an ancestry chain: basenames only, joined by
/// `" > "`, outermost first — e.g. `"outer > inner"`.
pub fn short_name(chain: &str) -> String {
    chain
        .split(':')
        .map(|path| {
            Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string())
        })
        .collect::<Vec<_>>()
        .join(" > ")
}

#[cfg(test)]
mod tests;
