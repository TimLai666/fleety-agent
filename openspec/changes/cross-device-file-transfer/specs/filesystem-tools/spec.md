## ADDED Requirements

### Requirement: Byte-level file read and write for binary and cross-device transfer

The workspace tools SHALL include `read_file_bytes` and `write_file_bytes` for byte-accurate file access, so binary files (and cross-device transfer) work where the UTF-8 `read_file`/`write_file` cannot. `read_file_bytes` SHALL return the file's bytes base64-encoded plus a SHA-256 and the byte length; `write_file_bytes` SHALL decode base64 content and write it, returning a SHA-256 and byte length. Both SHALL reuse the existing workspace path resolution and sensitive-path guard, and `write_file_bytes` SHALL back up an existing target before overwriting like `write_file`. Both SHALL enforce a size ceiling from `FLEETY_TRANSFER_MAX_BYTES` (default 64 MiB): a file or payload larger than the ceiling SHALL be rejected with an error naming the actual size and the limit, and nothing SHALL be read into a result or written. The byte read/write logic SHALL be a single shared implementation (used by the tools and by the server's transfer relay).

#### Scenario: binary round-trips byte-exact

- **WHEN** `read_file_bytes` reads a binary file and its base64 content is passed to `write_file_bytes` at another path
- **THEN** the written file's bytes equal the original and the two SHA-256 values match

#### Scenario: oversize is rejected without side effects

- **WHEN** a file exceeds `FLEETY_TRANSFER_MAX_BYTES` on read, or a payload exceeds it on write
- **THEN** the operation fails with an error naming the size and the limit, and no partial result is returned or file written

#### Scenario: sensitive paths stay guarded

- **WHEN** `write_file_bytes` targets a sensitive path the text `write_file` would refuse
- **THEN** it is refused the same way
