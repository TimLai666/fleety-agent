## ADDED Requirements

### Requirement: Connection is the canonical profile command group

The CLI SHALL expose `connection` as the canonical command group for named Server profiles. The existing `server` group SHALL remain an alias that maps to the same parsed command and persistence implementation.

#### Scenario: legacy server command remains compatible

- **WHEN** the user runs `fleety server list`
- **THEN** it SHALL return the same profiles, ordering, current marker, and exit status as `fleety connection list`

### Requirement: Interactive profile switching is explicit and observable

The workspace SHALL provide a profile picker that shows name, label, endpoint, current marker, reachability when known, and transient override state. Selecting a profile SHALL not enable remote actions until connection and snapshot refresh have completed.

#### Scenario: profile becomes usable only after refresh

- **WHEN** the user selects a reachable profile
- **THEN** the workspace SHALL show Connecting, then load Server and Daemon owner states, and only then enable their mutations

##### Example: switch from A to B

- **GIVEN** profile `A` is connected and profile `B` is reachable
- **WHEN** the user selects `B`
- **THEN** remote Apply actions stay disabled until `B` authenticates and both Server and Daemon snapshots have replaced all `A` revisions

### Requirement: Transient endpoints never inherit saved credentials

Raw `--server`／`--url`, `FLEETY_AGENT_URL`, and ACP-installed endpoint overrides SHALL remain transient and SHALL use only a non-empty caller-explicit `FLEETY_TOKEN`. They SHALL NOT search profiles by URL, inherit the current profile token when URLs match, or acquire durable profile ownership. A successful `Welcome`, identity pin, or authentication rejection on a transient target SHALL NOT create, pin, replace, or clear any saved profile. Stored credentials SHALL be available only when resolution selects an explicit named profile or the persisted current profile without a raw endpoint override.

A caller-explicit client token SHALL mean a non-empty value present in the `fleety`／`fleetyd` process environment. A Server-owned `FLEETY_TOKEN` persisted in `config.toml` SHALL be loaded only by `fleety-server` and SHALL NOT be seeded into CLI, Daemon, ACP, or raw-endpoint resolution.

#### Scenario: same-URL profiles cannot influence a raw target

- **GIVEN** profiles `A` and `B` both store URL `ws://same:8787`, with tokens `token-a` and `token-b`
- **WHEN** a raw URL or environment target resolves `ws://same:8787` without `FLEETY_TOKEN`
- **THEN** the resolved token SHALL be absent regardless of profile name order or current selection

##### Example: raw and named provenance matrix

| Target | Current | Explicit token | Expected `Hello.token` |
| --- | --- | --- | --- |
| `--server ws://same:8787` | `A` | none | none |
| `--url ws://same:8787` | `B` | none | none |
| `--server ws://same:8787` | `A` | empty string | none |
| `FLEETY_AGENT_URL=ws://same:8787` | `A` | `caller-token` | `caller-token` |
| `--profile B` | `A` | none | `token-b` |

#### Scenario: transient daemon and ACP sessions cannot mutate a matching profile

- **GIVEN** ACP or fleetyd receives `FLEETY_AGENT_URL=ws://same:8787` while saved profiles at that URL contain credentials
- **WHEN** the transient endpoint returns `Welcome` or rejects authentication
- **THEN** every saved profile SHALL remain byte-identical and no profile SHALL be selected by URL equality

#### Scenario: Server bootstrap configuration is not client input

- **GIVEN** Server scope in `config.toml` contains `FLEETY_TOKEN=bootstrap-secret`
- **AND** the CLI or Daemon process has no explicit `FLEETY_TOKEN`
- **WHEN** it resolves a raw or environment endpoint
- **THEN** `Hello` SHALL carry no token and the bootstrap secret SHALL remain Server-owned

#### Scenario: ACP authenticates before forwarding editor content

- **GIVEN** an ACP editor submits a prompt, resume, or cancellation
- **WHEN** the endpoint has not returned an authenticated `Welcome`, or sends a control frame first
- **THEN** ACP SHALL send no editor content or control request and SHALL close the unauthenticated session

#### Scenario: ACP never converts a partial turn into success

- **GIVEN** an authenticated ACP turn has emitted partial assistant output or invoked an editor tool
- **WHEN** the Server closes or a required tool／approval reply cannot be delivered before `Done`
- **THEN** ACP SHALL fail the turn and SHALL NOT return `end_turn`

#### Scenario: ACP tokens stay bound to one transient endpoint

- **GIVEN** an installed ACP editor entry contains `FLEETY_AGENT_URL=A` and `FLEETY_TOKEN=token-a`
- **WHEN** the installer changes the endpoint to `B` or removes the endpoint to resume saved-profile resolution
- **THEN** it SHALL remove `token-a`, preserve unrelated editor environment values, and fail non-zero without overwriting a concurrently recreated settings path when read, backup, comparison, or no-clobber publication fails
- **AND** when canonical restoration is unsafe, the displaced owner-only bytes SHALL remain at a reported unique recovery path
- **AND** every failed restoration attempt SHALL report the retained recovery path
- **AND** a successful canonical restoration followed only by recovery cleanup failure SHALL report the canonical settings as active and the retained recovery path
- **AND** a failed publication whose private temp cleanup also fails SHALL report the retained temp path
- **AND** when canonical publication succeeds but temp／recovery cleanup fails, it SHALL report success with every retained private path and SHALL NOT claim settings were unchanged

