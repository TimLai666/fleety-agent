## Context

更新機制現況：`fleety update`（CLI）是唯一 host-wide 入口——更新 CLI 自己、經 `{bin}` 模板守門更新同機 fleety-server（bare restart 觸發 deferred 重啟）、再委派 `fleetyd update`；但它只存在於裝了 CLI 的機器。`fleetyd update` 只做 self-update＋sidecar＋service restart。`fleety-server` 的動詞表（install/up/down/start/stop/restart/enable/disable/status）沒有 update。24 小時輪詢（poll_updates.rs）預設 notify、`FLEETY_AUTO_UPDATE=apply` 才裝，且 apply 也只呼叫 self_update（不帶 sibling）。收斂路徑（converge_to_server_version）是唯一會帶 sibling 的自動機制，但它只在「server 比裝置新」時觸發。fleety_tools::update 已有 self_update、update_named、sibling_exe、manifest_is_templated 等積木。install-server.sh 尾段現在教 `fleety-server up`。

## Goals / Non-Goals

**Goals:**

- server-only 主機一個指令自我更新：`fleety-server update`（binary＋sidecar＋deferred restart）。
- 已設 manifest 的裝置預設無人值守自動更新（輪詢 apply 為預設，可退回 notify）。
- 同一台機器上的 fleety 元件版本不分裂：daemon 的更新路徑（一次性動詞與輪詢 apply）把同機 CLI 與 server 一併帶上。
- host-wide sibling 更新只有一份實作（CLI 與 daemon 共用）。

**Non-Goals:**

- 不改收斂路徑（converge 已是 host-wide，維持）。
- 不改 `FLEETY_UPDATE_MANIFEST` 未設即全面停用的 opt-in 總開關。
- 不做跨裝置推播更新（fleet 更新仍靠收斂與各機輪詢）。

## Decisions

### 決策一：fleety-server 加 update 動詞

在 server 的動詞分派（service action 之前）攔 `update`：`self_update()`（經 `{bin}` 模板解析自己的 manifest）→ 成功換檔後呼叫既有 deferred restart 路徑（與收斂觸發的 bare restart 同語義：等 idle）；無論換檔與否都 `ensure_insyra(true)` 刷新 sidecar（與 fleetyd update 對稱）。install-server.sh 尾段在 `fleety-server up` 提示旁補一行 update 提示。

### 決策二：FLEETY_AUTO_UPDATE 預設 apply

`auto_apply_enabled()` 語義反轉：未設 → apply；設為 `notify` 或 `0` → 僅通知（其餘值視為 apply）。BREAKING 但受總開關保護：沒設 manifest 的裝置完全不受影響；已設 manifest 的裝置本來就表達了「要自動更新」的意圖，notify 預設反而是意圖與行為的落差。docs 表格與說明同步反轉。

### 決策三：host-wide sibling 更新抽為共用

fleety_tools::update 新增 `update_host_siblings(current bin 排除)`：對 `fleety`、`fleety-server`、`fleetyd` 中「非目前行程且同目錄存在」者，經 `manifest_is_templated` 守門逐一 update_named 到 latest；fleety-server 更新成功後以其 exe 跑 bare `restart`（deferred）；fleetyd 更新成功時回報需要重啟（呼叫端決定——CLI 委派 `fleetyd update` 的現況已處理 daemon 自身，daemon 路徑則是自己 self_update 在前）。模板缺 `{bin}` → 跳過全部 sibling 並回傳說明字串供呼叫端印出。CLI 的 update_all 改用共用函式（行為不變）；`fleetyd update` 動詞與輪詢 apply tick 在 self_update 之後呼叫它。

### 決策四：daemon 輪詢 apply 的重啟順序

apply tick：先 `self_update()`（fleetyd 自己），再 `update_host_siblings`（帶 CLI 與 server），最後才 request_self_restart（fleetyd 換檔時）——sibling 更新在自我重啟請求之前完成，避免重啟打斷 sibling 下載。fleety-server 的 deferred restart 由 update_host_siblings 內部觸發（等 server idle，互不相扰）。

## Implementation Contract

**行為（操作者視角）：**

- server-only 主機：`fleety-server update` → 有新版即下載驗證換檔並排定 idle 重啟、sidecar 刷新；已最新則說明並仍刷新 sidecar。
- 設了 manifest 的裝置：每日輪詢自動裝新版（fleetyd＋同機 fleety/fleety-server＋sidecar），fleetyd 於 idle 自我重啟、server deferred 重啟；`FLEETY_AUTO_UPDATE=notify` 退回僅記 log。
- `fleetyd update`：更新 fleetyd＋sidecar＋同機 fleety/fleety-server（有 `{bin}` 模板時），server 更新後 deferred 重啟。
- 模板無 `{bin}`：sibling 跳過並印補模板提示；self 更新照常。

**介面與資料形狀：**

- `fleety-server update` 子命令（無旗標）；exit code 沿 fleetyd update 慣例（更新失敗非零）。
- `fleety_tools::update::update_host_siblings`（確切簽名 apply 時定：輸入目前 bin 名，回傳更新結果摘要）；既有公開函式簽名不變。
- `auto_apply_enabled` 私有語義反轉；env 值 `notify`／`0` 為僅通知。

**失敗模式：**

- manifest 未設：server update 報既有 actionable 錯誤（set FLEETY_UPDATE_MANIFEST …）；輪詢不啟動（現狀）。
- sibling 個別失敗：警告該 binary、繼續其餘（與收斂一致）。
- sidecar 刷新失敗：非致命、印提示（現狀語義）。

**驗收準則：**

- cargo test：auto_apply 預設/notify/0 的解析測試更新；update_host_siblings 的守門與排除自身邏輯以純函式切分測試（sibling 名單推導、缺 {bin} 跳過訊息）；fleetyd update 與 poll tick 的組裝點編譯與既有測試不回歸。
- fleety-server update 動詞：分派測試（動詞被認得）＋與 fleetyd update 對稱的煙囪路徑（網路面維持專案手動驗證 posture）。
- 全 workspace test/clippy/fmt 乾淨；docs 表格反映新預設。
- 端到端（發版後人工）：Mac mini 跑 `fleety-server update` 從 0.1.3 升至新版。

**範圍邊界：**

- 範圍內：crates/fleety-server/src/{main.rs,service.rs}、crates/fleety-daemon/src/{main.rs,poll_updates.rs}、crates/fleety-tools/src/update.rs、scripts/install-server.sh、docs/env.md、README.md。
- 範圍外：收斂路徑、manifest 解析、CLI update 的 ACP self-heal 尾段。

## Risks / Trade-offs

- [預設 apply 讓裝置無人確認換 binary] → sha256 驗證＋rename-aside 可回滾＋idle 重啟＋forward-only；且僅影響已設 manifest（已表達自動更新意圖）的裝置；可 `notify` 退回。
- [同機三 binary 更新順序競態（收斂與輪詢同時跑）] → 換檔是 rename-aside 原子操作、needs_update 冪等（已最新即 no-op），重複跑無害。
- [server update 與收斂對 server 的 deferred restart 疊加] → restart 請求冪等（等 idle 一次生效）。

## Migration Plan

單版出貨。已設 `FLEETY_AUTO_UPDATE=apply` 的部署無感；想保留舊行為者設 `notify`。回滾 revert。

## Open Questions

- 無阻斷項。
