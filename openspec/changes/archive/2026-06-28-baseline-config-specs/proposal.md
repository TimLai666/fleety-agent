## Why

docs/env.md 記錄了每個 `FLEETY_*` 變數,但只是人類可讀說明,沒有可被 spectra 檢核的契約。其中一大類是**操作/執行期**行為(監聽位址、認證、政策、GC/保留、mDNS 探索、自更新、model provider),這些不屬於 agent 工具面(已由 baseline-tool-surface-specs 管轄),目前完全沒有正規規格。把它們納入 Spectra,讓執行期設定的預設值與行為有單一真相來源。

## What Changes

- 為 docs/env.md 中**操作/執行期**的設定行為建立能力規格(以實際讀取點與預設值為準)。
- 6 個 capability:`runtime-configuration`、`model-provider`、`retention-gc`、`service-discovery`、`device-enrollment`、`self-update`。
- 規格描述「runtime 讀哪個變數、預設值、行為契約」,不重述工具參數。
- **不改任何程式、不改任何變數的預設值或讀取行為。**

## Non-Goals

- **不重複規格已由工具面能力管轄的「能力專屬」變數**:`FLEETY_FS_SCOPE`(屬 filesystem-tools)、`FLEETY_DDGS_*`(屬 mcp-servers)、`FLEETY_CHROME_*`(屬 browser-automation)、`FLEETY_WIKI_EMBED`/`FLEETY_MODELS_DIR`(屬 knowledge-wiki)、`FLEETY_ALLOW_PRIVATE_NET`(屬 web-and-network)。新規格只在需要時引用它們。
- `FLEETY_SYSTEM_PROMPT` 屬系統提示組裝,留給後續變更 baseline-prompt-specs。
- 不重寫或刪除 docs/env.md;它續存為人類可讀參考,specs 為正規真相。
- 不更動任何變數的語意、預設或讀取位置。

## Capabilities

### New Capabilities

- `runtime-configuration`: 伺服器啟動設定 —— `FLEETY_ADDR`(監聽)、`FLEETY_AGENT_HOME`(耐久儲存根)、`FLEETY_WORKSPACE`(相對路徑基準)、`FLEETY_POLICY`(full_access vs require_approval 閘控)、`FLEETY_REQUIRE_AUTH`+`FLEETY_TOKEN`(Hello 認證)、`FLEETY_SCHED_TICK`(排程 tick)。
- `model-provider`: 模型供應端設定 —— `FLEETY_MODEL_BASE_URL`/`FLEETY_MODEL`/`FLEETY_MODEL_KEY`/`FLEETY_MODEL_STREAM`(OpenAI 相容端點;未設則 echo;`1` 啟用 SSE 串流)。
- `retention-gc`: 伺服器週期性清掃 —— `FLEETY_GC_DISABLED`、`FLEETY_GC_INTERVAL_SECS`(6h,60s 下限)、`FLEETY_BACKUPS_RETENTION_SECS`(7d)、`FLEETY_HISTORY_ROTATE_BYTES`(32MiB 輪替)。
- `service-discovery`: mDNS 服務探索 —— 伺服器宣告 `_fleety._tcp.local.`;`FLEETY_MDNS_DISABLED`、`FLEETY_MDNS_HOST_IP`(綁 0.0.0.0 時必填)、`FLEETY_MDNS_HOST`。
- `device-enrollment`: 裝置端連線與配對 —— `FLEETY_AGENT_URL`(mDNS→localhost)、`FLEETY_DEVICE_ID`、`FLEETY_DEVICE_ROOT`、`FLEETY_TOKEN`(持久化到 fleetyd.token)、`FLEETY_PAIRING_CODE`(以配對碼換 Welcome token 並寫盤)。
- `self-update`: 裝置端自更新輪詢與佈署 —— `FLEETY_UPDATE_MANIFEST`、`FLEETY_UPDATE_POLL_SECS`(24h,60s 下限)、`FLEETY_AUTO_UPDATE`(notify vs apply)、sidecar `FLEETY_INSYRA_BIN`/`FLEETY_INSYRA_URL`、安裝路徑 `FLEETY_INSTALL_DIR`。

### Modified Capabilities

(none)

## Impact

- Affected specs: 6 new capability specs under openspec/specs/.
- Affected code:
  - New: none (specs/documentation only)
  - Modified: none
  - Removed: none
- Source of truth: docs/env.md plus each variable's lookup site in crates/ (the `"FLEETY_<NAME>"` reads).
