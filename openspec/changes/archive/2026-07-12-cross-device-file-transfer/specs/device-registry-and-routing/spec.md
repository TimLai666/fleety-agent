## ADDED Requirements

### Requirement: transfer_file relays a file between two endpoints

The server SHALL provide a `transfer_file` tool that copies a single file from a source endpoint to a destination endpoint, where each endpoint is either a connected device (by `device_id`) or the server itself (`server`). It SHALL read the source bytes (from a device via the existing on-device dispatch of `read_file_bytes`, or from the server via the shared byte helper) and write them to the destination (via `write_file_bytes` on a device, or the shared helper on the server), then compare the source and destination SHA-256: a mismatch SHALL be reported as a corrupted transfer and NOT treated as success. On success it SHALL return the byte count and SHA-256. A destination device that has not advertised `write_file_bytes` (an older daemon) SHALL yield the existing "did not advertise" error, and a disconnected device the existing "not connected" error. The write SHALL back up an existing destination file, so a mismatched transfer is recoverable via rollback.

#### Scenario: device-to-device transfer verifies integrity

- **WHEN** `transfer_file` copies a file from one connected device to another and both SHA-256 values match
- **THEN** it succeeds and returns the byte count and SHA-256

#### Scenario: server is a valid endpoint in either direction

- **WHEN** `transfer_file` uses `server` as the source or the destination
- **THEN** it reads from / writes to the server's workspace via the shared byte helper, transferring to / from the other endpoint

#### Scenario: a corrupted relay is not a success

- **WHEN** the destination SHA-256 does not match the source
- **THEN** the tool reports a corrupted transfer rather than success, and the backed-up destination can be rolled back
