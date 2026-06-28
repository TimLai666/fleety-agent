## Context

fleety 核心是 Rust，但部分功能需外部執行環境/工具：ddgs（內建搜尋 MCP，Python 套件）、node/python 型外部 MCP server 與 skill 腳本、insyra sidecar。既有只有 ddgs 在 server boot 做偵測＋best-effort 安裝（crates/fleety-server/src/builtin_mcp.rs：pipx/pip --user，env FLEETY_DDGS_AUTO_INSTALL，非阻擋），insyra 有 provision 下載（crates/fleety-daemon/src/provision.rs）。缺：通用框架、daemon 端、以及「語言執行環境本身缺失」的處理。使用者已拍板：用**受管可攜版**裝 runtime（免 root、不污染系統、boot 即可成功）—— 因為服務以使用者身分跑（systemd --user / launchd agent）、boot 非互動，OS 套件管理員裝 runtime 在 Linux/Windows 多半要 sudo/admin，不適合。

## Goals / Non-Goals

**Goals:**
- fleetyd 與 fleety-server 啟動時自檢並確保各自需要的依賴；best-effort、非阻擋、可 env 關閉。
- 缺 node/python 時裝**受管可攜版**到 fleety 目錄並注入 PATH，免 root、不污染、子行程可用。
- 受管二進位（insyra）下載、使用者層套件（ddgs）安裝沿用既有做法，折入同一框架。
- 平台 URL/路徑/PATH/清單等以純函式呈現、可單元測試；agent-core 不受影響、維持 host-free。

**Non-Goals:**
- 不用 OS 套件管理員裝 runtime（需 sudo/admin、侵入；使用者已否決）。
- 不修改使用者系統 PATH/設定；只在 fleety 服務行程內注入 PATH。
- 不阻擋啟動（任何依賴失敗都繼續跑，回可行動訊息）。
- 不改 agent loop／協定；不擴充到 node/python/insyra/ddgs 以外的 runtime（框架可擴充，本變更條目固定這些）。

## Decisions

### 依賴 ensure 框架（清單＋偵測＋策略＋開關，啟動 best-effort 非阻擋）

在 fleety-tools 新增 deps 模組：`Dependency { name, probe, strategy, env_key }`，與 `ensure_all(set)`。`probe` 偵測（在含受管目錄的 PATH 上跑 `<cmd> --version` 是否成功，比照 ddgs_runs）。啟動時 fleetyd / fleety-server 各自呼叫 `ensure_all` 於自己的子集；每項：已在 → 完成（ddgs 維持背景升級）；缺且自動安裝開 → 跑 strategy；失敗 → log 可行動訊息。整體 best-effort 非阻擋（不讓任何一項擋住服務啟動）。全域開關 `FLEETY_AUTO_INSTALL_DEPS=0` 關閉全部；既有 `FLEETY_DDGS_AUTO_INSTALL` 折入並沿用。

### 安裝策略分層（皆免 root、不污染系統）

三種 strategy：
- **ManagedBinary**：下載單一二進位到 fleety 目錄（insyra 既有 provision 模式）。
- **UserPackage**：使用者層套件安裝（ddgs：pipx → pip --user，既有 try_install_ddgs）。
- **ManagedRuntime**：語言執行環境裝受管可攜版（見下）。

### 受管可攜 runtime：python 經 uv standalone、node 官方 portable，裝到 fleety 目錄並注入 PATH

受管根目錄 `~/.fleety/runtimes/`（env `FLEETY_RUNTIMES_DIR` 可覆寫）。
- **python**：先確保 `uv`（Astral 的單一靜態二進位，按平台下載到受管目錄，比照 insyra 的 target/URL 模式）；再 `uv python install` + 建受管 venv（`uv venv <runtimes>/py`），其 bin 即 python/pip。ddgs 可改裝進此 venv（或維持 pip --user）。
- **node**：下載官方 portable 發行檔（nodejs.org/dist 的 `node-v<ver>-<os>-<arch>.{tar.xz|zip}`，按平台），解壓到 `<runtimes>/node`，其 bin 含 node/npm/npx。
- **PATH 注入**：服務啟動時把受管 runtime 的 bin 目錄 prepend 進**自己行程的 PATH 環境變數**，於是它 spawn 的子行程（mcp_call、skill 的 run_command、ddgs）都找得到。不改使用者系統 PATH。

**替代方案：** OS 套件管理員——否決（sudo/admin、侵入、boot 非互動拿不到權限）；只偵測+提示——否決（不符「沒裝就自動裝」）。

### daemon 與 server 的依賴子集與 env 開關

- **server boot**：python(ManagedRuntime) + ddgs(UserPackage，裝進受管 python 或 pip --user) + node(ManagedRuntime) + insyra(ManagedBinary)。
- **fleetyd boot**：insyra(ManagedBinary，裝置端 insyra_exec) + 可選 python/node(ManagedRuntime，給裝置端 skill/MCP)。
清單與啟用以 env 控制：全域 `FLEETY_AUTO_INSTALL_DEPS`（預設 on）、`FLEETY_DEPS`（逗號清單覆寫該服務要確保的項目）、`FLEETY_RUNTIMES_DIR`。

