#!/usr/bin/env bash
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
files="
.agents/skills/spectra-archive/SKILL.md
.claude/skills/spectra-archive/SKILL.md
.opencode/skills/spectra-archive/SKILL.md
.opencode/commands/spectra-archive.md
"

test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
fake_bin="$test_root/bin"
mkdir -p "$fake_bin"
cat >"$fake_bin/spectra" <<'EOF'
#!/usr/bin/env bash
exit "${SPECTRA_GUARD_EXIT:-42}"
EOF
chmod +x "$fake_bin/spectra"

for relative in $files; do
    source_file="$repo_root/$relative"
    block_file="$test_root/archive-${relative//\//-}.sh"
    awk '
        /^[[:space:]]*# SPECTRA_SAFE_ARCHIVE_START$/ { inside = 1; found_start++; next }
        /^[[:space:]]*# SPECTRA_SAFE_ARCHIVE_END$/ { inside = 0; found_end++; next }
        inside { print }
        END {
            if (found_start != 1 || found_end != 1) {
                exit 3
            }
        }
    ' "$source_file" \
        | sed 's/change_name="<name>"/change_name="guard-fixture"/' \
        >"$block_file"

    case_root="$test_root/case-${relative//\//-}"
    mkdir -p "$case_root/.spectra/touched"
    tracking_file="$case_root/.spectra/touched/guard-fixture.json"
    printf '%s\n' '{"tasks":[]}' >"$tracking_file"

    if (
        cd "$case_root"
        PATH="$fake_bin:$PATH" SPECTRA_GUARD_EXIT=42 bash "$block_file" >/dev/null 2>&1
    ); then
        echo "$relative: failure fixture unexpectedly succeeded" >&2
        exit 1
    fi
    if [ ! -f "$tracking_file" ]; then
        echo "$relative: failed archive deleted its tracking file" >&2
        exit 1
    fi

    (
        cd "$case_root"
        PATH="$fake_bin:$PATH" SPECTRA_GUARD_EXIT=0 bash "$block_file" >/dev/null 2>&1
    )
    if [ -f "$tracking_file" ]; then
        echo "$relative: successful archive retained its tracking file" >&2
        exit 1
    fi
done

printf 'safe Spectra archive order verified in 4 generated instructions\n'
