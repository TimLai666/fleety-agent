## Context

跨裝置分派原語已備:`bridge::route_run_tool_via(sender, pending, tool, args)` 送一個 RunTool 到某連線的 sender、以 call_id 對應等回 ToolResult;`DeviceExec` 靠它 + hub 查 sender 分派。工具共享:`register_workspace(registry, root, backups)`(fleety-tools/lib.rs)把 read_file/write_file/… 註冊給 server 與每台 daemon;daemon 於 Hello 廣播 local_tools_json,device_exec 對已廣播裝置做 strict tool-name check。既有內部 helper:`resolve_in_root`/`resolve_for_write`(workspace 範圍解析)、`guard_sensitive`(敏感路徑守門)、`backup_existing`(write 前備份)。deps 已有 base64、sha2。現況缺口:read_file 是 read_to_string(UTF-8),無 binary 路徑;無檔案傳輸工具。

## Goals / Non-Goals

**Goals:**
- 位元組級讀寫工具(含 binary),沿用既有路徑守門與備份。
- 一個 `transfer_file` 工具在兩端點(裝置或 server)間搬單一檔案,經 hub 中繼、sha256 校驗、大小上限。
- 位元組讀寫的實作單一份,工具與 server 端點共用。

**Non-Goals:**
- 不做串流/分塊(整檔 base64 一次過;上限擋大檔)。
- 不做目錄/遞迴/多跳。
- 不改 wire protocol。

## Decisions

### 決策一:位元組讀寫抽為純函式 + 薄工具

fleety-tools 新增純函式:`read_file_bytes_at(root, rel) -> Result<Value>` 回 `{content_b64, sha256, bytes}`(resolve_in_root → 讀 bytes → 檢查上限 → base64 + sha256);`write_file_bytes_at(root, backups, rel, content_b64, overwrite) -> Result<Value>` 回 `{sha256, bytes, backup?}`(base64 decode → 檢查上限 → resolve_for_write + guard_sensitive → backup_existing(除非 !overwrite 且已存在則報錯或依 overwrite 語義)→ 寫)。工具 `ReadFileBytes`/`WriteFileBytes` 是薄 wrapper,register_workspace 一併註冊(隨 Hello 廣播、device_exec 可分派)。大小上限 `transfer_max_bytes()` 讀 `FLEETY_TRANSFER_MAX_BYTES`(預設 64*1024*1024),超過報含實際大小與上限的錯誤。

- 純函式讓 server 的 transfer_file 對「server 端點」直接呼叫(不繞工具註冊表),與裝置端(經 RunTool 跑同一函式)行為一致。

### 決策二:transfer_file 中繼工具(server)

在 bridge.rs 新增 `TransferFile` 工具,與 device_exec 同層註冊(build_connection_stack),持有 hub、pending、server workspace root、backups。參數 `from`、`from_path`、`to`、`to_path`、`overwrite?`。端點解析:值為 `server`(或空)→ server 本機(直接呼叫純函式,root=連線 workspace);否則視為 device_id → 經 hub 查 sender、route_run_tool_via 分派 `read_file_bytes`/`write_file_bytes`。流程:讀來源 → 拿 content_b64+sha256 → 寫目的(帶 content_b64)→ 拿寫入 sha256 → 比對;不符回「transfer corrupted (sha256 mismatch)」且不視為成功。回 `{bytes, sha256, from, to}`。risk=Mutate。

- 否決新增檔案傳輸 frame + 分塊:v1 重用 RunTool 最小面;整檔上限先擋大檔,分塊列後續(Non-Goal)。
- 否決只支援 device↔device:agent 檔案多在 server workspace,server 當端點是主要情境。

### 決策三:失敗與校驗

來源不存在/讀失敗 → 來源端工具錯誤(既有可讀形式)。目的端寫失敗(權限/磁碟/敏感路徑)→ guard/寫入錯誤。sha256 不符 → 明確錯誤,目的端可能已寫入(但 write 前有備份,可 rollback);訊息提示重試。裝置未連線 → device_exec 既有「not connected」錯誤。裝置未廣播新工具(舊 daemon)→ 既有「did not advertise」錯誤,指引升級。

## Implementation Contract

**行為(操作者視角):**
- agent 呼叫 `transfer_file {from, from_path, to, to_path}`:把 from 裝置(或 server)的檔案傳到 to 裝置(或 server),回傳位元組數與 sha256。可傳 binary。
- 兩端 sha256 一致才算成功;不一致回損毀錯誤。
- 超過 `FLEETY_TRANSFER_MAX_BYTES` → 讀/寫端報含大小與上限的錯誤,不傳。
- `read_file_bytes`/`write_file_bytes` 也可經 device_exec 單獨用(讀/寫某裝置的 binary 檔)。

**介面與資料形狀:**
- fleety-tools:`pub fn read_file_bytes_at(&Path, &str) -> Result<Value>`;`pub fn write_file_bytes_at(&Path, &Path, &str, &str, bool) -> Result<Value>`;`transfer_max_bytes() -> usize`。工具 `read_file_bytes`(參數 path)、`write_file_bytes`(path、content_b64、overwrite?)。
- server:`transfer_file`(from、from_path、to、to_path、overwrite?);`server`/空 = 本機端點。register 於 build_connection_stack(頂層,子註冊表不含——與 device_exec 一致?device_exec 子代有,transfer_file 依 device_exec 慣例決定,apply 時對齊)。
- config registry 加 `FLEETY_TRANSFER_MAX_BYTES`。

**失敗模式:**
- 大小超限、路徑守門拒、裝置未連線/未廣播、sha256 不符——皆回可讀錯誤,寫入端有 backup 可 rollback。

**驗收準則:**
- cargo test:read/write_file_bytes_at round-trip(binary bytes → b64 → 寫 → 讀回一致、sha256 相符);大小上限拒(超限報錯不寫);敏感路徑守門仍擋;transfer_max_bytes env 解析。transfer_file 的端點解析與 sha256 比對邏輯抽可測部分(server↔server 本機路徑以純函式覆蓋;device 分派沿專案手動/整合測試姿態)。
- 既有 filesystem-tools 與 device_exec 測試不回歸。
- 全 workspace test/clippy/fmt 乾淨。
- 端到端(發版後人工):兩台裝置間傳一個 binary(如小圖),sha256 一致;server↔device 兩向各一次。

**範圍邊界:**
- 範圍內:crates/fleety-tools/src/lib.rs、crates/fleety-server/src/bridge.rs、crates/fleety-server/src/conn.rs、crates/fleety-tools/src/config.rs、docs/env.md。
- 範圍外:串流/分塊、目錄傳輸、多跳、wire protocol、CLI 命令。

## Risks / Trade-offs

- [整檔 base64 佔記憶體 + 過大 tool result] → 上限擋(預設 64 MiB,可調);分塊列後續。
- [sha256 不符時目的端已寫入] → write 前 backup + rollback 可還原;訊息提示。可接受(比靜默損毀好)。
- [server 端點用連線 workspace root,對話重綁 cwd 後語義] → 與既有檔案工具一致(同 root),文件說明。

## Migration Plan

單版出貨,無資料遷移。舊 daemon 升級後才廣播新工具;升級前 transfer_file 對它報未廣播。回滾 revert。

## Open Questions

- 無阻斷項。
