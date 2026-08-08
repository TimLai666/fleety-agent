## Context

The provider configuration TUI contains four choice controls whose items are rendered on one horizontal line: provider type, model role, Codex OAuth action, and unsaved-exit action. Their input handlers currently advance and retreat only with Down and Up, while vertical provider, model, and settings lists correctly use vertical navigation. The existing connection-profile switch prompt already accepts Left/Up for the previous item and Right/Down for the next item.

## Goals / Non-Goals

**Goals:**

- Make every horizontally rendered choice control respond to Left and Right in the direction shown on screen.
- Preserve Up and Down as compatibility aliases.
- Make the visible navigation hints describe the primary Left/Right interaction.
- Lock the behavior and boundary handling with focused tests.

**Non-Goals:**

- Changing navigation for vertically rendered lists.
- Changing option order, selection persistence, or confirmation behavior.
- Introducing a shared widget abstraction solely for these four controls.

## Decisions

### Use layout direction as the primary navigation contract

Left selects the previous horizontal item and Right selects the next horizontal item. This matches the visible arrangement and the established connection-profile prompt. Keeping Up and Down as aliases avoids a breaking keyboard change. Replacing Up and Down entirely was rejected because it would remove a working interaction without providing a user benefit.

### Update all four horizontal choice surfaces together

Provider type, model role, Codex OAuth action, and unsaved-exit action use the same contract. Updating only the reported screen was rejected because the same visual/input mismatch would remain elsewhere in the same editor.

### Preserve vertical-list navigation

Provider rows, model rows, settings rows, and other vertically stacked lists continue to use Up and Down. Broadly adding Left and Right to all lists was rejected because it would blur the directional cue instead of fixing it.

### Show primary keys while retaining silent aliases

Hints for horizontal controls display Left/Right. They do not enumerate the compatibility aliases, keeping the footer compact while the old keys continue to work.

## Implementation Contract

- **Behavior:** On each horizontally rendered choice control, Left moves to the immediately preceding item and Right moves to the immediately following item. Up behaves exactly like Left, and Down behaves exactly like Right.
- **Boundary behavior:** Movement remains bounded exactly as it is today; the change does not introduce wrapping or change the selected item when movement cannot proceed.
- **Visible interface:** Each affected control's key hint presents Left/Right as the navigation keys. Vertical-list hints and behavior remain Up/Down.
- **Failure modes:** Unsupported keys remain no-ops. Enter, Esc, and action-specific shortcuts retain their current behavior.
- **Acceptance criteria:** Focused TUI tests exercise Left, Right, Up, and Down on each affected control, verify both boundaries, verify hints show Left/Right, and confirm at least one representative vertical list remains Up/Down-only.
- **Scope boundaries:** The implementation is confined to provider-configuration TUI input handling, hints, and tests. It does not change config data, protocol messages, persistence, or other binaries.

## Risks / Trade-offs

- [Risk] A horizontal surface is missed because its input logic is separate from its renderer. → Mitigation: enumerate and test all four named controls.
- [Risk] New aliases accidentally affect vertical lists. → Mitigation: change only state-specific handlers and add a regression assertion for a vertical list.
- [Trade-off] Hints do not advertise Up/Down aliases. → Mitigation: treat them as backward-compatible behavior, not the recommended interaction.
