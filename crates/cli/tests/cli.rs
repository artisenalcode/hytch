//! CLI integration tests, driving the real `hytch` binary via `assert_cmd`.
//!
//! Deliberately does not try to assert exact byte content round-tripped
//! through an interactive `attach`/`new` session: piped stdin closes
//! (EOF) as soon as `write_stdin` finishes, and the attach loop exits on
//! stdin EOF the same way atch's own C client does (`read() <= 0 ->
//! exit(1)`) -- a real race between "stdin EOF noticed" and "daemon's
//! echo arrives," inherent to non-interactive piped stdin against a tool
//! built around real terminals, not a bug in either implementation. The
//! `client` crate's own tests already cover exact byte-forwarding without
//! that race (see crates/client/src/attach/tests.rs). What's tested here
//! is the one-shot commands, which have no such race.

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;

fn hytch(home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("hytch").unwrap();
    cmd.env("HOME", home);
    cmd.env_remove("HYTCH_SESSION");
    cmd
}

fn wait_until(mut check: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    check()
}

#[test]
fn list_on_a_fresh_home_reports_no_sessions() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions"));
}

#[test]
fn current_outside_a_session_exits_1_silently() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .arg("current")
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty());
}

#[test]
fn kill_of_a_nonexistent_session_reports_it_does_not_exist() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["kill", "nosuchsession"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("does not exist"));
}

#[test]
fn push_to_a_nonexistent_session_reports_it_does_not_exist() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["push", "nosuchsession"])
        .write_stdin("data")
        .assert()
        .failure()
        .stdout(predicate::str::contains("does not exist"));
}

#[test]
fn rm_of_a_nonexistent_session_reports_it_does_not_exist() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["rm", "nosuchsession"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("does not exist"));
}

#[test]
fn start_creates_a_session_visible_in_list_and_kill_removes_it() {
    let home = tempfile::tempdir().unwrap();

    hytch(home.path())
        .args(["start", "clitest", "--", "cat"])
        .assert()
        .success()
        .stdout(predicate::str::contains("session 'clitest' started"));

    let socket = home.path().join(".cache/hytch/clitest");
    assert!(
        wait_until(|| socket.exists(), Duration::from_secs(2)),
        "socket should appear after start"
    );

    hytch(home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("clitest"))
        .stdout(predicate::str::contains("[running]"));

    hytch(home.path())
        .args(["kill", "clitest"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stopped"));

    assert!(
        wait_until(|| !socket.exists(), Duration::from_secs(2)),
        "socket should be removed after kill"
    );
}

#[test]
fn push_reaches_a_running_session() {
    let home = tempfile::tempdir().unwrap();

    hytch(home.path())
        .args(["start", "pushtest", "--", "cat"])
        .assert()
        .success();

    let socket = home.path().join(".cache/hytch/pushtest");
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .args(["push", "pushtest"])
        .write_stdin("marker-bytes\n")
        .assert()
        .success();

    let log = home.path().join(".cache/hytch/pushtest.log");
    assert!(
        wait_until(
            || std::fs::read(&log)
                .map(|c| c.windows(12).any(|w| w == b"marker-bytes"))
                .unwrap_or(false),
            Duration::from_secs(2),
        ),
        "pushed bytes should show up in the session log"
    );

    hytch(home.path())
        .args(["kill", "pushtest"])
        .assert()
        .success();
}

#[test]
fn rm_refuses_a_running_session() {
    let home = tempfile::tempdir().unwrap();

    hytch(home.path())
        .args(["start", "rmtest", "--", "cat"])
        .assert()
        .success();
    let socket = home.path().join(".cache/hytch/rmtest");
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .args(["rm", "rmtest"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("is running"));

    hytch(home.path())
        .args(["kill", "rmtest"])
        .assert()
        .success();
}

#[test]
fn rm_removes_a_stopped_session() {
    let home = tempfile::tempdir().unwrap();

    hytch(home.path())
        .args(["start", "rmtest2", "--", "cat"])
        .assert()
        .success();
    let socket = home.path().join(".cache/hytch/rmtest2");
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .args(["kill", "rmtest2"])
        .assert()
        .success();
    assert!(wait_until(|| !socket.exists(), Duration::from_secs(2)));

    let log = home.path().join(".cache/hytch/rmtest2.log");
    assert!(log.exists(), "log should survive the kill");

    hytch(home.path())
        .args(["rm", "rmtest2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("removed"));
    assert!(!log.exists(), "rm should remove the log too");
}

#[test]
fn help_lists_the_documented_subcommands() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("attach"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("start"))
        .stdout(predicate::str::contains("kill"))
        .stdout(predicate::str::contains("list"))
        // The internal re-exec entry point must never appear in --help.
        .stdout(predicate::str::contains("__daemon-run").not());
}

#[test]
fn attach_to_a_nonexistent_session_fails() {
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["attach", "nosuchsession"])
        .assert()
        .failure();
}
