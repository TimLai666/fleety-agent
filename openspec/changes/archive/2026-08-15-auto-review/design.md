## Context

Fleety's execution loop already separates tool policy from tool execution: `agent-core` evaluates a `Policy` and invokes an `ApprovalGate` before calling a tool. The current policies are `FullAccess` and `RequireApproval`; the latter uses a connected client's `Approve` or `Deny` response. The server already has a named `cheap` provider tier, but it is not used as an execution reviewer. Dangerous command and sensitive-path checks currently reject some critical mutations before a policy gate can make a decision.

This change adds an unattended review posture for deployments where no human is present. It must preserve the single pre-execution gate, keep the reviewer unable to execute tools, and make deterministic danger detection visible to the reviewer without allowing candidate tool text to override the review policy.

## Goals / Non-Goals

**Goals:**

- Add an opt-in `auto_review` policy that evaluates every non-read tool call, including critical calls, before execution.
- Use the existing `cheap` provider tier for the review call.
- Give the reviewer the user's objective, bounded relevant context, candidate tool and arguments, risk class, and trusted danger signals.
- Replace critical pre-execution refusal with a review warning only when `auto_review` is active.
- Fail closed on reviewer timeout, provider failure, invalid output, tool-call output, or missing review context.
- Redact secrets from review context, danger signals, audit fields, and reviewer reasons.
- Record every auto-review decision and its outcome in the existing audit trail.

**Non-Goals:**

- Changing the default `full_access` policy.
- Removing human approval support from `require_approval`.
- Granting the reviewer tools, filesystem access, network actions, or authority to modify the candidate call.
- Replacing operating-system permissions or making a failed tool call succeed.
- Adding a new model-provider implementation or a new transport-level approval frame.

## Decisions

### Use a third policy at the existing gate

Add `Policy::AutoReview` and route it through the existing `ApprovalGate` seam. Extend the gate request contract with a bounded `ReviewContext` containing the current objective and sanitized conversation context. Existing `AutoApprove`, `AutoDeny`, `MandateGate`, and the interactive connection gate ignore that context and retain their current behavior. This keeps the decision immediately before `tools.call` and prevents a tool surface from bypassing the new policy.

Alternatives considered:

- Adding review logic separately to every tool would duplicate enforcement and leave bypass paths.
- Wrapping only the WebSocket connection would miss SSE, recovery, subagent, and scheduled execution paths.
- Passing the full message history to the gate would leak unnecessary data and make prompt-injection boundaries unclear.

### Build one server-side auto-review gate on the cheap provider

The Server constructs an `AutoReviewGate` with `ProviderTiers::resolve("cheap")`. The reviewer call is non-streaming, receives no tool specifications, and cannot call tools. The existing cheap-tier alias-to-main fallback remains valid but emits an operational warning when no distinct cheap provider is configured.

The reviewer prompt has trusted policy instructions followed by explicitly delimited untrusted data sections for objective, context, tool, arguments, risk, and danger signals. The required response is exactly a JSON object with `decision` equal to `approve` or `deny` and a bounded `reason` string. Markdown fences, tool calls, unknown decisions, missing fields, oversized output, and invalid JSON are denials.

### Preserve deterministic detectors as trusted warning signals

Keep pure detection for irreversible command patterns and sensitive paths, but expose structured non-secret `DangerSignal` values to the auto-review prompt. In `auto_review`, detection no longer rejects the candidate before review. The reviewer is explicitly told that a signal is machine-generated evidence, that the candidate data is untrusted, and that it must deny when the user's objective does not clearly justify the danger.

Outside `auto_review`, existing critical refusal and sensitive-path behavior remains unchanged unless a corresponding capability delta explicitly changes it.

### Make all auto-review failures deny without human fallback

A reviewer timeout, provider failure, missing cheap provider, context-redaction failure, or response parse failure returns `ApprovalDecision::Deny`. The tool is not called, the model receives a synthetic denial result, and the audit records the failure category. There is no interactive fallback because `auto_review` is explicitly unattended.

Use a bounded `FLEETY_AUTO_REVIEW_TIMEOUT_SECS` setting with a positive default and the existing provider retry policy within that budget. A disabled or invalid value falls back to the documented default rather than allowing an unbounded review wait.

### Audit decisions without exposing candidate secrets

Write `auto_review` audit records with a decision, risk class, tool name, reviewer provider/model label, danger-signal codes, latency, and sanitized reason. Do not persist raw arguments, prompt text, tokens, API keys, passwords, or unredacted paths. A denied candidate receives the existing denied-tool event as well as the review metadata needed to explain why it did not run.

