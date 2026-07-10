## MODIFIED Requirements

### Requirement: Server bootstrap configuration

The server SHALL read `FLEETY_ADDR` for its WebSocket listen address (default `0.0.0.0:8787`), `FLEETY_AGENT_HOME` for its durable store root (default `$HOME/.fleety/agent`), `FLEETY_WORKSPACE` for the base directory that workspace tools resolve relative paths against (default the current working directory), and `FLEETY_SCHED_TICK` for the scheduler fire-loop interval in seconds (default `60`). Any unset variable SHALL use its default. The `FLEETY_ADDR` default exposes the server on all interfaces so it is reachable across devices out of the box; this is paired with authentication being required by default (see access policy), so an exposed address still needs a paired token to connect. An operator who wants loopback-only SHALL set `FLEETY_ADDR=127.0.0.1:8787` explicitly.

#### Scenario: defaults apply when unset

- **WHEN** the server starts with none of these variables set
- **THEN** it listens on `0.0.0.0:8787`, stores under `$HOME/.fleety/agent`, resolves relative paths against the current directory, and ticks the scheduler every 60 seconds

##### Example: bootstrap defaults

| Variable | Unset default |
| -------- | ------------- |
| `FLEETY_ADDR` | `0.0.0.0:8787` |
| `FLEETY_AGENT_HOME` | `$HOME/.fleety/agent` |
| `FLEETY_WORKSPACE` | current working directory |
| `FLEETY_SCHED_TICK` | `60` |
