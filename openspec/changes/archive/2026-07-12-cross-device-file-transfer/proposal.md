## Why

Fleety 目前沒有原生的跨裝置傳檔能力:`move_file` 只在單機 workspace 內改名,`read_file`/`write_file` 只吃 UTF-8 文字(binary 讀不了),要把檔案從裝置 A 搬到 B 只能靠 agent 手動組合 device_exec read+write、且限文字檔。要「真正跨裝置工作」,需要一個能傳任意檔案(含 binary)、經 Fleety hub 從一端中繼到另一端、agent 一個工具就能驅動的傳檔能力。

## What Changes

- **binary 位元組檔案工具(fleety-tools,每台裝置與 server 共享)**:新增 `read_file_bytes`(讀檔回 base64 內容 + sha256 + 位元組數)與 `write_file_bytes`(base64 內容寫檔,回 sha256 + 位元組數)。沿用既有 workspace 路徑解析、敏感路徑守門、write 前備份。大小上限 `FLEETY_TRANSFER_MAX_BYTES`(預設 64 MiB)避免 OOM / 過大 frame,超過報明確錯誤。實作抽為純函式,工具與 server 端點共用一份。
- **`transfer_file` 中繼工具(server,與 device_exec 同層)**:參數 `from`(device_id 或 `server`)、`from_path`、`to`(device_id 或 `server`)、`to_path`、`overwrite?`。從來源端讀位元組(裝置端經既有 RunTool 分派 `read_file_bytes`,server 端直接呼叫共用函式),寫到目的端(同理),兩端 sha256 比對,不符即報損毀、不留半套。回傳傳輸位元組數與 sha256。支援 device↔device、device↔server、server↔device(agent 檔案多在 server workspace,server 當端點是常見情境)。
- device 端把新工具納入 advertised tool 清單(register_workspace 註冊即隨 Hello 廣播,device_exec 的 strict check 自然放行)。docs/env.md 記 `FLEETY_TRANSFER_MAX_BYTES` 與 transfer_file/位元組工具。

## Non-Goals

- 不做串流/分塊(v1 是整檔 base64 經一次 tool result;大檔靠上限擋,分塊列後續)。
- 不做三台以上鏈式傳輸、不做目錄遞迴傳輸(單檔;目錄由 agent 逐檔或先打包)。
- 不改 wire protocol(重用 RunTool/ToolResult,無新 frame)。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `filesystem-tools`: 新增 `read_file_bytes` / `write_file_bytes`(base64、sha256、大小上限、沿用路徑守門與備份),補足 binary 與跨裝置搬移所需的位元組級讀寫。
- `device-registry-and-routing`: 新增 `transfer_file` 中繼工具——在兩個端點(連線中的裝置或 server)間傳單一檔案,經 hub 中繼、sha256 校驗。

## Impact

- Affected specs: `filesystem-tools`、`device-registry-and-routing`
- Affected code:
  - Modified:
    - crates/fleety-tools/src/lib.rs
    - crates/fleety-server/src/bridge.rs
    - crates/fleety-server/src/conn.rs
    - crates/fleety-tools/src/config.rs
    - docs/env.md
  - New: （無）
  - Removed: （無）
- 相容性:純新增工具,無 wire 變更;舊 daemon 不廣播新工具則 transfer_file 對它報「未廣播」的既有錯誤。文字工具與 device_exec 行為不變。
- 安全:位元組工具沿用敏感路徑守門與 workspace 範圍;大小上限防 OOM;sha256 校驗防中繼損毀;傳輸屬 Mutate 風險等級(寫入端),走既有 audit/rollback(write 前備份)。
