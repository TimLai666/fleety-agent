## Summary

Make a bare-metal server reachable across devices out of the box: default `FLEETY_ADDR` to `0.0.0.0:8787`, and fix mDNS so a `0.0.0.0` bind still advertises a routable IP (auto-detected) instead of going silent.

## Motivation

The server's listen address defaults to `127.0.0.1:8787`, so a fresh bare-metal server is unreachable from other devices until the operator finds and sets `FLEETY_ADDR=0.0.0.0:8787` — the opposite of Fleety's cross-device promise. The Docker image already sets `0.0.0.0`; the bare-metal default should match. This was a blocking product decision in `docs/roadmap.md` §待決策略, now resolved: with `auth-default-on` shipped (`FLEETY_REQUIRE_AUTH` defaults on) and first-run pairing guidance, "exposed listen address + connection requires auth + first run prints a pairing code" line up, so exposing the address is safe.

The catch (found by reading `mdns.rs`): today, when the server binds `0.0.0.0` it does not know which interface IP to advertise, so `local_ips` returns empty and mDNS registration is **skipped** unless the operator sets `FLEETY_MDNS_HOST_IP`. Flipping the default to `0.0.0.0` without addressing this would silence auto-discovery by default — the server would run but peers could not find it. So the two changes must land together.

## Proposed Solution

- **Default `FLEETY_ADDR` to `0.0.0.0:8787`** in the typed registry and in the server's own env fallback, with the description noting the address is exposed and connections require auth by default.
- **Auto-detect a routable IP for mDNS** when the bind address is unspecified (`0.0.0.0`) and `FLEETY_MDNS_HOST_IP` is unset: open a UDP socket and `connect` it to a public IP (e.g. `8.8.8.8:80`) — no packet is sent, but the OS picks the local IP of the outbound interface, which is read from the socket's `local_addr`. A loopback/unspecified result is discarded. `FLEETY_MDNS_HOST_IP` still overrides (multi-homed hosts). When detection fails, `local_ips` returns empty and advertisement is skipped as before.
- The **loopback warning** in the server startup path is kept — it now fires only when the operator explicitly binds `127.0.0.1`/`localhost`, which is the correct signal.

## Non-Goals (optional)

- An mDNS "pick from the discovered servers" interactive menu for first pairing (a separate feature; discovery still auto-uses the first server found).
- Any `wss`/TLS transport requirement.
- Removing the now-redundant explicit `0.0.0.0` from the Docker image (harmless; left alone to avoid scope creep).

## Alternatives Considered (optional)

- **Flip the default only, leave mDNS as-is.** Rejected: the server would listen on all interfaces but stop advertising by default, so cross-device auto-discovery would silently fail — defeating the point of the change.
- **Enumerate all interfaces (a networking dep).** Rejected: the UDP-connect trick is zero-dependency and picks the single outbound-route IP, which is what a peer needs; multi-homed edge cases keep the `FLEETY_MDNS_HOST_IP` override.

## Impact

- Affected specs: `runtime-configuration` (modified — `FLEETY_ADDR` default `127.0.0.1:8787` → `0.0.0.0:8787`), `service-discovery` (modified — a `0.0.0.0` bind auto-detects a routable advertise IP, with `FLEETY_MDNS_HOST_IP` as an override rather than a requirement)
- Affected code:
  - New: (none)
  - Modified: crates/fleety-tools/src/config.rs (registry FLEETY_ADDR default + description), crates/fleety-server/src/main.rs (FLEETY_ADDR fallback default), crates/fleety-server/src/mdns.rs (local_ips auto-detects a routable IP when bound to 0.0.0.0 and no host IP is pinned), docs/env.md, docs/roadmap.md
  - Removed: (none)
