# Fleety Agent — Tool Surface (canonical)

This is the source of truth for the tools the Fleety Agent (the LLM) may call. `prompts/protocol.md` describes how to use them in prose; this file fixes the **names, typed inputs, returns, and risk class**. When a name changes, change it here first, then sync `prompts/protocol.md` and the runtime. The runtime still exposes each tool's real JSON Schema at call time — that schema wins over this doc for argument shape.

## Conventions

- **Session.** Every tool except `harness` requires `session_id` (string), obtained from `harness`. A missing / stale / fabricated id is rejected before the tool runs.
- **Targeting.** Device-scoped tools take two optional arguments:
  - `device` (string) — device id / name / known alias. Empty = the **origin device** of the current conversation.
  - `project` (string) — project id / name / absolute path on that device. Empty = the device's current working directory.
  The runtime resolves the device to a concrete connector (session > daemon > ssh > http) and dispatches there. You do not pick the connector; you may read which one was used from the result.
- **Risk class** (drives the access policy in `prompts/policy.md`):
  - `read` — no state change. Executes directly under any policy.
  - `mutate` — changes state. Under `full_access` executes directly but is **audited + rollback-backed**. Under stricter policy returns `approval_required`.
  - `critical` — irreversible / no rollback path. **Always requires explicit user confirmation**, even under `full_access`.
- **Return envelope.** Every result carries at least: `ok` (bool), `status` (`"ok" | "approval_required" | "error" | "waiting_for_device"`), `device_id`, `connector`, and a tool-specific `data`. Mutating results also carry a `history_step_id` (the rollback handle). Never treat a result as success without checking `status`.
- **Device-scoped handles.** Every handle a tool returns (tab id, pid, port, session, workspace ref, …) is bound to its `device_id` — there are no global handles. The runtime rejects using a handle against a different device; identical-looking ids on different devices are different things. Keep each handle paired with its device; never pass a bare handle. A cross-device rejection is **actionable**: it returns the handle's owning `device_id` and the remediation — re-issue against the owning device, or acquire a fresh handle on the device you actually meant. Errors in general state cause + how to proceed, not just "rejected".
- **Approvals.** `approval_required` returns an `approval` record; re-call the same tool with `approval_id` set once the user approves. Never fabricate an `approval_id`.

## Orientation & discovery

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `harness` | Open a session; returns `session_id` + runtime/policy info. Call first. | — | read |
| `device_list` | List known devices with status, roles, connectors. | `status?`, `tag?` | read |
| `device_show` | Full record for one device: device.yaml, all `connectors[]` + each one's state and scope (`local`/`remote`), `mobility` (stationary/mobile/unknown), `site`, last_seen. | `device` | read |
| `site_list` | List known sites (locations). | — | read |
| `site_show` | A site plus the devices located there. | `site` | read |
| `site_set` | Create/update a site (location). | `id`, `name?`, `description?` | mutate |
| `site_delete` | Delete a site (leaves device records). | `id` | mutate |
| `device_set_site` | Set a device's current site (a registered id, or `away`/`unknown` for a mobile/in-transit device). | `device`, `site` | mutate |
| `device_set_mobility` | Mark a device `stationary` / `mobile` / `unknown`. | `device`, `mobility` | mutate |
| `project_list` | List registered projects/workspaces (optionally for one device). | `device?` | read |
| `project_current` | Resolve which workspace a `device`/`project` points at. | `device?`, `project?` | read |
| `list_skills` | List available skills + metadata. | — | read |
| `mcp_list` | List configured external MCP servers (`probe:true` to connect + list tools). | `probe?` | read |
| `approval_list` | Pending approval records for this session. | — | read |

## Device memory & capability

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `memory_read` | Read a memory file. With `device`: that device's memory (NOTES.md, facts.json, capabilities.yaml, links.yaml, resources.yaml, history.jsonl). Without `device`: an agent-level core file (`ME.md`/`USER.md`/`TODO.md`/`TOOLS.md`). | `device?`, `file` | read |
| `memory_write` | Update a memory file (append/replace). With `device`: device memory (Agent-side; does not touch the remote device). Without `device`: an agent-level core file. | `device?`, `file`, `content`, `mode?` | mutate |
| `capability_list` | The device's known capability index with status (available / unavailable / unknown / stale / blocked). | `device` | read |
| `capability_probe` | Run a discovery probe and update facts/capabilities. `mode` selects intrusiveness. | `device`, `capability?`, `mode` (`passive`\|`active`\|`acquire`) | `passive`/`active`: read · `acquire`: mutate |

