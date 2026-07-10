# container-deployment Specification

## Purpose

TBD - created by archiving change 'deploy-hardening'. Update Purpose after archive.

## Requirements

### Requirement: Container image runs as a non-root user

The Fleety server container image SHALL run the server process as a dedicated non-root user, so files the server writes into bind-mounted volumes (/workspace and /data) are owned by that user rather than root on the host. The image SHALL ensure the runtime paths the server writes to (/data and /workspace, including the agent home, managed runtimes, models, and chrome directories under /data) are writable by that user, and SHALL keep the built-in ddgs web-search tool resolvable on that user's PATH.

#### Scenario: files written into a volume are not root-owned

- **WHEN** the container runs and the server writes a file into the /workspace bind mount
- **THEN** the process runs as the non-root user and the written file is owned by that non-root uid, not root

#### Scenario: built-in tools remain available to the non-root user

- **WHEN** the server (running as the non-root user) invokes the built-in ddgs web-search MCP
- **THEN** the ddgs binary is found on PATH and the server can write its agent state under /data without permission errors

<!-- @trace
source: deploy-hardening
updated: 2026-07-10
code:
  - crates/fleety-cli/src/main.rs
  - docs/env.md
  - crates/fleety-server/src/storage.rs
  - crates/fleety-server/src/conn.rs
  - crates/fleety-tools/src/service.rs
  - crates/fleety-cli/src/provider_tui.rs
  - crates/fleety-server/src/service.rs
  - crates/fleety-cli/src/config.rs
  - crates/fleety-server/src/restart_watch.rs
  - crates/fleety-server/src/scheduler.rs
  - crates/fleety-tools/src/config.rs
  - Dockerfile
  - scripts/install.sh
  - README.md
  - crates/fleety-cli/src/auth.rs
  - crates/fleety-cli/src/voice.rs
  - crates/fleety-server/src/privacy.rs
  - crates/fleety-server/src/schedules.rs
  - crates/fleety-server/src/main.rs
  - crates/fleety-cli/src/clipboard.rs
  - crates/fleety-daemon/src/service.rs
  - crates/fleety-cli/src/tui.rs
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/markdown.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-cli/src/acp.rs
  - crates/fleety-server/src/identity.rs
  - crates/fleety-cli/src/input.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
-->