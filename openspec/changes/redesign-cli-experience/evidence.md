# Implementation evidence

## 2026-07-15 — parser baseline and dependency gate

### RED baseline

- `cargo test -p fleety-cli --test cli_smoke every_command_node_supports_generated_help_without_side_effects -- --nocapture`
- Result: failed as intended before parser migration.
- First observed divergence: `fleety init --help` was parsed as the WebSocket URL, exited 2, and printed `error: '--help' is not a ws:// or wss:// URL` instead of side-effect-free help on stdout.

### Windows clean release baseline

Both builds used separate empty `CARGO_TARGET_DIR` directories and built `fleety-cli`, `fleety-daemon`, and `fleety-server` together.

| Measurement | Before dependencies | After declaring dependencies | Delta |
| --- | ---: | ---: | ---: |
| Cold build wall time | 310,434 ms | 348,751 ms | +38,317 ms (+12.3%) |
| `fleety.exe` | 15,390,208 bytes | 15,390,208 bytes | 0 |
| `fleetyd.exe` | 15,467,520 bytes | 15,467,520 bytes | 0 |
| `fleety-server.exe` | 59,173,376 bytes | 59,173,376 bytes | 0 |

The size delta is zero because no command code referenced clap yet, so the linker excluded it. The cold-build delta includes first-time dependency compilation and is not an incremental-build measurement.

### Dependency and MSRV result

- A loose `clap = "4.5"` resolved to `clap 4.6.1`, whose package manifest requires Rust 1.85. This violates the workspace's Rust 1.80 contract and was rejected.
- Exact pins: `clap 4.5.4` and `clap_complete 4.5.2`; both `cargo info` records declare `rust-version: 1.74`.
- The dependency tree adds five packages: `clap`, `clap_builder`, `clap_derive`, `clap_lex`, and `clap_complete`.
- A real Rust 1.80 build is not yet proven. Two rustup installation attempts were interrupted by stalled component downloads; the partial toolchain was removed afterward. Task 1.2 remains incomplete until the full three-binary check succeeds.

## 2026-07-15 — independent review round 1

Read-only agent review of task 2.1 found no Critical findings, two High findings, and one Medium finding.

- High: canonical command nodes were absent from the exhaustive help inventory and still reached initialization or usage errors. Canonical nodes were added to the task 2.2 RED matrix; the typed pre-initialization parser is the active fix.
- High: nested `config provider login|logout|status` mapped to config execution instead of the canonical Provider Auth value. The normalizer now maps all three actions, including legacy `--target server`, to the same typed value; parser table tests pass.
- Medium: `--owner` is recognized by the temporary alias normalizer but not yet consumed by execution. It fails closed and does not fall back to local persistence. Task 2.3 owns the complete typed `--owner`/legacy `--target` context parser and conflict checks.

## 2026-07-15 — independent review round 2

Read-only agent review of the three-binary clap migration found no Critical findings and three High findings.

- `fleetyd config help` was swallowed by opaque passthrough and could migrate legacy files before rendering help.
- `fleetyd` and `fleety-server` silently accepted trailing config arguments such as `config list unexpected`.
- canonical and nested provider/model commands did not enforce required `--type` and `--member` values before owner I/O.

All three came from duplicating or omitting config grammar. The fix moved one strict generated config subtree to `fleety_tools::config`, with a CLI variant for provider OAuth compatibility aliases. Regression tests now prove `config help` byte identity, trailing-argument exit 2, required provider/model arguments before owner I/O, and identical nested OAuth typed values.

## 2026-07-15 — independent review round 3

Read-only review found no Critical findings, one High finding, and one Medium finding.

- High: generated config usage permits options before positional provider/model identifiers, while the execution parser required positional-first order. Provider and model parsing is now order-independent, with typed equality tests and a real Server options-first smoke test.
- Medium: Server help advertised the CLI-only interactive `config provider edit` action. Shared grammar now exposes that action only in the CLI variant.

The same pass also hardened direct owner boundaries: Daemon grammar excludes provider/model, Daemon execution uses `DAEMON_SCOPES`, Server execution uses `SERVER_SCOPES`, and scoped provider/model execution rejects any non-Server owner before persistence.

## 2026-07-15 — independent review round 4

Read-only review found zero Critical and zero High findings. It confirmed the options-first parser fix, CLI-only provider editor exposure, execution-layer owner scope checks, and Daemon/Server grammar separation. This is the first consecutive clean evaluation round; change-level completion still requires a second consecutive clean round.

## 2026-07-15 — independent review round 5

Read-only review of invocation context and owner routing found no Critical findings, one High finding, and one Medium finding.

- High: clap accepted `fleety ask -- --server literal`, but the execution parser rejected the preserved `--` as an unknown flag after initialization. The parser now consumes the option terminator and treats every following token as prompt text; a fake Server smoke test proves the exact `--server literal` text reaches `UserMessage` and exits successfully.
- Medium: `init` and `pair` learned the Server fingerprint from `Welcome` but omitted it from the successful result. Both enrollment paths now render the shared context again after the successful handshake, including the known identity; the enrollment smoke fixture verifies both results.

The same task pass added a true pre-I/O owner guard: an explicit config owner/key mismatch now fails before legacy migration. A sentinel `config.json` smoke test proves it is neither renamed nor converted to `connections.toml`.

## 2026-07-15 — independent review round 6

The fix review found zero Critical, zero High, and one Medium finding. It verified the ask option-terminator payload, enrollment identity output, owner preflight, and alias payload fixes. The remaining Medium showed that untrusted fingerprint text could inject terminal control characters through the context renderer.

All context fields now pass through one terminal-safe boundary: C0/C1 controls are escaped, ordinary Unicode remains intact, and tokens are never read by the renderer. The regression test covers CR, LF, tab, ESC, CJK, and a sentinel token.

## 2026-07-15 — independent review round 7

Read-only fix review found zero Critical, High, Medium, or Low findings. It ran the terminal-control regression plus enrollment, ask terminator, relayed Daemon owner, and diff checks. This is the first clean round after the latest implementation change; final change-level completion still requires another consecutive clean review after later tasks stop changing the CLI.

## 2026-07-15 — machine output and multi-owner configuration

Task 2.4 introduced one CLI output sink and a stable envelope with `schema_version`, `ok`, `context`, `data`, and `errors`. Status uses semantic fields; other non-interactive commands are captured into one generic result without mixed stdout. JSON usage/runtime/success exit classes, pre-Welcome context, token redaction, quiet/no-color behavior, and opt-in compatibility warnings have smoke coverage.

Task 3.1 changed `config list` from fail-fast to an owner-by-owner partial read. Daemon failure no longer skips Server: human output marks `PARTIAL`, JSON keeps CLI/Server data plus the Daemon error/remediation, both exit 1, and non-owner config/provider files remain byte-for-byte unchanged.

## 2026-07-15 — independent review round 8

Read-only review of tasks 2.4 and 3.1 found zero Critical, zero High, one Medium, and one Low finding.

- Medium: `connection list --quiet` still included the `FLEETY_AGENT_URL` context note because the domain renderer embedded it in the result string.
- Low: semantic JSON success paths could omit an explicitly requested compatibility warning because they flushed before draining captured diagnostics.

The connection domain result now excludes environment context; the command layer emits that note only for non-quiet human output. Every semantic envelope now drains and merges captured diagnostics without replacing semantic fields. Local config JSON also uses the rendered API so the external tools crate cannot bypass the CLI sink.

## 2026-07-15 — independent review round 9

Read-only fix review found zero Critical, High, Medium, or Low findings. It verified quiet connection output, semantic JSON warning preservation, local config ownership, canonical/alias equivalence, generic ask JSON, double-drain behavior, and absence of qualified output writes outside the sink/final envelope. This is the first clean round after the latest output-policy changes.

## 2026-07-15 — diagnosis and completion

Task 2.5 added `fleety doctor` and `fleety completion <bash|zsh|fish|powershell|elvish>` as pre-initialization commands.

- Completion is generated from the typed clap tree, writes only shell source to stdout, accepts the standard `--` option terminator, and does not seed, migrate, create, or modify user files. JSON wrapping is rejected with usage exit 2 because stdout belongs to the shell source.
- Doctor reports CLI, Profile, Server identity/version, config protocol, Providers, OAuth, active model, Daemon installation, and Daemon connection as PASS/WARN/FAIL with concrete remediation. WARN-only partial state exits 0; any FAIL exits 1; JSON uses the common envelope.
- Doctor runs before config seeding and migration. Its remote path has a five-second deadline and deliberately skips TOFU pinning and version convergence. The Windows `tasklist` probe has its own one-second child deadline and kills/reaps a stuck child.
- Endpoint display removes userinfo and fragments, preserves Unicode path and query keys, redacts every query value, rejects invalid raw overrides before I/O, escapes terminal controls, and redacts URLs embedded in transport errors.
- Fake healthy, partial, and offline Server/Daemon environments cover protocol frames, exit classes, JSON, remediation, and temp-home byte identity. Completion covers all five shells and PowerShell source validity.

Verification after the final fixes:

- `cargo test -p fleety-cli -p fleety-tools`: 175 CLI unit tests, 52 CLI smoke tests, 185 tools unit tests, 3 tools integration tests, and doc tests passed.
- `cargo clippy -p fleety-cli -p fleety-tools --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed; only the checkout's existing LF-to-CRLF notices were emitted.

## 2026-07-15 — independent review round 10

Initial task 2.5 review found one High and three Medium findings.

- High: URL userinfo/query credentials leaked through Doctor context, checks, and transport errors.
- Medium: human Doctor detail allowed terminal control injection.
- Medium: clap accepted `completion -- bash`, but dispatch rejected it.
- Medium: the Windows Daemon PID probe used an unbounded synchronous `tasklist` child outside the remote timeout.

All four were fixed with shared endpoint redaction, terminal-safe Doctor rendering, option-terminator normalization, and a bounded child wait/kill/reap helper plus Windows regression test.

## 2026-07-15 — independent review round 11

Fix review confirmed the four round-10 findings, then found one Medium and one Low issue.

- Medium: an invalid raw endpoint could bypass parse-based secret redaction.
- Low: URL reserialization percent-encoded a readable Unicode path and removed all query diagnostics.

Raw URL overrides now require a valid `ws://` or `wss://` URL before initialization. Display reconstruction preserves the original Unicode path and query keys while replacing every query value with `<redacted>`.

## 2026-07-15 — independent review round 12

Review confirmed the invalid-endpoint and Unicode/query fixes, then found one High embedded-URL scanner bypass: `]` in IPv6 authority and `)` in a legal path were incorrectly treated as URL terminators, allowing SSE transport error URLs to retain credentials.

The scanner now stops only at actual whitespace, quotes, or angle brackets. Parse failures become `<invalid endpoint>` and never fall back to the candidate text. IPv6 and parenthesized-path Doctor smoke tests cover context, checks, errors, and both output modes.

## 2026-07-15 — independent review round 13

Final focused review found zero Critical, High, Medium, or Low findings. It reran both parenthesized-path and IPv6 SSE reproductions in human and JSON modes, confirmed all sentinel credentials were absent from context/checks/errors, and verified parse-fail redaction plus monotonic scanner progress.

## 2026-07-15 — Provider, authentication, and model workflow

Task 3.2 introduced one pure Provider application service for command and terminal surfaces. Provider mutations, role mutations, validation, auth state, catalog state, retry identity, manual recovery, and rendered Provider views now share typed values. CLI reads and mutations use Server `ConfigSnapshot` and `ConfigApply`; the interactive editor stages in memory and never writes `providers.toml` directly.

The canonical `fleety model catalog <provider> [--role main|cheap]` command and its `config --target server model catalog` compatibility alias now perform the same typed request. OAuth catalog access first checks the Server-owned credential state, then requests models on the selected Server. Human, quiet, and JSON output are covered. A missing or expired login blocks the catalog request and returns the canonical login remediation.

Codex catalog requests use the hosted Codex backend path plus the current Codex-compatible `originator`, `version`, account, and user-agent headers. Backend detail is retained for diagnosis, while the exact bearer and account ID are removed before any wire, human, JSON, or TUI output. API Provider errors remain endpoint-blind.

The terminal model picker exposes Loading, Available, Failed, Unavailable, retry, login, details, and manual-ID recovery. Catalog work runs outside the draw/input loop, so Loading and the previous error are visible and Esc remains usable. Every reconnect uses the immutable target resolved when the editor opened. TUI save now uses the same `apply_snapshot` adapter and wire error mapping as commands.

Verification after the final local fixes:

- `cargo test -p fleety-cli --bin fleety --no-fail-fast`: 183 passed.
- `cargo test -p fleety-cli --test cli_smoke --no-fail-fast`: 54 passed.
- `cargo test -p fleety-tools --lib --no-fail-fast`: 190 passed.
- `cargo test -p fleety-server --no-fail-fast`: 325 unit and 9 smoke tests passed.
- `cargo clippy -p fleety-cli -p fleety-tools -p fleety-server --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed with only checkout LF-to-CRLF notices.

## 2026-07-15 — independent review round 14

The first task 3.2 review found zero Critical, zero High, three Medium, and one Low finding.

- Medium: a Codex backend could reflect the bearer or account ID in `error.message`. Exact credential values are now redacted before output truncation; the regression body contains both sentinels.
- Medium: model discovery blocked the terminal event loop, so Loading was not observable. Fetch now runs in a background worker against the fixed original target; a headless render proves the previous error and Esc action remain visible.
- Medium: the config-nested compatibility group omitted `model catalog`. The CLI-only config grammar and normalizer now map it to the same typed value and fake-Server payload as the canonical command.
- Low: TUI save duplicated `ConfigApply` reply mapping. It now calls the shared `apply_snapshot` adapter and only maps typed saved/conflict UI outcomes.

## 2026-07-15 — independent review round 15

The first fix review confirmed all round-14 corrections, then found zero Critical, zero High, two Medium, and zero Low findings in the new background-fetch path.

- Medium: Esc removed Loading from the screen but did not stop the worker or transport, and the shared `FnMut` mutex could block a second request behind the cancelled request. Each active fetch now owns a cancellation signal. Leaving its exact Fetching state signals cancellation and frees the slot; the async worker selects cancellation against connect/fetch, which drops the transport. Fetch callbacks are concurrent `Fn + Sync` values without a serial mutex.
- Medium: reconnect fixed the URL and token but did not compare Server identity. The editor now retains the initial Welcome fingerprint and requires every catalog reconnect to present the same identity before sending `ProviderModelList`; missing or changed identity fails with a typed remediation.

New tests prove cancellation signalling releases the active slot, the async cancellation waiter completes, and equal, changed, missing-original, and missing-reconnect identity cases fail closed as intended.

## 2026-07-15 — independent review round 16

The cancellation and catalog-identity fix review confirmed both round-15 findings, then found zero Critical, zero High, one Medium, and zero Low finding in adjacent editor reconnect paths.

- Medium: OAuth login/logout/switch and conflict/action snapshot reloads reused the immutable URL/token but did not compare the reconnect fingerprint with the editor's initial Server identity. One shared identity validator now guards every editor-owned reconnect before snapshot, Provider lookup, browser authorization, credential mutation, catalog request, or later config apply. Original missing, reconnect missing, and changed identity all fail closed with typed remediation.

The same identity matrix is exercised from both catalog/reconnect and OAuth action tests, and CLI all-target check plus warnings-denied Clippy pass.

## 2026-07-17 — pure terminal workspace state and entry boundary

Task 4.1 added the pure workspace domain layer: Route and Settings page navigation, ConnectionState with reconnect attempt/backoff, shared context, per-owner Loading/Available/Dirty/Applying/Conflict/Failed/Unavailable state, persistent notices, actions, and emitted effects. Reducer tests prove navigation/back behavior, owner apply isolation, staged-state retention on conflict/failure, unresolved error persistence across transient status and help navigation, and deterministic notice IDs owned by the state rather than global mutable data.

Bare invocation now selects help or Chat solely from stdin/stdout terminal detection. Non-terminal bare invocation still returns generated help before initialization. Bare TTY, canonical `chat`, legacy `tui`, and bare TTY `config` all cross the same workspace entry boundary at Chat or Settings. The existing Chat and Settings loops remain temporary route adapters; task 4.2 owns replacing them with the shared renderer/event loop and persistent header/footer.

Verification:

- RED: workspace tests failed on every missing domain type and reducer action before implementation.
- `cargo test -p fleety-cli --bin fleety workspace::tests --no-fail-fast`: 6 passed.
- `cargo test -p fleety-cli --test cli_smoke no_args_prints_top_level_help --no-fail-fast`: passed and proves captured/non-TTY execution does not connect or consume input.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed.

## 2026-07-17 — shared workspace shell, keys, notices, and command palette

Task 4.2 added the shared shell renderer around the live Chat route. Its persistent header names profile, connection state including reconnect attempt/backoff, active provider/model, and route. The shell owns an unresolved-notice region, route content region, and contextual footer. Chat keeps its existing transport, conversation, input, attachment, approval, cancellation, and reconnect behavior inside the content region.

Global key handling now has one state matrix: Esc closes the current modal/route before reaching route-local behavior; `?` opens contextual help when a text composition is not active; Ctrl+K opens a searchable command palette; Ctrl+C emits turn cancellation first, otherwise requests confirmation for unsent/dirty state or exits. The palette filters and executes route/effect commands, and Esc restores the previous route. Notice retry/dismiss use Alt+R and Alt+D so ordinary Chat text is not intercepted.

Persistent unresolved notices take display priority over later transient statuses. Their summary, details, and remediation remain visible across help/palette navigation until retry or dismissal. Server errors and authentication/reconnect states feed the workspace state rather than only replacing the Chat status line.

Verification:

- RED: shell/key tests failed before `render`, `on_key`, KeyContext, KeyOutcome, and cancellation effects existed.
- `cargo test -p fleety-cli --bin fleety workspace::tests --no-fail-fast`: 10 state/render tests passed.
- `cargo test -p fleety-cli --bin fleety --no-fail-fast`: 197 passed after live Chat shell integration.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-17 — independent review round: 3 High, 1 Medium

The fresh read-only review was not a clean Critical/High round. It found no Critical issue, three High owner/input/presentation failures, and one Medium parser-dispatch mismatch. The clean-round counter was reset to zero and each accepted finding became task 5.15–5.18.

- Explicit `config open/edit --owner remote-B` lost the device ID and reconstructed the local device for snapshot, Apply, and profile reload.
- Nested Provider exit could leave queued keys for Settings, while OAuth acknowledgement used a competing direct stdin reader.
- Settings, Provider/OAuth, and workspace notices lacked a shared endpoint-redaction and terminal-control presentation boundary.
- Clap accepted `-s=URL` and `-sURL`, but handwritten dispatch rejected both.

The fixes preserve the requested Daemon owner in `WorkspaceSession` and `Panel`, use it for every Daemon wire frame, and display it in Settings. `WorkspaceInput` now owns OAuth acknowledgement and establishes a queue-draining handoff boundary after nested editors. Human/TUI rendering sanitizes all dynamic Settings, Provider, OAuth, header, modal, and notice text; endpoints lose userinfo/query values/fragments while JSON and transport identity remain raw. Short attached selectors normalize before the existing selector extractor and still stop at `--`.

Verification:

- Recording WebSocket tests assert `ConfigApply(Device("remote-B"))` and profile-reload `ConfigSnapshot(Device("remote-B"))` exactly.
- Injected input tests assert stale queued keys are discarded and OAuth Enter is consumed by the sole workspace stream. Source scan finds one `event::read` and no OAuth/config-panel `stdin().read_line()`.
- Headless Settings, Provider, OAuth status, workspace header, modal, and notice tests cover userinfo/password/query/fragment plus ESC, OSC 52, BEL, CR, and LF sentinels.
- Unit and real CLI smoke tests cover separated `-s URL`, `-s=URL`, `-sURL`, and the `--` terminator.
- `cargo test -p fleety-cli --bin fleety --locked`: 239 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked`: 68 passed.

