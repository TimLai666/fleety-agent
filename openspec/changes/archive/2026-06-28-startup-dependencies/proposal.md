## Why

fleetyd 與 fleety-server 在不同機器上跑，某些功能需要外部執行環境/工具才能用：內建搜尋 MCP（ddgs，Python 套件）、外部 node/python 型 MCP server、skill 腳本、insyra 資料分析 sidecar。今天只有 ddgs 在 server boot 時做「偵測＋best-effort 自動裝」，且不通用、不涵蓋 daemon、也不會在缺「語言執行環境本身」時補上。使用者要的是：兩端啟動時都自檢依賴（如 node、python），沒裝就自動裝，且不該每次要 sudo、不該污染使用者系統。

## What Changes

- **通用依賴 ensure 框架**：一份依賴清單，每項定義偵測方式、安裝策略、env 開關；fleetyd 與 fleety-server 啟動時各自確保自己需要的子集。**best-effort、非阻擋**——任何依賴裝不成都不阻止服務啟動，只記錄可行動訊息（沿用既有 never-crash 與 ddgs 的姿態）。
- **安裝策略分層（皆免 root、不污染系統）**：
  - 受管二進位（insyra、未來 whisper.cpp）→ 下載到 fleety 目錄（沿用既有 provision 模式）。
  - 使用者層套件（ddgs）→ pipx / pip --user（沿用既有 builtin_mcp 做法）。
  - **語言執行環境（node / python）→ 受管可攜版**：python 經 `uv` 裝 standalone CPython、node 下載官方 portable，裝到 fleety 受管目錄並加進該服務的 PATH，子行程（MCP/skill）即可找到。免 root、boot 即可成功。
- **daemon / server 各自子集**：server 啟動確保 python+ddgs、node、insyra；daemon 啟動確保 insyra，並可選 python/node（給裝置端 skill/MCP 用）。清單與開關以 env 設定（全域關閉 + 既有 FLEETY_DDGS_AUTO_INSTALL 折入）。
- **與既有整合**：把 ddgs 的 boot 檢查與 insyra 的 provision 折成框架的條目，不改其行為。

## Non-Goals

（細節取捨見 design.md 的 Goals/Non-Goals。）

## Capabilities

### New Capabilities

- `startup-dependencies`: fleetyd 與 fleety-server 啟動時的通用依賴自檢與確保框架——受管二進位下載、使用者層套件安裝、語言執行環境裝受管可攜版（python 經 uv、node 官方 portable）到 fleety 目錄並注入 PATH；best-effort 非阻擋、免 root、不污染系統、可由 env 開關。

### Modified Capabilities

（無。ddgs 與 insyra 的既有行為折成框架條目，不改規格。）

## Impact

- 受影響 specs：新增 startup-dependencies。修改：無。
- 受影響程式：
  - 新增：跨平台依賴框架模組（放 fleety-tools，daemon 與 server 共用），含受管可攜 runtime 安裝（uv/standalone python、node portable）與受管目錄/PATH 注入
  - 修改：crates/fleety-server/src/main.rs（boot 時跑 server 依賴子集）、crates/fleety-server/src/builtin_mcp.rs（ddgs 折入框架）、crates/fleety-daemon/src/main.rs（boot 時跑 daemon 依賴子集）、crates/fleety-daemon/src/provision.rs（insyra 折入框架或沿用）、docs/env.md（依賴開關與受管目錄 env）
  - 移除：無
- 關鍵驗收：缺 node/python 時啟動自動裝受管可攜版到 fleety 目錄、免 root、子行程可用；裝不成不阻擋啟動且回可行動訊息；ddgs/insyra 既有行為不變；可用 env 關閉；agent-core 不受影響仍 host-free；平台 URL/路徑/PATH 的純函式可單元測試；workspace fmt + clippy -D + test 全綠。
- 環境相依、需手動驗證：真正下載並執行 node/uv-python、實機 PATH 注入後子行程可呼叫。