#### Scenario: pairing and pinning require the frozen profile generation

- **GIVEN** a named／current profile resolves and begins a pairing or authenticated TOFU handshake
- **WHEN** that profile switches current owner, changes any stored field, or is deleted and recreated before the reply commits
- **THEN** the reply SHALL NOT write a token or fingerprint, even when the replacement keeps the same name and URL

#### Scenario: pairing notification uses current-at-commit

- **GIVEN** an explicitly named profile changes current selection while pairing is in flight
- **WHEN** the owner-leased credential commit succeeds
- **THEN** the same commit SHALL report whether the profile is current and that result alone SHALL decide whether the Daemon reconnect is required

#### Scenario: profile lifecycle generation survives visible-field ABA

- **GIVEN** a saved profile has a persisted lifecycle generation and an in-flight authenticated handshake
- **WHEN** another process deletes and recreates the same name, URL, token, label, and fingerprint
- **THEN** the recreated profile SHALL receive a different generation and the old handshake SHALL fail owner validation

#### Scenario: pairing publication failure is recoverable without reusing the code

- **GIVEN** pair or explicit init has atomically published a complete new token and fingerprint
- **WHEN** the final file or directory sync fails
- **THEN** the CLI SHALL retry publication sync under the owner lease only while the canonical profile still matches the committed generation, still attempt to notify the current Daemon, and report owner replacement or the visible partial state without directing the user to redeem the one-time code again

#### Scenario: incomplete pairing Welcome cannot persist

- **GIVEN** `fleety pair` is connected to a saved profile
- **WHEN** `Welcome` omits the Server fingerprint or returns an empty／whitespace-only minted token
- **THEN** pairing SHALL fail with actionable guidance and the profile SHALL remain byte-identical

#### Scenario: explicit transport token does not replace reconnect owner provenance

- **GIVEN** current profile `B` stores its own token and `fleetyd` has a non-empty `FLEETY_TOKEN`
- **WHEN** an owner-requested reconnect resolves and authenticates `B`
- **THEN** `Hello` SHALL send the explicit transport token while drift checks compare the frozen disk profile, and a valid unchanged owner SHALL complete reconnect

### Requirement: Automatic discovery never borrows another profile's identity

mDNS TXT metadata SHALL be treated as an untrusted discovery hint, not as Server identity proof or an operational connection target. A saved current profile with an explicit endpoint SHALL connect automatically without scanning mDNS. Without such an endpoint, automatic discovery SHALL only populate an explicit selection flow and SHALL NOT attach stored or caller-explicit credentials, create a control session, persist a minted token, or accept control frames. A credentialed profile SHALL NOT adopt or persist an automatically discovered endpoint change until the user explicitly reselects and re-pairs that Server. Explicit `connection set-url` and Settings edits SHALL persist the user-authored URL only after clearing the old token and fingerprint; they SHALL remain uncredentialed until re-pairing succeeds. Re-pairing is an explicit credential-recovery action, not cryptographic endpoint identity proof. Daemon pin, token-clear, and token-persist mutations SHALL target only the exact explicitly selected owner profile.

#### Scenario: current A cannot borrow pinned B

- **GIVEN** profile `A` is current without a URL and profile `B` has a pinned fingerprint and token
- **WHEN** automatic discovery sees `B`
- **THEN** it SHALL NOT send `B`'s token as `A`, pin `B` onto `A`, or mutate either profile's identity implicitly

#### Scenario: copied TXT fingerprint cannot receive a stored token

- **GIVEN** profile `A` has a stored token and fingerprint at endpoint `old`, and an mDNS advertiser at endpoint `new` copies that fingerprint
- **WHEN** automatic discovery or sticky recovery evaluates `new`
- **THEN** it SHALL NOT send `A`'s token to `new`, persist `new`, or report the profile healed; the user SHALL be directed to explicitly reselect and re-pair

#### Scenario: configured current profile reconnects automatically

- **GIVEN** current profile `A` has an explicit saved endpoint and valid credential
- **WHEN** the CLI or Daemon starts or reconnects
- **THEN** it SHALL connect directly to `A` without an mDNS scan or additional selection prompt

#### Scenario: unconfigured discovery cannot become control authority

- **GIVEN** no current profile has an explicit saved endpoint
- **WHEN** automatic mDNS discovers an advertiser
- **THEN** the advertiser SHALL remain a display-only candidate until the user explicitly selects and pairs it

#### Scenario: fresh explicit environment target remains transient

- **GIVEN** no profile exists and an explicit environment endpoint authenticates
- **WHEN** `Welcome` returns a minted token and Server identity
- **THEN** the session SHALL NOT create a default profile or persist either value; the user SHALL explicitly enroll the Server to create durable ownership

#### Scenario: authenticated owner cannot drift during Welcome

- **GIVEN** the Daemon froze owner `A` before `Hello`, including its durable token independently of any caller-explicit token override
- **WHEN** current changes to same-endpoint owner `B`, or `Welcome` omits the Server identity
- **THEN** the Daemon SHALL persist no returned credential, accept no control frame, and SHALL NOT mutate `B`