## 2026-07-17 — endpoint presentation boundary

Task 5.9 centralizes endpoint presentation sanitization in `fleety-tools` and delegates CLI display to it. Credentials, every query value, and fragments are removed before an endpoint reaches status, OAuth labels, target-resolution notes, transport errors, SSE fallback diagnostics, or tracing. Invalid endpoint text is replaced rather than echoed. JSON and human status use the same safe endpoint representation.

Verification:

- RED: fake transport smoke tests exposed userinfo, query, and fragment sentinels in the environment-resolution note, trace fallback warning, and successful human status.
- `cargo test -p fleety-tools redaction --locked`: 2 sanitizer tests passed for credentials, multiple query fields, fragments, and multiple embedded URLs.
- `cargo test -p fleety-cli --bin fleety auth::tests:: --locked`: 11 OAuth/auth tests passed, including secret-free remote status and sanitized Server labels.
- Both fake transport smoke tests passed, including the failure path with `RUST_LOG=trace`; stdout and stderr contained none of the sentinels.

## 2026-07-17 — one terminal event stream per workspace

Task 5.10 moves terminal input ownership into `WorkspaceSession`. The reader starts lazily once and its ordered receiver moves with the session through Chat, Conversations, and Settings. The nested Provider editor consumes that same receiver and only polls its own model-fetch worker; it no longer calls crossterm `poll` or `read`. Returning from the nested editor therefore cannot expose a queued event from an older route-local reader.

Verification:

- RED: the route-handoff event test failed to compile because no workspace-owned input stream existed.
- `cargo test -p fleety-cli --bin fleety workspace::tests::workspace_input_is_one_ordered_stream_across_route_handoffs --locked`: passed, preserving exact event order through ownership moves without replay.
- Source inspection found the sole shared-workspace `event::read` inside `WorkspaceInput`; the remaining legacy config editor reader is isolated and is explicitly removed by pending task 5.14.

## 2026-07-17 — terminal-safe remote human scalars

Task 5.11 applies the existing single-line terminal-control sanitizer at every human rendering boundary for Server/Daemon status, device IDs, sidecar names/status/paths, conversation list IDs/previews, resume role/content, audit kind/tool, rollback IDs/paths/results, and the workspace Conversations renderer. Ordinary Unicode is preserved; ESC, BEL, CR, LF, tab, and other controls become visible text. Structured JSON status retains the original semantic strings and relies on JSON escaping rather than mutating data.

Verification:

- RED: the fake Server status test printed a raw OSC 52 sequence and forged extra lines from version, device ID, and sidecar fields.
- `cargo test -p fleety-cli --test cli_smoke remote_status_human_scalars_are_terminal_safe_while_json_stays_semantic --locked`: passed for human ESC/OSC 52/BEL/CR/LF containment and exact raw JSON values.
- `cargo test -p fleety-cli --test cli_smoke conversation_audit_rollback_and_resume_scalars_cannot_inject_terminal_controls --locked`: passed across four fake Server workflows with no control bytes or forged lines.
- `cargo test -p fleety-cli --bin fleety tui::tests::conversation_list_parses_and_renders_real_server_rows --locked`: passed after renderer-boundary sanitation.

## 2026-07-17 — equals-form parser and dispatch parity

Task 5.12 now validates the original argv with Clap, then expands each validated long `--option=value` into the one separated token form consumed by execution handlers. Expansion stops at `--`, preserves additional equals signs inside values, and runs before profile/URL extraction and compatibility normalization. This covers global selectors, config owner aliases, ACP's command-local `--server`, and every other value-bearing local option without parallel handwritten parsers.

Verification:

- RED: `--server=ws://… status` passed Clap but reached dispatch as an unknown command and never contacted the fake Server.
- `cargo test -p fleety-cli --bin fleety coverage_tests::equals_options_expand_once_and_stop_at_the_option_terminator --locked`: passed for profile/owner expansion, embedded equals preservation, and literal option-like data after `--`.
- `cargo test -p fleety-cli --test cli_smoke equals_form_ --locked`: passed for `--server=`, `--url=`, `--profile=`, `--owner=`, `--target=`, exact Server `ConfigExec` target/payload, and command-local `--label=` persistence.
- Existing separated-form parser and smoke tests remain the comparison baseline for identical target, payload, exit class, and side effects.

## 2026-07-17 — truthful audit execution context

Task 5.13 routes `audit list/show` as Server-owned operations because the connected Server reads its own audit storage. `FLEETY_DEVICE_ID` remains unchanged in the `AuditList`/`AuditShow` payload, is displayed as a device filter, and is recorded in JSON context without relabeling execution as Daemon-owned.

Verification:

- RED: the human integration test printed `owner: Daemon 'cli-smoke'` even though the recording Server received and executed the request.
- `cargo test -p fleety-cli --test cli_smoke audit_context_names_server_owner_and_device_only_as_filter --locked`: passed for selected profile B, human Server owner, visible device filter, JSON `owner=server`, JSON `device_id=cli-smoke`, and exact wire payload.
- `cargo test -p fleety-cli --test cli_smoke resume_audit_and_rollback_render_server_results --locked`: existing audit list/show behavior remained green.

## 2026-07-17 — canonical shared Settings entry

Task 5.14 adds `fleety config open` as the canonical explicit Settings entry and keeps `config edit` only as a visible compatibility alias. Both route to the existing shared workspace, select the requested owner page, and require interactive stdin/stdout. Off-TTY invocation returns the same actionable error without touching config bytes. The old CLI-only ratatui/line editor and its Enter-triggered file write were removed; CLI mutation remains behind the shared Settings Apply boundary.

Direct `fleetyd config open` and `fleety-server config open` continue to operate on their own host-local configuration, matching owner responsibility. A `fleety` invocation never falls back to those files.

Verification:

- RED: `fleety config open --help` failed as an unrecognized subcommand.
- `cargo test -p fleety-cli --test cli_smoke config_open_is_canonical_and_edit_is_a_side_effect_free_alias_off_tty --locked`: canonical/alias help parity passed; both off-TTY paths returned the same error and preserved seeded config bytes.
- `cargo test -p fleety-cli --bin fleety config_panel::tests::cli_owner_apply_is_the_only_write_and_preserves_server_scope --locked`: staging remained byte-identical and only explicit CLI-owner Apply wrote, preserving Server scope.
- `cargo test -p fleety-cli --test cli_smoke every_command_node_supports_generated_help_without_side_effects --locked`: canonical `config open` is in the exhaustive side-effect-free help matrix.
- Source inspection found exactly one CLI `event::read`, inside the lazy workspace-owned `WorkspaceInput`; the removed config editor no longer owns a reader or write path.

## 2026-07-17 — final validation and manual critical pass

The complete normal-Windows workspace test run passed after distinguishing three sandbox-only service PID failures from product failures. In the restricted sandbox, `tasklist` returned `ERROR: Access denied`; the same 22 `fleety-tools` service tests and then the full workspace passed with ordinary Windows process-query access. This is live test evidence for the current Windows checkout, not cross-platform runtime proof.

The manual owner-boundary audit traced CLI mutations to their execution owners. Server settings and Provider/Model/OAuth payloads use Server `ConfigSnapshot`/`ConfigApply`; Daemon settings use the selected device target and are executed by `fleetyd`; rollback uses the connected Server workspace. Neither unavailable remote path calls a local config/provider serializer. Both interactive CLI-settings entry points now use the same CLI-owner apply boundary, which replaces only CLI scopes and preserves settings owned by other runtimes.

The critical pass found one brittle fail-closed branch: Chat reconnect classified a Server identity change by searching the human error message. It now carries a typed `ChatReconnectError::IdentityChanged`, so wording changes cannot turn an identity mismatch into an ordinary retry. The Rust baseline comment was also narrowed: the exact Clap pins support the declared baseline, but the full dependency graph is not represented as Rust 1.80 compatible.

