#!/bin/sh
# Fleety CLI installer — downloads the latest release of `fleety` for this
# platform and installs it onto your PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install.sh | sh
#
# Override the install dir with FLEETY_INSTALL_DIR=/some/bin.
set -eu

REPO="TimLai666/fleety-agent"
BIN="fleety"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
      *) echo "fleety: unsupported Linux arch '$arch' (only x86_64 has prebuilt binaries)" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "fleety: unsupported macOS arch '$arch'" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "fleety: unsupported OS '$os'. On Windows use scripts/install.ps1." >&2
    exit 1
    ;;
esac

asset="${BIN}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"

# Pick an install dir: explicit override, else /usr/local/bin if writable, else ~/.local/bin.
if [ -n "${FLEETY_INSTALL_DIR:-}" ]; then
  dir="$FLEETY_INSTALL_DIR"
elif [ -w /usr/local/bin ]; then
  dir="/usr/local/bin"
else
  dir="$HOME/.local/bin"
fi
mkdir -p "$dir"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "fleety: downloading $asset ..."
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
  echo "fleety: download failed from $url" >&2
  echo "        (has a release been published yet? see github.com/${REPO}/releases)" >&2
  exit 1
fi

tar -C "$tmp" -xzf "$tmp/$asset"
chmod 755 "$tmp/$BIN"
mv "$tmp/$BIN" "$dir/$BIN"

echo "fleety: installed to $dir/$BIN"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "fleety: add $dir to your PATH to run 'fleety' directly" ;;
esac
