## MODIFIED Requirements

### Requirement: config subcommands manage providers, groups, and roles

The `config` command surface SHALL manage the two-tier model: `config provider add <name> --type api --base-url <url> [--key <secret>]` and `config provider add <name> --type oauth:codex`, plus `provider set`, `provider remove`, and `provider list` (listing SHALL show each provider by `type` with its type-appropriate fields and mask secrets). Model roles SHALL be managed with `config model set <main|cheap> --member <provider>/<model> [--stream] [--modalities <list>] [--effort <level>] [--member …] --strategy <single|round_robin|failover>`, plus `model show` and `model unset`. Removing a provider that a role member references SHALL be refused.

#### Scenario: add a provider then bind a model role to it

- **WHEN** `config provider add openai1 --type api --base-url https://api.openai.com/v1 --key sk-x` then `config model set main --member openai1/gpt-4o --strategy single` run
- **THEN** `providers.toml` holds provider `openai1` (type api) and a `main` role with one member `openai1/gpt-4o`

#### Scenario: an oauth provider takes no base_url or key on the command line

- **WHEN** `config provider add codex1 --type oauth:codex` runs
- **THEN** it is accepted with no `base_url`/`key`, and the token is obtained separately via `fleety auth login codex1`
