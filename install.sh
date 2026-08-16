#!/usr/bin/env bash
# Installs hytch from a GitHub release. Mirrors atch's own curl|tar|mv
# install UX. Safe to re-run to upgrade/reinstall.
#
#   curl -fsSL https://raw.githubusercontent.com/artisenalcode/hytch/main/install.sh | bash
#
# Env vars:
#   HYTCH_VERSION      release tag to install, e.g. "v0.1.0" (default: latest)
#   HYTCH_INSTALL_DIR  install directory (default: $HOME/.local/bin)

set -euo pipefail

repo="artisenalcode/hytch"
install_dir="${HYTCH_INSTALL_DIR:-$HOME/.local/bin}"
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

mkdir -p "$install_dir"
install -m 755 "${tmpdir}/hytch" "${install_dir}/hytch"

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
