## ADDED Requirements

### Requirement: Bare fleety config opens a three-region interactive panel

On a TTY, `fleety config` with no arguments SHALL open a single interactive panel with three regions — Connection, This device, and Server — switchable without any `--target` flag. The Connection region edits `connections.toml`, the This-device region edits the local Cli/Shared settings, and the Server region edits the connected server's settings and provider/model configuration. Without a TTY, `fleety config` SHALL fall back to the non-interactive text commands.

#### Scenario: the panel exposes all three layers from one entry

- **WHEN** `fleety config` runs on a TTY
- **THEN** a panel opens with Connection / This device / Server regions, and switching regions needs no `--target` flag

#### Scenario: no TTY falls back to text

- **WHEN** `fleety config list` runs without a TTY
- **THEN** it uses the non-interactive text command path, not the panel

### Requirement: The server region edits remote settings via the structured channel

The Server region SHALL populate from a `ConfigSnapshot` and apply edits via `ConfigApply` when the server supports the structured protocol, falling back to the legacy `ConfigExec` text flow otherwise. Secret fields SHALL be masked and write-only (edits send a new value or clear, never the masked placeholder); a provider's fields SHALL render per its `type`; and when a change takes effect SHALL be shown.

#### Scenario: server settings edit remotely and show effect timing

- **GIVEN** the panel's Server region is populated from a snapshot of a supporting server
- **WHEN** the user changes a setting and applies it
- **THEN** the change is sent as a `ConfigApply` and the result shows when it takes effect (next connection or restart)

### Requirement: Sensitive server-key changes require auth and are warned and audited

A `ConfigApply` that mutates a Server-scope setting SHALL require the server to have authentication enabled (per the auth-default-on rule). Overwriting a key that could redirect data or credentials off-box (a provider `base_url`/`key`, the backup repo/token, an oauth endpoint) SHALL prompt a prominent confirmation and be recorded in the audit log (with old/new host), and a secret SHALL be reported in a snapshot only as is-set with the read recorded.

#### Scenario: overwriting an exfiltration-risk key warns and audits

- **WHEN** the user changes a provider's `base_url` (a data-redirect risk) in the panel and applies it
- **THEN** a prominent confirmation is shown before applying, and the change is written to the audit log
