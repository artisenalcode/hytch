//! Captures the short git commit hash at build time so `hytch --version`
//! can report exactly which commit a binary was built from -- useful for
//! a downloaded release binary where the only other signal is the tag.
//! Falls back to "unknown" rather than failing the build when `.git`
//! isn't available (e.g. building from a source tarball with no git
//! history), so this is never a hard build dependency on git.

use std::process::Command;

fn main() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=HYTCH_GIT_HASH={hash}");
    // Re-run only when HEAD actually moves, not on every build.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}