Verification:

- `cargo test --workspace --no-fail-fast`: passed under normal Windows privileges.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed with checkout LF-to-CRLF notices only.
- `cargo test -p fleety-cli --no-fail-fast`: 227 unit tests and 55 smoke tests passed after the final owner/reconnect cleanup.
- `spectra analyze redesign-cli-experience`: Coverage, Consistency, Ambiguity, and Gaps all clean.
- `spectra validate redesign-cli-experience`: valid.
- `spectra status --change redesign-cli-experience`: all required artifacts complete.
- Stable release build and artifact sizes are recorded in the release-size section above.

## 2026-07-17 — latest independent review availability

The first independent review of the final UX pass reported three High, four Medium, and one Low finding. All accepted findings were corrected, including send/approval prepare-commit semantics, reconnect input handling, rollback ownership, canonical Conversations syntax, OAuth remediation, status documentation, and 50x16/grapheme-safe rendering.

A fresh post-fix independent reviewer was then requested, but the agent service rejected the run because the account usage limit had been reached. This is not a clean review round and is not counted toward task 5.3. Historical clean rounds predate the latest fixes and likewise are not reused as evidence. Task 5.3 remains incomplete until two consecutive fresh reviews report no Critical or High findings.

## 2026-07-17 — independent review after integrated workspace completion

A fresh read-only agent review found zero Critical, three High, four Medium, and one Low finding. All findings were independently checked against the implementation and accepted.

- High: Chat cleared the draft, attachments, transcript state, and approval queue before the WebSocket write succeeded. Submission is now a prepare/commit transaction; only a successful transport write commits local state.
- High: reconnect attempts blocked the event loop, discarded non-Ctrl+C keys, and exited the workspace when retries were exhausted. Connection attempts and backoff now select over local key events; draft editing, Help, command palette, and Settings navigation remain available, and exhausted retries enter Offline Connection Settings without quitting.
- High: rollback connected with a Daemon owner label although the Server handler reads and mutates the Server workspace/backups directory. Both rollback commands now identify Server as the execution owner; frame `device_id` remains audit attribution only.
- Medium: documented `conversations list --limit N` was rejected. The typed tree now accepts canonical `--limit`, retains hidden positional compatibility, and normalizes both to one handler value.
- Medium: the auth alias warning recommended nonexistent `provider auth`. It now names the exact `provider login`, `logout`, or `status` command.
- Medium: README overextended the partial-read contract from `config list` to `status`. Documentation now states the actual status requirement and limits partial owner reads to `config list`.
- Medium: the 50x16 footer clipped contextual keys and the header selected its long form by viewport width rather than measured content. The footer now wraps across two inner rows and the header measures the complete line before choosing its form.
- Low: column truncation split Unicode scalar values rather than grapheme clusters. It now truncates on extended grapheme boundaries and tests ZWJ emoji, flags, and combining marks.

Regression verification at this stage:

- `cargo test -p fleety-cli --bin fleety tui::tests --no-fail-fast`: 54 passed.
- `cargo test -p fleety-cli --bin fleety workspace::tests --no-fail-fast`: 16 passed.
- `cargo test -p fleety-cli --bin fleety reconnect_ --no-fail-fast`: 6 passed.
- Typed command, owner, and auth-remediation focused tests passed.

## 2026-07-17 — release-size baseline and Rust 1.80 compatibility boundary

A final stable release build of all three shipped binaries completed from an empty release cache in the workspace target in 343,500 ms. Compared with the pre-dependency baseline, the final artifacts are:

| Binary | Before | Final | Delta |
| --- | ---: | ---: | ---: |
| `fleety.exe` | 15,390,208 B | 17,272,320 B | +1,882,112 B (+12.23%) |
| `fleetyd.exe` | 15,467,520 B | 16,014,848 B | +547,328 B (+3.54%) |
| `fleety-server.exe` | 59,173,376 B | 59,660,288 B | +486,912 B (+0.82%) |

The command-tree dependencies remain exact, conservative pins: `clap 4.5.4` and `clap_complete 4.5.2`; their package metadata declares Rust 1.74 support. A real clean workspace build with the repaired Rust 1.80 toolchain did not reach Fleety code, however. Cargo 1.80 rejected pre-existing transitive packages using edition 2024 across the Boa workflow, grep/search, and FastEmbed/ORT dependency chains. A final isolated `cargo +1.80.0 check -p fleety-cli --locked` temporarily pinned the grep/ignore family to edition-2021 releases and then reached the independent `agent-workflow → boa_engine 0.21.1 → time ^0.3.44` constraint. Changing Boa/workflow or the embedding model/runtime would be a functional architecture change, not a CLI dependency fix.

All exploratory dependency and API downgrades were reverted. The remaining manifest/lock changes are only the intended Clap additions plus the editor's direct `unicode-segmentation` dependency. Task 1.2's selection gate is complete: the measured size cost and exact feature tree are accepted, and the negative Rust 1.80 result is attributed to a reproduced pre-existing dependency boundary rather than misreported as a Clap failure. The repository as a whole is **not** verified compatible with Rust 1.80; repairing that false baseline requires a separate architecture change.

Verification:

- `cargo build --release -p fleety-cli -p fleety-daemon -p fleety-server`: passed in 343,500 ms after compiling the complete release dependency graph.
- Artifact sizes were read directly from `target/release` after that build.
- `cargo +1.80.0 check` in a clean target exposed edition-2024 parse failures before Fleety compilation in the dependency chains described above.
- After temporary edition-2021 grep/ignore pins, `cargo +1.80.0 check -p fleety-cli --locked` advanced to Boa's incompatible `time ^0.3.44` constraint; all temporary pins were then reverted and stable `cargo check -p fleety-cli --locked` passed.
- After reverting all compatibility experiments, `cargo check -p agent-workflow -p fleety-tools -p fleety-server` passed.

Settings and Conversations currently render safe route placeholders from Chat; tasks 4.3 and 4.5 own their full shared-shell content and effects.

## 2026-07-17 — independent convergence review after tasks 5.4–5.8

A fresh, read-only principal review completed without interruption and covered command parsing/dispatch, owner routing, connection/profile transactions, Provider/OAuth/catalog, Chat/Conversations/approvals, reconnect, Unicode rendering, all three binaries, Server handlers, docs/specs, and the full uncommitted change. It reported zero Critical, two High, four Medium, and zero Low findings, so this is a valid non-clean round and the consecutive clean count remains zero.

- High: raw endpoint credentials/query values can leak through normal human transport errors, fallback warnings, status/OAuth display, and tracing.
- High: route-local and nested Provider crossterm readers can coexist, steal keys, and replay queued Settings actions after handoff.
- Medium: remote human scalar fields do not all cross the terminal-control sanitizer.
- Medium: Clap accepts `--option=value` while hand-written extraction rejects several equals forms.
- Medium: audit reads execute on Server storage but CLI context labels Daemon as owner.
- Medium: canonical `config open` is absent and legacy `config edit` remains an immediate-write editor outside shared Settings transactions.

Each accepted finding is tracked as task 5.9–5.14. Reviewer verification independently passed 235 CLI unit tests, 59 CLI smoke tests, 11 Daemon smoke tests, and 9 Server smoke tests. Live Unix PTY scheduling and a real browser OAuth round trip remained unavailable; static path review and headless tests covered those surfaces only.

## 2026-07-17 — owner-aware transactional Settings route

Task 4.3 replaced the old top-level config menu with one shared-shell Settings route containing Connection, CLI, Daemon, Server, and Providers & Models pages. The workspace state survives Chat↔Settings session handoff, while each Settings owner retains an independent Loading/Available/Dirty/Applying/Conflict/Failed/Unavailable state. Page titles identify the selected profile and owner; the Provider page identifies the connected Server endpoint and contains no storage filename.

All value edits now stage first. CLI Apply calls the in-process CLI owner service only after `a`; a byte-level test proves staging does not touch the file and applying the CLI scope preserves a seeded Server scope. Daemon and Server Apply continue to use one owner-specific `ConfigApply`. Wire error kind/remediation now survive into Conflict or Failed state, staged values remain present, and success reports Restart required or next-connection timing. An unavailable owner fails visibly and never falls back to another owner.

Providers & Models opens the structured Provider workflow against the exact active Settings target and initial Server fingerprint. It cannot re-resolve a different current profile or silently cross from Server A to another Server at the same URL. The Settings shell restores after the nested OAuth/catalog workflow and retains any typed failure.

Verification:

- RED: tests failed on the missing fifth page, CLI staged/apply flags, owner-aware area renderer, and state mapping before implementation.
- `cargo test -p fleety-cli --bin fleety config_panel::tests --no-fail-fast`: 17 tests passed, including typed conflict recording Server, owner-scoped CLI bytes, effect timing, five-page navigation, dirty badges, and no `providers.toml` title.
- `cargo test -p fleety-cli --bin fleety --no-fail-fast`: 202 passed after shared session handoff.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed after boxing the large handoff state.

## 2026-07-17 — explicit transactional profile switching

