## Summary

Replace `providers.toml`'s "one provider binds one model + group + role" shape with a genuine two tiers — `type`-tagged providers (endpoint/auth) and `main`/`cheap` model pools whose members carry the model and its call-time traits — so `key`/`base_url` belong to the provider and one provider can serve different models to different roles.

## Motivation

The flat `FLEETY_MODEL_BASE_URL` / `FLEETY_MODEL` / `FLEETY_MODEL_KEY` env tier puts a provider's endpoint and secret under a *model* name — but `key`/`base_url` belong to the provider, not the model (design §1.2). `providers.toml` fixed half of this yet still binds exactly one `model` per provider, so "the same provider serves gpt-4o to main and gpt-4o-mini to cheap" is impossible without duplicating the provider. Groups + a role→name map are an awkward stand-in for real model pools. The result is a data model that fights the user's mental model (provider ⊃ model) and multiplies near-duplicate providers.

Separately, `PoolProvider::capabilities()` takes `members.first()`, assuming a pool is homogeneous — which blocks a mixed pool's audio hint even when a member could handle audio.

## Proposed Solution

`providers.toml` becomes two tiers (design §3.3):

- **Providers** are a `type`-tagged enum: `type = "api"` requires `base_url`, allows `key`, forbids an oauth token; `type = "oauth:codex"` sources a per-provider token from `fleety auth login` and forbids `base_url`/`key`. The `type` is an extensible registry, not a hardcoded if.
- **Model roles** are fixed `main` and `cheap`; each is a pool with a `strategy` (`single`/`round_robin`/`failover`) and `members`, where each member is `{ provider, model, stream, modalities, effort }`. The call-time traits `stream`/`modalities`/`effort` sink from the provider onto the member (they follow the model/call, not the account).
- **Mixed pools are allowed.** Native-vs-degrade and send-effort decisions already happen per member inside each member's `complete()`; the pool's aggregate `capabilities()` changes from `members.first()` to the **union** across members (so the audio hint reflects "any member can" and the routed member degrades as needed) — never `first` or intersection.
- **Referential integrity is validated before write** (not runtime fail-soft): every `member.provider` must be a defined provider; removing a referenced provider is refused; `strategy = "single"` requires exactly one member.
- **Migration** is one-time and idempotent: old providers that differ only by `model` (same `base_url`+`key`) merge into one provider with multiple members; `stream`/`modalities`/`effort` move onto the matching member; anything that can't be migrated is reported loudly, never silently dropped.
- **Flat `FLEETY_MODEL_*` stays as a bootstrap seed** (headless/CI/Docker escape hatch): with no structured providers it auto-forms `models.main`. A present-but-broken structured config becomes a hard startup error (refuse to boot) instead of silently degrading to echo; the echo stub survives only as the no-config placeholder.
- **Commands** move to the two-tier shape: `config provider add <name> --type api|oauth:codex …`, `config model set main --member <provider>/<model> [--member …] --strategy …`, plus `provider set/remove/list` and `model show/unset`.

## Non-Goals (optional)

- Authentication-default-on and "remote write ⇒ auth on" (separate change `auth-default-on`).
- Filtering `fleety config local` to Cli/Shared scope (separate change `local-config-scope`).
- The `ConfigSnapshot`/`ConfigApply` wire protocol and the interactive all-in-one panel (Phase 2 change `remote-config-panel`).
- Owner/normal device tiering, encryption-at-rest, and any transport (wss) requirement.

## Alternatives Considered (optional)

- **Keep the group+role model, only sink traits to members.** Rejected: leaves the "one model per provider" limitation and the awkward group-as-pool indirection that the two-tier model removes.
- **Pool `capabilities()` = intersection of members.** Rejected by design §3.3 — too restrictive (blocks an attachment any member could take); union + per-member degrade is correct.

## Impact

- Affected specs: `provider-pool` (modified — roles become member pools), `provider-config-surface` (modified — provider/model command shape), `capability-aware-modality` (modified — pool capability is the member union), `economy-model-tier` (modified — cheap is a member pool); `two-tier-provider-model` (new — the tagged-provider + member-pool data model, referential integrity, migration, bootstrap seed / hard-error).
- Affected code:
  - Modified: crates/fleety-tools/src/providers_config.rs (data model, parse, validate, migration), crates/fleety-server/src/providers.rs (runtime build from the two-tier model, bootstrap seed, hard-error), crates/fleety-server/src/pool.rs (union capabilities), crates/fleety-tools/src/config.rs (provider/model command dispatch), crates/fleety-cli/src/provider_tui.rs (interactive editor for the new model), docs/roadmap.md, docs/STATUS.md
  - New: (none)
  - Removed: (none)
