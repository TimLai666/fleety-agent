## Context

Zed accepts JSONC settings, including comments and trailing commas. Fleety's ACP refresh path intentionally uses strict JSON parsing so it never rewrites a file it cannot safely preserve. The refresh path currently parses the entire file before checking whether `agent_servers.Fleety` exists, so an unrelated JSONC file can make `fleety update` fail even when Fleety is not installed in Zed.

## Goals / Non-Goals

**Goals:**

- Make an absent `agent_servers.Fleety` entry a no-op even when unrelated Zed settings use JSONC syntax.
- Preserve the fail-closed behavior for a file that appears to contain an installed Fleety entry but cannot be safely parsed or validated.
- Avoid changing or rewriting Zed settings during the no-op path.

**Non-Goals:**

- Add a JSONC parser or make Fleety rewrite JSONC settings.
- Change `fleety acp install zed` behavior for settings that require manual JSONC editing.
- Change endpoint validation, token preservation, or atomic publication rules for valid Fleety entries.

## Decisions

### Detect an installed Fleety entry before strict parsing

Use a conservative raw-text preflight for the exact JSON key token `"Fleety"`. When that token is absent, return `Ok(None)` before invoking the strict JSON parser. This makes unrelated JSONC settings a no-op while preserving the existing safe behavior whenever the file appears to contain a Fleety entry.

Alternative rejected: introduce a JSONC dependency and parse every settings file. The refresh path does not need to understand or rewrite unrelated JSONC, and a new parser would expand the dependency and compatibility surface for a no-op decision.

### Keep the installed-entry path fail closed

When the conservative preflight finds `"Fleety"`, continue through strict JSON parsing, object-shape checks, endpoint validation, and atomic replacement exactly as before. An unparseable or malformed installed entry remains an update error and its file remains unchanged.

## Implementation Contract

**Behavior**

- `refresh_zed_settings` SHALL return `Ok(None)` for a settings string that contains comments or trailing commas but no literal `"Fleety"` key.
- `refresh_zed_settings` SHALL retain its current refresh result for valid plain-JSON settings containing `agent_servers.Fleety`.
- `refresh_zed_settings` SHALL return an error, without producing replacement output, when a settings string appears to contain `"Fleety"` but is not safely parseable or has an invalid installed entry.

**Interface / data shape**

- No public function signature, settings schema, command name, or release format changes.
- The no-op result remains `Ok(None)` and the successful refresh result remains `Ok(Some(pretty_json))`.

**Failure modes**

- Unrelated JSONC syntax SHALL NOT make `fleety update` incomplete when no Fleety entry is present.
- JSONC or malformed content that appears to contain a Fleety entry SHALL continue to fail closed with the existing incomplete-update error path.
- The existing settings file SHALL NOT be written in either no-op or parse-failure cases.

**Acceptance criteria**

- A unit test proves JSONC without `agent_servers.Fleety` returns `Ok(None)`.
- A unit test proves a JSONC or malformed settings value containing a Fleety entry still returns an error.
- Existing valid-entry refresh, endpoint validation, token preservation, and atomic publication tests remain green.
- `cargo fmt --all -- --check`, focused ACP tests, CLI tests, and `cargo clippy -p fleety-cli --all-targets -- -D warnings` pass.

**Scope boundaries**

- In scope: the ACP refresh decision and its regression tests, plus the matching `acp-adapter` requirement.
- Out of scope: JSONC rewriting, Zed installation flow, other editors, and unrelated update components.

## Risks / Trade-offs

- [A comment or unrelated string contains the literal `"Fleety"`] → The conservative preflight may still attempt strict parsing and report an incomplete update; this is safer than silently rewriting a file that may contain an installed entry.
- [A real Fleety key is represented with an unusual escaped spelling] → The refresh may conservatively skip it; the existing manual install path and explicit parse failure remain available, and no settings are overwritten.

## Migration Plan

No data migration is required. Updating Fleety changes only the decision made when no installed Fleety entry is present. Existing valid Fleety settings continue through the same safe refresh path.

## Open Questions

None.
