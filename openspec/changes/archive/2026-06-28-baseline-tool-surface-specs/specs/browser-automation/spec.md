## ADDED Requirements

### Requirement: Drive a browser over the DevTools Protocol on any device

The system SHALL provide `browser_open`, `browser_navigate`, `browser_eval`, `browser_screenshot`, and `browser_close` that drive a Chrome over CDP. These tools SHALL be registered on the server and on every device daemon, so `device_exec` can target a specific device's browser. `browser_screenshot` SHALL be a read-risk observation; the others SHALL be mutate-risk.

#### Scenario: screenshot a device's browser via device_exec

- **WHEN** `device_exec(device, tool="browser_screenshot")` is invoked for an online device
- **THEN** the screenshot is captured on that device and returned as a base64 PNG

### Requirement: Auto-provision a local Chrome

When the local CDP endpoint is not reachable, the system SHALL ensure one: use a reachable endpoint, else launch an installed Chrome/Chromium headless on the port, else (when auto-install is enabled) install via the OS package manager or a chrome-for-testing download. Remote (non-loopback) endpoints SHALL NOT be auto-provisioned.

#### Scenario: launch a local Chrome when none is running

- **WHEN** a browser tool targets `http://127.0.0.1:9222` and nothing is listening
- **THEN** the runtime launches a headless Chrome on that port before connecting
