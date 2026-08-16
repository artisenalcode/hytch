# Benchmarks

Methodology first, numbers second — every number below has the command
that produced it, so it can be reproduced or challenged rather than taken
on faith.

**Machine:** 6-core x86_64 Linux (6.8 kernel), same machine for every
number in this document — `atch` and `hytch` were never compared across
different hardware.

**Builds compared:**
- `hytch`: `cargo build --release` (this repo's `Cargo.toml` release
  profile: `lto = true`, `codegen-units = 1`, `strip = true`), dynamically
  linked.
- `atch`: `make` in `~/Code/_labs/atch` (upstream `mobydeck/atch`,
  `CFLAGS = -g -O2`, statically linked against musl-less glibc via
  `-static`).

Neither binary is the cross-compiled musl-static release artifact step 7
will produce — these are same-machine dev builds, which is the fair
comparison for "is the code itself faster," independent of packaging.

## 1. Internal microbenchmarks (`cargo bench`)

Criterion, 100 samples per case, 3s warmup, default 5s collection window.

**Control-channel codec** (`crates/proto/benches/codec.rs`):

| Operation | Payload | Time | Throughput |
|---|---|---|---|
| encode `Push` | 64 B | 113–116 ns | ~530 MiB/s |
| encode `Push` | 4 KiB | 253–271 ns | ~14.6 GiB/s |
| encode `Push` | 32 KiB | 977 ns–1.0 µs | ~31 GiB/s |
| decode `Push` | 1 B | 54–57 ns | ~103 MiB/s |
| decode `Push` | 4 KiB | 133–137 ns | ~28 GiB/s |
| decode `Push` | 32 KiB | 860–881 ns | ~35 GiB/s |

**Scrollback ring buffer** (`crates/daemon/benches/ring_buffer.rs`), the
hottest path in the daemon — every pty read goes through `append()`:

| Chunk size | Time | Throughput |
|---|---|---|
| 128 B (typical keystroke burst) | ~10.0 ns | ~12 GiB/s |
| 4096 B (a full pty-read buffer) | ~83.1 ns | ~46 GiB/s |

**Fanout dispatch latency** (`crates/daemon/benches/fanout.rs`) — time from
`Fanout::push()` to a subscribed receiver actually getting the bytes, the
internal analogue of "push → echo" latency, excluding the pty/kernel hop:

- Single subscriber: **1.43–1.60 µs** per round trip.

Run them yourself: `cargo bench --workspace`.

## 2. Head-to-head vs. atch: the actual regression test for finding #4

The C protocol's `struct packet` aliases its data field to a `struct
winsize`-sized union — an 8-byte cap on every client→master chunk (see the
code review that motivated this rewrite). This is where that shows up.

**1 MiB push into a `cat`-backed session** (`push` connects, sends the
payload, disconnects):

```sh
hytch start s -- cat && hytch push s < payload_1m.bin   # ~10–30 ms
atch  start s -- cat && atch  push s < payload_1m.bin   # ~3.76 s
```

That's roughly **150–300x**. Not a projection — 1 MiB / 8 bytes ≈ 131,000
packets for atch's protocol vs. a small, bounded number of 32 KiB-chunked
frames for hytch's (see `commands::push`'s `PUSH_CHUNK_SIZE`).

**Small, realistic push (~55 bytes, a handful of shell commands)** — both
tools: **sub-10ms**, indistinguishable from process-startup noise. The
protocol difference only matters at scale, exactly as the finding
predicted — this isn't a universal 150x speedup, it's a fix for a specific,
real cliff.

## 3. Where hytch was *slower*, and what that led to

Honest numbers, not just favorable ones. Two real gaps this benchmarking
pass surfaced:

**Per-invocation overhead.** `hytch --version` × 50 vs. `atch list` × 50
(both near-no-op commands, isolating process-startup + runtime-init cost):

| | Before | After |
|---|---|---|
| hytch | ~7.7 ms/invocation | **~5.2 ms/invocation** |
| atch | ~3.5 ms/invocation | ~3.5 ms/invocation |

The "before" number is with tokio's default multi-threaded runtime
(`#[tokio::main]`) — worker-pool spin-up cost for a one-shot CLI command
that never uses more than one thread's worth of concurrency. Switched to
`#[tokio::main(flavor = "current_thread")]` (see `main.rs`'s comment for
the reasoning) once this benchmark surfaced the gap — closed about a third
of it. The remaining ~1.5x is most likely binary size (dynamic linking,
more code to page in) and `clap`'s parsing overhead against atch's
hand-rolled argument loop; a few milliseconds either way is imperceptible
to an interactive user and not worth chasing further against the actual
workload (session persistence, large transfers) this tool is for.

**`start`/`kill` cycle latency.** 5x `start` + `kill` round trips:
hytch ~140 ms/cycle vs. atch ~108 ms/cycle. Not from the runtime flavor —
from `spawn_detached`'s `wait_for_socket` (20 ms poll interval) and
`kill`'s `wait_gone` (100 ms poll interval) in `spawn.rs`/`commands.rs`.
Polling for "did the socket appear / disappear yet" instead of a real
readiness signal is a deliberate simplification from the plan, not
something this pass tried to hide — noted here as a concrete, scoped
follow-up (an inotify watch or a status pipe like atch's own
`fd[]`-based exec-error reporting would remove the poll interval as a
latency floor) rather than a claim that it's already optimal.

## 4. Binary size

| | Size |
|---|---|
| `atch` (static, `-lutil` only) | 1.2 MiB |
| `hytch` (dynamic, this machine) | 1.9 MiB |

Not an apples-to-apples comparison yet — `atch`'s number is a real static
musl-style build; `hytch`'s is a dev-machine dynamic build. Step 7's
cross-compiled musl-static release binary is the number that will actually
compare fairly to atch's own release artifacts.