#### Scenario: control cannot precede authenticated Welcome

- **GIVEN** the Daemon connected to a durable profile endpoint and sent `Hello`
- **WHEN** the endpoint sends `RunTool` before its `Welcome` identity is authenticated
- **THEN** the Daemon SHALL execute no tool, close the session, and settle any pending reconnect as failure

#### Scenario: duplicate Welcome cannot rewrite authenticated credentials

- **GIVEN** the Daemon authenticated one `Welcome` and committed its credential result
- **WHEN** the same session sends another `Welcome`
- **THEN** the Daemon SHALL close the session without applying the duplicate token or identity

#### Scenario: presence waits for authenticated Welcome

- **GIVEN** presence reporting is enabled
- **WHEN** the Daemon has sent `Hello` but has not authenticated `Welcome`
- **THEN** the Daemon SHALL send no co-location fingerprint, subnet, or peer metadata

#### Scenario: empty minted credential is rejected

- **GIVEN** a durable profile already has a valid token
- **WHEN** its endpoint returns `Welcome` with an empty or whitespace-only minted token
- **THEN** the Daemon SHALL preserve the valid token, close the session, execute no control frame, and settle any pending reconnect as failure

### Requirement: Paired profiles roam across authenticated Server endpoints

After authentication, `Welcome` SHALL advertise a bounded list of syntactically valid WebSocket endpoints derived from the Server's listening port and usable non-loopback network interfaces. Fleety SHALL remain network-overlay agnostic and SHALL NOT invoke, install, configure, or require Tailscale or another overlay product. A durable named／current profile SHALL store deduplicated endpoints learned from its authenticated Server session. CLI one-shot connections and Daemon reconnects SHALL try the last successful endpoint first and then the stored candidates. A candidate SHALL become the primary saved URL only after `Welcome` returns the profile's pinned Server identity. Transient raw／environment targets SHALL NOT persist learned endpoints.

#### Scenario: LAN enrollment learns an overlay endpoint without using it

- **GIVEN** the Server listens on `0.0.0.0:8787` with interfaces `192.168.1.20` and `100.64.0.8`
- **WHEN** profile `home` pairs through `ws://192.168.1.20:8787`
- **THEN** authenticated `Welcome` SHALL let `home` store both endpoints without requiring a previous connection through `100.64.0.8`

#### Scenario: leaving LAN reconnects through a learned endpoint

- **GIVEN** `home` last succeeded at `ws://192.168.1.20:8787` and stores learned endpoint `ws://100.64.0.8:8787`
- **WHEN** the LAN endpoint is unreachable and the second endpoint returns the pinned Server identity
- **THEN** CLI and Daemon SHALL connect through `ws://100.64.0.8:8787` and promote it as the primary endpoint

#### Scenario: an unknown later endpoint cannot be inferred

- **GIVEN** the Server exposes a new endpoint only after the client has left every previously reachable network
- **WHEN** the profile has never received or explicitly configured that endpoint
- **THEN** Fleety SHALL NOT claim automatic discovery and SHALL keep retrying only its saved endpoints

#### Scenario: identity mismatch cannot promote a learned endpoint

- **GIVEN** a stored candidate answers with a Server fingerprint different from the profile pin
- **WHEN** CLI or Daemon attempts that candidate
- **THEN** it SHALL reject the session, execute no control action, and SHALL NOT promote or persist credentials from that candidate

### Requirement: Endpoint roaming never widens what a credential reaches

A profile SHALL distinguish addresses by provenance. An address the user configured SHALL be recorded separately and SHALL remain attemptable exactly as stored, whatever its host form. An address a Server advertised SHALL be accepted only as an IP literal that preserves the connected endpoint's scheme, port, path, and query, and SHALL be attempted exactly as stored so an authenticated session is never refused by the client's own ownership checks. Only a session that completed the encrypted handshake SHALL add endpoints to a profile: a fingerprint is public, so a cleartext session that presents the pinned one SHALL be able to promote the endpoint it is already on but SHALL NOT teach the profile new addresses. Pairing SHALL send its pairing code only to the endpoint the user configured, never to one roaming promoted. A deliberate re-pair SHALL clear the learned endpoints and the secure-channel record along with the credential they were earned with, so a profile can always be recovered onto a Server that speaks a different protocol version. A current binary SHALL bind the presence of the learned endpoint list, configured address, and secure-channel latch into the profile's opaque lifecycle generation. If an older writer preserves that generation but removes or changes the bound state, the current binary SHALL reject the profile before transport or credential use and SHALL require every installed Fleety binary to be updated before explicit re-pairing.

#### Scenario: a cleartext session cannot seed addresses

- **GIVEN** a profile that has never observed its Server complete the handshake
- **WHEN** an endpoint answers in the clear with the profile's pinned fingerprint and advertises further endpoints
- **THEN** Fleety SHALL NOT store those endpoints

#### Scenario: pairing follows the configured address

- **GIVEN** roaming has promoted a learned endpoint to be the profile's primary
- **WHEN** the user runs `fleety pair <code>`
- **THEN** the pairing code SHALL be sent to the endpoint the user originally configured, and the profile SHALL return to that endpoint so a Server-advertised address never inherits the standing of one the user chose

#### Scenario: explicit named pairing overrides a transient environment endpoint

