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