> `capability_probe mode:acquire` (install a tool, pull a binary, spin a temp container) is a state change and is audited like any mutate. `passive`/`active` only observe.

## Conversation recall (v0.1+)

Cross-conversation memory. Within a conversation the event stream is replayed on reconnect (always on); these tools recall *past* conversations. See `docs/spec-v0.md` §5.1. (Agent-level core memory — `ME.md`/`USER.md`/`TODO.md`/`TOOLS.md` — is read/written via `memory_read`/`memory_write` with no `device`; `ME.md`/`USER.md`/`TODO.md` are auto-injected each turn — see §5.2.)

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `conversation_list` | List past conversations (device-scoped by default). | `device?`, `limit?` | read |
| `conversation_search` | Search past conversations / their summaries. | `query`, `device?` | read |
| `conversation_read` | Read a past conversation or its summary. | `conversation_id` | read |

## Workspace (device-scoped, dispatched over the connector)

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `workspace_list_files` | List a directory on the target device. | `device?`, `project?`, `path?` | read |
| `workspace_read_file` | Read a file; returns `numbered_content` (1-based, cat -n). | `device?`, `project?`, `path` | read |
| `workspace_search` | Search file contents on the target device. | `device?`, `project?`, `query`, `glob?` | read |
| `workspace_write_file` | Write a whole file (new / small files). | `device?`, `project?`, `path`, `content` | mutate |
| `workspace_apply_patch` | Apply a multi-hunk patch. | `device?`, `project?`, `patch` | mutate |
| `workspace_replace_lines` | Replace inclusive 1-based `start_line`..`end_line` with `content` (insert: `end_line = start_line - 1`). | `device?`, `project?`, `path`, `start_line`, `end_line`, `content` | mutate |

> Read before you rely; re-read before each line-range edit (line numbers shift). Mutating a non-git workspace first creates a backup / patch journal (`.fleety-backups/`, `.fleety-patches/`) so the `history_step_id` is restorable.

## Terminal

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `terminal_run` | Run one command on the target device; returns stdout/stderr/exit code. File changes it causes are recorded as history steps. | `device?`, `project?`, `command`, `cwd?`, `timeout?` | mutate, or **critical** if the command is irreversible (wipe/mkfs/dd/HOME delete/ssh-config/key-rotate/firewall/remote-only reboot) |

> The runtime classifies obviously-destructive commands as `critical` and blocks them pending confirmation. Do not try to disguise such a command to bypass the gate.

## Git

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `git_status` | Working-tree status on the target device. | `device?`, `project?` | read |
| `git_diff` | Diff (optionally staged / a path). | `device?`, `project?`, `path?`, `staged?` | read |
| `git_log` | Commit log. | `device?`, `project?`, `limit?` | read |
| `git_show` | Show a commit / object. | `device?`, `project?`, `ref` | read |

## Project registry

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `project_add` | Register an existing directory the device can see. | `device?`, `path`, `name?` | mutate |
| `project_create` | Create an empty persistent managed workspace. | `device?`, `name` | mutate |
| `project_clone` | `git clone` into a persistent managed workspace. | `device?`, `url`, `name?` | mutate |

## Skills

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `use_skill` | Return a skill's `SKILL.md` and mark it active for the session (hot-reload: call again for the current version). | `skill` | read |

## External MCP

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `mcp_call` | Call a tool on a configured MCP server; arguments validated against that tool's schema. | `server`, `tool`, `arguments` | depends on target tool; untrusted server → `mutate`/`critical` per policy |
| `mcp_add` | Register an MCP server (hot-reloaded). | `name`, `command`, `args?`, `env?` | mutate |
| `mcp_remove` | Remove an MCP server. | `name` | mutate |

## Knowledge wiki (post-v0)

