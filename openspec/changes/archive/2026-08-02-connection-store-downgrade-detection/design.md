## Context

`connections.toml` is shared by `fleety` and `fleetyd`. The current file has no store-level writer marker, so a pre-security binary can read the file, discard fields it does not understand, and write a plausible but incomplete document. Per-profile generation validation catches some mismatches, but a legacy profile without that evidence can still be mistaken for a safe migration candidate. The affected profile may then lose learned endpoints, the configured address, or the secure-channel proof.

This change is a compatibility boundary for durable profile state. It does not try to make an old serializer preserve fields it cannot represent. It makes the current binaries reject ambiguous durable state before they use credentials or mutate profiles, and gives the operator an explicit way to rebuild the affected profile.

## Goals / Non-Goals

**Goals:**

- Record a store-level format and writer marker that every current durable writer validates.
- Treat a missing, unsupported, or inconsistent marker as an incompatible-store error before credential use, network I/O, profile mutation, or daemon startup with that store.
- Keep the existing per-profile generation and secure-channel proof checks as independent protections.
- Make every persistent writer, including CLI and Daemon paths, use one shared compatibility gate.
- Give users a deterministic recovery path that updates all Fleety binaries and explicitly re-pairs without guessing a lost endpoint or secure latch.
- Preserve atomic `0600` writes and keep transient URL, environment, ACP, and Doctor targets side-effect free.

**Non-Goals:**

- Do not make legacy Fleety binaries preserve fields introduced after their release.
- Do not silently infer a missing configured URL, learned endpoint, secure state, token, or profile identity.
- Do not redesign the authenticated candidate handshake or the per-profile generation envelope.
- Do not add a separate remote migration service or a database.

## Decisions

### Store compatibility is an explicit durable contract

The serialized store SHALL carry a supported format version and a writer marker whose value is produced by the current Fleety store layer. The loader SHALL validate the marker before returning a durable store to any caller. The contract SHALL distinguish a supported current store from an unmarked legacy store and an unsupported future store.

A current writer SHALL emit the marker on every atomic write. A marker is not evidence that a profile's secure-channel proof is valid; profile generation and authenticated Server validation remain separate checks.

### Ambiguous durable state fails closed

A present store with a missing, unsupported, or malformed compatibility marker SHALL return a typed compatibility error. Current surfaces SHALL not resolve a token, open a network connection, mutate the store, or start the Daemon's operational connection loop from that state. A failure message SHALL identify the store as incompatible and point to recovery that updates all Fleety binaries before explicit re-pairing.

The loader SHALL not auto-rewrite an incompatible file because that could destroy the only remaining evidence of an older writer. Recovery SHALL be an explicit operation that creates a valid current store from user-supplied connection details or a new pairing flow.

### One shared gate covers every durable writer

The compatibility check and current-format write path SHALL live in the shared connection layer. The CLI server commands, TUI configuration mutations, Daemon resolver/startup path, ACP, and Doctor flows SHALL call that layer rather than deserialize or rewrite the store independently. Transient targets SHALL bypass durable loading and SHALL not create or update the marker.

### Recovery is explicit and credential-safe

Recovery SHALL require all Fleety binaries that share the store to be updated before the user re-pairs the affected profile. The recovery command/path SHALL accept an explicit URL/profile name and pairing code or an equivalent current enrollment flow. It SHALL not send a token from the rejected store, and it SHALL not recommend a bare pairing operation that would guess a learned endpoint.

### Verification covers downgrade evidence and writer boundaries

Tests SHALL exercise a legacy/old-writer rewrite that removes current-only fields, unsupported and malformed markers, healthy unrelated profiles, all persistent writer entry points, atomic permissions, and the absence of credential/network activity on rejection. Tests SHALL also prove that transient resolution remains side-effect free.

## Implementation Contract

### Behavior

- Add the store-level compatibility fields and validation in `crates/fleety-tools/src/connection.rs`.
- Return a distinguishable compatibility error for missing, unsupported, or malformed store markers.
- Make all durable readers and writers validate the store before granting owner/mutation authority or using a saved token.
- Keep the rejected file intact for diagnosis and require explicit recovery/re-pairing to create a valid marked store.
- Update user-facing remediation consistently in CLI, TUI, Daemon, ACP, Doctor, and documentation.

### Interface and data shape

- The store format version and writer marker are durable top-level fields in `connections.toml`.
- The loader exposes a typed error or equivalent stable classification that callers can render without matching on error text.
- No new field is written into transient target representations.
- Existing per-profile generation fields retain their current meaning and validation.

### Failures and safety

- Missing or malformed compatibility data is an error, not an empty store.
- Unsupported future versions fail closed without destructive rewrite.
- A write that cannot prove the input store is compatible fails before replacing the file.
- A rejection never loads a saved credential into a transport or sends it to a Server.
- Atomic replacement preserves `0600` permissions and leaves the original file recoverable when a write fails.

### Acceptance criteria

- A current store round-trips its marker and remains readable by all current binaries.
- A store rewritten by an older serializer is rejected when its marker or current-only state is missing.
- Rejection happens before network I/O, credential use, or profile mutation on every listed surface.
- Explicit recovery creates a marked store and re-pairs only from user-provided current details.
- Tests cover the positive, legacy, future-version, malformed, permission, unrelated-profile, and transient-target cases.

### In scope

`fleety-tools` connection-store serialization and validation, CLI/Daemon/TUI/ACP/Doctor call paths, smoke/unit tests, and the associated configuration and recovery documentation.

### Out of scope

Changes to the authenticated transport protocol, per-profile secure handshake, endpoint roaming algorithm, mDNS selection policy, or legacy binary behavior.

## Risks / Trade-offs

Fail-closed loading can interrupt users with old unmarked files after an upgrade. That interruption is intentional because silently migrating an ambiguous file can preserve a token while losing the security state it was meant to protect. The cost is reduced convenience and a required explicit re-pair.

A top-level marker detects unsupported or older writers only after a current writer has established the contract; it cannot retroactively prove what a legacy binary did to an unmarked file. Therefore unmarked files must use the explicit recovery path rather than being treated as trusted migrations.

Centralizing the gate reduces inconsistent behavior but makes the shared connection module a compatibility boundary for every binary. Tests must cover all callers so a new direct serializer cannot bypass it.

## Migration Plan

1. Ship the shared loader, writer, error classification, and recovery guidance together.
2. On encountering an unmarked or incompatible store, preserve it and show the update-all-binaries and explicit re-pair instructions.
3. After the user supplies current connection details, write a new marked store atomically and verify the new profile through the existing authenticated pairing path.
4. Keep the rejected file as a backup until the user confirms the recovered profile works.

## Open Questions

- Resolved: explicit recovery requires the operator to preserve and move the incompatible file first. This keeps the rejected bytes available for diagnosis and avoids an implicit destructive replacement.
- Resolved: `fleety init <ws-url> --name <profile> --pairing-code <code>` is the canonical recovery entry point. `fleety pair` remains for an already compatible named store and never guesses an endpoint during recovery.
