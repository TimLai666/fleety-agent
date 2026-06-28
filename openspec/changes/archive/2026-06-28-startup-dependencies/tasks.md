<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同 crate、無相依）。真實下載/解壓 runtime 與 PATH 注入後子行程可呼叫為環境相依，需手動驗證。 -->

## 1. 依賴框架（fleety-tools）

- [x] 1.1 在 crates/fleety-tools 新增 deps 模組：Strategy { ManagedBinary, UserPackage, ManagedRuntime }、Dependency { name, probe, strategy, env_key }、async ensure_all（逐項偵測→缺且開啟則安裝→回 already/installed/skipped/failed）、純函式（各 binary 預設依賴子集、FLEETY_AUTO_INSTALL_DEPS/FLEETY_DEPS 解析、受管 bin 目錄路徑與 PATH-prepend 字串），整體 best-effort 非阻擋——交付 "Startup checks and ensures dependencies" 與 "Auto-install is best-effort and configurable"（決策「依賴 ensure 框架（清單＋偵測＋策略＋開關，啟動 best-effort 非阻擋）」「安裝策略分層（皆免 root、不污染系統）」「daemon 與 server 的依賴子集與 env 開關」）。驗證:以可注入 probe/installer 模擬 已存在/缺→裝成功/缺→裝失敗不阻擋/全域關閉只偵測;cargo test -p fleety-tools 全綠。
- [x] 1.2 在 deps 模組實作 ManagedRuntime：python 經 uv（下載 uv 受管二進位 + 建受管 venv）、node 下載官方 portable 解壓到 ~/.fleety/runtimes/（FLEETY_RUNTIMES_DIR 可覆寫），並把 bin 目錄 prepend 進本行程 PATH——交付 "Missing runtimes install as managed portable, no root"（決策「受管可攜 runtime：python 經 uv standalone、node 官方 portable，裝到 fleety 目錄並注入 PATH」）。驗證:node/uv 的 target/下載 URL/受管路徑/PATH 字串純函式單元測試（含不支援平台回 None）;真實下載/解壓/uv install 標為手動驗證。
- [x] 1.3 把既有 ddgs（UserPackage，保留 try_install/upgrade 與 FLEETY_DDGS_AUTO_INSTALL）與 insyra（ManagedBinary，沿用 provision 的 target/URL/dest）折成框架條目，行為不變——交付:既有依賴統一由框架驅動（決策「與既有 ddgs / insyra 整合（折成框架條目）」）。驗證:對映/行為純函式測試;builtin_mcp 與 provision 既有測試仍綠;cargo test 全綠。

## 2. boot 接線

- [x] 2.1 [P] 在 crates/fleety-server/src/main.rs 啟動早期以背景任務跑 ensure_all(server 子集：python+ddgs+node+insyra)，ensure 失敗不影響服務啟動——交付 server 端 "Startup checks and ensures dependencies"。驗證:cargo build -p fleety-server 綠;「ensure 失敗→服務仍啟動」的非阻擋邏輯測試/審查。
- [x] 2.2 [P] 在 crates/fleety-daemon/src/main.rs 啟動早期以背景任務跑 ensure_all(daemon 子集：insyra + 可選 python/node)，非阻擋；與既有 provision::ensure_insyra 整併避免重複——交付 daemon 端 "Startup checks and ensures dependencies"。驗證:cargo build -p fleety-daemon 綠。

## 3. 文件

- [x] 3.1 docs/env.md 增 FLEETY_AUTO_INSTALL_DEPS / FLEETY_DEPS / FLEETY_RUNTIMES_DIR 與受管 runtime 目錄說明、ddgs/insyra 折入說明——交付:文件與行為一致。驗證:內容審查。

## 4. 整體驗收

- [x] 4.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄實機下載/PATH 注入需手動驗證。
