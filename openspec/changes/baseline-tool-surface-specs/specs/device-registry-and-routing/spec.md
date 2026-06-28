## ADDED Requirements

### Requirement: Register devices and sites

The system SHALL provide `device_list`, `device_show`, `device_set_site`, `device_set_mobility`, `site_set`, `site_list`, `site_show`, `site_delete`, and `pair_create`. `device_show` SHALL return the device record, its `NOTES`, and the tools it advertised when it last connected. `pair_create` SHALL mint a short-lived pairing code to enroll a new device.

#### Scenario: show a device's advertised tools

- **WHEN** `device_show` is called for a device that advertised its tools at connect time
- **THEN** the result includes that device's record, NOTES, and advertised tool list

### Requirement: Route a tool call to another device

The system SHALL provide `device_exec` that runs a named tool on a connected device by dispatching a `RunTool` frame to that device's daemon and awaiting the reply. When the target device advertised its tools, `device_exec` SHALL strict-check the requested `tool` against that advertised list. Handles a device returns (sessions, pids, ports) SHALL be bound to that device and SHALL be rejected if used against another device.

#### Scenario: reject a foreign handle

- **WHEN** a handle returned by device A is used in a `device_exec` call targeting device B
- **THEN** the call is rejected with an actionable error naming the owning device