Task 4.4 makes a Settings profile switch explicit whenever the old profile has staged Daemon or Server state. The modal identifies both profiles and offers Apply, Discard, or Cancel. Cancel retains the old profile, transport, and every staged value. Apply sends each dirty owner through its existing owner-scoped `ConfigApply`; a typed conflict pauses the switch, retains the unapplied staging and remediation, and does not persist or reconnect. Discard is only committed after the new profile selection is durably saved.

The switch is a two-phase transaction. It resolves the exact saved target and persists `connections.toml` before touching the old transport or snapshots. Persistence failure rolls the in-memory current profile back, restores the decision prompt, retains staged values, and leaves the active target, fingerprint, and transport unchanged. After persistence succeeds, the old transport is closed, all state tied to the old Server is invalidated, the selected profile reconnects, and fresh Server and Daemon snapshots are loaded. A reconnect failure keeps the explicitly selected new profile but leaves both remote owners unavailable; it never restores or reuses the previous transport, fingerprint, revision, entries, or staging.

Verification:

- RED: dirty Apply/Discard/Cancel tests initially failed because no decision state or transaction existed.
- `cargo test -p fleety-cli --bin fleety config_panel::tests::profile_switch --no-fail-fast`: 8 passed, including typed Apply conflict, persistence failure before transport close, A→B persistence/reconnect/two-owner reload, and reconnect failure without old-state reuse.
- `cargo test -p fleety-cli --bin fleety config_panel::tests::dirty_profile_switch --no-fail-fast`: 4 passed, including the rendered old/new profile modal and all three resolutions.
- `cargo test -p fleety-cli --bin fleety config_panel::tests --no-fail-fast`: 26 passed.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-17 — persistent Chat and Conversations workspace session

Task 4.5 replaces route-local Chat construction with one `WorkspaceSession` that owns the workspace reducer, Chat application state, and the active Chat transport context. Chat messages, multi-line draft, exact cursor position, pending attachments, approvals, active conversation id, and replay sequence now survive navigation through Conversations, Settings, help, and command palette. Returning from Settings reconnects the selected profile without recreating Chat state. An initial connection or handshake failure preserves the session and routes to Connection Settings with an actionable notice instead of terminating the workspace.

Conversations is no longer a placeholder. Entering the route requests the selected Server's `ConversationList`, renders real summaries, supports selection, and sends `Resume` over the same authenticated transport. Returning to Chat leaves the unsent composer unchanged.

Chat submission now has an explicit readiness gate. Connected state, header profile, endpoint, Server identity/version, provider/model, and the stored transport context must agree before Enter can consume the draft. Initial connect and reconnect require an authenticated `Welcome`; reconnect verifies the prior Server identity before requesting model context or sending `Resume`. A changed identity fails closed, sends no Resume, and routes to Connection Settings. The Server-owned structured config snapshot supplies the active main provider/model, with legacy `FLEETY_MODEL` as a read-only display fallback. Settings clears old model/identity context when its active target changes, so profile A metadata is never displayed as profile B.

Verification:

- RED: workspace-session and atomic transport-context tests failed before `WorkspaceSession` and `ChatTransportContext` existed.
- `cargo test -p fleety-cli --bin fleety chat_reconnect --no-fail-fast`: 2 fake-Server tests passed, proving Welcome identity is checked before Resume, profile B context is installed, and draft/cursor/attachment state remains unchanged.
- `cargo test -p fleety-cli --bin fleety workspace::tests --no-fail-fast`: 12 passed, including Chat→Conversations→Chat→Settings→Chat state preservation and header/transport submission gating.
- `cargo test -p fleety-cli --bin fleety --no-fail-fast`: 217 passed, including real conversation-list parsing/rendering and structured/legacy model context.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-17 — responsive and Unicode-safe workspace rendering

Task 4.6 defines 50x16 as the smallest full workspace viewport. The 120x30, 80x24, and 50x16 render matrix covers Chat, Conversations, Providers & Models Settings, command palette, and contextual help with ASCII, Traditional Chinese, emoji, a long Unicode endpoint, provider/model labels, drafts, and conversation previews. Supported sizes retain the persistent shell, active route, profile, connection, model context, and footer. Narrow headers switch to a width-budgeted form that truncates profile/model on terminal-column boundaries while preserving connection state and route.

Below 50x16 the shell does not invoke route content at all. It renders one bounded fallback containing the observed size, 50x16 requirement, contextual-help key, and Esc/Ctrl+C exit keys. This prevents nested Chat or Settings layouts from receiving unusable rectangles and remains panic-free down to 1x1.

Verification:

- RED: the 50x16 semantic golden initially lost Chat, Settings, and Help route labels; every below-minimum case still invoked normal content.
- `cargo test -p fleety-cli --bin fleety workspace::tests --no-fail-fast`: 14 passed, including the 15-case supported-size/route semantic golden and 49x16, 50x15, 20x5, and 1x1 fallback matrix.
- `cargo test -p fleety-cli --bin fleety config_panel::tests::settings_content_is_safe_at_supported_sizes_with_unicode_and_long_endpoint --no-fail-fast`: passed at 120x30, 80x24, and 50x16.
- All matrix buffers contain no Unicode replacement character and all draws complete without panic.
- `cargo clippy -p fleety-cli --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-17 — canonical CLI documentation and generated-help drift gate

Task 5.1 synchronizes README, the config architecture, environment reference, and project status with the shipped typed command tree and terminal workspace. The canonical user surface is now documented as `chat`, `conversations`, `connection`, `provider`, `model`, `config`, `status`, `doctor`, and `completion`. The exact compatibility table maps `tui`, `server`, `auth`, config-nested Provider/Model, and `--target` to their canonical typed commands. The previous incorrect claim that `-s` selected a profile is removed: `--profile` is the non-mutating saved-profile selector and legacy `--server` is a transient raw WebSocket URL.

Owner documentation now states the complete contract: CLI settings apply only through the CLI owner service; Daemon, Server, Provider, Model, and OAuth mutations go to their owning runtime with no direct-file fallback. Multi-owner `config list` reads retain available data, label human output `PARTIAL`, emit one stable JSON envelope with owner errors, and exit 1. Settings documentation now names the five shared-workspace pages, per-owner staging, and transactional profile switching instead of the removed top-level menu/four-region panel.

One executable drift test parses the generated top-level `Commands:` block, compares it to an exhaustive canonical inventory, verifies every command is represented in README, checks the compatibility mapping, confirms canonical `--owner` plus `target` alias in config help, and verifies generated Bash and PowerShell completion examples.

Verification:

- `cargo test -p fleety-cli --test cli_smoke --no-fail-fast`: 55 passed, including `generated_help_and_documented_command_inventory_cannot_drift`.
- Content scan found no remaining `four-region`, top-level config menu, provider-CLI-missing, `fleety -s <name>`, or `--url <ws>` claims in README, `docs/env.md`, `docs/design-cli-config.md`, or `docs/STATUS.md`.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-17 — second independent convergence review and tasks 5.19–5.22

A second fresh read-only principal review re-audited the complete change after tasks 5.15–5.18. It reported zero Critical, three High, one Medium, and zero Low findings, so the consecutive clean count remained zero. The accepted findings became tasks 5.19–5.22:

- High: an invocation-only `--profile B` connected to B but Settings still derived parts of its identity and switch decisions from persisted current profile A.
- High: draining the key channel at Provider/OAuth return was not a synchronization barrier, so a stale key could arrive after the drain.
- High: terminal-control sanitization did not yet cover every human-output boundary, including command/voice/config errors and hostile argv echoed by Clap.
- Medium: trailing-help normalization did not skip attached short global value forms such as `-sURL` and `-s=URL`.

The fixes separate active target identity from persisted selection, use one crossterm reader with an acknowledged epoch handoff, route dynamic human output through terminal-safe scalar or multiline boundaries while preserving semantic JSON, and normalize all supported short server selector forms before trailing-help analysis. Focused tests reproduce the previous failure shapes, including persisted A with invocation override B, a delayed old-epoch key delivered after the handoff request, ESC/OSC/BEL/CR/LF payloads across stdout/stderr/TUI, and the short-option parser matrix.

Verification after all four fixes:

- `cargo test -p fleety-cli --bin fleety --locked`: 242 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked`: 71 passed.
- `cargo test -p fleety-daemon --test fleetyd_smoke --locked`: 11 passed.
- `cargo test -p fleety-server --test server_smoke --locked`: 9 passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, `spectra analyze redesign-cli-experience --json`, and `spectra validate redesign-cli-experience`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-18 — implementation and verification for tasks 5.34–5.41

Tasks 5.34–5.41 were implemented without changing the owner-routing contract. CLI-owned connection state is still mutated through the CLI owner path, Daemon state is targeted to the exact selected Daemon/profile, and Server/Provider/Model/OAuth state is applied by the currently connected Server. No mutation path falls back to editing an owner configuration file directly.