## Implementation Contract

### Behavior

When `FLEETY_POLICY=auto_review` is active, read tools run directly. Every mutate and critical tool call pauses at the shared gate, invokes the cheap reviewer, and runs only after a valid `approve` decision. This includes local workspace tools, device-routed tools, remote execution, subagents, scheduled turns, WebSocket connections, SSE connections, and recovery continuations. The default and human-approval policies retain their existing semantics.

A detected dangerous command or sensitive path is represented as a trusted warning in the review request. It is not an unconditional refusal in the `auto_review` path. The reviewer can approve or deny it based on the stated objective. The operating system can still deny the resulting action, and ordinary tool validation remains active.

### Interface / data shape

The core gate request gains a sanitized review context with:

- `objective`: the current user objective, bounded and redacted
- `conversation_context`: bounded relevant prior user/assistant content, redacted
- `tool`: the candidate tool name
- `arguments`: the candidate arguments after secret/path sanitization
- `risk`: `read`, `mutate`, or `critical`
- `danger_signals`: structured codes and human-readable warnings without secrets

The reviewer response is:

    {"decision":"approve","reason":"..."}

or:

    {"decision":"deny","reason":"..."}

The server does not expose a new client approval frame for auto review. Configuration surfaces accept `auto_review`, and audit listings expose the decision metadata through the existing audit mechanisms.

### Failure modes

- No distinct cheap provider, provider error, timeout, or retry exhaustion: deny and record `review_unavailable`.
- Invalid or non-conforming reviewer response: deny and record `review_invalid`.
- Secret or context redaction failure: deny and record `review_redaction_failed`.
- Reviewer attempts a tool call or returns an oversized response: deny and record `review_protocol_violation`.
- Reviewer denies: do not call the candidate tool; feed a synthetic denial result to the agent and record the denial.
- Candidate tool fails after approval: preserve the normal tool error and audit behavior; approval does not imply successful execution.

### Acceptance criteria

- Policy parsing accepts `auto_review` and keeps `full_access` as the default.
- Unit tests prove read bypass, mutate review, critical review, strict response parsing, timeout/error denial, and absence of reviewer tools.
- Integration tests prove a danger signal reaches the reviewer, approval permits the candidate call, denial prevents it, and reviewer failure prevents it.
- Tests cover the same policy through WebSocket, SSE, subagent, scheduled, and recovery execution paths, or explicitly prove that each path calls the shared gate.
- Audit tests prove decisions and denial reasons are queryable while secrets and raw candidate arguments are absent.
- Documentation describes the opt-in policy, cheap-provider selection, dangerous-operation warnings, fail-closed behavior, and the fact that no human approval is requested.

### Scope boundaries

In scope: policy/config registry and docs, the shared gate context, the server-side cheap-model reviewer, deterministic danger-signal plumbing for command and filesystem tools, audit records, and tests for every execution surface.

Out of scope: new provider protocols, new transport approval frames, changes to loopback authentication, changes to normal operating-system permission checks, and automatic execution of commands that the OS rejects.

## Risks / Trade-offs

- [Reviewer false approval] → Include the original objective, risk class, deterministic danger signals, and explicit deny-on-ambiguity instructions; preserve audit evidence for later review.
- [Prompt injection in user text, tool arguments, or filenames] → Delimit all candidate data as untrusted, place policy instructions in the trusted reviewer instruction, and never expose tools to the reviewer.
- [Secret leakage to the cheap provider or audit log] → Apply one shared redaction path before prompt construction and before persistence; deny if redaction fails.
- [Review latency and cost] → Use the cheap provider, bounded context, non-streaming calls, retries within a timeout budget, and record latency for tuning.
- [Cheap tier silently aliases the main tier] → Preserve compatibility but emit a warning and document how to configure a distinct cheap provider.
- [Critical behavior differs by policy] → Add explicit tests for `auto_review` versus `full_access` and `require_approval`, so the hard-refusal change is scoped to the new posture.

## Migration Plan

1. Ship the policy and gate behind opt-in `FLEETY_POLICY=auto_review`; existing deployments remain unchanged.
2. Configure a distinct cheap provider and review timeout, then enable auto review on a non-production server.
3. Inspect audit decisions and denial rates before enabling it for destructive workloads.
4. Roll back by setting `FLEETY_POLICY=full_access` or `require_approval`; no persistent data migration is required.

## Open Questions

- Whether the reviewer reason should be shown in the agent's synthetic denial result in full, or only a sanitized short form.
- Whether deployments should be able to require a distinct cheap provider instead of accepting the existing cheap-to-main fallback.
