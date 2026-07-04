## Why

② 讓對話復用發起端 Claude Code plugin 的 skills + MCP。使用者也用 **Codex**,而 Codex 的 MCP servers(`~/.codex/config.toml`)與指令檔(`~/.codex/AGENTS.md`)目前對話用不到。把相容層延伸到 Codex 的宣告式資源(MCP + AGENTS.md),讓對話也復用使用者在 Codex 已設定好的東西,與 Claude Code 側對稱。

## What Changes

- 新純函式模組 **codex_sources**:讀 `~/.codex/config.toml` 的 `[mcp_servers.<name>]`(TOML,command/args/env)→ 轉成 Fleety 的 `{name,command,args}`。best-effort(缺檔/壞 TOML/格式不符 → 略過)。
- Codex 的 MCP servers 以 **per-conversation** 方式併入 `mcp::register`(沿用 ② 的對話級 server 來源機制),歸 **使用者 scope**(Codex 是使用者級設定)。
- `~/.codex/AGENTS.md` 併入指令檔注入的 **user 全域層**(instruction-file-injection 目前 user 全域讀 `~/.claude/CLAUDE.md`、`~/.agents/AGENTS.md`,再加 `~/.codex/AGENTS.md`)。
- 全程 best-effort、同主機首版(讀本機 `~/.codex`);跨裝置經 device_exec 列後續。

## Non-Goals (optional)

(詳見 design.md;關鍵排除:不做 hooks(④)、不做 Codex prompts / custom commands(slash 概念與 skills 不對映)、不做 Codex skills(格式未定)、跨裝置列後續、不引入 Codex 安裝機制。)

## Capabilities

### New Capabilities

- `codex-compat`: 解析發起端 Codex 的 `~/.codex/config.toml` MCP servers 與 `~/.codex/AGENTS.md`,供對話復用(使用者 scope、best-effort)。

### Modified Capabilities

- `mcp-servers`: 對話可額外看到 Codex `config.toml` 宣告的 MCP servers(per-conversation 併入,使用者 scope)。
- `instruction-file-injection`: user 全域指令檔多讀一個 `~/.codex/AGENTS.md`。

## Impact

- Affected specs: codex-compat(new)、mcp-servers、instruction-file-injection
- Affected code:
  - New:
    - crates/fleety-server/src/codex_sources.rs — 純函式:讀 Codex config.toml → Fleety MCP servers、best-effort
  - Modified:
    - crates/fleety-server/src/conn.rs — build_connection_stack 綁定時把 Codex MCP servers 併入對話級 conversation_mcp
    - crates/fleety-server/src/instructions.rs — collect_instruction_paths 的 user 全域層加入 `~/.codex/AGENTS.md`
    - crates/fleety-server/Cargo.toml — 加 toml crate 依賴(workspace 已有,fleety-tools 已用)
  - Removed: (none)
