# service-discovery Specification

## Purpose

TBD - created by archiving change 'baseline-config-specs'. Update Purpose after archive.

## Requirements

### Requirement: mDNS service discovery

The server SHALL announce `_fleety._tcp.local.` over mDNS, and the CLI and daemon SHALL browse for it as the last fallback when no URL is configured. `FLEETY_MDNS_DISABLED` SHALL, when set to any value, skip both announce and browse. When `FLEETY_ADDR` binds a wildcard address (`0.0.0.0`), the server SHALL auto-detect a single routable (non-loopback, non-wildcard) local IP to advertise — by opening a UDP socket and connecting it to a public address so the OS selects the outbound interface's IP, sending no packet — so discovery works out of the box on the exposed default. `FLEETY_MDNS_HOST_IP` SHALL, when set, force the advertised IP (overriding auto-detection, for multi-homed hosts). When neither an explicit host IP nor an auto-detected routable IP is available, the server SHALL skip the announcement (it never advertises a loopback or wildcard address). `FLEETY_MDNS_HOST` SHALL set the mDNS instance name (default the hostname).

#### Scenario: disabling mDNS skips announce and browse

- **WHEN** `FLEETY_MDNS_DISABLED` is set
- **THEN** the server does not announce and clients do not browse

#### Scenario: wildcard bind auto-detects a routable advertised IP

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0`, `FLEETY_MDNS_HOST_IP` is unset, and the host has an outbound route
- **THEN** the server advertises the auto-detected routable IP rather than an unusable wildcard address

#### Scenario: an explicit host IP overrides auto-detection

- **WHEN** `FLEETY_ADDR` binds `0.0.0.0` and `FLEETY_MDNS_HOST_IP` is set
- **THEN** the server advertises that pinned IP instead of the auto-detected one


<!-- @trace
source: expose-server-by-default
updated: 2026-07-11
code:
  - crates/fleety-tools/src/config.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-server/src/mdns.rs
  - docs/env.md
  - docs/roadmap.md
-->

---
### Requirement: mDNS is a sticky, fingerprint-guarded fallback in the resolver

Within the shared connection resolver, a saved current profile with an explicit URL SHALL connect automatically and SHALL rank above every discovery path. Authenticated endpoints previously learned from that same profile's `Welcome` SHALL be part of the saved profile rather than mDNS discovery. When no saved current URL or learned endpoint can be resolved, mDNS SHALL collect candidates for display and explicit selection only; the resolver SHALL NOT return an mDNS candidate as an operational target. Matching, mismatched, and absent TXT fingerprints SHALL NOT authorize sending a stored or caller-explicit token, sending a pairing code, persisting a `Welcome` token, accepting control frames, or assigning saved profile provenance. A user SHALL explicitly select an endpoint and complete pairing before that endpoint can become an operational profile. A credentialed profile without any saved endpoint SHALL require explicit endpoint selection and re-pair instead of falling through to mDNS.

#### Scenario: an enrolled device does not drift to mDNS

- **WHEN** a device has a current profile with a saved URL and an mDNS advertiser appears on the LAN
- **THEN** the resolver SHALL stay on the current profile's URL and SHALL NOT query mDNS

#### Scenario: learned endpoints remain profile-owned

- **GIVEN** profile `home` learned `ws://100.64.0.8:8787` from an authenticated `Welcome`
- **WHEN** its primary LAN endpoint is unreachable
- **THEN** the resolver SHALL try the learned endpoint as part of `home` without querying mDNS or treating another advertiser as trusted

##### Example: current profile wins over a live mDNS advertiser

- **GIVEN** `connections.toml` has `current = "home"` and `profiles.home.url = "ws://192.168.1.20:8787"`
- **AND** an mDNS advertiser is publishing `ws://192.168.1.99:8787` on the LAN
- **WHEN** the resolver runs with no command override and no `FLEETY_AGENT_URL`
- **THEN** it SHALL resolve `ws://192.168.1.20:8787` and SHALL NOT query mDNS

