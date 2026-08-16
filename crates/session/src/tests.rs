use crate::{
    ancestry_contains, extend_ancestry, resolve_session_dir, resolve_socket_path, short_name,
};
use std::path::{Path, PathBuf};

// --- resolve_session_dir --------------------------------------------------

#[test]
fn session_dir_uses_home_when_set() {
    let dir = resolve_session_dir(Some("/home/alice"), None, "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/home/alice/.cache/hytch"));
}

#[test]
fn session_dir_falls_back_to_passwd_home_when_env_unset() {
    let dir = resolve_session_dir(None, Some("/home/bob"), "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/home/bob/.cache/hytch"));
}

#[test]
fn session_dir_falls_back_to_passwd_home_when_env_empty() {
    let dir = resolve_session_dir(Some(""), Some("/home/bob"), "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/home/bob/.cache/hytch"));
}

#[test]
fn session_dir_falls_back_to_tmp_when_home_is_root() {
    // $HOME="/" is explicitly rejected, same as unset — matches atch's
    // "use $HOME only if set and not the root directory" rule.
    let dir = resolve_session_dir(Some("/"), None, "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/tmp/.hytch-1000"));
}

#[test]
fn session_dir_falls_back_to_tmp_when_nothing_available() {
    let dir = resolve_session_dir(None, None, "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/tmp/.hytch-1000"));
}

#[test]
fn session_dir_falls_back_to_tmp_when_passwd_home_empty() {
    let dir = resolve_session_dir(None, Some(""), "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/tmp/.hytch-1000"));
}

#[test]
fn session_dir_falls_back_to_tmp_when_both_unusable() {
    let dir = resolve_session_dir(Some(""), Some("/"), "hytch", 1000);
    assert_eq!(dir, PathBuf::from("/tmp/.hytch-1000"));
}

// --- resolve_socket_path ---------------------------------------------------

#[test]
fn socket_path_joins_bare_name_with_session_dir() {
    let dir = Path::new("/home/alice/.cache/hytch");
    let path = resolve_socket_path("work", dir);
    assert_eq!(path, PathBuf::from("/home/alice/.cache/hytch/work"));
}

#[test]
fn socket_path_treats_name_with_slash_as_literal() {
    let dir = Path::new("/home/alice/.cache/hytch");
    let path = resolve_socket_path("/tmp/mysession", dir);
    assert_eq!(path, PathBuf::from("/tmp/mysession"));
}

#[test]
fn socket_path_treats_relative_slash_name_as_literal_too() {
    let dir = Path::new("/home/alice/.cache/hytch");
    let path = resolve_socket_path("sub/dir/name", dir);
    assert_eq!(path, PathBuf::from("sub/dir/name"));
}

// --- extend_ancestry / ancestry_contains -----------------------------------

#[test]
fn extend_ancestry_starts_a_fresh_chain() {
    assert_eq!(extend_ancestry(None, "/a"), "/a");
}

#[test]
fn extend_ancestry_appends_to_existing_chain() {
    assert_eq!(extend_ancestry(Some("/a"), "/b"), "/a:/b");
}

#[test]
fn extend_ancestry_treats_empty_current_as_none() {
    assert_eq!(extend_ancestry(Some(""), "/a"), "/a");
}

#[test]
fn ancestry_contains_none_is_false() {
    assert!(!ancestry_contains(None, "/a"));
}

#[test]
fn ancestry_contains_direct_match() {
    assert!(ancestry_contains(Some("/a"), "/a"));
}

#[test]
fn ancestry_contains_match_anywhere_in_chain() {
    // Covers the indirect-loop case: A -> B -> A. When session A tries to
    // attach to itself via an intermediate hop through B, the ancestry
    // chain is "/a:/b" and the target is "/a" — must be caught even though
    // "/a" isn't the *last* link.
    assert!(ancestry_contains(Some("/a:/b"), "/a"));
    assert!(ancestry_contains(Some("/a:/b"), "/b"));
}

#[test]
fn ancestry_contains_rejects_prefix_false_positive() {
    // Must compare whole ':'-delimited segments, not do a substring search —
    // "/path/ab" contains the substring "/path/a" but is not "/path/a".
    assert!(!ancestry_contains(Some("/path/ab"), "/path/a"));
}

#[test]
fn ancestry_contains_no_match() {
    assert!(!ancestry_contains(Some("/a:/b"), "/c"));
}

// --- short_name -------------------------------------------------------------

#[test]
fn short_name_single_session() {
    assert_eq!(short_name("/home/alice/.cache/hytch/outer"), "outer");
}

#[test]
fn short_name_nested_chain() {
    assert_eq!(
        short_name("/home/alice/.cache/hytch/outer:/home/alice/.cache/hytch/inner"),
        "outer > inner"
    );
}
