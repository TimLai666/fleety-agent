## ADDED Requirements

### Requirement: Windows lifecycle verbs pre-flight an elevation check

On Windows, before invoking the Service Control Manager for any state-changing lifecycle verb (install, uninstall, start, stop, restart, enable, disable, up, down), the CLI SHALL detect whether the current process is running elevated (administrator). When it is not elevated, the CLI SHALL abort before issuing any `sc` command — leaving no partial service state — and SHALL print an actionable message on stderr telling the user to re-run the command from an elevated (Administrator) terminal. The elevation detection SHALL NOT use `unsafe` code and SHALL NOT add a new crate dependency. Query-only verbs (status) SHALL NOT require elevation and SHALL NOT perform the check.

#### Scenario: install without admin aborts before touching the SCM

- **WHEN** `install` or `up` runs on Windows without administrator rights
- **THEN** the CLI detects the missing elevation, exits non-zero before running any `sc` command, and prints a message on stderr telling the user to re-run it from an elevated terminal

#### Scenario: elevated run proceeds

- **WHEN** a state-changing lifecycle verb runs on Windows in an elevated terminal
- **THEN** the elevation check passes and the verb proceeds to invoke the Service Control Manager

#### Scenario: status needs no elevation

- **WHEN** `status` runs on Windows without administrator rights
- **THEN** the CLI performs no elevation check and reports the service status normally