The agent's long-term Obsidian-format knowledge vault — its second brain, separate from per-device memory and workspaces. All tools write only into the vault; the runtime enforces location and conventions (see `docs/spec-v0.md` §14).

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `wiki_search` | Search the vault (orient / dedup before writing). | `query` | read |
| `wiki_read` | Read a page (or `index.md` / `SCHEMA.md` / `log.md`). | `path` | read |
| `wiki_write` | Create or update a page; maintains frontmatter, `[[wikilinks]]`, index and log. | `path`, `content`, `frontmatter` | mutate |
| `wiki_list` | List pages by type. | `type?` | read |
| `wiki_lint` | Surface orphans, broken links, stale/contradictory/index issues. | — | read |

## Browser (skill-provided, post-v0)

Drive a chosen device's own Chrome via CDP — the CDP control runs locally on the target device, only high-level intents cross the connector (see `docs/spec-v0.md` §12). Delivered by a browser skill, not core runtime.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `browser` | Operate a browser session on a device. One tool, many actions. snapshot-ref based acting (not CSS selectors). | `device?`, `profile` (`managed`/`user`), `action` (`snapshot`/`screenshot`/`act`/`navigate`/`open`/`tabs`/`wait`/`evaluate`/`dialog`/`status`), action args | read for snapshot/screenshot/status; mutate for navigate/act/evaluate; **critical** for acts that send/pay/post/delete in a logged-in (`user`) profile. `user` profile attach is co-location/approval gated. |

## Computer-use (built-in MCP, post-v0)

Control a device's desktop (screen / mouse / keyboard) via the built-in `computer-use-mcp`, which runs on that device. Called through `mcp_call` dispatched to the device; handles are device-scoped. See `docs/spec-v0.md` §13.

| Action | Purpose | Risk |
|---|---|---|
| `screenshot` | See what a device is doing. | read — **exempt from frequency limits**, fine even while the user is active |
| `click` / `type` / `move` / `scroll` / `key` | Drive the desktop UI. | mutate, **intrusive** — hijacks the user's input; use sparingly, prefer API/MCP > browser(CDP) > computer-use; **if the user is active on that device, warn first**; destructive desktop actions are `critical` |

## Scheduling (self-managed cron)

The agent creates and manages its own schedules. Schedules persist on the Server and fire even with no CLI connected; a fired job spawns an agent run with the stored prompt + context. Unattended runs follow the policy in `prompts/policy.md` (critical actions are parked, not executed). See `docs/spec-v0.md` §10.2.

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `schedule_create` | Create a schedule. `mandate` = the concrete authorized actions (incl. critical) **inferred from the user's request** at creation, recorded explicitly; fire-time enforcement matches it strictly. | `trigger` (`{cron}`/`{at}`/`{every}`), `prompt`, `mandate`, `context?` (device/project/conversation binding), `label?` | mutate |
| `schedule_list` | List schedules with next/last run. | `enabled?` | read |
| `schedule_show` | One schedule + run history. | `schedule_id` | read |
| `schedule_update` | Enable/disable or modify a schedule. | `schedule_id`, fields to change | mutate |
| `schedule_delete` | Remove a schedule. | `schedule_id` | mutate |

## History & audit / rollback

| Tool | Purpose | Key inputs | Risk |
|---|---|---|---|
| `history_list` | Recent history steps; filter by device/project/session. | `device_id?`, `project_id?`, `session_id?` | read |
| `history_show` | One step + its before/after diff. | `step_id` | read |
| `history_restore_preview` | Preview the diff a restore would apply (no change). | `step_id` | read |
| `history_restore` | Restore a workspace to a recorded version. | `step_id` | mutate (critical if the rollback itself is irreversible in context) |

## Risk class → policy (summary)

| Class | `full_access` (default) | stricter policy |
|---|---|---|
| `read` | direct | direct |
| `mutate` | direct, audited + rollback-backed (`history_step_id`) | `approval_required` → re-call with `approval_id` |
| `critical` | **blocked pending explicit user confirmation** | blocked pending explicit user confirmation |

Cross-device tasks add their own gates regardless of class: lock single-owner resources (serial / GPU / USB / container / workspace) before use, unlock after, honor timeouts, and clean up executor scratch on completion (see `prompts/protocol.md` → Cross-Device Tasks).
