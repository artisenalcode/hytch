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

#[test]
fn attach_exits_with_the_session_ended_code_when_the_child_process_exits() {
    // Deliberately NOT assert_cmd's `write_stdin` -- that closes stdin
    // immediately after writing, which races ahead of the daemon closing
    // its end (see this file's module doc comment) and would hit the
    // *local* InputClosed outcome instead of the real SessionEnded one
    // this test targets. Also deliberately NOT `Child::wait()`/
    // `wait_with_output()` -- both close stdin as their very first action
    // regardless of whether the caller still holds the `ChildStdin`
    // handle (documented std behavior, there to avoid the *opposite*
    // deadlock: a child blocked reading stdin forever while the parent
    // blocks in wait()). Polling `try_wait()` is the one std API that
    // doesn't touch stdin, so it's the only way to actually keep the pipe
    // open for the whole run and prove the daemon closing the connection
    // -- not local EOF -- is what ends this process.
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join(".cache/hytch/endtest");

    hytch(home.path())
        .args(["start", "endtest", "--", "sh", "-c", "sleep 0.3; exit 0"])
        .assert()
        .success();
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("hytch"))
        .env("HOME", home.path())
        .env_remove("HYTCH_SESSION")
        .args(["attach", "endtest"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("DEBUG timed out; socket.exists()={}", socket.exists());
            panic!("attach did not exit within the timeout");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(90));
}

#[test]
fn default_bare_invocation_also_exits_with_the_session_ended_code() {
    // Same scenario as the test above, but through the bare/default
    // invocation (`hytch <session>`, no `attach` subcommand) instead --
    // this is the path the README's SSH auto-attach snippet actually uses
    // (`hytch main`), and it used to hardcode `return 0` on any successful
    // attach regardless of AttachOutcome, so a real session end here was
    // silently indistinguishable from a detach. Caught by hand, not by the
    // test above, precisely because that test only exercises the explicit
    // `attach` subcommand's own code path (cmd_attach_replayed) -- this one
    // exists so the same regression can't slip back in through cmd_default.
    let home = tempfile::tempdir().unwrap();
    let socket = home.path().join(".cache/hytch/endtest2");

    hytch(home.path())
        .args(["start", "endtest2", "--", "sh", "-c", "sleep 0.3; exit 0"])
        .assert()
        .success();
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("hytch"))
        .env("HOME", home.path())
        .env_remove("HYTCH_SESSION")
        .args(["endtest2"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            eprintln!("DEBUG timed out; socket.exists()={}", socket.exists());
            panic!("bare attach did not exit within the timeout");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(90));
}

#[test]
fn crashed_daemon_log_survives_and_replays_on_next_attach() {
    // Simulate a real crash (SIGKILL on the daemon process itself, not the
    // graceful `Message::Kill` control path that only signals the child)
    // and verify the on-disk log -- not just the ring buffer, which dies
    // with the process -- is what makes "resume after a crash" work.
    let home = tempfile::tempdir().unwrap();
    let session_dir = home.path().join(".cache/hytch");
    std::fs::create_dir_all(&session_dir).unwrap();
    let socket = session_dir.join("crashtest");
    let log = session_dir.join("crashtest.log");

    // Spawn __daemon-run directly (bypassing `start`'s own detach dance)
    // so the test holds a real Child handle to SIGKILL.
    let mut daemon = std::process::Command::new(assert_cmd::cargo::cargo_bin("hytch"))
        .args([
            "__daemon-run",
            socket.to_str().unwrap(),
            log.to_str().unwrap(),
            "1048576",
            "131072",
            "24",
            "80",
            "cat",
        ])
        .env("HOME", home.path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .args(["push", "crashtest"])
        .write_stdin("survivor-bytes\n")
        .assert()
        .success();
    assert!(wait_until(
        || std::fs::read(&log)
            .map(|c| c.windows(14).any(|w| w == b"survivor-bytes"))
            .unwrap_or(false),
        Duration::from_secs(2),
    ));

    // The actual crash.
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    // A crashed daemon leaves its socket behind (no graceful cleanup ran)
    // -- connecting to it must fail, proving this is really testing the
    // cold path and not accidentally still talking to a live daemon.
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_err());

    // `attach` against the now-dead socket should still show the log.
    hytch(home.path())
        .args(["attach", "crashtest"])
        .assert()
        .failure() // nothing is actually listening -- this is expected
        .stdout(predicate::str::contains("survivor-bytes"));
}

