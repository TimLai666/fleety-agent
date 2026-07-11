## Why

skill sync 的 SHA 短路（本地記錄的 commit SHA 等於遠端就跳過下載）有一個壞狀態會被鎖死：synced tier 被清空但 SHA 檔仍記著當下的 commit —— 例如被 plugin 佈局改版前的舊版 server 清空過的機器。升級到新版後，只要 skills repo 沒有新 commit，SHA 相同就永遠跳過下載，空的 tier 不會自癒，只能等上游任意一次 push 或人工刪目錄。空 tier 配上有效 SHA 幾乎必然是故障殘留，不是使用者想要的狀態。

## What Changes

- SHA 短路增加一個生效前提：**本地 synced tier 至少含有一個 skill 目錄**。tier 為空（不存在、或只剩 SHA 記錄檔）時，同步視同「沒有記錄過 SHA」—— 照樣下載重建，故障殘留在下一次同步（含開機那次）自動復原。
- 判斷「tier 是否為空」是純函式：synced 目錄下有沒有任何子目錄（tier 只會由 skill 目錄構成）。
- 寫入端行為不變：重建後照樣記錄 SHA（包括空集合），mirror 語義與原子換入不動；真正空的來源 repo 會每個週期重抓一次 zip（每小時一次，成本可忽略），這是刻意的 —— 空 repo 幾乎必然是錯誤狀態，值得持續重試。

## Non-Goals

- 不在寫入端「空集合就不記 SHA」：那對已經被清空的既有機器無效（SHA 檔早已存在），且讀取端的守門已完整覆蓋兩種情境。
- 不做「空集合時拒絕換入、保留舊 tier」：那會違反 mirror 語義（上游真的清空 repo 時本地應跟著空），也會讓探索規則故障時舊內容永不更新。
- 不加新的設定項或告警通道：既有 log 已足夠。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `synced-skill-tier`: 「Syncing is conditional on the repo's latest commit」條款 —— SHA 短路只在本地 tier 非空時生效；空 tier 視同無 SHA 記錄，必定下載重建。

## Impact

- Affected specs: synced-skill-tier（MODIFIED）
- Affected code:
  - Modified: crates/fleety-server/src/skill_sync.rs、docs/env.md
  - New: 無
  - Removed: 無
