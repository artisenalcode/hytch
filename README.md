# hytch

A fast, lean terminal session daemon: attach, detach, and resume exactly
where you left off — across disconnects, crashes, and reboots — with no
terminal emulation in the way, so mouse reporting, scroll, and true-color
pass through untouched.

Independent reimplementation inspired by [`atch`](https://github.com/mobydeck/atch)
and `dtach`'s design goals — not a fork, no shared code (see [Licensing](#licensing)).
Built as a ground-up Rust rewrite after a [code review](#why-rust) of `atch`
surfaced concrete memory-safety and throughput bugs; this project exists to
fix those structurally, with real test coverage and benchmark evidence
instead of assertions.

```sh
hytch start work -- claude    # launch something long-running, detached
hytch work                    # attach — or reattach after any disconnect
# ^\ to detach; the session and program keep running
```

## Status

Functional and end-to-end tested (89 tests, `cargo test --workspace`),
manually verified against the actual target scenario — start a detached
session, disconnect, reconnect, see full history. Not yet released as a
downloadable binary (no tag cut yet — build from source below). See
[Known gaps](#known-gaps) for what's deliberately not built yet.

## Install

**From source** (works today):

```sh
git clone https://github.com/artisenalcode/hytch
cd hytch
cargo build --release
sudo cp target/release/hytch /usr/local/bin/
```

**From a release** (once a version is tagged — the release workflow builds
static musl binaries for x86_64 and aarch64 Linux):

```sh
arch=$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')
curl -Lo hytch.tgz https://github.com/artisenalcode/hytch/releases/latest/download/hytch-linux-${arch}.tgz
tar -xzf hytch.tgz hytch
sudo mv hytch /usr/local/bin/
```

## Usage

| Command | What it does |
|---|---|
| `hytch [<session> [cmd...]]` | Attach to a session, creating it if it doesn't exist. |
| `hytch attach <session>` (`a`) | Strict attach — fails if the session isn't running. |
| `hytch new <session> [cmd...]` (`n`) | Create a session and attach to it. |
| `hytch start <session> [cmd...]` (`s`) | Create a session, detached — no terminal needed. |
| `hytch push <session>` (`p`) | Copy stdin verbatim into a running session. |
| `hytch kill [-f] <session>` (`k`) | Stop a session (SIGTERM, then SIGKILL after a grace period). |
| `hytch clear [<session>]` | Truncate a session's on-disk log. |
| `hytch list [-a]` (`l`/`ls`) | List sessions; `-a` also shows exited ones with a log on disk. |
| `hytch rm <session>` | Remove a stopped session's socket and log. |
| `hytch current` | Print the current session name (for shell prompts). |

Detach with `^\` (configurable via `-e`, or disable with `-E`). Full flag
reference: `hytch --help`.

## Architecture

```mermaid
flowchart LR
    subgraph Client processes
        C1["hytch attach<br/>(client 1)"]
        C2["hytch attach<br/>(client 2)"]
    end

    subgraph "hytch daemon (one per session)"
        Sock(["Unix socket"])
        Fanout["Fanout<br/>(ring buffer + broadcast)"]
        PtyLoop["pty command loop<br/>(single task owns the Pty)"]
        Log[("on-disk log<br/>own OS thread")]
    end

    Pty[/pty/] --> Child["child program<br/>(shell, claude, ...)"]

    C1 <-->|"framed control msgs<br/>(Push/Attach/Winch/Kill)"| Sock
    C2 <-->|"raw pty bytes back<br/>(unframed, untouched)"| Sock
    Sock --> PtyLoop
    PtyLoop --> Pty
    Pty -->|pty output| PtyLoop
    PtyLoop --> Fanout
    Fanout -.->|broadcast| C1
    Fanout -.->|broadcast| C2
    PtyLoop -.->|async, off the<br/>reactor thread| Log
```

The one asymmetry that matters: **client→daemon is framed, daemon→client is
not.** Control messages (keystrokes, resize, kill) are length-prefixed
frames; pty output flows back as a raw, unmodified byte stream. That's what
keeps mouse reporting, scroll sequences, and true-color passthrough intact
— there's no terminal emulator re-encoding anything in either direction.

Workspace layout:

- `crates/proto` — the wire framing above; no I/O, no OS calls.
- `crates/session` — session naming/directory/socket-path resolution.
- `crates/daemon` — the daemon itself: pty, ring buffer, on-disk log, fanout.
- `crates/client` — attach-side: raw terminal mode, the attach loop.
- `crates/cli` — the `hytch` binary: subcommand dispatch, daemon launch.

## Comparison

| | tmux / screen | atch (C) | hytch |
|---|---|---|---|
| Terminal emulation | Yes — re-encodes the stream | No | No |
| Mouse/scroll passthrough | Often breaks, needs config | Untouched | Untouched |
| History survives daemon exit/reboot | No (memory only) | Yes, on disk | Yes, on disk |
| Background start (no terminal needed) | Yes | Yes | Yes |
| Multiple panes/windows | Yes | No | No |
| Memory safety | N/A (mature C) | Manual (see [findings](#why-rust)) | Compiler-enforced |
| Language | C | C | Rust |

hytch and atch solve the same narrow problem (one program, one resumable
session, zero terminal re-encoding); tmux/screen solve a different, bigger
one (a full multiplexer). Pick hytch/atch when you want the narrow tool;
pick tmux/zellij when you actually want split panes and window management.

## Benchmarks

Real numbers, reproducible commands, methodology, and an honest negative
result (not just favorable ones) in [`BENCHMARKS.md`](BENCHMARKS.md).
Headline: a 1 MiB `push` completes in ~10–30ms versus atch's ~3.76s on the
same machine — **~150–300x** — directly attributable to fixing the 8-byte
push cap below. A small, realistic push is sub-10ms on both; the win is
specific to bulk transfer, not a universal claim.

## Why Rust

This wasn't a "rewrite it in Rust for fun" — it followed a code review of
`atch` that surfaced nine concrete findings, several of them real memory-
safety bugs, not style complaints. Each design choice below retires a
specific one, cited by number:

| Design choice | Retires |
|---|---|
| `tokio::net::UnixListener` + per-client task, no `select()`/`fd_set` | An unbounded `fd_set` with no `FD_SETSIZE` guard — real stack corruption risk under many concurrent clients or a raised fd ulimit. |
| `tokio::signal::unix` instead of raw `signal()` handlers | Signal handlers calling non-async-signal-safe functions (`printf`, `malloc`-adjacent) — reentrancy corruption if a signal lands mid-call. |
| Length-prefixed `proto` frames, not a `winsize`-union-aliased struct | An 8-byte cap on every push chunk — a 1 MiB paste needed ~131,000 syscalls instead of a small, bounded number. |
| Log rotation via `spawn_blocking`, off the reactor thread | Synchronous multi-megabyte log rotation running inline in the single-threaded event loop, stalling every attached client. |
| `copy_from_slice` in the ring buffer, never a per-byte loop | A per-byte copy loop on the hottest path in the daemon (every pty read). |
| `broadcast` channel with explicit `Lagged` handling | A slow client's unwritten tail silently dropped on `EAGAIN` — permanent, undetected data loss for that client. |
| `tokio_util::codec::Framed`, buffers partial reads automatically | `write()` returning `0`/partial writes not checked, and a `read()` assumed to always deliver one whole packet atomically. |
| Safe Rust allocation (aborts on OOM) | An unchecked `malloc()` result fed straight into `snprintf()` — null-pointer write under memory pressure. |

What this buys: the specific bug classes above can't recur by construction,
not "it's Rust so it's safe." `unsafe` isn't eliminated — pty allocation and
termios manipulation are inherently syscall-level regardless of language —
it's contained to a few small, deliberately narrow modules (`daemon::pty`,
`client::raw_mode`) instead of spread through 2,800 lines of C.

## Known gaps

Deliberate scope cuts, not oversights — tracked here instead of left
silent:

- Legacy single-letter flag mode (atch's `-a/-A/-c/...`) — existed only to
  not break scripts for an established tool; there's no installed base to
  preserve here.
- `run` (foreground non-daemonizing start), `tail`, `rm -a` sweep, and the
  `-r`/`-R`/`-t` options.
- Real `^Z` suspend orchestration — the client reports a suspend request
  (and tells the daemon) but nothing raises `SIGTSTP`/waits for `SIGCONT`
  yet.
- `list`'s `[running]` label reflects a successful connect, not true
  attached-vs-not (atch signals that via the socket's executable bit,
  which isn't wired up here).
- aarch64 releases are CI-built but not yet verified on real aarch64
  hardware (the dev machine this was built on is x86_64-only).

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo bench --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

CI (`.github/workflows/ci.yml`) runs fmt/clippy/build/test on every push.
The release workflow (`.github/workflows/release.yml`) builds static musl
binaries for `x86_64` and `aarch64` on a `v*.*.*` tag push.

## Licensing

MIT OR Apache-2.0, your choice — see `LICENSE-MIT` / `LICENSE-APACHE`. This
is an independent implementation written from `atch`/`dtach`'s documented
*behavior* (protocol shape, CLI surface, on-disk layout), not from their
GPL-licensed source. No code was copied.