#[test]
fn large_push_completes_quickly() {
    // Regression test for review finding #4: the C protocol's winsize-union
    // alias capped every push chunk at 8 bytes, so a payload this size would
    // have needed hundreds of thousands of syscalls. The new length-prefixed
    // proto frames it as (close to) one shot; this should complete in well
    // under a second, not visibly stall.
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["start", "bigpush", "--", "cat"])
        .assert()
        .success();
    let socket = home.path().join(".cache/hytch/bigpush");
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    let payload = "x".repeat(2 * 1024 * 1024); // 2 MiB
    let started = std::time::Instant::now();
    hytch(home.path())
        .args(["push", "bigpush"])
        .write_stdin(payload)
        .timeout(Duration::from_secs(5))
        .assert()
        .success();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "a 2MiB push took {elapsed:?} -- should be near-instant with no 8-byte cap"
    );

    hytch(home.path())
        .args(["kill", "bigpush"])
        .assert()
        .success();
}

#[test]
fn self_attach_is_refused_instead_of_hanging() {
    // Without this guard, `attach` from a shell that's already inside the
    // session it's attaching to would just block forever against the
    // daemon's own accept loop -- a real risk once a shell auto-attach
    // hook is in the picture (its own recursion guard depends on
    // HYTCH_SESSION being set correctly, but a user can still type the
    // session's own name by hand).
    let home = tempfile::tempdir().unwrap();
    hytch(home.path())
        .args(["start", "selftest", "--", "cat"])
        .assert()
        .success();
    let socket = home.path().join(".cache/hytch/selftest");
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .args(["attach", "selftest"])
        .env("HYTCH_SESSION", socket.to_str().unwrap())
        .timeout(Duration::from_secs(3))
        .assert()
        .failure()
        .stdout(predicate::str::contains("itself"));

    hytch(home.path())
        .args(["kill", "selftest"])
        .assert()
        .success();
}

#[test]
fn long_home_path_still_works_via_the_af_unix_workaround() {
    // AF_UNIX's sun_path is ~107 usable bytes on Linux. A tempdir root is
    // already long enough on its own that a realistic session name pushes
    // the full socket path over the limit -- this is exactly the class of
    // path that failed outright before the chdir-based workaround (bind()
    // erroring "path must be shorter than SUN_LEN").
    let home = tempfile::Builder::new()
        .prefix("hytch-edge-case-long-home-directory-name-to-exceed-af-unix-sun-path-limit-")
        .tempdir()
        .unwrap();
    let session = "a-reasonably-descriptive-session-name";
    assert!(
        home.path()
            .join(".cache/hytch")
            .join(session)
            .as_os_str()
            .len()
            > 107,
        "test setup should actually exceed the AF_UNIX limit"
    );

    hytch(home.path())
        .args(["start", session, "--", "cat"])
        .assert()
        .success();

    let socket = home.path().join(".cache/hytch").join(session);
    assert!(wait_until(|| socket.exists(), Duration::from_secs(2)));

    hytch(home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("[running]"));

    hytch(home.path())
        .args(["kill", session])
        .assert()
        .success();
}

#[test]
fn dotdot_and_empty_session_names_are_rejected_with_a_clear_message() {
    // Regression test: `..` (or `.`, or "") as a bare session name resolves
    // via Path::join to the session directory itself once the kernel
    // processes the `..` component at bind() time -- previously this
    // silently reported "started" against an already-existing directory
    // (see spawn.rs's is_socket() and session::is_valid_bare_name() doc
    // comments for the full mechanism).
    let home = tempfile::tempdir().unwrap();

    for bad_name in ["..", ".", ""] {
        hytch(home.path())
            .args(["start", bad_name, "--", "cat"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("not a valid session name"));
    }

    hytch(home.path())
        .arg("list")
        .assert()
        .success()
        .stdout(predicate::str::contains("no sessions"));
}
