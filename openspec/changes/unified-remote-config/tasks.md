## 1. 協定訊息(fleety-protocol)

- [x] 1.1 [P] 在 crates/fleety-protocol/src/lib.rs 新增 `ConfigTarget { Server, Local, Device(String) }`、`Effect { NextConnection, Restart }`、單一 ClientMsg `ConfigExec { target: ConfigTarget, args: Vec<String> }`、ServerMsg `ConfigResult{ ok, output, effect: Option<Effect>, error: Option<WireError> }`(對齊既有 AuditList/RollbackList 成對模式),交付 "The CLI manages a connected server's config over the connection" 的線格式面;對應設計「協定:Config* request/result(對齊既有 device 定址模式)」。先寫失敗測試:ConfigExec + ConfigResult + ConfigTarget + Effect 的 serde round-trip(沿用既有協定測試樣式)。

## 2. effect 分類純函式(fleety-tools)

- [x] 2.1 [P] 在 crates/fleety-tools/src/config.rs 新增純函式 `config_effect(args: &[String]) -> Option<ConfigEffect>`(本地 `enum ConfigEffect { NextConnection, Restart }`,因 fleety-tools 不依賴 fleety-protocol;server 端再映射到 protocol Effect)。規則依**動詞**:mutating 的 provider/group/role(args[1] != "list")→ NextConnection;`set`/`unset` → Restart;`list`/`get`/其餘讀取 → None,交付 "Apply-time is reported honestly" 的判定面;對應設計「套用語意:寫檔即持久化,生效時機誠實回報」。先寫失敗測試:用 spec example 表逐列驗證(provider add/group set/role set → NextConnection;set/unset 扁平鍵 → Restart;list/get/provider list → None)。

## 3. server handler(fleety-server)

- [x] 3.1 在 crates/fleety-server/src/conn.rs 處理 Config* 訊息:先過認證(連線未認證且 require_auth → 回 ConfigResult 帶 unauthenticated WireError、不改檔);target=Server → 在 server 的 config_path()/providers_path() 上跑既有 config list/get/set/unset 與 run_providers_at 等價邏輯,擷取輸出/錯誤包成 ConfigResult 並標 effect(用 #2 的 config_effect);target=Device → 回 unsupported 提示為後續;mutate 寫 audit,交付 "The CLI manages a connected server's config over the connection"、"Remote config requires an authenticated connection"、"Local target preserves existing behavior; device target is deferred" 的 device 拒絕面;對應設計「server handler:認證閘 + 在 server host 重用既有 config 函式」與「target 定址:server(預設)/ local;device 留後續」。先寫失敗測試:臨時 config/providers 路徑下 set→list 反映變更、未知鍵→error 且檔案不變、模擬 require_auth 未認證→unauthenticated 不改檔、target=Device→unsupported。

## 4. CLI --target 線路(fleety-cli)

- [x] 4.1 在 crates/fleety-cli(config.rs + main.rs)的 config 進入點解析 `--target server|local`(預設 server);target=Local → 走既有本機路徑(回歸不變);target=Server → 建立(認證)連線送對應 Config* 訊息、印 ConfigResult 的 output 與 effect 提示;連不上 → 明確錯誤並提示 --target local 或到 server host 用 fleety-server config,交付 "The CLI manages a connected server's config over the connection" 與 "Local target preserves existing behavior; device target is deferred" 的 local 面;對應設計「CLI 線路與既有指令相容」。驗證:--target local 走本機(既有 config 測試全綠);--target server 送出正確 Config*(可注入連線/序列化層單元測試,真往返手動)。

## 5. 文件

- [x] 5.1 [P] 更新 docs/env.md 與 README.md 的「Connecting & configuring」:CLI 可用 `config [--target server|local] …` 遠端管 server 設定(含 provider/group/role)、effect(下次連線 vs 需重啟)語意、需認證連線 + 遠端建議 TLS、fleety-server config 仍為 bootstrap、裝置端為後續 change,交付各 requirement 的文件面。驗證:內容審查涵蓋 --target、effect 兩值、認證需求、device 後續、本機後備。
