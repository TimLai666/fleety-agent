## ADDED Requirements

### Requirement: A persistent PTY session the agent drives turn by turn

The system SHALL provide tools to open a process under a PTY, read its output, send input, and close it: `terminal_open`, `terminal_read`, `terminal_input`, `terminal_close`. Each is an ordinary tool returning a single result (no protocol change). The PTY and its child process SHALL persist across calls in a process-global registry keyed by a session id, so a later `terminal_input`/`terminal_read` drives the same live process. `terminal_open` SHALL return the session id, the output read so far, whether the process is still running, and its exit code once it has exited. An unknown session id SHALL return an error message, never a crash.

#### Scenario: open, drive, and close an interactive program

- **WHEN** `terminal_open` starts an interactive program (e.g. a REPL), then `terminal_input` sends a line, then `terminal_close` is called
- **THEN** the open result carries a session id and the program's initial output, the input result carries the program's response to that line, and close returns the exit code

#### Scenario: unknown session id is reported

- **WHEN** `terminal_input` / `terminal_read` / `terminal_close` names a session id that is not in the registry
- **THEN** it returns an error message naming the missing session, and nothing crashes

### Requirement: One PTY implementation covers local, device, and SSH

A single PTY-session implementation SHALL back all three targets. The child process is the local shell/command, or — when an SSH host is given — `ssh -tt <host> <command>` (SSH is an argument-vector variant, not a separate backend). Because the terminal tools live in the shared tool layer, a device runs the very same tools via the existing on-device dispatch, and the daemon process's registry persists the session across calls. The SSH argument-vector construction SHALL be a pure function.

#### Scenario: SSH session uses a PTY over ssh

- **WHEN** `terminal_open` is given an `ssh_host`
- **THEN** the child is a forced-PTY ssh invocation (`ssh -tt <host> <command>`), driven by the same input/read/close tools

#### Scenario: device session persists across calls

- **WHEN** a terminal session is opened on a device and a later `terminal_input` targets it
- **THEN** the daemon process drives the same persisted session (the registry is process-global), with no protocol change

### Requirement: Each turn reads until the output goes quiet

After `terminal_open` / `terminal_input`, the tool SHALL accumulate PTY output and return for that turn when any of these holds: the **quiet gap** — time since the last output byte, or since the turn started if none has arrived yet — reaches a configured interval; a configured per-turn maximum window has elapsed; or the child has exited. Measuring the quiet gap from the turn start (when no output yet) gives output time to begin, so a turn does not return instantly empty right after input. This stop decision SHALL be a pure function of the timings and exit state. The returned `output` SHALL have ANSI control sequences stripped for readability, with the raw bytes also available. (Output that arrives after a turn returns is captured by the next `terminal_read`.)

##### Example: stop-reading decision

| quiet gap (since last output, or turn start) | total this turn | child exited | stop |
|---|---|---|---|
| past quiet window | within max window | no | yes (gone quiet) |
| within quiet window | past max window | no | yes (hit max) |
| within quiet window | within max window | yes | yes (child done) |
| within quiet window | within max window | no | no (keep reading) |

#### Scenario: ANSI is stripped for the agent, raw kept

- **WHEN** a program emits ANSI escape sequences
- **THEN** the `output` field is the human-readable text with escapes removed, and `raw_output` carries the original bytes

### Requirement: Sessions are bounded and never crash the host

The number of concurrent sessions SHALL be capped (configurable); opening beyond the cap SHALL return an error rather than leak resources. Idle sessions SHALL be reclaimed after a configurable time-to-live. Any PTY, spawn, read, or write failure SHALL surface as an error message; a failure in one session SHALL NOT crash the server or daemon, nor affect other sessions.

#### Scenario: the session cap is enforced

- **WHEN** the number of open sessions is at the configured cap and another `terminal_open` is attempted
- **THEN** it returns an error naming the cap, and no new session is created

#### Scenario: an idle session is reclaimed

- **WHEN** a session has been idle past its time-to-live and another session is opened
- **THEN** the idle session is closed and its resources are released
