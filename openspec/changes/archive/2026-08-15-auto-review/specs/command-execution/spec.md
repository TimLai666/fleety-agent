## MODIFIED Requirements

### Requirement: Run shell commands with a critical-command guard

The system SHALL provide a `run_command` tool that runs one shell command (default working directory the root, overridable by `cwd`) and returns `stdout`, `stderr`, and `exit_code`. It SHALL detect clearly irreversible command shapes, including disk wipe, `mkfs`, `dd` to a device, `rm -rf /`, host shutdown/reboot, and similar patterns. Under `full_access` or `require_approval`, a detected critical command SHALL be refused with an actionable error before execution. Under `auto_review`, the detector SHALL emit a trusted danger signal to the reviewer and SHALL NOT refuse the command before that review; the command SHALL execute only after a valid reviewer approval.

#### Scenario: ordinary command runs

- **WHEN** `run_command` is given `echo hi`
- **THEN** it returns `exit_code` 0 and the captured output

#### Scenario: default policy refuses a catastrophic command

- **WHEN** `run_command` is given `rm -rf /` under `full_access`
- **THEN** it is refused with a critical-command error and is not executed

#### Scenario: auto review evaluates a catastrophic command

- **WHEN** `run_command` is given `rm -rf /` under `auto_review`
- **THEN** the reviewer receives a catastrophic-delete danger signal and the command executes only if the reviewer approves
