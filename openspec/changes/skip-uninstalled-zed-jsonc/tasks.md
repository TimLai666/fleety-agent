## 1. Reproduce the update failure

- [x] 1.1 Add a failing ACP unit test for the requirement "The CLI configures editors to launch the agent" using JSONC comments and trailing commas without `agent_servers.Fleety`; verify it currently fails because strict parsing happens before the no-entry decision.

## 2. Fix the refresh decision

- [x] 2.1 Implement the design decision "Detect an installed Fleety entry before strict parsing": add the conservative preflight for the literal `"Fleety"` key and return `Ok(None)` before strict parsing when no installed entry is indicated.
- [x] 2.2 Implement the design decision "Keep the installed-entry path fail closed": add coverage proving an unparseable settings value that contains a Fleety entry still fails closed, while valid installed entries retain endpoint validation and refresh behavior.

## 3. Verify the contract

- [x] 3.1 Run the focused ACP tests and the CLI test suite, then run formatting and clippy checks required by the design contract.
