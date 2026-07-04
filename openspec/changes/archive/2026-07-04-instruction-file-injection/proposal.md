## Why

Fleety 目前完全沒有指令檔自動注入:`prompts/protocol.md` 只指示 agent 在動檔前自己 `read_file` 逐層讀 AGENTS.md / CLAUDE.md。這在同主機時堪用,但有三個缺口:跨裝置對話讀不到發起端的指令檔(那些檔在別台、server 本地沒有);完全沒有「發起裝置 user 全域」層(~/.claude、~/.agents);而且依賴 agent 記得去讀,不保證。結果是 agent 常在缺少專案與使用者慣例的情況下動手,跨裝置時尤其明顯。這個 change 讓 runtime 主動、保證地把這些指令檔餵給對話,補上 user 層與跨裝置能力。

## What Changes

- 對話綁定時,runtime 自動蒐集「專案根 → origin cwd 逐層」的 AGENTS.md 與 CLAUDE.md,加上「發起裝置的 user 全域」指令檔(~/.claude/CLAUDE.md、~/.agents/AGENTS.md),注入該對話的 context。
- 跨裝置時這些檔在發起裝置上,透過既有 device_exec 從該裝置讀回;同主機直接讀。建在 session-workspace-origin-injection 的 origin 定位之上(**依賴**該 change 先落地)。
- 注入沿用 origin-injection 的 ephemeral 每輪重放 preamble 通道,確保不被長 context 壓縮摘要洗掉;對注入內容去重(同一路徑的檔只注入一次)並設每檔與總量大小上限,避免爆 context。
- 綁定時注入初始樹(root → cwd)與 user 全域一次;之後 agent 讀到初始樹以外的目錄時,才按需補注該目錄鏈的指令檔——不做「每次 read_file 都重掃全樹」的昂貴版本。
- 注入的作用域僅限該對話,不外洩到其他對話。
- 這是對 protocol.md 既有「agent 按需自己讀」指示的**補強**(保證讀到、補上 user 層與跨裝置),不是取代;agent 仍能主動 read_file 讀更深或更新的內容。

## Non-Goals (optional)

(詳見 design.md 的 Goals / Non-Goals;關鍵排除:不載入 skills、不引入 plugin / hook 執行框架——那些屬後續獨立 change。)

## Capabilities

### New Capabilities

- `instruction-file-injection`: runtime 自動蒐集並注入專案逐層與使用者全域的 AGENTS.md / CLAUDE.md 指令檔到單一對話的 context,含跨裝置讀取、去重、大小上限與作用域隔離。

### Modified Capabilities

(none)

## Impact

- Affected specs: instruction-file-injection(新);依賴 session-workspace-origin-injection 提供的 origin 注入與跨裝置定位
- Affected code:
  - New:
    - crates/fleety-server/src/instructions.rs — 純函式決定「給定專案根、cwd、user home 下,要蒐集哪些指令檔路徑、逐層順序、去重集合」;以及大小上限裁切邏輯
  - Modified:
    - crates/fleety-server/src/conn.rs — 綁定時蒐集初始樹 + user 全域指令檔並存入該對話狀態;每輪把去重後的指令檔內容注入 ephemeral system preamble;agent 讀到初始樹外的目錄時按需補注
    - crates/fleety-server/src/workspace.rs — 對話綁定攜帶「已注入指令檔集合」的來源資訊(專案根、user home、origin device),供跨裝置讀取與去重
    - crates/fleety-tools/src/lib.rs — 檔案讀取路徑供跨裝置(device_exec)與本機共用蒐集
  - Removed: (none)
