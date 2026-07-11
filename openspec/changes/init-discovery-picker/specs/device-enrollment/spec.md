## ADDED Requirements

### Requirement: Guided first-run init discovers, picks, and pairs

When `fleety init` runs without a URL on a TTY (and mDNS is not disabled), the CLI SHALL scan the LAN with the collecting discovery, present the found servers as a numbered list (display name, URL, and a marker on entries whose URL already exists in a saved profile), and let the user pick one by number. Empty input or end-of-input SHALL cancel; an out-of-range or non-numeric choice SHALL re-prompt. The picked server SHALL be saved through the existing profile upsert (profile name defaulting to the display name, `--name` overriding; an existing profile of that name keeps its token) and made current. The CLI SHALL then prompt for a pairing code, naming where codes come from (the server's first-run console line, or `pair_create` on an already-paired device): a non-empty code runs the existing pairing flow and reports the profile as paired and current; empty input skips pairing and prints how to pair later. A pairing failure SHALL leave the saved profile in place. When the scan finds nothing, or stdout is not a TTY, or mDNS is disabled, `fleety init` SHALL print the existing usage guidance instead of entering the interactive flow; `fleety init <ws-url>` SHALL behave exactly as before.

#### Scenario: pick and pair in one flow

- **WHEN** `fleety init` runs on a TTY, the scan lists one server, the user picks it and enters a valid pairing code
- **THEN** the profile is saved and current, the device is paired, and the CLI reports both

#### Scenario: skipping the code leaves an unpaired current profile

- **WHEN** the user picks a server and presses Enter at the pairing-code prompt
- **THEN** the profile is saved and current, and the CLI prints how to pair later with `fleety pair`

#### Scenario: re-running init on a saved server keeps its token

- **WHEN** the picked server's profile name already exists with a stored token
- **THEN** the upsert updates its URL, keeps the token, and marks it current

#### Scenario: nothing found falls back to usage

- **WHEN** the scan window ends with no server discovered
- **THEN** the CLI says no server was found and prints the existing usage guidance with the explicit-URL form

#### Scenario: explicit URL is unchanged

- **WHEN** `fleety init ws://host:8787` runs
- **THEN** the behavior and messages are identical to before the guided flow existed
