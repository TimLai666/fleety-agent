## 1. 純函式決定指令檔蒐集集合與逐層順序

- [x] 1.1 測試先行:加 `collect_instruction_paths_layers_and_dedupes`,斷言給定 project_root / cwd / user_home 時,回傳「root→cwd 每層的 AGENTS.md 與 CLAUDE.md,再加 ~/.claude/CLAUDE.md 與 ~/.agents/AGENTS.md」的有序(淺→深)且去重路徑清單,先紅。驗證:`cargo test -p fleety-server collect_instruction_paths_layers_and_dedupes` 先紅。
- [x] 1.2 在新模組實作蒐集純函式,產生上述候選路徑清單——這是「Requirement: Runtime injects project and user instruction files into a conversation」的路徑蒐集基礎。驗證:1.1 測試轉綠。

## 2. 去重與大小上限避免 context 爆量

- [x] 2.1 測試先行:加 `collect_skips_missing_and_caps_size`,斷言缺檔的路徑被略過、單檔超過每檔位元組上限時裁切並帶截斷標記、整體超過總量上限時後續檔裁切/省略並標記,先紅。驗證:`cargo test -p fleety-server collect_skips_missing_and_caps_size` 先紅。
- [x] 2.2 實作讀取與裁切:對蒐集路徑讀內容、集合去重、套用每檔與總量上限(具名常數、可經環境變數覆寫)並標截斷——落實「Requirement: Injection is deduplicated and size-bounded」與 design「去重與大小上限避免 context 爆量」。驗證:2.1 測試轉綠。

## 3. 跨裝置經 device_exec 讀回發起端指令檔

- [x] 3.1 讀取層依 origin device 決定來源:同主機讀本機、origin 在別台時經 device_exec 從該裝置讀回檔案內容;跨裝置讀取失敗或裝置離線時略過該來源並加簡短註記、不阻斷對話——落實 design「跨裝置經 device_exec 讀回發起端指令檔」。驗證:`cargo test -p fleety-server cross_device_read_failure_is_skipped`(以離線/失敗來源斷言被略過且不 panic、對話續行)。

## 4. 綁定時注入初始樹與 user 全域,離開初始樹再按需補注

- [x] 4.1 綁定時注入初始樹(root→cwd)與 user 全域一次,內容掛每輪 ephemeral preamble 重放——落實「Requirement: Runtime injects project and user instruction files into a conversation」的注入時機與防洗。驗證:`cargo test -p fleety-server initial_tree_injected_at_bind`(綁定後 preamble 含各層與 user 全域內容)、`injection_survives_compaction`(壓縮後每輪仍重放)。
- [x] 4.2 按需補注:當 agent 讀到初始樹以外目錄的檔案時,注入該目錄鏈尚未注入的指令檔,且不重掃全樹、不重複已注入路徑——落實「Requirement: Out-of-tree directories are covered on demand」與 design「綁定時注入初始樹與 user 全域,離開初始樹再按需補注」。驗證:`cargo test -p fleety-server on_demand_appends_out_of_tree_dir`。

## 5. 注入走 ephemeral 每輪重放且作用域僅限該對話

- [x] 5.1 蒐集集合作為該對話執行期狀態,注入內容掛每輪 ephemeral system preamble 且不 append 進 conversation history,不外洩到其他對話——落實 design「注入走 ephemeral 每輪重放且作用域僅限該對話」。驗證:`cargo test -p fleety-server injection_is_per_conversation`(兩個綁定不同專案/裝置的對話,各自 preamble 只含自身指令檔、且注入內容不出現在 storage.load 的持久歷史)。

## 6. 全量驗證

- [x] 6.1 跑全 workspace 測試與 lint,確認無回歸且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
