#!/usr/bin/env bash
# Installs hytch from a GitHub release. Mirrors atch's own curl|tar|mv
# install UX. Safe to re-run to upgrade/reinstall.
#
#   curl -fsSL https://raw.githubusercontent.com/artisenalcode/hytch/main/install.sh | bash
#
# Installs to /usr/local/bin by default -- one canonical location on PATH
# for every shell (interactive, login, and the non-interactive shell an
# `ssh host "hytch ..."` remote command runs under), not just the ones that
# source a user rc file. A split between a user-writable dir and a
# root-owned one is exactly how a stale, unfixed binary hid in the other
# location during real use (a non-interactive SSH session resolved a
# different `hytch` than the interactive shell did) -- one location removes
# the ambiguity instead of trading it for a different one.
#
# Not writable by the invoking user (true for /usr/local/bin under a normal
# account)? Re-execs the copy step under `sudo`, once, the same way
# rustup's installer elevates only the final privileged step rather than
# requiring the whole pipeline to run as root.
#
# Env vars:
#   HYTCH_VERSION      release tag to install, e.g. "v0.1.0" (default: latest)
#   HYTCH_INSTALL_DIR  install directory (default: /usr/local/bin)

set -euo pipefail

repo="artisenalcode/hytch"
install_dir="${HYTCH_INSTALL_DIR:-/usr/local/bin}"
version="${HYTCH_VERSION:-latest}"

os="$(uname -s)"
if [ "$os" != "Linux" ]; then
  echo "hytch: only Linux is supported (got: $os)" >&2
  exit 1
fi

arch="$(uname -m)"
case "$arch" in
  x86_64) arch=amd64 ;;
  aarch64 | arm64) arch=arm64 ;;
  *)
    echo "hytch: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

if [ "$version" = "latest" ]; then
  url="https://github.com/${repo}/releases/latest/download/hytch-linux-${arch}.tgz"
else
  url="https://github.com/${repo}/releases/download/${version}/hytch-linux-${arch}.tgz"
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

echo "hytch: downloading ${url}"
if ! curl -fsSL -o "${tmpdir}/hytch.tgz" "$url"; then
  echo "hytch: download failed -- check the URL above, or try 'gh release download --repo ${repo}'." >&2
  exit 1
fi

tar -xzf "${tmpdir}/hytch.tgz" -C "$tmpdir" hytch

# Try as the invoking user first; a custom HYTCH_INSTALL_DIR is often
# user-writable and shouldn't need sudo just because the default location
# sometimes does. Only escalate on an actual permission failure.
if ! { mkdir -p "$install_dir" && install -m 755 "${tmpdir}/hytch" "${install_dir}/hytch"; } 2>/dev/null; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "hytch: ${install_dir} isn't writable and 'sudo' isn't available -- set HYTCH_INSTALL_DIR to a writable directory instead." >&2
    exit 1
  fi
  echo "hytch: ${install_dir} needs elevated privileges -- retrying with sudo"
  sudo mkdir -p "$install_dir"
  sudo install -m 755 "${tmpdir}/hytch" "${install_dir}/hytch"
fi

echo "hytch: installed to ${install_dir}/hytch"
"${install_dir}/hytch" --version

case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *)
    echo
    echo "hytch: ${install_dir} is not on your PATH. Add this to your shell rc (~/.bashrc, ~/.zshrc, ...):"
    echo "  export PATH=\"${install_dir}:\$PATH\""
    ;;
esac
