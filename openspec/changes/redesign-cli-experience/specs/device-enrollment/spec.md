## MODIFIED Requirements

### Requirement: Guided first-run init discovers, picks, and pairs

When `fleety init` runs without a URL on a TTY and mDNS is enabled, the CLI SHALL scan the LAN with collecting discovery, present every found Server as a numbered list, and let the user pick one by number. Empty input SHALL pick the first entry; an out-of-range or non-numeric choice SHALL re-prompt. The profile name SHALL default to the display name unless `--name` overrides it. A picked URL that equals the saved profile URL SHALL retain that profile's credential. A picked URL that differs from a credentialed profile with the same name SHALL NOT receive the old token or change `connections.toml` unless the user supplies a non-empty pairing code and that endpoint returns a newly minted token; the URL, token, fingerprint, and current selection SHALL then commit atomically. Empty pairing input on such a changed endpoint SHALL fail with explicit re-pair guidance and leave the saved profile byte-identical. When discovery finds nothing, or stdout is not a TTY, or mDNS is disabled, `fleety init` SHALL print explicit-URL usage guidance. `fleety init <ws-url>` SHALL apply the same endpoint-change credential boundary without entering the picker.

#### Scenario: pick and pair in one flow

- **WHEN** `fleety init` runs on a TTY, the scan lists one Server, and the user picks it and enters a valid pairing code
- **THEN** the profile SHALL be saved as current with the newly minted token and observed fingerprint

#### Scenario: skipping the code leaves a new unpaired current profile

- **GIVEN** the selected profile name has no stored credential
- **WHEN** the user picks the Server and leaves the pairing-code prompt empty
- **THEN** the profile SHALL be saved as current without a token and the CLI SHALL print how to pair later

#### Scenario: re-running init on the same saved endpoint keeps its token

- **WHEN** the picked Server's profile name and URL equal a saved credentialed profile
- **THEN** the profile SHALL keep its token and become current

##### Example: same office endpoint

- **GIVEN** profile `office` stores URL `ws://office:8787` and token `office-token`
- **WHEN** init selects `ws://office:8787` as `office`
- **THEN** the Hello SHALL carry `office-token` and the saved credential SHALL remain associated with that URL

#### Scenario: re-running init on a changed saved endpoint requires re-pair

- **GIVEN** profile `office` stores URL `ws://old:8787` and token `old-token`
- **WHEN** guided or explicit init selects `ws://new:8787` as `office`
- **THEN** the CLI SHALL send neither `old-token` nor a profile mutation until a supplied pairing code mints a new token

#### Scenario: nothing found falls back to usage

- **WHEN** the scan window ends with no Server discovered
- **THEN** the CLI SHALL say no Server was found and print explicit-URL usage guidance

##### Example: empty LAN scan

- **GIVEN** no local or LAN Server answers during the bounded scan
- **WHEN** guided init completes discovery
- **THEN** it SHALL make no profile mutation and SHALL print `fleety init <ws-url> --name <name> --pairing-code <code>`

#### Scenario: explicit URL skips the picker

- **WHEN** `fleety init ws://host:8787` runs
- **THEN** the CLI SHALL validate and connect to that explicit URL without running guided discovery
