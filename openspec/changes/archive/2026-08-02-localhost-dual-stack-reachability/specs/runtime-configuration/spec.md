## MODIFIED Requirements

### Requirement: Server bootstrap configuration

The server SHALL read `FLEETY_ADDR` for its WebSocket listen address (default `0.0.0.0:8787`), `FLEETY_AGENT_HOME` for its durable store root (default `$HOME/.fleety/agent`), `FLEETY_WORKSPACE` for the base directory that workspace tools resolve relative paths against (default the current working directory), and `FLEETY_SCHED_TICK` for the scheduler fire-loop interval in seconds (default `60`). Any unset variable SHALL use its default. The `FLEETY_ADDR` default exposes the server on all interfaces so it is reachable across devices out of the box; this is paired with authentication being required by default (see access policy), so an exposed address still needs a paired token to connect. An operator who wants loopback-only SHALL set `FLEETY_ADDR=127.0.0.1:8787` explicitly.

When the configured listen address is the IPv4 wildcard (`0.0.0.0`) or the IPv4 loopback (`127.0.0.1`), the server SHALL additionally attempt to bind a best-effort IPv6 companion listener on the same port (`[::]` and `[::1]` respectively), because on a dual-stack host a client that spells the endpoint `localhost` resolves to `::1` first and pays a multi-second fallback when nothing listens there. A companion bind failure SHALL NOT fail startup: the server SHALL continue IPv4-only and log that the companion was not established. Any other explicitly configured address SHALL be bound exactly as given, with no companion.

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

#### Scenario: both address families reach a default server

- **WHEN** the server starts with `FLEETY_ADDR` unset (or set to a `0.0.0.0` or `127.0.0.1` address) on a host where IPv6 is available
- **THEN** connections to both the IPv4 address and its IPv6 companion (`::` / `::1`) on the same port are accepted immediately

#### Scenario: a failed companion bind degrades to IPv4-only

- **WHEN** the IPv6 companion cannot be bound (no IPv6, or the port is taken on IPv6)
- **THEN** the server starts and serves IPv4 exactly as before, and logs that the companion listener was not established

#### Scenario: an explicit address is bound exactly

- **WHEN** `FLEETY_ADDR` names any address other than a `0.0.0.0` or `127.0.0.1` form
- **THEN** the server binds exactly that address and starts no companion listener
