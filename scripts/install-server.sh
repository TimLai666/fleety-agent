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

# Best-effort: also install the fleety-insyra data-analysis sidecar next to the
# server so the `insyra_exec` tool works out of the box. It ships as a raw
# per-target binary. Non-fatal if the asset isn't published yet — insyra_exec
# then returns an actionable error until it is.
sidecar_url="https://github.com/${REPO}/releases/latest/download/fleety-insyra-${target}"
if curl -fsSL "$sidecar_url" -o "$tmp/fleety-insyra" 2>/dev/null; then
  chmod 755 "$tmp/fleety-insyra"
  mv "$tmp/fleety-insyra" "$dir/fleety-insyra"
  echo "fleety-insyra: installed to $dir/fleety-insyra (data-analysis sidecar)"
else
  echo "fleety-insyra: sidecar asset not available yet; insyra_exec stays off until it is" >&2
fi

# Best-effort: install (or upgrade) the ddgs[mcp] Python package so the
# built-in `ddgs` MCP (web search: text/images/news/videos/books +
# extract_content) works the first time the server runs, AND so re-running
# this script after a fleety-server release refreshes the bundled MCP to
# latest. Tries pipx (cleanest, isolated venv) then `pip --user`. Non-fatal —
# the server logs an actionable warning at boot if it still can't find the
# binary, and the 24h background loop will keep retrying.
install_ddgs() {
  # Already installed? Upgrade to latest. Re-running install-server.sh after
  # a release should refresh the MCP, not no-op.
  if command -v ddgs >/dev/null 2>&1; then
    if command -v pipx >/dev/null 2>&1 && pipx upgrade ddgs >/dev/null 2>&1; then
      echo "ddgs: upgraded to latest via pipx ($(command -v ddgs))"
      return 0
    fi
    for py in python3 python; do
      if command -v "$py" >/dev/null 2>&1; then
        if "$py" -m pip install --user -U "ddgs[mcp]" >/dev/null 2>&1; then
          echo "ddgs: upgraded to latest via $py -m pip --user"
          return 0
        fi
      fi
    done
    echo "ddgs: already installed but upgrade failed; server's background loop will retry." >&2
    return 0
  fi
  # Fresh install path.
  if command -v pipx >/dev/null 2>&1; then
    if pipx install "ddgs[mcp]" >/dev/null 2>&1; then
      echo "ddgs: installed via pipx (built-in web-search MCP)"
      return 0
    fi
  fi
  for py in python3 python; do
    if command -v "$py" >/dev/null 2>&1; then
      if "$py" -m pip install --user -U "ddgs[mcp]" >/dev/null 2>&1; then
        echo "ddgs: installed via $py -m pip --user (built-in web-search MCP)"
        return 0
      fi
    fi
  done
  echo "ddgs: could not install automatically; the server will log a warning at boot." >&2
  echo "      Install manually: pip install -U 'ddgs[mcp]'   (or: pipx install 'ddgs[mcp]')" >&2
  return 1
}
install_ddgs || true

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
