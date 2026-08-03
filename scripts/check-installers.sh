#!/bin/sh
# Contract checks for the public client installers. The Unix flow also runs
# against local mock archives so it proves the bootstrap order without network
# access or service-manager side effects.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
failures=0

require_text() {
  file="$1"
  pattern="$2"
  if ! grep -F -- "$pattern" "$root/$file" >/dev/null 2>&1; then
    echo "installer check: $file is missing: $pattern" >&2
    failures=$((failures + 1))
  fi
}

require_not_text() {
  file="$1"
  pattern="$2"
  if grep -F -- "$pattern" "$root/$file" >/dev/null 2>&1; then
    echo "installer check: $file must not contain: $pattern" >&2
    failures=$((failures + 1))
  fi
}

require_text scripts/install.sh 'asset="${bin}-${target}.tar.gz"'
require_text scripts/install.sh 'download_binary fleetyd'
require_text scripts/install.sh '"$dir/fleetyd" install'
require_text scripts/install.sh '"$dir/fleetyd" start'
require_not_text scripts/install.sh '"$dir/fleetyd" enable'

require_text scripts/install.ps1 '$asset = "$bin-$target.zip"'
require_text scripts/install.ps1 "Download-Asset 'fleetyd'"
require_text scripts/install.ps1 '& $fleetyd install'
require_text scripts/install.ps1 '& $fleetyd start'
require_not_text scripts/install.ps1 '& $fleetyd enable'

if [ "$failures" -ne 0 ]; then
  echo "installer check: $failures contract check(s) failed" >&2
  exit 1
fi

mock_root=$(mktemp -d)
trap 'rm -rf "$mock_root"' EXIT
fixture_dir="$mock_root/fixtures"
fake_bin="$mock_root/bin"
install_dir="$mock_root/install"
mkdir -p "$fixture_dir" "$fake_bin" "$install_dir"

case "$(uname -s):$(uname -m)" in
  Linux:x86_64|Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
  Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
  Darwin:x86_64) target="x86_64-apple-darwin" ;;
  *)
    echo "installer check: mocked Unix install is unsupported on $(uname -s)/$(uname -m)" >&2
    exit 1
    ;;
esac

printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "$*" >> "$FLEETY_DAEMON_LOG"' > "$fixture_dir/fleetyd"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$fixture_dir/fleety"
chmod 755 "$fixture_dir/fleetyd" "$fixture_dir/fleety"
for bin in fleety fleetyd; do
  (cd "$fixture_dir" && tar -czf "$fixture_dir/$bin-$target.tar.gz" "$bin")
done

printf '%s\n' \
  '#!/bin/sh' \
  'set -eu' \
  'output=' \
  'url=' \
  'while [ "$#" -gt 0 ]; do' \
  '  case "$1" in' \
  '    -o) output="$2"; shift 2 ;;' \
  '    -*) shift ;;' \
  '    *) url="$1"; shift ;;' \
  '  esac' \
  'done' \
  'asset=${url##*/}' \
  'cp "$FLEETY_TEST_FIXTURES/$asset" "$output"' > "$fake_bin/curl"
chmod 755 "$fake_bin/curl"

FLEETY_DAEMON_LOG="$mock_root/daemon.log" \
FLEETY_TEST_FIXTURES="$fixture_dir" \
FLEETY_INSTALL_DIR="$install_dir" \
PATH="$fake_bin:$PATH" \
  sh "$root/scripts/install.sh" >/dev/null

test -x "$install_dir/fleety"
test -x "$install_dir/fleetyd"
grep -Fx 'install' "$mock_root/daemon.log" >/dev/null
grep -Fx 'start' "$mock_root/daemon.log" >/dev/null
if grep -Fx 'enable' "$mock_root/daemon.log" >/dev/null 2>&1; then
  echo "installer check: Unix client installer unexpectedly enabled autostart" >&2
  exit 1
fi

echo "installer check: client fleetyd bootstrap contract and Unix mock pass"