- **GIVEN** `FLEETY_AGENT_URL=ws://transient:8787` and saved profile `office`
- **WHEN** the user runs `fleety --profile office pair CODE`
- **THEN** Fleety SHALL redeem `CODE` against `office` without sending its old token and SHALL NOT contact the environment endpoint
- **AND WHEN** the user runs `fleety pair CODE` without a named override
- **THEN** Fleety SHALL reject the ambiguous transient environment target before transport

#### Scenario: legacy devices migrate before explicit pairing

- **GIVEN** an existing device has only legacy `config.json` with a URL and token
- **WHEN** the user runs `fleety pair CODE`
- **THEN** Fleety SHALL migrate the legacy record into the current named profile before resolving the pairing owner
- **AND** the pairing transport SHALL carry `CODE` but SHALL NOT carry the migrated old token

#### Scenario: a Server that cannot key the channel is refused, not downgraded

- **GIVEN** an endpoint answers the handshake by stating it holds no credential for this device
- **WHEN** the client receives that answer
- **THEN** it SHALL fail with re-pair guidance and SHALL NOT retry that endpoint in the clear

#### Scenario: an older writer cannot silently erase the secure latch

- **GIVEN** a current binary persisted profile `home` with a versioned generation bound to a secure-channel latch and learned endpoints
- **WHEN** an older binary rewrites the profile while preserving the opaque generation but omitting `secure`, `configured_url`, or `endpoints`
- **THEN** every current Fleety surface SHALL reject `home` before opening a transport or using its credential and SHALL direct the user to update every binary and explicitly re-pair

### Requirement: A paired profile proves the Server before revealing a credential

A paired profile SHALL open its control channel with a mutually authenticated, encrypted handshake keyed by the per-device token it already holds and bound to the Server identity it is pinned to. The device token SHALL NOT be transmitted, and every subsequent control frame SHALL be encrypted and integrity-protected, including `Welcome`, the advertised endpoint list, configuration values, and credential frames. The negotiated channel version and key handle SHALL be bound into the handshake so a modified advertisement aborts it rather than negotiating a weaker path. Each candidate endpoint SHALL be given one bounded attempt covering transport connect, handshake, `Hello`, authenticated `Welcome`, and identity validation together; a candidate that fails any step SHALL be abandoned in favour of the next without mutating any saved credential, identity pin, endpoint list, or primary URL. An endpoint the profile learned rather than the user configured SHALL NOT fall back to the cleartext path. The endpoint the user configured MAY fall back while its profile has never observed this Server complete the handshake; once observed, the profile SHALL record that support and SHALL refuse the cleartext path thereafter. Pairing, same-host loopback trust, and targets with no saved credential have no pre-shared secret to key a handshake with and remain on the existing path.

Before opening each candidate transport, including every advance after a stalled
or failed candidate, Fleety SHALL revalidate the frozen durable profile owner
against the current connection store. This applies equally when the owner is
write-capable and when `doctor` has reduced it to read-only diagnostic authority.
A concurrent pairing, owner mutation, or generation mismatch SHALL abort the
remaining sweep before another endpoint receives the stale token or identity pin.
Doctor SHALL allow the shared aggregate sweep deadline to cover every candidate
it promises to diagnose. Settings SHALL keep its shorter whole-operation
deadline and SHALL divide that budget across the shared sweep bound rather than
allowing an early candidate to consume it all.

#### Scenario: repeated init proves an existing paired Server first

- **GIVEN** profile `office` stores a token and pinned identity for `ws://office:8787`
- **WHEN** the user runs `fleety init ws://office:8787 --name office` without a pairing code
- **THEN** `init` SHALL resolve `office` as the durable credential owner and attempt the secure handshake before `Hello`
- **AND** the saved token SHALL appear only inside the encrypted control channel
- **AND** a successful secure session SHALL latch secure-channel support on `office`

#### Scenario: repeated init uses one authoritative owner snapshot

- **GIVEN** another Fleety process clears, rotates, or creates profile `office`'s credential while `init` is completing a legacy-generation preflight
- **WHEN** `init` reloads the profile before transport
- **THEN** its target, secure policy, identity pin, `Hello`, and commit preconditions SHALL all derive only from that final frozen snapshot
- **AND** no token or security state from the earlier snapshot SHALL reach a transport or overwrite the newer owner

#### Scenario: a tokenless secure latch remains fail closed

- **GIVEN** profile `office` retains its secure latch and pinned identity after authentication rejection cleared its token
- **WHEN** the user repeats same-URL `init` without a pairing code
- **THEN** `init` SHALL retain `office` as the durable owner and fail before transport because it cannot key the required secure channel
- **AND** it SHALL NOT open a cleartext session, accept a replacement credential, or mutate the profile

#### Scenario: protected connection state cannot move to another URL without pairing

- **GIVEN** profile `office` retains any token, identity pin, secure latch, learned endpoint, or separately configured endpoint
- **WHEN** the user runs `init` for `office` at a different URL without a pairing code
- **THEN** Fleety SHALL reject before transport and direct explicit-URL `init --pairing-code`
- **AND** the old profile SHALL remain byte-identical

#### Scenario: owner drift between candidates aborts the sweep