### 與既有 ddgs / insyra 整合（折成框架條目）

ddgs 變成一個 `Dependency{ name:"ddgs", probe:ddgs_runs, strategy:UserPackage }`（保留 try_install/upgrade 與 FLEETY_DDGS_AUTO_INSTALL）；insyra 變成 `ManagedBinary`（沿用 provision 的 URL/target/dest 邏輯）。行為不變，只是改由框架統一驅動。

## Implementation Contract

**Behavior:** fleetyd / fleety-server 啟動時，對各自的依賴清單逐項偵測；缺的且自動安裝開啟時，照其策略安裝（runtime → 受管可攜裝到 `~/.fleety/runtimes/` 並注入本行程 PATH；套件 → 使用者層；二進位 → 下載到 fleety 目錄）。裝得成 → 之後該服務 spawn 的子行程能呼叫 node/python/ddgs/insyra。裝不成（離線、下載失敗、平台不支援）→ 記錄可行動訊息、**繼續啟動服務**。`FLEETY_AUTO_INSTALL_DEPS=0` 完全關閉自動安裝（僅偵測）。不修改使用者系統。

**Interfaces / data shapes:**
- fleety-tools `deps` 模組：`enum Strategy { ManagedBinary, UserPackage, ManagedRuntime }`；`struct Dependency { name, probe, strategy, env_key }`；`async fn ensure_all(deps: &[Dependency]) -> Vec<EnsureOutcome>`（每項回 installed/already/skipped/failed + 訊息）；純函式：node/uv 的 `target_triple`/下載 URL、受管 bin 目錄路徑、`FLEETY_DEPS`/開關解析、PATH 注入字串組成。
- 受管根：`~/.fleety/runtimes/`（`FLEETY_RUNTIMES_DIR` 覆寫），python venv bin 與 node bin 子目錄。
- env：`FLEETY_AUTO_INSTALL_DEPS`（預設 on）、`FLEETY_DEPS`、`FLEETY_RUNTIMES_DIR`、既有 `FLEETY_DDGS_AUTO_INSTALL`、`FLEETY_INSYRA_URL`。
- 啟動接點：fleety-server main 與 fleety-daemon main 在 boot 早期 spawn/await 一次 ensure（非阻擋：失敗不 return error）。

**Failure modes:** 下載/解壓/uv 安裝失敗 → 該項 failed + 可行動 log，其他項與服務照常。平台不支援（無對應 target）→ skipped + 訊息。受管目錄不可寫 → failed + log。PATH 注入只影響本行程；不改系統。env 關閉 → 全 skipped。永不 panic、永不阻擋啟動。

**Acceptance criteria:**
- 純函式單元測試：node 與 uv 的 target/URL 對映（含不支援平台回 None）、受管 bin 目錄與 PATH-prepend 字串組成、`FLEETY_DEPS`/`FLEETY_AUTO_INSTALL_DEPS` 解析、各 binary 的預設依賴子集。
- 框架邏輯單元測試：以可注入的 probe/installer 模擬「已存在→不裝」「缺→裝→成功」「缺→裝→失敗→不阻擋且回 failed」「全域關閉→只偵測」。
- 整合（可測部分）：fleety-server / fleety-daemon boot 呼叫 ensure 後仍正常啟動（ensure 失敗不影響）。
- 內容審查：docs/env 有新 env 與受管目錄、ddgs/insyra 折入說明。
- 環境相依、需手動驗證（design 標明、不阻擋）：真正下載 uv/node、`uv python install`、解壓、PATH 注入後子行程可呼叫。
- agent-core 不受影響（cargo tree 無 fleety-*）；cargo fmt + clippy --workspace -D warnings + test --workspace 全綠。

**Scope boundaries:**
- In：fleety-tools deps 框架（偵測/策略/開關/PATH 注入）、受管可攜 runtime（uv-python、node-portable）、daemon+server boot 接線、ddgs/insyra 折入、env、docs、純函式與可注入邏輯測試。
- Out：OS 套件管理員安裝、修改使用者系統 PATH/設定、阻擋啟動、agent loop/協定改動、agent-core 改動、node/python/insyra/ddgs 以外的新 runtime。

## Risks / Trade-offs

- [受管 runtime 下載大/慢] → 非阻擋背景進行；已存在則跳過；env 可關。
- [uv / node-portable 各平台取得不一] → 純函式 target/URL 對映 + 不支援平台 skipped；比照 insyra 的成熟模式。
- [PATH 注入只影響本行程] → 刻意如此（不污染系統）；子行程繼承即可，符合需求。
- [boot 時安裝拖慢啟動] → ensure 以背景任務跑、不擋主迴圈；服務先起來，依賴就緒後才可用。
- [離線/受限環境] → 全 best-effort，失敗只記錄；`FLEETY_AUTO_INSTALL_DEPS=0` 給 air-gapped。
- [與既有 ddgs/insyra 行為回歸] → 折入時保留原 env 與安裝鏈，純函式測試守住對映。
