use crate::{
    MAX_SOCKET_PATH_LEN, ancestry_contains, extend_ancestry, is_valid_bare_name,
    resolve_session_dir, resolve_socket_path, short_name, shorten_for_unix_socket,
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

// --- shorten_for_unix_socket -------------------------------------------------

#[test]
fn shorten_returns_none_for_a_path_that_already_fits() {
    assert_eq!(
        shorten_for_unix_socket(Path::new("/home/x/.cache/hytch/work")),
        None
    );
}

#[test]
fn shorten_returns_none_right_at_the_boundary() {
    let exactly_max = "/".to_string() + &"a".repeat(MAX_SOCKET_PATH_LEN - 1);
    assert_eq!(exactly_max.len(), MAX_SOCKET_PATH_LEN);
    assert_eq!(shorten_for_unix_socket(Path::new(&exactly_max)), None);
}

#[test]
fn shorten_splits_into_parent_dir_and_short_name_when_too_long() {
    let long_home = "/home/".to_string() + &"x".repeat(100);
    let path = PathBuf::from(format!("{long_home}/.cache/hytch/work"));
    assert!(path.as_os_str().len() > MAX_SOCKET_PATH_LEN);

    let (dir, name) = shorten_for_unix_socket(&path).expect("should need shortening");
    assert_eq!(dir, PathBuf::from(format!("{long_home}/.cache/hytch")));
    assert_eq!(name, PathBuf::from("work"));
    // The whole point: the short name alone must fit even though the full
    // path didn't.
    assert!(name.as_os_str().len() <= MAX_SOCKET_PATH_LEN);
}

// --- is_valid_bare_name ------------------------------------------------------

#[test]
fn valid_bare_names_are_accepted() {
    assert!(is_valid_bare_name("work"));
    assert!(is_valid_bare_name("my session"));
    assert!(is_valid_bare_name("a.b.c"));
    assert!(is_valid_bare_name(".hidden"));
    assert!(is_valid_bare_name("..two-leading-dots-but-not-just-dots"));
}

#[test]
fn empty_dot_and_dotdot_are_rejected() {
    assert!(!is_valid_bare_name(""));
    assert!(!is_valid_bare_name("."));
    assert!(!is_valid_bare_name(".."));
}

#[test]
fn names_containing_slash_are_rejected_here_too() {
    // resolve_socket_path treats these as literal paths rather than bare
    // names; is_valid_bare_name is specifically about the *bare* case, so
    // a slash means "not what this function is for" -> reject.
    assert!(!is_valid_bare_name("a/b"));
    assert!(!is_valid_bare_name("/etc/passwd"));
}