- **GIVEN** a durable profile has primary endpoint `A`, learned endpoint `B`, and a frozen token／pin owner snapshot
- **WHEN** `A` fails after another process changes that saved profile
- **THEN** Fleety SHALL fail owner revalidation before opening a transport to `B`
- **AND** `B` SHALL receive neither a connection nor any stale credential

#### Scenario: a saved endpoint that cannot prove itself receives nothing

- **GIVEN** profile `home` stores a learned endpoint whose address has since been taken over by another host
- **WHEN** Fleety attempts that endpoint
- **THEN** it SHALL send no device token, SHALL refuse the endpoint when the handshake is not completed, SHALL leave the saved token, identity pin, and endpoint list unchanged, and SHALL continue to the next candidate

#### Scenario: an unauthenticated rejection cannot unpair the device

- **GIVEN** a candidate endpoint answers an attempt with an authentication rejection before proving the Server identity
- **WHEN** the CLI or Daemon observes that rejection
- **THEN** it SHALL treat it as this candidate failing only, SHALL NOT clear the profile token, and SHALL continue to the next candidate

#### Scenario: jamming the handshake cannot win a downgrade

- **GIVEN** profile `home` has previously completed the encrypted handshake with its Server
- **WHEN** a later attempt to any of its endpoints does not complete the handshake
- **THEN** Fleety SHALL fail with actionable guidance instead of connecting in the clear

#### Scenario: an endpoint that stalls does not hide the ones behind it

- **GIVEN** the first candidate accepts the connection and then never completes the handshake
- **WHEN** the per-candidate deadline elapses
- **THEN** Fleety SHALL abandon that candidate and attempt the remaining endpoints before reporting one aggregated failure

#### Scenario: bounded Settings and Doctor still reach a later endpoint

- **GIVEN** a saved primary accepts a connection but stalls and a later saved endpoint can authenticate
- **WHEN** Doctor or Settings starts its bounded candidate sweep
- **THEN** the stalled primary SHALL NOT consume the entire caller deadline
- **AND** the later endpoint SHALL be attempted within that surface's documented aggregate bound

#### Scenario: a proven but silent Server cannot hang diagnostics

- **GIVEN** `fleety doctor` receives an authenticated `Welcome` whose config protocol supports Provider and Daemon snapshots
- **WHEN** the Server does not answer either snapshot request
- **THEN** each request SHALL stop at its diagnostic reply deadline
- **AND** Doctor SHALL preserve the successful Server check, report the unavailable snapshot checks with remediation, and return
- **AND** after any timeout Doctor SHALL close the request stream and mark later owner checks blocked, because a late uncorrelated reply cannot be safely attributed

#### Scenario: an SSE upstream stall cannot hide a later candidate

- **GIVEN** a candidate opens its SSE downstream but its `Hello` POST never returns
- **WHEN** fleetyd reaches the deadline created before that candidate transport attempt
- **THEN** it SHALL abandon the candidate and continue its ordinary or owner-requested sweep
- **AND** the same deadline SHALL also bound the candidate's authenticated `Welcome`

#### Scenario: diagnostics report which endpoint answered without changing anything

- **GIVEN** a profile whose configured endpoint is unreachable and whose learned endpoint is serving
- **WHEN** `fleety doctor` runs
- **THEN** it SHALL report the Server as reachable and name the redacted endpoint that answered, and SHALL NOT promote it, persist a credential, pin an identity, or record secure-channel support

#### Scenario: diagnostics stop when their frozen profile changes

- **GIVEN** `fleety doctor` holds a read-only snapshot for primary endpoint `A` and alternative `B`
- **WHEN** `A` fails after another process replaces the saved profile
- **THEN** Doctor SHALL fail owner revalidation before opening a transport to `B`
- **AND** it SHALL neither send the old credential nor gain profile mutation authority

### Requirement: Persisted profile switching reconnects the active Daemon

After any operation changes the persisted current profile selection, endpoint, or paired credential, the CLI SHALL notify the running local `fleetyd` through its owner control path. This includes implicit first-profile selection, `connection use`, current-profile rename／set-url, pairing, guided init, Settings URL save, and interactive profile switch. Removing any current profile SHALL fail until the user explicitly selects its replacement; `--force` SHALL NOT choose a replacement from profile ordering. Each request SHALL remain durable through consumption until its terminal result is observed, SHALL NOT be silently overwritten by a later request, and SHALL receive exactly one durable success or failure settlement for its nonce; this does not promise exactly-once transport connection attempts. A later caller SHALL validate any accepted result's success proof before preserving an older terminal active record as a nonce-addressed receipt; every caller SHALL return only the settlement for the nonce it submitted. Success SHALL be settled only after the selected Server sends `Welcome`, authentication completes, its identity matches the saved pin, every returned token and identity pin is atomically persisted and file／publication-synced under the selected profile's owner lease, that lease is released, the accepted journal append reports durability, a nonce-addressed durable success proof exists, and the persisted owner snapshot still matches that committed credential at the settlement boundary. A publication retry SHALL revalidate the frozen committed target and fingerprint; owner drift SHALL settle failure, while a storage error SHALL retain retryable state. A caller SHALL reject an accepted journal record or receipt without its matching success proof. A restart SHALL convert a surviving accepted journal without proof into failure, and successful delivery SHALL reap the proof after removing its terminal carrier; startup SHALL reap orphan proofs left by interrupted cleanup. Resolve, connect, `Hello`, authentication, identity, credential persistence, owner drift, stop, restart, and bounded-handshake failures SHALL settle failure. Notification failure SHALL preserve the saved profile but report a recoverable incomplete state rather than claiming the switch is fully active. Settings SHALL bound both connection upgrade and `Welcome` wait and SHALL discard the old transport and snapshots on failure.