- Windows service ownership now distinguishes Alive, Dead, and Unknown. A failed, denied, slow, or indeterminate probe cannot claim or remove another process's pidfile, and Windows no longer depends on localized `tasklist` text.
- Server structured apply and direct Server configuration share one cross-process transaction lease. Revision check, config/provider reads, validation, and writes are serialized. A barrier test proves that two writers using the same revision yield one success and one conflict without a lost update.
- `connections.toml` writes now run as exact closure mutations under a cross-process lease. Profile selection, init persistence, pair/pin/heal, fingerprint cleanup, and legacy migration preserve unrelated concurrent state and validate the expected target before writing.
- An occupied `default` profile is never repurposed for a different env or mDNS target. The unmatched target remains invocation-only instead of inheriting or overwriting another profile's token/current identity.
- mDNS resolution retains advertised fingerprints, waits for the discovery window, and prefers the advertiser matching a saved pin even when it is not first. Legacy URL-less token-only profiles are required to re-pair rather than trusting the first advertiser.
- Direct Server Provider/Model value options accept both `--flag value` and `--flag=value`. The Provider `--url` alias is exposed only where it does not collide with the top-level CLI connection URL.
- Embedded URL redaction recognizes schemes case-insensitively, removes userinfo/query values/fragments, preserves harmless path and query-key text, and handles multiple URLs, punctuation, brackets, and control characters.
- Provider endpoint/key additions and changes require an explicit second confirmation before ConfigApply. Server audit records provider identity, old/new host, and key-rotation metadata without recording credentials or URL secrets.

Verification after tasks 5.34–5.41:

- `CARGO_BUILD_JOBS=1 cargo test --workspace --all-targets --locked --quiet`: passed. The complete run included 125, 4, 261, 74, 19, 12, 7, 3, 3, 25, 325, 11, 206, 3, and 1 passing test groups with zero failures.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed with one build job.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed; output contained only checkout LF-to-CRLF notices.
- `spectra analyze redesign-cli-experience --json`: all four dimensions Clean with zero findings.
- `spectra validate redesign-cli-experience`: valid.

The first parallel workspace attempt exhausted available memory and is not treated as proof. The successful complete run above used one Cargo build job and exited 0 naturally. No real browser OAuth/OpenAI catalog round trip or multi-host LAN mDNS advertisement was performed, so those remain fake-Server and program-path verification rather than live external proof.

## 2026-07-22 — independent review after tasks 5.34–5.41

A fresh independent Principal Rust CLI reviewer completed a full staged, unstaged, and untracked diff review without a time limit. It inspected owner routing, Provider snapshots and catalog authority, mDNS provenance, Daemon reconnect behavior, Windows PID probing, profile-switch concurrency, OAuth output, effect timing, and diff hygiene. The result was one Critical, four High, two Medium, and two Low findings, so the convergence count is reset to zero.

- Critical: Server ConfigSnapshot serializes Provider API keys in plaintext. With auth disabled, an unauthenticated connection can request the snapshot; even with auth enabled, the key unnecessarily leaves the Server owner.
- High: mDNS may select another profile's pinned advertiser while Daemon persistence attributes the connection to the URL-less current profile, allowing the next reconnect to send the current profile's token to the other Server.
- High: auth-disabled Server accepts ProviderModelList and uses its stored Provider credential for the outbound catalog request.
- High: the one-second Windows PowerShell PID probe is shorter than the measured process-query latency on this machine and yields Unknown for a live process, breaking service lifecycle confirmation.
- High: `connection use` and Settings switch only update CLI connection state; an already-connected fleetyd remains on the old Server until its session fails naturally.
- Medium: Settings snapshots the target token before acquiring the mutation lease and validates only the URL, so concurrent token rotation can make the immediate reconnect use stale credentials.
- Medium: OAuth login prints the complete authorization URL, including state and PKCE challenge, to terminal scrollback.
- Low: structured Provider/Model mutations omit the NextConnection effect timing shown by the shared config registry.
- Low: `git diff --check HEAD` reports an EOF whitespace error in the owner-routed configuration spec.

The accepted findings are tracked as tasks 5.42–5.50. Main-agent source review reproduced the direct Provider-key serialization, missing ProviderModelList gate, cross-profile mDNS provenance path, absent fleetyd reconnect signal, lease-external token snapshot, raw OAuth URL print, and missing effect metadata. The reviewer independently ran fmt and clippy successfully, all CLI/Daemon/Server unit and smoke suites successfully, and observed 204/206 fleety-tools tests with the two Windows live-PID cases failing. It did not perform real Windows SCM start/restart, multi-Server LAN mDNS, browser OpenAI OAuth/catalog, or an external Provider catalog proxy test.

## 2026-07-18 — unrestricted full-diff review after tasks 5.26–5.27

The reviewer was allowed to complete its own full inspection and test run without an imposed conclusion deadline. It reported zero Critical, two High, five Medium, and zero Low findings, so the clean-review streak is reset to zero.

- High: an env URL different from current profile A can inherit A's token, while fleetyd can persist the env target's minted token/fingerprint onto A or clear A after an authentication rejection.
- High: Provider endpoint query credentials can reach direct Server/Daemon output and the CLI JSON `data.output` string without redaction.
- Medium: invocation-only `--profile B` is omitted from TOFU pinning and sticky healing, so the exact selected profile is not maintained.
- Medium: OAuth callback parsing does not percent-decode form query values or reject malformed/duplicate security parameters.
- Medium: ConfigApply success followed by snapshot refresh failure is reported as Saved while stale revision state remains available.
- Medium: mDNS picker labels, init profile names, and direct config output still contain terminal-unsafe dynamic values.
- Medium: CLI JSON uses the legacy rendered `ConfigResult.output` payload. The accepted security defect is tracked in 5.29. Replacing the additive legacy protocol with typed records is not required by the current JSON-envelope contract and is not folded into this repair without a separate protocol design.

The accepted findings are tracked as tasks 5.28–5.33. The reviewer independently ran CLI, Daemon, Server, tools, and workspace tests. All relevant suites passed except three pre-existing Windows service timing/ownership tests in `fleety-tools`; it also confirmed `git diff --check` had no whitespace error. It did not run a real remote Server, browser OAuth flow, malicious live mDNS advertiser, or real Windows terminal capture.

Tasks 5.28–5.33 now preserve resolved-target provenance end to end. A different env URL receives no current-profile token and fleetyd cannot persist, pin, or clear that profile; an explicit env token is also distinguished from a saved token before cleanup. Named overrides pin and heal the exact named profile, with URL revalidation around the discovery wait. Provider/config URLs are redacted at the rendered payload boundary and direct binary terminal boundary, while CLI JSON remains one valid envelope. OAuth callback parameters are form-decoded and malformed or duplicate code/state values fail closed. Apply success followed by refresh failure clears the confirmed staged state and stale revision, reports reload-required, and blocks Server, Daemon, and Provider edits until reopen. mDNS and init identities are terminal-safe.

Verification after tasks 5.28–5.33:

