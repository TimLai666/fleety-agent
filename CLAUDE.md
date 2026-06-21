# Fleety — project notes for Claude

Workspace-wide engineering rules live in the code (`#![warn(clippy::unwrap_used,
clippy::expect_used)]`, never-crash errors-as-messages, `agent-core` depends on
no Fleety crate). This file holds things that aren't derivable from the code.

## Releasing

**Before cutting a release, update the bundled Insyra version.** The
`fleety-insyra` sidecar ([`sidecars/fleety-insyra`](sidecars/fleety-insyra))
embeds `github.com/HazelnutParadise/insyra`; we want releases to ship the latest
Insyra. CI's release workflow does this automatically
(`go get -u github.com/HazelnutParadise/insyra@latest && go mod tidy`, then build
+ smoke test). If you ever cut a release outside that workflow, run the same
`go get -u …@latest` in `sidecars/fleety-insyra/` first and commit the updated
`go.mod`/`go.sum`. A breaking Insyra change will surface as a sidecar build/test
failure — fix the sidecar, don't pin around it silently.
