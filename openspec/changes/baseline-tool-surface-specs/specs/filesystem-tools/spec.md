## ADDED Requirements

### Requirement: Read and inspect workspace files

The system SHALL provide `read_file`, `list_dir`, and `search_files` tools. `read_file` SHALL return the raw `content`, a line-numbered `numbered` view, and `line_count`, and SHALL accept optional 1-based `start_line`/`end_line` to return a slice. `list_dir` SHALL list a directory (default `.`). `search_files` SHALL run a ripgrep search that respects `.gitignore` and skips binaries.

#### Scenario: read a slice with line numbers

- **WHEN** `read_file` is called with `start_line=2` and `end_line=3` on a file of 5 lines
- **THEN** `content` contains only lines 2-3, `numbered` shows those lines prefixed with their 1-based numbers, and `line_count` is 5

### Requirement: Mutate files with backup and rollback

The system SHALL provide `write_file`, `edit_file`, `delete_file`, `move_file`, `make_dir`, and `rollback`. Every mutating call SHALL first copy the prior content into a managed backup store outside the edited directory and return a `backup` with an `id`. `rollback` SHALL restore a file from such a `backup_id`. `edit_file` SHALL support a substring mode (unique `old` → `new`) and a line-range mode (`start_line`..`end_line` → `new`), and SHALL return a unified `diff` plus an `applied` line-numbered view of the changed region.

#### Scenario: edit then roll back

- **WHEN** `edit_file` replaces a unique substring and a later `rollback` is called with the returned `backup.id`
- **THEN** the edit returns a `diff` and `applied` region, and the rollback restores the file to its pre-edit content

### Requirement: Filesystem scope and sensitive-path guard

By default the file tools SHALL resolve relative paths against the configured root and SHALL allow absolute paths and paths outside the root (audited and rollback-backed). When `FLEETY_FS_SCOPE=workspace` is set, the tools SHALL confine every path to the root and reject `..`, absolute, and symlink-escaping paths. Regardless of scope, the tools SHALL refuse **mutation** of critical paths (SSH keys/config, `/etc/shadow`, `/dev`, Windows system directories, and similar) with an actionable error; reads SHALL NOT be restricted by the sensitive-path guard.

#### Scenario: refuse a sensitive write but allow an outside-root write

- **WHEN** `write_file` targets an absolute path outside the root that is not sensitive
- **THEN** the write succeeds and is backed up
- **WHEN** `write_file` targets a sensitive path such as an SSH `authorized_keys`
- **THEN** the call is refused with an actionable critical-path error
