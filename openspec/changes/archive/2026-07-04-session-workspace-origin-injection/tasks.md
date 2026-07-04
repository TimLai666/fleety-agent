## 1. 將 origin 原始欄位存入 WorkspaceBinding

- [x] 1.1 測試先行:為 WorkspaceBinding 加序列化往返測試 `binding_roundtrip_preserves_origin`(帶 origin_cwd/origin_hostname/origin_os 往返後保留)與 `old_binding_json_loads_origin_as_none`(缺這些欄位的舊 JSON 反序列化為 None),此時應編譯後失敗。驗證:`cargo test -p fleety-server binding_roundtrip_preserves_origin old_binding_json_loads_origin_as_none` 先紅。
- [x] 1.2 為 WorkspaceBinding 新增 `origin_cwd`/`origin_hostname`/`origin_os`(皆 `Option<String>`,以 `#[serde(default)]` 向後相容),使既有已持久化 binding 缺欄位時載回 None,這是「Requirement: Conversation works in the originating directory and device」所需的綁定資料。驗證:1.1 兩個測試轉綠。
- [x] 1.3 測試先行:加 `resolve_binding_records_origin_fields`,斷言 resolve_binding 於同主機與跨裝置兩情境都回傳填好 origin_cwd/origin_hostname/origin_os 的 binding,先紅。驗證:`cargo test -p fleety-server resolve_binding_records_origin_fields` 先紅。
- [x] 1.4 擴充 resolve_binding 簽章以接收 origin os(cwd/hostname 已是入參),並在同主機與跨裝置兩分支填入 origin 欄位;更新既有 resolve_binding 測試的呼叫。驗證:1.3 測試轉綠,且 workspace 模組既有測試全綠。

## 2. 以 ephemeral system preamble 每輪注入 origin,而非寫入對話歷史

- [x] 2.1 測試先行:為純函式 origin preamble 產生器加 `origin_preamble_same_host`(文字含 cwd、標示本機、不含 device_exec 指引)、`origin_preamble_cross_device`(含 origin device id、cwd,且指示用 `device_exec(device=<id>)`)、`origin_preamble_absent`(無可用 origin 時回傳 None),此時應紅。驗證:`cargo test -p fleety-server origin_preamble_` 先紅。
- [x] 2.2 實作純函式依「同主機與跨裝置採不同注入措辭」產生 origin preamble:同主機標示工具已 root 在 cwd,跨裝置指示裸工具在 server、需 device_exec 到 origin device;無 origin 回傳 None。驗證:2.1 三個測試轉綠。
- [x] 2.3 在每輪組 turn 的 ephemeral system preamble 區(核心記憶 / 當前時間 `Message::system` 旁)push origin preamble,資料取自持久化 binding、不 append 進 conversation,並在對話綁定點把 origin 存入 binding——落實「Requirement: Runtime injects origin context into each turn」。驗證:新增/更新測試斷言組 turn 的 messages 於跨裝置情境含 origin system 段、且該段不出現在 storage.load 的持久歷史(`cargo test -p fleety-server turn_injects_ephemeral_origin`);同主機情境亦含對應措辭。

## 3. 交由 agent 自行 device_exec,不在分派層自動路由

- [x] 3.1 守護測試:加 `cross_device_stays_server_rooted`,斷言跨裝置時 binding.root 為 server fallback、binding.device 記錄 origin device,且無任何裸工具被自動改路由(維持現行分派)——固定「交由 agent 自行 device_exec,不在分派層自動路由」的行為,對應 spec scenario「cross-device tools stay server-rooted and defer to device_exec」。驗證:`cargo test -p fleety-server cross_device_stays_server_rooted` 綠。
- [x] 3.2 內容審查:確認 2.2 的跨裝置注入文字與 `prompts/protocol.md` 既有 Origin Awareness 指引一致,能驅動 spec scenario「cross-device origin drives device_exec and reading origin instructions」——即 agent 依注入的 device+cwd 用 device_exec 逐層讀 origin 的 `AGENTS.md`/`CLAUDE.md`;若措辭有落差則對齊注入文字(不擴張改 protocol.md)。驗證:對照 protocol.md 與注入文字的人工審查紀錄,確認兩者不矛盾。

## 4. resume 一致性

- [x] 4.1 測試先行 + 實作:加 `resume_reinjects_persisted_origin`,斷言 resume 一個已綁定對話、下一則訊息不帶 origin 時,origin preamble 由持久化 binding 重建且與 resume 前一致——對應 spec scenario「origin persists across resume」;若 binding 為舊格式(無 origin 欄位)則退化為省略 origin 段而不報錯。驗證:`cargo test -p fleety-server resume_reinjects_persisted_origin` 由紅轉綠。

## 5. 全量驗證

- [x] 5.1 跑全 workspace 測試與 lint,確認無回歸且無新 `unwrap_used`/`expect_used`(非測試碼)。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
