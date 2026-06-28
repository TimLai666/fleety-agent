## ADDED Requirements

### Requirement: Periodic retention sweep

The server SHALL run a periodic background sweep that bounds the audit and backup surfaces. `FLEETY_GC_DISABLED` SHALL, when set to any value, skip the loop entirely. `FLEETY_GC_INTERVAL_SECS` SHALL set the sweep cadence (default `21600`, i.e. 6 hours) and SHALL be clamped to a 60-second floor. `FLEETY_BACKUPS_RETENTION_SECS` SHALL set the maximum backup age before deletion (default `604800`, i.e. 7 days). `FLEETY_HISTORY_ROTATE_BYTES` SHALL set the size at which a device's `history.jsonl` is rotated to an archive and reset (default `33554432`, i.e. 32 MiB).

#### Scenario: sweep deletes aged backups and rotates oversized history

- **WHEN** a sweep runs with defaults and a backup directory is older than 7 days
- **THEN** that backup directory is deleted
- **WHEN** a device's `history.jsonl` exceeds 32 MiB during a sweep
- **THEN** it is renamed to a timestamped archive and the live file resets

#### Scenario: interval is floored

- **WHEN** `FLEETY_GC_INTERVAL_SECS` is set below 60
- **THEN** the effective cadence is clamped to 60 seconds
