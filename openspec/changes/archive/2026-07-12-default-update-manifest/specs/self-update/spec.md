## MODIFIED Requirements

### Requirement: Manifest URL templating

`FLEETY_UPDATE_MANIFEST` SHALL hold a single URL or URL template serving every resolution mode; when it is unset, the updater SHALL fall back to a built-in default template pointing at this project's own GitHub releases (`https://github.com/<owner>/<repo>/releases/latest/download/{bin}-manifest.json`), so a stock install's manual `fleety update` works with no configuration and a fork or mirror overrides it by setting the variable. The updater SHALL substitute `{bin}` with the name of the binary being updated. For latest resolution (background polling, `fleetyd update`, `fleety update`), the updater SHALL substitute `{version}` with the literal string `latest`. For pinned resolution, the updater SHALL substitute `{version}` with the exact target version and SHALL fail when the effective template lacks `{version}` (the built-in default is the latest form and carries no `{version}`, so pinned resolution requires either an env template with `{version}` or the manifest's own `versioned_manifest` field). A template without `{bin}` SHALL be treated as the manifest of the running binary only: the updater SHALL NOT resolve a manifest for a different binary from a template lacking `{bin}`, and SHALL skip that binary's update with a warning naming the missing `{bin}` placeholder. The built-in default fallback SHALL NOT enable the daemon's unattended auto-update poll, which SHALL continue to require `FLEETY_UPDATE_MANIFEST` to be set explicitly.

#### Scenario: unset variable resolves the built-in default

- **WHEN** `FLEETY_UPDATE_MANIFEST` is unset and `fleety update` resolves the latest manifest URL for `fleety`
- **THEN** it resolves `https://github.com/<owner>/<repo>/releases/latest/download/fleety-manifest.json` and treats the template as `{bin}`-templated

#### Scenario: environment variable overrides the built-in default

- **WHEN** `FLEETY_UPDATE_MANIFEST` is `https://host/dl/{bin}/{version}/manifest.json` and fleetyd resolves its latest manifest URL
- **THEN** it substitutes `{version}` with `latest`, yielding `https://host/dl/fleetyd/latest/manifest.json`

#### Scenario: sibling update requires the bin placeholder

- **WHEN** the daemon updates sibling binaries and `FLEETY_UPDATE_MANIFEST` holds `{version}` but not `{bin}`
- **THEN** it skips the sibling binaries with a warning naming the missing `{bin}` placeholder