#### Scenario: mDNS-discovered server never receives a stored profile token

- **WHEN** automatic mDNS resolves a URL with a matching, mismatched, or absent TXT fingerprint
- **THEN** no saved profile token SHALL be attached, the candidate SHALL remain display／selection metadata only, and no operational result SHALL be returned

##### Example: copied matching fingerprint gets no token

- **GIVEN** saved profile `home` has token `home-token` and fingerprint `AA:BB`
- **AND** an mDNS advertiser publishes `ws://192.168.1.99:8787` with copied TXT fingerprint `AA:BB`
- **WHEN** automatic discovery evaluates the advertiser
- **THEN** it SHALL NOT attach `home-token`, attribute the target to `home`, or persist the discovered URL

#### Scenario: unconfigured discovery does not create an operational session

- **GIVEN** no saved current profile has an explicit URL
- **WHEN** mDNS discovers one or more Server advertisers
- **THEN** the candidates SHALL be displayed only through an explicit selection flow and none SHALL receive a token, pairing code, `Hello`, or control-session authority

#### Scenario: caller-explicit credentials do not follow automatic discovery

- **GIVEN** `FLEETY_TOKEN` or `FLEETY_PAIRING_CODE` is set without an explicit endpoint or saved current URL
- **WHEN** automatic resolution discovers an mDNS advertiser
- **THEN** resolution SHALL fail with explicit selection guidance before sending either credential

#### Scenario: a credentialed URL-less current profile requires repair

- **GIVEN** current profile `home` has a stored token but no URL
- **WHEN** the resolver runs without an explicit endpoint
- **THEN** it SHALL fail with explicit selection and re-pair guidance before querying mDNS


