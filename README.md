# hytch

A fast, lean terminal session daemon: attach, detach, and resume exactly
where you left off — across disconnects, crashes, and reboots — with no
terminal emulation in the way, so mouse reporting, scroll, and true-color
pass through untouched.

Design goals, protocol, and architecture rationale live in the project plan:
`~/Code/_labs/docs/ideation/vps-remote-sessions/2026-08-16-rust-rewrite-plan.md`
(to be folded into `docs/` here once the workspace stabilizes).

Independent reimplementation inspired by `atch`/`dtach`'s design goals — not
a fork, no shared code. See the plan doc for the licensing rationale.

Status: early scaffold, not yet functional. See the plan doc's Steps section
for build order.

## Workspace layout

- `crates/proto` — wire framing for the client↔daemon control channel.
- `crates/session` — session naming/directory/socket-path resolution.
- `crates/daemon` — the session daemon: pty, ring buffer, log, client fanout.
- `crates/client` — attach-side: raw terminal mode, attach loop.
- `crates/cli` — the `hytch` binary: subcommand dispatch.

## Building

```sh
cargo build --workspace
cargo test --workspace
```