#### Scenario: A to B updates every live owner view

- **WHEN** the user changes current profile from `A` to `B`
- **THEN** CLI Server state, Server snapshot, Daemon snapshot, and the running `fleetyd` session SHALL all resolve to `B` before the workflow reports a fully refreshed state

#### Scenario: a later current switch aborts the stale Settings reconnect

- **GIVEN** Settings persisted current profile `B` and froze `B` from that current-owner state
- **WHEN** another process changes current to `C` before Settings opens the `B` transport
- **THEN** Settings SHALL fail owner revalidation before transport, load no `B` snapshots, and report the selection conflict

#### Scenario: timed-out request is not overwritten

- **GIVEN** the Daemon is busy with an inline tool and has not consumed reconnect request `r1`
- **WHEN** the caller times out and another caller submits request `r2`
- **THEN** `r1` SHALL remain durable, `r2` SHALL be rejected as already pending, and the Daemon SHALL later settle `r1` exactly once

#### Scenario: later caller cannot consume an older terminal result

- **GIVEN** request `r1` has settled and its original caller is still waiting
- **WHEN** a later caller starts request `r2` before the `r1` caller observes its result
- **THEN** `r1` SHALL remain observable under its own nonce, `r2` SHALL submit a different nonce, and neither caller SHALL receive the other request's result

#### Scenario: reconnect success is immediately restart-ready

- **GIVEN** the selected Server returns a new token and identity pin in `Welcome`
- **WHEN** the reconnect caller observes a success settlement and `fleetyd` immediately restarts
- **THEN** the persisted selected profile SHALL already contain both values and the restarted Daemon's first `Hello` SHALL use the new token

#### Scenario: interrupted credential publication cannot become success

- **WHEN** credential persistence fails or `fleetyd` stops after credential commit but before success settlement
- **THEN** the reconnect nonce SHALL NOT settle success; any completely committed credential SHALL remain available for the next resolve

#### Scenario: storage failure after credential commit remains retryable

- **GIVEN** the returned token and identity are durably committed for the frozen owner
- **WHEN** reconnect lease acquisition or owner inspection fails before success publication
- **THEN** the Daemon SHALL retain the committed authenticated snapshot for retry and SHALL NOT replace it with a permanent failure decision

### Requirement: Daemon runtime requires unique local ownership

Before starting any update poller, dependency provisioning, reconnect poller, or Server transport, `fleetyd` SHALL acquire every applicable service PID owner and its reconnect-control owner. The reconnect ready record and journal events SHALL declare a supported control version. The ready record SHALL bind its PID and Daemon generation to a process-start token backed by a lifetime OS file lock, so a reused PID without the matching lock is stale rather than a live owner. Ready publication SHALL sync a same-directory temp file before rename, flush the canonical file after rename, and sync the control directory where the platform's safe filesystem API exposes directory sync before startup continues; a failed post-rename durability step SHALL hide the ambiguous canonical record before releasing ownership. A current requester SHALL reject a legacy or unknown ready version immediately with actionable update guidance. A current Daemon SHALL read a legacy unversioned request only through an explicit compatibility path whose versioned settlement remains readable by that legacy requester. An existing owner, an unknown identity-lock state, an unreadable ready record, permission denial, unknown control version, or any ownership claim failure SHALL make the new process exit non-zero. The rejected process SHALL send no `Hello`, SHALL start no background runtime work, and SHALL NOT remove or replace an ownership artifact whose live state is unknown.

#### Scenario: second process cannot join the same control root

- **GIVEN** one `fleetyd` process owns the reconnect-control root and has an active Server session
- **WHEN** a second `fleetyd` process starts with the same control root
- **THEN** the second process SHALL exit non-zero before sending `Hello`, and the first process SHALL remain connected

#### Scenario: uncertain ownership fails closed

- **WHEN** service PID liveness is unknown, an ownership file is permission-denied, or the reconnect ready record is unreadable
- **THEN** `fleetyd` SHALL exit non-zero before poller, dependency, or network work and SHALL preserve the uncertain ownership artifact

#### Scenario: a reused PID is not the previous Daemon

- **GIVEN** a stale ready record names a PID that now belongs to another process
- **WHEN** the recorded process-start identity lock is no longer held
- **THEN** `fleetyd` SHALL reclaim the stale generation without treating the reused PID as its live owner

#### Scenario: mixed control versions do not time out silently

- **WHEN** a current requester reads a legacy ready record or either side reads an unknown control version
- **THEN** it SHALL fail immediately with guidance to update the CLI and Daemon together
- **AND WHEN** a current Daemon reads a legacy unversioned request through its compatibility path
- **THEN** it SHALL produce a terminal result that the legacy requester can read

##### Example: version 1 requester and an unversioned Daemon

