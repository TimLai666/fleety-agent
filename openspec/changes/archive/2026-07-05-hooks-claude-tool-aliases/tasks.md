## 1. Claude→Fleety 工具名別名比對 — 需求 Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks

- [x] 1.1 (紅) 在 `crates/fleety-server/src/hooks_compat.rs` 寫測試 `matcher_maps_claude_tool_names`:斷言 `matches("Bash","run_command")`、`matches("Read","read_file")`、`matches("Write","write_file")`、`matches("Edit","edit_file")`、`matches("LS","list_dir")` 皆真;`matches("Bash","read_file")` 假;未知 `matches("Frobnicate","run_command")` 假、`matches("run_command","run_command")` 仍真
- [x] 1.2 (綠) 加 `fn claude_alias(matcher: &str) -> Option<&'static str>` 別名表(Bash→run_command、Read→read_file、Write→write_file、Edit／MultiEdit→edit_file、LS→list_dir、Glob／Grep→search_files、WebFetch→fetch_url);`matches` 在 `*`／空／精確相等外,再認 `claude_alias(matcher) == Some(tool_name)`,讓 1.1 通過(實現需求 Reuse an originating device's Claude Code PreToolUse/PostToolUse hooks 的別名比對)
- [x] 1.3 跑 `cargo test -p fleety-server`(含既有 hook 測試不回歸)、`cargo clippy -p fleety-server` 無新違規