<!-- @trace
source: redesign-cli-experience
updated: 2026-07-29
code:
  - crates/fleety-tools/src/secure.rs
  - crates/fleety-markdown/src/style.rs
  - crates/fleety-cli/src/commands.rs
  - crates/fleety-textarea/README.md
  - scripts/check-spectra-archive-instructions.sh
  - crates/fleety-cli/src/tui.rs
  - docs/HANDOFF.md
  - crates/fleety-textarea/src/editor_tests/keys.rs
  - .agents/skills/spectra-archive/SKILL.md
  - crates/fleety-textarea/src/editor_tests/mod.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-markdown/src/latex/mod.rs
  - crates/fleety-tools/src/config.rs
  - scripts/install-server.sh
  - crates/fleety-markdown/src/latex/math_box.rs
  - docs/STATUS.md
  - crates/fleety-markdown/src/open_code_highlighter.rs
  - crates/fleety-markdown/src/hyperlinks.rs
  - .opencode/skills/spectra-archive/SKILL.md
  - crates/fleety-inline/src/common.rs
  - crates/fleety-server/Cargo.toml
  - crates/fleety-textarea/LICENSE
  - crates/fleety-markdown-core/Cargo.toml
  - crates/fleety-inline/src/terminal.rs
  - crates/fleety-textarea/src/editor_tests/planning.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-textarea/Cargo.toml
  - crates/fleety-markdown/src/parse.rs
  - crates/fleety-cli/src/workspace.rs
  - crates/agent-core/src/codex_responses.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-tools/src/oauth.rs
  - Cargo.toml
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-inline/src/scrollback.rs
  - crates/fleety-textarea/src/editor_tests/viewport.rs
  - AGENTS.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-markdown/src/syntax.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-textarea/src/editor_keys.rs
  - crates/fleety-tools/src/provider_service.rs
  - crates/fleety-tools/src/providers_config.rs
  - crates/fleety-markdown/assets/tokyo-night.tmTheme
  - crates/fleety-inline/src/segment.rs
  - crates/fleety-markdown/src/latex/commands.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-tools/src/deps/runtime.rs
  - crates/fleety-server/src/http.rs
  - crates/fleety-markdown-core/LICENSE
  - crates/fleety-server/src/mdns.rs
  - .opencode/commands/spectra-archive.md
  - crates/fleety-cli/src/server.rs
  - crates/fleety-markdown/src/output.rs
  - crates/fleety-inline/src/lib.rs
  - crates/fleety-textarea/src/textarea.rs
  - crates/fleety-textarea/src/editor.rs
  - crates/fleety-markdown/src/streaming.rs
  - crates/fleety-markdown/src/render.rs
  - crates/fleety-inline/LICENSE
  - crates/fleety-markdown/src/latex/tests.rs
  - crates/fleety-markdown/src/buffers.rs
  - crates/fleety-tools/src/device.rs
  - crates/fleety-markdown/src/colors.rs
  - docs/env.md
  - crates/fleety-markdown/src/mermaid.rs
  - crates/fleety-tools/src/transport.rs
  - crates/fleety-markdown/src/checkpoint.rs
  - crates/fleety-markdown/src/source_map.rs
  - crates/fleety-markdown/Cargo.toml
  - docs/roadmap.md
  - crates/fleety-cli/src/provider_service.rs
  - crates/fleety-inline/src/resize.rs
  - crates/fleety-textarea/src/wrapping.rs
  - crates/fleety-server/src/auth.rs
  - crates/fleety-cli/Cargo.toml
  - crates/fleety-markdown/src/latex/symbols.rs
  - crates/fleety-markdown/src/latex/cursor.rs
  - crates/fleety-markdown/src/lib.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-textarea/src/render/mod.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-textarea/src/lib.rs
  - docs/tools.md
  - crates/fleety-tools/Cargo.toml
  - docs/acp.md
  - crates/fleety-cli/src/config.rs
  - crates/fleety-textarea/src/render/line_utils.rs
  - crates/fleety-markdown/src/latex_delimiters.rs
  - README.md
  - crates/fleety-markdown/src/latex/environments.rs
  - crates/fleety-daemon/Cargo.toml
  - crates/fleety-inline/Cargo.toml
  - crates/fleety-markdown/src/url_scan.rs
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-tools/src/chrome.rs
  - docs/design-cli-config.md
  - crates/fleety-cli/src/input.rs
  - .github/workflows/ci.yml
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-server/src/providers.rs
  - crates/fleety-markdown/README.md
  - crates/fleety-markdown/LICENSE
  - crates/fleety-textarea/src/editor_tests/editing.rs
  - crates/fleety-inline/src/tests.rs
  - crates/fleety-inline/README.md
  - crates/fleety-cli/src/config_panel.rs
  - crates/fleety-markdown-core/src/lib.rs
  - crates/fleety-daemon/src/winsvc.rs
tests:
  - crates/fleety-cli/src/test_terminal.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: Interactive discovery lists every advertised server

For guided onboarding, the CLI SHALL provide a collecting discovery mode that browses `_fleety._tcp.local.` for a fixed window and returns **every** server resolved in it — each entry carrying a display name derived from the advertised instance name (the `fleety-` prefix stripped; the URL stands in when no name is available) and the `ws://ip:port` URL — de-duplicated by URL, in discovery order. The existing implicit single-result fallback used by the connection resolver SHALL keep its early-return behavior unchanged. When mDNS is disabled or the browse cannot start, collecting discovery SHALL return an empty list rather than failing.

#### Scenario: multiple servers are all collected

- **WHEN** two servers advertise on the LAN during the collection window
- **THEN** the collecting discovery returns both entries with their display names and URLs

#### Scenario: duplicate announcements collapse

- **WHEN** the same server is resolved more than once during the window
- **THEN** the returned list contains it once

##### Example: name derivation

| Advertised instance | Display name |
| ------------------- | ------------ |
| fleety-mini         | mini         |
| fleety-nas-01       | nas-01       |
| (none resolved)     | the ws URL   |

#### Scenario: disabled discovery yields an empty list

- **WHEN** `FLEETY_MDNS_DISABLED` is set and collecting discovery runs
- **THEN** it returns an empty list without browsing

<!-- @trace
source: init-discovery-picker
updated: 2026-07-11
code:
  - docs/env.md
  - README.md
  - crates/fleety-cli/src/main.rs