- **GIVEN** `fleetyd reconnect --profile B` supports control version `1` and the running Daemon published `{ "pid": 42, "instance": "old-a" }`
- **WHEN** the requester reads that ready record
- **THEN** it SHALL submit no journal event and SHALL immediately direct the user to update and restart `fleetyd`

##### Example: unversioned requester and a version 1 Daemon

- **GIVEN** a legacy requester appends an unversioned `submitted` event for nonce `r1`
- **WHEN** the version `1` Daemon claims and settles `r1`
- **THEN** its appended events SHALL declare version `1`, retain the legacy field shape, and let the legacy requester observe the terminal result

#### Scenario: interrupted ready publication is not authoritative

- **WHEN** fleetyd stops while staging, renaming, or syncing its ready record
- **THEN** no ambiguous canonical owner SHALL survive as an authoritative live generation and no runtime network work SHALL start

##### Example: directory sync fails after rename

- **GIVEN** the version `1` ready temp file was synced and renamed to `fleetyd.control-ready.json`
- **WHEN** syncing the control directory fails
- **THEN** fleetyd SHALL remove the canonical ready record, sync its absence, retain ownership until that absence is durable, and exit before sending `Hello`

### Requirement: Profile switching consumes one live leased target snapshot

The profile URL, token, and fingerprint used for reconnect SHALL be read together inside the `connections.toml` mutation lease after the current-profile update. The reconnect SHALL NOT reuse credentials captured before that lease.

#### Scenario: concurrent credential rotation wins

- **GIVEN** profile `B`'s token or fingerprint rotates while a switch is waiting for the connection lease
- **WHEN** the switch acquires the lease
- **THEN** its reconnect SHALL use the latest complete `B` target snapshot

## MODIFIED Requirements

### Requirement: Connection profiles are the single persistent source of the connection target

The connection target (which server + its token) SHALL live in one file, `~/.fleety/connections.toml`, holding a device-wide `device_id`, a `current` profile name, and named `profiles` each carrying `url`, an optional list of alternate `endpoints` learned from that Server, an optional `configured_url` recording the address the user chose when roaming has moved `url`, a `secure` flag recording that this Server has proven it can open the encrypted control channel, an optional `token`, an optional `label`, an optional server `fingerprint`, and an internal lifecycle `generation`. The generation SHALL remain opaque to callers and SHALL use a versioned envelope that binds its lifecycle nonce to the presence state of `endpoints`, `configured_url`, and `secure`. A newly created or explicitly selected durable profile SHALL receive a non-empty generation before it grants mutation authority; raw URL and environment targets SHALL NOT trigger generation writes. A legacy plain generation SHALL remain readable until the selected durable profile passes through that authorized migration path. A versioned generation whose bound state does not match the serialized profile SHALL be treated as evidence of an incompatible older writer and SHALL fail closed. Deleting and recreating a profile SHALL mint a different lifecycle nonce even when all user-visible fields are identical. The file SHALL be written atomically (temp + rename) with `0600` permissions. Loading a missing file SHALL yield an empty set (not an error); loading a present-but-unparseable file SHALL return an explicit error rather than being silently treated as empty. `FLEETY_AGENT_URL` SHALL NOT be a registry setting — `config set FLEETY_AGENT_URL` returns an unknown-key error and the value is never seeded from `config.toml`.

#### Scenario: profiles round-trip with restricted permissions

- **WHEN** a profile is added and `connections.toml` is written then read back
- **THEN** the profile's url/token/label/generation survive the round-trip and the file's permissions are `0600`

#### Scenario: a corrupt connections.toml is a hard error, not empty

- **WHEN** `connections.toml` exists but cannot be parsed
- **THEN** the resolver returns an explicit error and does not silently fall back as if there were no connection configured

#### Scenario: transient resolution does not migrate saved profiles

- **GIVEN** a legacy profile lacks a lifecycle generation
- **WHEN** a raw URL or `FLEETY_AGENT_URL` target is resolved
- **THEN** the legacy profile SHALL remain byte-identical and SHALL grant no owner capability

#### Scenario: selected durable profile records forward-compatible state

- **GIVEN** profile `home` has a legacy plain generation and is selected as the durable operational target
- **WHEN** Fleety acquires the profile mutation lease before connecting
- **THEN** it SHALL preserve the lifecycle nonce, write a versioned generation that matches the profile's roaming and secure-channel fields, and only then grant owner capability

#### Scenario: incompatible profile state fails every current surface

- **GIVEN** profile `home` has a versioned generation whose bound state does not match its serialized roaming or secure-channel fields
- **WHEN** a CLI command, TUI route, ACP turn, `fleety doctor`, or `fleetyd` resolves or mutates `home`
- **THEN** the operation SHALL fail before network I/O or credential use with guidance to update every Fleety binary and explicitly re-pair

#### Scenario: a lost configured address names an executable recovery

- **GIVEN** a versioned generation records `configured_url` but an older writer removed that field
- **WHEN** a current surface rejects the mismatched profile
- **THEN** its remediation SHALL direct the user to `fleety init <ws-url> --name <profile> --pairing-code <code>`
- **AND** it SHALL NOT recommend bare `pair`, because that command safely refuses to guess the learned primary

#### Scenario: empty environment URL is unset

