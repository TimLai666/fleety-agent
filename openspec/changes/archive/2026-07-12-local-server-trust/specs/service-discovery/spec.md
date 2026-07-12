## ADDED Requirements

### Requirement: Guided init probes the local server before scanning

Before the LAN scan, guided `fleety init` SHALL probe the local server on loopback with a short timeout and, when it answers, include it as a discovery entry ahead of the mDNS results. The probe SHALL be bounded so a host with no local server is not delayed noticeably, and SHALL never error the init flow — a failed or timed-out probe simply omits the local entry.

#### Scenario: local entry precedes mDNS results

- **WHEN** a local server answers the loopback probe and mDNS also finds LAN servers
- **THEN** the local entry appears first in the picker, ahead of the discovered LAN servers

#### Scenario: a failed probe never blocks discovery

- **WHEN** the loopback probe times out or errors
- **THEN** no local entry is added and the mDNS scan proceeds normally
