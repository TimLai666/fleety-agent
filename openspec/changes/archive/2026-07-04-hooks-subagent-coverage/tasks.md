## 1. conn 抽出共用包裝點 — 決策一:抽出 `HookContext` 與 `wrap_registry_with_hooks`；需求 Hooks apply to subagent tool calls

- [x] 1.1 在 `crates/fleety-server/src/conn.rs` 定義 `pub(crate) struct HookContext { hooks: Vec<hooks_compat::HookEntry>, device: Option<String>, cwd: Option<String> }`
- [x] 1.2 在 conn 定義 `pub(crate) fn wrap_registry_with_hooks(tools: &mut ToolRegistry, ctx: &HookContext, hub, pending, storage, device_id, conversation)`:hooks 空即 return;否則建 `OriginHookRunner` + `HistoryHookAudit`，`wrap_tools(tools.drain(), …)` 後逐一 re-register
- [x] 1.3 (紅) 寫測試 `wrap_registry_with_hooks_denies_on_nonzero_pre`（conn）:device=None、PreToolUse `*` command 跨平台 `exit 1`、真實臨時 Storage、假工具，斷言呼叫回 `denied`、工具未執行、audit 有紀錄;`empty_hook_context_leaves_registry_unwrapped` 斷言空 ctx 為 no-op、工具正常執行
- [x] 1.4 (綠) 讓 1.3 通過:實作 `wrap_registry_with_hooks` 並將主對話綁定點現有 inline 包裝迴圈改呼叫之（DRY），行為不變（既有 fleety-server 測試維持綠）

## 2. FleetyHost 注入 HookContext — 決策二:以設定式 handle 把 HookContext 注入 FleetyHost；需求 Hooks apply to subagent tool calls

- [x] 2.1 在 `crates/fleety-server/src/subagent.rs` 的 `FleetyHost` 增 `hook_ctx: OnceLock<Arc<crate::conn::HookContext>>` 欄位與 `pub fn set_hook_context(&self, ctx: Arc<crate::conn::HookContext>)`（`OnceLock::set` 忽略重複設定的 Err）
- [x] 2.2 在 conn 綁定點:收集 hooks、包好主對話 registry 後，若 hooks 非空則 `Arc::new(HookContext{…})` 一份，主對話包裝與 `subagent_host.set_hook_context(Arc::clone(&ctx))` 共用同一 Arc

## 3. subagent registry 包裝 — 決策三:在 async 呼叫點包裝，audit 掛對的 conversation；決策四:巢狀 subagent 自動覆蓋；需求 Hooks apply to subagent tool calls

- [x] 3.1 (綠) 在 `FleetyHost::child_registry` 建好 base registry 後，若 `hook_ctx` 有值則讀 `active_conversation` 取 conversation，呼叫 `crate::conn::wrap_registry_with_hooks(&mut tools, ctx, &self.hub, &self.pending, &self.storage, &self.device_id, &conv)`（實現需求 Hooks apply to subagent tool calls）
- [x] 3.2 (綠) 在 `FleetyHost::on_complete` 於 `register_orchestration` 之後、`drive_turn` 之前，若 `hook_ctx` 有值則以 `context`（父對話）呼叫 `wrap_registry_with_hooks` 包裝喚醒回合 registry;巢狀 subagent 因共用同一 host 自動沿用（決策四，無需額外碼）

## 4. 驗證

- [x] 4.1 跑 `cargo test -p fleety-server` 全綠（含新 conn 測試與既有 subagent/conn 測試）、`cargo clippy -p fleety-server`（`unwrap_used`/`expect_used` 無新違規）;修正殘留