- **GIVEN** `FLEETY_AGENT_URL` is present but empty and a legacy current profile has a usable URL
- **WHEN** the CLI or Daemon resolves its operational target
- **THEN** it SHALL treat the environment URL as unset, upgrade the selected profile generation, and carry that exact durable owner capability

#### Scenario: FLEETY_AGENT_URL is no longer a config key

- **WHEN** the user runs `fleety config set FLEETY_AGENT_URL ws://x`
- **THEN** it is rejected as an unknown setting (the connection target is managed via `fleety server`, not the registry)

### Requirement: The fleety server command group manages named server profiles

The CLI SHALL provide a `fleety server` command group to manage profiles: `add <name> <url>` (with optional `--label`, `--pair <code>`, `--use`), `use <name>`, `list`, `show [<name>]`, `current`, `rename <old> <new>`, `remove <name>`, and `set-url <name> <url>`. `connection` SHALL be the canonical spelling and `server` SHALL remain its compatible alias. `use` SHALL change only the user-visible `current` field, plus the selected legacy profile's internal generation binding when migration is required; every other user-visible profile field SHALL remain unchanged. `list` SHALL mark the current profile and, when an env override is in effect, print a prominent notice at the top. Removing the current profile SHALL always require explicitly switching to another profile first; `--force` SHALL remain parse-compatible but SHALL NOT choose a replacement from profile ordering. `fleety init <url>` SHALL enroll and select the named profile only after authentication succeeds. `fleety pair <code>` SHALL be an explicit recovery action that sends no old saved token and atomically replaces the exact resolver-frozen profile generation with the newly minted token and Server fingerprint.

#### Scenario: add then use selects the connection

- **WHEN** the user runs `fleety connection add home ws://h:8787 --use`
- **THEN** `fleety connection current` SHALL print `home` and later commands SHALL connect to `ws://h:8787`

#### Scenario: selecting a legacy profile binds its security state atomically

- **GIVEN** profile `home` has a legacy plain generation and is not current
- **WHEN** the user runs `fleety connection use home`
- **THEN** the same connection-store mutation lease SHALL bind `home`'s current roaming and secure-channel presence state before setting it current
- **AND** no later resolve SHALL be responsible for that first binding

#### Scenario: first remote init redeems its code directly

- **WHEN** a new device enrolls with `fleety init ws://x --pairing-code CODE`
- **THEN** the Server SHALL redeem `CODE`, and only then SHALL the CLI create and select the default profile with the newly minted token and identity

#### Scenario: explicit re-pair replaces a rebuilt Server identity

- **GIVEN** a saved profile has an old token and fingerprint at the same URL
- **WHEN** the user explicitly runs `fleety pair CODE`
- **THEN** the pairing connection SHALL send no old token and SHALL atomically replace both credential fields only if the complete saved owner generation is unchanged

#### Scenario: removing the current profile requires an explicit switch

- **WHEN** the user runs `fleety connection remove <current>` with or without `--force`
- **THEN** it SHALL be rejected with guidance to run `fleety connection use <replacement>` first

### Requirement: Sticky connections heal by fingerprint when the address moves

When connecting to a credentialed profile's saved URL fails, the client SHALL NOT treat an mDNS TXT fingerprint as identity proof, attach the stored token to a discovered endpoint, persist a discovered URL, or report the profile healed. The CLI one-shot path and Daemon reconnect loop SHALL preserve the saved profile and direct the user to explicitly reselect and re-pair. A successful saved connection SHALL proceed without a discovery scan. Transparent endpoint healing SHALL NOT return unless the transport supplies cryptographic Server identity proof.

#### Scenario: the server moves to a new IP

- **WHEN** the saved URL stops answering and a scan finds an advertiser with the pinned fingerprint at a new URL
- **THEN** the profile SHALL remain unchanged, no stored token SHALL be sent to the advertiser, and the user SHALL be directed to explicitly reselect and re-pair

##### Example: copied fingerprint at a new address

- **GIVEN** profile `office` stores URL `ws://10.0.0.2:8787`, token `old-token`, and fingerprint `server-a`
- **WHEN** `ws://10.0.0.9:8787` advertises TXT fingerprint `server-a` after the saved URL stops answering
- **THEN** `office` SHALL retain its original URL and credential, and the new endpoint SHALL receive neither `old-token` nor a healed status

#### Scenario: a different server on the LAN is never adopted

- **WHEN** the saved URL stops answering and a scan finds only advertisers with different or absent fingerprints
- **THEN** the profile SHALL remain unchanged, no stored token SHALL be sent to any advertiser, and the original failure plus explicit recovery guidance SHALL be reported

##### Example: unrelated advertiser

- **GIVEN** profile `office` is pinned to `server-a`
- **WHEN** the saved URL stops answering and discovery returns only `server-b`
- **THEN** `office` SHALL remain byte-identical and the failure SHALL direct `fleety --profile office pair <code>`

#### Scenario: healthy connections never scan

- **WHEN** the current profile's URL answers
- **THEN** no discovery scan SHALL run and the saved connection SHALL proceed

##### Example: saved endpoint remains reachable

- **GIVEN** profile `office` has a reachable saved URL
- **WHEN** a CLI one-shot command or Daemon reconnect uses `office`
- **THEN** it SHALL connect to the saved URL without starting mDNS discovery or changing the profile
