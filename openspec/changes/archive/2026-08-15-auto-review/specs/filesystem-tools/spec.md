## MODIFIED Requirements

### Requirement: Filesystem scope and sensitive-path guard

By default the file tools SHALL resolve relative paths against the configured root and SHALL allow absolute paths and paths outside the root (audited and rollback-backed). When `FLEETY_FS_SCOPE=workspace` is set, the tools SHALL confine every path to the root and reject `..`, absolute, and symlink-escaping paths. Under `full_access` or `require_approval`, the tools SHALL refuse mutation of critical paths such as SSH keys/config, `/etc/shadow`, `/dev`, Windows system directories, and similar targets. Under `auto_review`, the tools SHALL emit a trusted sensitive-path danger signal and SHALL defer the mutation decision to the auto reviewer. Reads SHALL NOT be restricted by the sensitive-path guard.

#### Scenario: default policy refuses a sensitive write

- **WHEN** `write_file` targets an SSH `authorized_keys` path under `full_access`
- **THEN** the call is refused with an actionable critical-path error

#### Scenario: auto review evaluates a sensitive write

- **WHEN** `write_file` targets an SSH `authorized_keys` path under `auto_review`
- **THEN** the reviewer receives a sensitive-path danger signal and the write executes only after reviewer approval

#### Scenario: workspace scope still confines paths

- **WHEN** `FLEETY_FS_SCOPE=workspace` and a candidate path escapes the workspace through an absolute path, `..`, or a symlink
- **THEN** the path is rejected before auto review because it violates the filesystem scope boundary