- `cargo test -p fleety-cli --bin fleety --locked`: 260 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked --quiet`: 74 passed.
- `cargo test -p fleety-daemon --test fleetyd_smoke --locked --quiet`: 12 passed.
- `cargo test -p fleety-server --test server_smoke --locked --quiet`: 10 passed.
- `cargo test -p fleety-daemon --bin fleetyd --locked`: 18 passed.
- `cargo test -p fleety-tools --lib --locked`: 192 passed; the same three Windows service timing/ownership tests failed independently of the CLI UX paths.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-18 — unrestricted full review after tasks 5.28–5.33

The first new reviewer completed its own full-diff inspection without an imposed deadline. It reported zero Critical, five High, three Medium, and zero Low findings, so the clean-review streak remains zero.

- High: Windows PID probe failures are treated as dead owners, allowing pidfile takeover and false lifecycle state.
- High: Server ConfigApply revision check and two-file mutation are not protected by one transaction lease, so concurrent same-revision applies can both succeed and lose one update.
- High: CLI and fleetyd perform unlocked whole-snapshot `connections.toml` read-modify-save operations, allowing profile/token/fingerprint/current lost updates.
- High: an occupied `default` profile with current unset can receive an unrelated env/mDNS target token and be made current.
- High: single-result mDNS helpers discard advertised fingerprints, preventing pinned URL-less profiles from safely attaching their token and reconnecting.
- Medium: direct Server config execution accepts equals forms in Clap but rejects them in the raw-argv parser.
- Medium: embedded URL redaction recognizes only lowercase schemes.
- Medium: Provider endpoint/key mutations lack the sensitive confirmation and non-secret host audit required by the design.

The accepted findings are tracked as tasks 5.34–5.41. The reviewer independently reproduced the direct equals failures and the three Windows service failures even with single-threaded service tests; the latter are therefore no longer classified as harmless test timing noise. It also reran all CLI, Daemon, Server, smoke, clippy, fmt, diff, and Spectra checks. No live OpenAI OAuth login or multi-Server LAN mDNS advertisement was performed.

## 2026-07-22 — implementation and verification for tasks 5.42–5.50

The review findings were repaired without weakening owner routing. Server Provider snapshots now require config protocol 5, omit every API-key value, and expose only `key_present` metadata. Apply carries explicit Keep, Set, and Clear semantics. A redacted `None` means Keep, while `--clear-key` and the Provider editor's `k` action send a separate `clear_keys` intent. The Server validates and merges that intent under its configuration transaction lease. Old or unsafe snapshots fail closed in every CLI Server-snapshot consumer.

Provider model catalog requests now require the Server authentication boundary before Provider configuration or credentials are read. mDNS automatic discovery uses only the current profile's own pin/token and leaves unowned discovery uncredentialed. The running Daemon has a local request/ack reconnect control path. `connection use` and Settings profile switches persist the selected profile, notify `fleetyd`, close the old session, re-resolve, and reconnect. Settings obtains URL, token, and fingerprint together under the live connection mutation lease.

Windows ownership probing first uses native, language-independent `tasklist /FO CSV /NH`. A five-second bounded PowerShell fallback runs only when the native query itself fails, and Unknown remains fail-safe. Browser OAuth output contains only the sanitized origin/path. Explicit `--no-browser` uses the clipboard and prints a warned full URL only when the clipboard is unavailable. Provider/model mutations report `NextConnection`; queries report no effect. The owner-routed spec whitespace defect is removed.

Follow-up inspection found that write-only snapshots had made explicit Provider-key removal impossible. The final implementation adds the missing Clear operation to the shared parser/service, CLI payload, Server transaction, Provider TUI, help surface, documentation, and tests. It does not infer Clear from a redacted or blank key.

Verification:

- `CARGO_BUILD_JOBS=1 cargo test --workspace --locked -- --test-threads=1`: passed in 10m 1s with zero failures across all unit, smoke, integration, and doc-test groups. The first parallel attempt hit Windows `LNK1102` linker memory exhaustion and is not counted as proof.
- Focused Provider-key tests passed: 5 shared Provider-service tests, the parser clear-key test, 31 Provider TUI tests, and the Server Keep/Clear structured snapshot test.
- `cargo test -p fleety-cli --test cli_smoke --locked -- --test-threads=1`: 75 passed, including protocol-5 snapshots, OAuth catalog gating, and Provider/model effect output.
- `CARGO_BUILD_JOBS=1 cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check HEAD`: passed.
- `spectra analyze redesign-cli-experience`: Coverage, Consistency, Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

- `CARGO_BUILD_JOBS=1 cargo build --release --locked -p fleety-cli -p fleety-server -p fleety-daemon`: passed in 18m 15s.

No real browser OAuth/OpenAI catalog request, hostile live LAN mDNS advertiser, or Windows SCM install/start/restart cycle was performed. Those paths have deterministic unit, fake-Server, child-process, and program-flow proof, not live external proof. Independent convergence review remains pending and task 5.3 stays open.
- A real browser OAuth/model-catalog round trip was not available. Fake-Server and program-path tests prove routing, credential delivery, catalog request, and rendering behavior, but are not live OpenAI proof.

## 2026-07-18 — third independent convergence review

A fresh independent Principal Rust CLI review inspected the full staged and unstaged change, owner routing, profile transactions, input handoff, OAuth/catalog, terminal output, parser forms, Chat transport identity, and cross-platform tests. It independently ran 242 CLI unit tests, 71 CLI smoke tests, 11 Daemon smoke tests, and 9 Server smoke tests. The result was zero Critical, one High, two Medium, and zero Low findings, so this is a non-clean round and the consecutive clean count remains zero.

- High: when persisted profile A is current and invocation-only profile B cannot connect, Settings loses B before constructing the panel and incorrectly presents A as the active identity.
- Medium: Provider TUI still has raw render paths for status and catalog error fields supplied by the connected Server.
- Medium: OAuth legacy-token migration/status notes can echo an environment-selected filesystem path without terminal-control sanitization.

The accepted findings are tracked as tasks 5.23–5.25. The reviewer also confirmed no production breakage in owner-scoped mutation/no-fallback routing, short server selector forms, the single-reader epoch handoff, OAuth credential/catalog Server identity, OAuth catalog independence from Provider `base_url`, JSON/endpoint contracts, Chat approval transport identity, and the existing Windows/Unicode/minimum-terminal test surfaces. It did not perform a real browser OAuth or live OpenAI model-catalog round trip.

Follow-up source review confirmed the Provider TUI finding was already protected by the final render boundary in `render`: both title and body pass through `terminal_safe_text` before `Paragraph` construction. Task 5.24 therefore added explicit hostile status and catalog-error regression cases instead of duplicating production sanitization. Task 5.23 now resolves the invocation target before connection and retains it as the unavailable active identity on transport failure. Task 5.25 sanitizes/redacts the legacy path before it enters status or cleanup notes.

Verification after tasks 5.23–5.25:

- `cargo test -p fleety-cli --bin fleety --locked`: 243 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked`: 71 passed.
- `cargo test -p fleety-daemon --test fleetyd_smoke --locked`: 11 passed.
- `cargo test -p fleety-server --test server_smoke --locked`: 9 passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-18 — first clean-threshold candidate after tasks 5.23–5.25

A new independent reviewer reported zero Critical, zero High, two Medium, and zero Low findings after inspecting the full diff and the repaired unavailable-override path. This met the severity threshold but is not retained as clean round one because both Medium findings are accepted UX/output-contract defects and the implementation will change before the next review.

- Sticky profile healing prints raw prose and the discovered URL to stdout without respecting `--json`, `--quiet`, or endpoint redaction.
- `fleety init` custom scheme errors echo the raw positional URL before the shared terminal-safe error boundary.

These findings are tracked as tasks 5.26–5.27. The reviewer did not run new tests or live browser OAuth/Unix PTY/external Server verification; its conclusions are static control-flow evidence only.

Tasks 5.26–5.27 move the healing notice to sanitized human-only stderr and sanitize/redact invalid init URLs before interpolation. JSON and quiet modes receive no healing prose; the refreshed JSON context remains the machine-readable representation of the adopted endpoint.

Verification:

- `cargo test -p fleety-cli --bin fleety --locked`: 244 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked`: 72 passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, `spectra analyze redesign-cli-experience --json`, and `spectra validate redesign-cli-experience`: passed; diff check emitted only checkout LF-to-CRLF notices.

## 2026-07-23 — task 5.53 removes unsigned-TXT credential healing

The shared resolver now treats mDNS TXT fingerprints only as discovery ordering hints. Automatic mDNS never borrows a stored profile token or profile provenance. A credentialed saved endpoint failure leaves the profile byte-identical and directs explicit selection and pairing instead of scanning, attaching its token, persisting a discovered URL, or reporting a heal. The CLI one-shot path and fleetyd reconnect path share that recovery contract.

Explicit user-authored endpoint changes remain possible through `connection set-url`, Settings, and `init`. `set-url` and Settings validate `ws://` or `wss://` URLs before mutation, clear the old token and fingerprint, persist the requested URL, and require pairing before credentialed use. `init` requires a pairing code for a credentialed endpoint change, never sends the old token, and atomically replaces URL, token, and fingerprint only after the pairing response mints a new token. Invocation-only `--profile B pair <code>` repairs `B` without changing persisted current profile `A`.

TDD proof:

- Red resolver tests reproduced three unsafe contracts: copied TXT authorized URL healing, a matching discovery hint received the saved token, and a matching TXT fingerprint attached that token. The pre-fix `connection set-url` test also retained old credentials and lacked pairing guidance.
- A follow-up red test proved an occupied URL-less `default` profile could still receive a rogue discovered `Welcome.token`; it failed before the owner-selection fix. Mdns, Default, and unowned Env targets may now create `default` only when the profile store is truly empty.
- `cargo test -p fleety-tools --lib --locked -- --test-threads=1`: 213 passed. Two vacuous fixture-only healing assertions were removed; copied matching TXT now runs through the production resolver, and explicit endpoint reselection has a same/changed URL × none/token/pin/both boundary matrix.
- `cargo test -p fleety-cli --bin fleety --locked -- --test-threads=1`: 271 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked -- --test-threads=1`: 78 passed, including copied-hint isolation through the resolver contract, byte-identical saved-profile failure, credential-free endpoint repair, invalid URL rejection before I/O, and non-current profile pairing.
- `cargo test -p fleety-daemon --bin fleetyd --locked -- --test-threads=1`: 34 passed.
- `cargo test -p fleety-daemon --test fleetyd_smoke --locked -- --test-threads=1`: 20 passed, including explicit repair guidance after reconnect transport failure.
- `cargo clippy -p fleety-tools -p fleety-cli -p fleety-daemon --all-targets --locked -- -D warnings`, `cargo fmt --all -- --check`, and `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --json`: Coverage, Consistency, Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

No hostile live-LAN advertiser was used. Deterministic resolver, fake-Server, child-process, and byte-identity tests prove the stored-profile credential boundary. Automatic discovery can still establish an untrusted fresh control session, and caller-explicit `FLEETY_TOKEN` or `FLEETY_PAIRING_CODE` can still follow discovery; that broader operational-session boundary is recorded in `AGENTS.md` rather than silently expanding task 5.53.

## 2026-07-23 — task 5.54 preserves non-secret Provider key state

The CLI now parses `key_present` as a strict array of unique Provider-name strings, rejects missing, non-string, duplicate, unknown, and non-key-capable Provider metadata, and preserves the resulting set beside the redacted Provider graph. The shared Provider view, canonical human/JSON command renderer, file-backed compatibility renderer, and Provider TUI all use that sidecar to render API Providers as `key=Set` or `key=Not set`; OAuth Providers do not receive an API-key label.

Provider add, set, endpoint-only Keep, clear, and remove mutations update the same presence set. A successful TUI save converts pending Set values into presence, clears every in-memory plaintext key, and keeps only the non-secret state for later edits. Conflict and transport failures retain the pending secret, dirty state, and deferred OAuth／quit action for an explicit retry. Server and saver errors are redacted against staged key values before either human or JSON rendering.

TDD proof:

- The pre-fix malformed-metadata fake Server test succeeded after receiving `["openai", 7]`; the strict parser made it fail closed. Additional fixtures cover missing or wrong containers, duplicates, unknown／invalid Provider names, and OAuth Providers. Hostile metadata names and a plaintext snapshot sentinel prove failure output does not echo Server-controlled secret material.
- The pre-fix shared view and headless TUI omitted both key labels. The new fixtures use protocol-5-realistic redacted configs plus an independent presence set, so deriving state from `Provider.key` cannot satisfy them.
- The shared transition test covers add-without-key → Not set, add-with-key／Set → Set, a fresh redacted snapshot plus endpoint-only mutation → preserved Set, API→OAuth → Not set without an invalid Clear intent, and Clear → Not set. The successful-save test proves plaintext pending Set values are removed from both editor and persisted snapshots; retry tests prove conflict and error do not consume a deferred OAuth action.
- Fake Server CLI tests bind each Provider row to its own label in human output, compatibility `data.output`, and typed boolean `data.providers[].key_present`. Error-echo fixtures prove submitted, staged, and overlapping key bytes are absent from human, JSON, and TUI failure output.
- A Server snapshot regression starts from a manually persisted blank key. Before validation the Server emitted `key_present`; the fixed emitter now rejects the invalid graph before serializing a snapshot, so `Set` always means a non-empty Server-owned key.

Verification:

- `cargo test -p fleety-tools --locked`: 216 unit, 3 smoke, and 1 doc test passed.
- `cargo test -p fleety-cli --bin fleety --locked`: 277 passed.
- `cargo test -p fleety-cli --test cli_smoke --locked`: 86 passed.
- `cargo test -p fleety-server --bin fleety-server --locked`: 329 passed.
- `cargo clippy -p fleety-tools -p fleety-cli -p fleety-server --all-targets --locked -- -D warnings`: passed at the workspace MSRV.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --json`: Coverage, Consistency, Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

