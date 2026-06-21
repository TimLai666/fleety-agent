#!/bin/sh
# Fleety server installer — downloads the latest `fleety-server` for this host
# and installs it onto your PATH (non-Docker deployment).
#
#   curl -fsSL https://raw.githubusercontent.com/TimLai666/fleety-agent/main/scripts/install-server.sh | sh
#
# Override the install dir with FLEETY_INSTALL_DIR=/some/bin.
set -eu

REPO="TimLai666/fleety-agent"
BIN="fleety-server"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
      *) echo "$BIN: unsupported Linux arch '$arch' (only x86_64 has prebuilt binaries)" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "$BIN: unsupported macOS arch '$arch'" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "$BIN: unsupported OS '$os'. Use Docker (docker compose up -d --build) instead." >&2
    exit 1
    ;;
esac

asset="${BIN}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"

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

echo "$BIN: downloading $asset ..."
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
  echo "$BIN: download failed from $url" >&2
  echo "        (has a release been published yet? see github.com/${REPO}/releases)" >&2
  exit 1
fi

tar -C "$tmp" -xzf "$tmp/$asset"
chmod 755 "$tmp/$BIN"
mv "$tmp/$BIN" "$dir/$BIN"

echo "$BIN: installed to $dir/$BIN"
echo
echo "Run it (listens on FLEETY_ADDR, default 127.0.0.1:8787 — set 0.0.0.0:8787 to expose):"
echo "  FLEETY_ADDR=0.0.0.0:8787 $BIN"
echo
echo "Autostart with systemd (Linux), as the current user:"
echo "  mkdir -p ~/.config/systemd/user"
echo "  cat > ~/.config/systemd/user/fleety-server.service <<EOF"
echo "  [Unit]"
echo "  Description=Fleety Agent server"
echo "  [Service]"
echo "  ExecStart=$dir/$BIN"
echo "  Environment=FLEETY_ADDR=0.0.0.0:8787"
echo "  Restart=on-failure"
echo "  [Install]"
echo "  WantedBy=default.target"
echo "  EOF"
echo "  systemctl --user daemon-reload && systemctl --user enable --now fleety-server"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo; echo "$BIN: add $dir to your PATH" ;;
esac
