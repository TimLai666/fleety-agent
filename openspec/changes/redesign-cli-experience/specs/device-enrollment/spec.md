## MODIFIED Requirements

### Requirement: Guided first-run init discovers, picks, and pairs

When `fleety init` runs without a URL on a TTY and mDNS is enabled, the CLI SHALL scan the LAN with collecting discovery, present every found Server as a numbered list, and let the user pick one by number. Discovery alone SHALL NOT create an operational session. Empty input SHALL pick the first entry; an out-of-range or non-numeric choice SHALL re-prompt. The profile name SHALL default to the display name unless `--name` overrides it. Only the selected endpoint SHALL be contacted for enrollment. A same-host loopback pick SHALL skip pairing. A LAN pick SHALL reuse a non-empty credential only when the selected profile name and URL exactly match the saved profile; every other LAN pick SHALL require a non-empty pairing code and a `Welcome` carrying a newly minted token before the URL, token, fingerprint, or current selection is persisted. Empty pairing input SHALL fail before connecting and leave `connections.toml` byte-identical. A picked URL that differs from a credentialed profile with the same name SHALL NOT receive the old token. When discovery finds nothing, or stdout is not a TTY, or mDNS is disabled, `fleety init` SHALL print explicit-URL usage guidance. `fleety init <ws-url>` SHALL apply the endpoint-change credential boundary without entering the picker. When it includes `--pairing-code`, it SHALL send neither an existing saved token nor pin and SHALL replace both only after receiving newly minted complete credentials against the unchanged saved generation.

#### Scenario: pick and pair in one flow

- **WHEN** `fleety init` runs on a TTY, the scan lists one Server, and the user picks it and enters a valid pairing code
- **THEN** the profile SHALL be saved as current with the newly minted token and observed fingerprint

#### Scenario: unselected advertisers remain discovery-only

- **WHEN** guided init displays multiple mDNS candidates and the user selects one
- **THEN** every unselected candidate SHALL receive no `Hello`, token, pairing code, profile mutation, or control authority

#### Scenario: a new LAN pick cannot skip pairing

- **GIVEN** the selected profile name has no stored credential
- **WHEN** the user picks the Server and leaves the pairing-code prompt empty
- **THEN** the CLI SHALL fail before sending `Hello` and SHALL NOT save or select the profile

#### Scenario: pairing acknowledgement must mint a credential

- **GIVEN** a new LAN candidate was explicitly selected and a pairing code was supplied
- **WHEN** the candidate replies with `Welcome` but no newly minted token
- **THEN** enrollment SHALL fail and `connections.toml` SHALL remain byte-identical

#### Scenario: same-host loopback remains frictionless

- **WHEN** guided init selects the locally probed loopback Server
- **THEN** it SHALL enroll without a pairing code because the transport peer is same-host trusted

##### Example: local default selection

- **GIVEN** a Server answers at `ws://127.0.0.1:8787`
- **WHEN** guided init selects that locally probed entry
- **THEN** the CLI SHALL send no pairing code, save the verified `local` endpoint, and make it current

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
