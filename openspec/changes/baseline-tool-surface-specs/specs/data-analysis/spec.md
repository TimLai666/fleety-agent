## ADDED Requirements

### Requirement: Run the Insyra data-analysis DSL

The system SHALL provide an `insyra_exec` tool that runs the Insyra `.isr` DSL (load CSV/Parquet/Excel/SQL, transform, compute statistics, plot) via the bundled Go sidecar. It SHALL be stateful per `session`, SHALL accept a single `command` or a multi-line `script`, and SHALL support `reset` to clear a session. `save <var> <file>` SHALL write results into the workspace.

#### Scenario: stateful session across two calls

- **WHEN** one `insyra_exec` call defines a variable in a named `session` and a second call references it in the same `session`
- **THEN** the second call sees the variable defined by the first
