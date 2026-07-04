## Why

稽核長期把 session-workspace 標為「實作落後文件」,但根因不是實作偷懶,而是原 spec 的方向本身是誤設計。原 spec 承諾「conversation 的 file/command/git 工具 SHALL 預設在 origin device 上、經 device routing 自動執行」。Fleety 是 full-access agent,已能用 `device_exec` 顯式把任一工具路由到指定裝置,不需要在工具分派層硬把裸工具搬到 origin device 執行——那是過度設計。真正缺、也真正該做的,是讓 agent「知道」這則訊息來自哪一台裝置的哪個路徑,由它依 `prompts/protocol.md` 既有指示(target = origin device、需要時 `device_exec`)自行決定在何處操作。

目前 runtime 完全沒把 origin 注入給模型:conn.rs 只用 origin 決定工具的實體 root 與寫 log,沒有把 origin 以模型讀得到的文字呈現。後果是跨裝置時 agent 既不知道該 `device_exec` 到哪,也讀不到 origin 專案的逐層 AGENTS.md / CLAUDE.md(那些檔在別台、server 本地讀不到),於是拿錯的專案慣例改檔而不自知。

## What Changes

- **BREAKING**(spec 需求語意變更,非破壞使用者資料):session-workspace 需求方向從「工具自動經 device routing 在 origin device 執行」改為「runtime 注入 origin context,agent 預設在此操作、跨裝置時自行 device_exec」。
- runtime 每輪把 origin context(device id、hostname、os、cwd,可選 git branch + dirty 狀態)以 ephemeral、不寫入對話歷史的 system message 注入,沿用現有 core-memory / current-time preamble 同款機制,使其每輪重建而不被長 context 的壓縮摘要洗掉。
- WorkspaceBinding 擴充儲存 origin 的原始資訊(cwd 原文、hostname、os),使 resume 與後續 turn(未必再帶 origin)仍能一致重放同一段 origin 提示。
- 同主機與別台的注入措辭區分:同主機時工具已 root 在 cwd,提示說明「origin 即本機」;別台時裸工具在 server 執行,提示明確指出要 `device_exec` 到 origin device 才能動它的檔案。
- session-workspace spec 改寫:把「自動路由」的 SHALL 改為「注入 origin」的 SHALL,並新增「跨裝置時 agent 能經 device_exec 逐層讀 origin device 的 AGENTS.md / CLAUDE.md」的驗證 scenario。

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `session-workspace`: 需求從「工具 SHALL 預設在 origin device/dir 經 device routing 自動執行」改為「runtime SHALL 每輪注入 origin context(device 與 cwd),使 agent 預設在此操作,跨裝置時由 agent 自行 device_exec 路由」;fallback 與 resume 一致性、以及 origin 不可信輸入的既有 requirement 保留。

## Impact

- Affected specs: session-workspace
- Affected code:
  - Modified:
    - crates/fleety-server/src/workspace.rs — WorkspaceBinding 擴充 origin 欄位(cwd 原文、hostname、os)、resolve_binding 保留並回傳這些欄位
    - crates/fleety-server/src/conn.rs — 每輪在 ephemeral system preamble 注入 origin 提示;綁定時把 origin 存入 binding
    - crates/fleety-server/src/storage.rs — conversation workspace binding 的序列化擴充新欄位,resume 時載回
    - openspec/specs/session-workspace/spec.md — 需求改寫(archive 時由 spec delta 落地)
  - New: (none)
  - Removed: (none)
