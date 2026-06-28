## ADDED Requirements

### Requirement: Run shell commands with a critical-command guard

The system SHALL provide a `run_command` tool that runs one shell command (default working directory the root, overridable by `cwd`) and returns `stdout`, `stderr`, and `exit_code`. It SHALL refuse clearly irreversible command shapes (disk wipe, `mkfs`, `dd` to a device, `rm -rf /`, host shutdown/reboot, and similar) with an actionable error rather than executing them.

#### Scenario: ordinary command runs, catastrophic command refused

- **WHEN** `run_command` is given `echo hi`
- **THEN** it returns `exit_code` 0 and the captured output
- **WHEN** `run_command` is given `rm -rf /`
- **THEN** it is refused with a critical-command error and is not executed

### Requirement: Diff files a command changed

`run_command` SHALL accept an optional `track` array of paths and return a unified before/after `diff` for each, so file changes a command makes are observable.

#### Scenario: track a file the command edits

- **WHEN** `run_command` runs a command that appends to a tracked file
- **THEN** the result includes a diff showing the appended line for that path