-->

---
### Requirement: The server advertises a persistent identity fingerprint

The server SHALL generate a persistent identity id on first start (stored under the agent home, stable across restarts and address changes; an unreadable or unwritable id file degrades to a per-run id with a warning and never crashes the server). The id SHALL be advertised as an mDNS TXT property alongside the existing version property, and SHALL be carried in `Welcome` as an additive optional field, sourced from the same value. Discovery (both the single-result fallback and the collecting scan) SHALL surface the advertised fingerprint on each found server; a server that advertises none yields a fingerprint-less entry.

#### Scenario: fingerprint is stable across restart and address change

- **WHEN** the server restarts on a different IP
- **THEN** it advertises the same identity fingerprint as before

#### Scenario: discovery carries the fingerprint

- **WHEN** a collecting scan resolves a server that advertises an identity fingerprint
- **THEN** the returned entry carries that fingerprint alongside the name and URL

#### Scenario: an old server yields a fingerprint-less entry

- **WHEN** a scan resolves a server that advertises no fingerprint
- **THEN** the entry's fingerprint is absent and no client treats it as any pinned identity

<!-- @trace
source: sticky-heal-fingerprint
updated: 2026-07-12
code:
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/mdns.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/update.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Guided init probes the local server before scanning

Before the LAN scan, guided `fleety init` SHALL probe the local server on loopback with a short timeout and, when it answers, include it as a discovery entry ahead of the mDNS results. The probe SHALL be bounded so a host with no local server is not delayed noticeably, and SHALL never error the init flow — a failed or timed-out probe simply omits the local entry.

#### Scenario: local entry precedes mDNS results

- **WHEN** a local server answers the loopback probe and mDNS also finds LAN servers
- **THEN** the local entry appears first in the picker, ahead of the discovered LAN servers

#### Scenario: a failed probe never blocks discovery

- **WHEN** the loopback probe times out or errors
- **THEN** no local entry is added and the mDNS scan proceeds normally

<!-- @trace
source: local-server-trust
updated: 2026-07-12
code:
  - crates/fleety-server/src/http.rs
  - crates/fleety-server/src/conn.rs
  - scripts/install-server.sh
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->

---
### Requirement: The CLI prefers a co-located loopback server over mDNS

When the fleety CLI resolves a connection with no `--server`/`--url` override, no `FLEETY_AGENT_URL`, and no current profile, its discovery step SHALL first probe for a local server on loopback (`127.0.0.1:<port>`, the port taken from `FLEETY_ADDR` or the default) and, when one answers, resolve that loopback URL — ranking above mDNS. mDNS discovery SHALL be consulted only when no local loopback server answers. A loopback-resolved server SHALL carry no token, since a same-host connection is loopback-trusted by the server. This prevents a co-located CLI from resolving the host's own outward LAN IP via mDNS — a non-loopback address the server refuses without pairing. The preference applies to the CLI only; the daemon's resolution is unchanged.

#### Scenario: co-located CLI resolves loopback, not its own LAN IP

- **WHEN** the CLI resolves on the server host with no profile, the local server answers on `127.0.0.1`, and mDNS would advertise the host's own LAN IP
- **THEN** the resolver returns the `127.0.0.1` loopback URL (same-host trusted, no pairing), not the LAN IP

#### Scenario: no local server falls through to mDNS

- **WHEN** no server answers on loopback
- **THEN** the resolver proceeds to mDNS discovery exactly as before

##### Example: loopback wins over a live mDNS advertiser on the same host

- **GIVEN** a local server answers on `ws://127.0.0.1:8787`
- **AND** mDNS is advertising this host's own `ws://192.168.1.109:8787`
- **WHEN** the CLI resolves with no override, no `FLEETY_AGENT_URL`, and no current profile
- **THEN** it resolves `ws://127.0.0.1:8787` and reports it as this host's local server

<!-- @trace
source: loopback-first-resolution
updated: 2026-07-12
code:
  - crates/fleety-cli/src/main.rs
-->