## ADDED Requirements

### Requirement: Login fails fast on an unavailable loopback port

Because the Codex OAuth redirect URI is registered to a fixed loopback port, `fleety auth login` SHALL check that the port is available before opening the browser, and when it is already in use SHALL abort with an actionable message that states the fixed-port constraint and how to resolve it (free the port or close a stuck prior login, then retry), instead of sending the user through authorization only to fail at the redirect. The check SHALL NOT print token values and SHALL leave any existing stored tokens untouched.

#### Scenario: busy port aborts before the browser opens

- **WHEN** the fixed OAuth loopback port is already in use and the user runs login
- **THEN** the CLI aborts before opening the browser with an actionable message that explains the fixed-port requirement and how to free the port

#### Scenario: free port proceeds normally

- **WHEN** the fixed OAuth loopback port is available
- **THEN** login opens the browser and captures the authorization code as before