The Provider JSON schema keeps the existing envelope and compatibility `data.output`, and adds typed `data.providers` rows with a boolean `key_present` only for API Providers. Protocol frames were already protocol-5 compliant and did not change; the Server emitter now validates the graph before snapshot serialization. Daemon is not a Provider owner surface.

## 2026-07-23 — task 5.55 preserves Spectra tracking until archive succeeds

All four generated archive instructions now execute `spectra archive` before
removing `.spectra/touched/<change>.json`. Their shared guarded block retains
tracking and exits non-zero on archive failure; cleanup runs only in the success
branch.

TDD proof:

- The new guard initially exited 3 because none of the four generated files
  contained the safe archive block.
- The guard extracts and executes each file's actual fenced shell block with a
  deterministic fake `spectra`: exit 42 must retain tracking, while exit 0 must
  remove it.
- CI runs the guard on every push and pull request, so a later `spectra update`
  that regenerates the unsafe order fails before build or test.

Verification:

- `bash scripts/check-spectra-archive-instructions.sh`: all four generated
  instructions passed both failure-retention and success-cleanup fixtures.
- `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --json`: Coverage, Consistency,
  Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

## 2026-07-23 — task 5.57 covers immediate browser-launch failure end to end

The OAuth delivery path now exposes an internal injection seam for the browser
launcher, clipboard writer, and instruction output. Production still uses the
platform launcher and system clipboard; the regression test drives the same
delivery pipeline with a launcher whose bounded probe reports an immediate
non-zero exit.

TDD proof:

- Before the injection seam existed, the new test failed to compile because
  `present_authorization_with` did not exist.
- The test runs the real bounded launcher probe, proves clipboard fallback
  receives the exact full authorization URL, asserts the launcher and probe are
  each invoked exactly once, and requires the whole pipeline to return within
  200 ms.
- Captured terminal output must identify the clipboard fallback while excluding
  both OAuth sentinel values and the `state=`／`code_challenge=` query fields.

Verification:

- `cargo test -p fleety-cli --bin fleety --locked -- --test-threads=1`: 278
  passed, 0 failed.
- `cargo clippy -p fleety-cli --all-targets --locked -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --json`: Coverage, Consistency,
  Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

## 2026-07-23 — task 5.56 records config protocol v5

The protocol crate's top-level config history now records version 5 as the
write-only Provider-key boundary. Snapshot and apply frame documentation use
the same explicit vocabulary: `key_present` is non-secret metadata, omission is
Keep, a non-empty value is Set, and `clear_keys` is Clear.

TDD proof:

- The new consistency test initially failed with `config protocol v5 history is
  missing \`5\`` while the version constant was already 5.
- The test locks `CONFIG_PROTOCOL_VERSION == 5`, checks the source-level history
  for all v5 terms, and round-trips representative snapshot／apply payloads for
  `key_present`, Keep, Set, and Clear.

Verification:

- `cargo test -p fleety-protocol --locked`: 26 passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --json`: Coverage, Consistency,
  Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict`: valid.

## 2026-07-23 — independent convergence review after tasks 5.51–5.57

A fresh read-only Sol high review covered command IA, owner safety, Settings
transactions, TUI input and accessibility, Provider／Model／OAuth boundaries,
Daemon reconnect, Windows and cross-platform behavior, docs, protocol, and the
complete worktree. It reported two High, six Medium, and one Low finding, so
the clean-review streak remains zero.

- High: automatic mDNS can turn an untrusted advertiser into an operational
  fleetyd session, forward caller-explicit credentials, persist a rogue
  `Welcome` token, and accept `RunTool`. The previous stored-credential fix
  explicitly left this wider boundary open; task 5.58 now tracks it.
- High: fleetyd logs PID／control ownership failures and continues into the
  network loop, allowing multiple processes with the same device identity.
  Task 5.59 requires fail-closed single ownership.
- Medium: a settled reconnect for the same profile can satisfy a new caller;
  reconnect success is published before a minted token is durable; profile
  switch bypasses the per-owner refresh barrier; reconnect control lacks
  version, process-start identity, and crash-durable publication; and a raw URL
  borrows the first saved profile token with the same URL. Tasks 5.60–5.64
  track these independently testable boundaries.
- Medium: running a later Spectra task command regenerated all four unsafe
  archive instructions. The CI guard correctly failed, so task 5.55 was
  reopened and must be repaired only after the final Spectra state mutation.
- Low: `docs/design-cli-config.md` described `--server` as a profile selector.
  It now distinguishes `--profile <name>` from transient
  `--server <ws-url>`.

Two independent Sol high revalidation passes confirmed tasks 5.59–5.64.
Task 5.58 was confirmed as a reachable security boundary rather than a task
5.53 regression: the old change protected stored credentials but did not make
automatic discovery safe for caller-explicit secrets or remote control.

Reviewer verification passed workspace check and clippy, protocol 26,
connection 29, CLI command 6, CLI unit 278, Server unit 329, Daemon unit 34,
CLI smoke 86, and Daemon smoke 20. Fmt, full diff check, Spectra analyze, and
strict validation also passed. It did not run the complete workspace test,
release build, a Windows-native session, a hostile live LAN advertiser, or a
real OAuth browser／clipboard flow; those remain required final gates or
explicit live-environment limitations.

## 2026-07-26 — task 5.64 freezes durable profile ownership and transient provenance

Raw `--server`／`--url`, ACP, and daemon transport overrides now remain
transient and use only a non-empty caller-explicit token. Stored credentials,
TOFU pins, minted tokens, pairing replacement, and auth-rejection cleanup are
authorized only by the exact named／current profile generation frozen by the
resolver. An empty `FLEETY_AGENT_URL` is treated as unset without mutating
`connections.toml`; diagnostic resolution retains immutable fingerprint
expectations while removing mutation authority, including repeated read-only
conversion.

Pairing and guided init now retain the exact committed generation when rename
succeeds but publication sync is ambiguous. A retry revalidates that generation
under the mutation lease before syncing the canonical file, so owner drift
fails explicitly. Current-profile notification is decided inside the same
credential commit lease. Doctor remains byte-for-byte read-only.

Zed ACP installation now uses private no-clobber publication and reports every
partial state precisely: canonical publication with cleanup warnings,
recoverable displaced bytes, retained temporary files after write, permission,
backup, or publication failure, and restored canonical bytes with retained
recovery cleanup. Users must close Zed before retrying because a
non-cooperating process that already holds the file open remains outside the
cooperative publication contract.

TDD and regression proof:

- `cargo test -p fleety-cli
  acp::tests::permission_failure_reports_a_retained_private_temp_file --locked`:
  1 passed; the complete CLI unit suite now has 315 tests.
- `cargo test -p fleety-tools
  connection::tests::repeated_read_only_conversion_preserves_identity_expectation
  --locked`: 1 passed; the complete fleety-tools unit suite now has 237 tests.
- `cargo test --workspace --locked`: passed, including 315 CLI unit tests, 107
  CLI smoke tests, 69 daemon unit tests, 41 fleetyd smoke tests, 237
  fleety-tools unit tests, and 329 Server unit tests.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo build --workspace --release --locked`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `spectra analyze redesign-cli-experience --no-color`: Coverage, Consistency,
  Ambiguity, and Gaps are all Clean with zero findings.
- `spectra validate redesign-cli-experience --strict --no-color`: valid.
- Three context-isolated Sol medium reviewers independently reported exact
  `No findings.` after the final permission-cleanup and read-only-idempotence
  fixes.
