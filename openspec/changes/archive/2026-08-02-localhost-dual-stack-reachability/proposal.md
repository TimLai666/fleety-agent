## Problem

在雙棧 Windows 主機上，任何把 Server 位址拼成 `localhost` 的使用者都付出隱形代價：`localhost` 先解析到 `::1`，而 Server 預設只綁 IPv4（`0.0.0.0:8787`），連向沒人聽的 `[::1]:8787` 要約 2.04 秒才落回 IPv4（實測 `localhost` 2.0365 秒、`127.0.0.1` 1.7 毫秒）。

後果分兩層。輕的：每次連線多付約 2 秒。重的：連線 sweep 的每候選預算遠小於 2 秒（`open_budget_within` 在傳輸開始前就把呼叫端的份額砍半），所以一個拼成 `localhost` 的端點在 sweep 裡**永遠不可達**——不是慢，是根本測不到。

內建的 `DEFAULT_URL` 用 IPv4 字面值避開了這件事，但手打或貼上的 `ws://localhost:8787`（`fleety init` 引數、`FLEETY_AGENT_URL`、ACP 的 `--server`、設定面板）沒有任何保護。修漫遊測試時已被迫在測試伺服器加綁 `[::1]` 伴聽器，等於測試不再能暴露這個產品層缺陷——它已記錄於 AGENTS.md follow-up（2026-08-02）。

## Root Cause

兩個各自合理的預設疊出來的縫：

1. Server 預設 `TcpListener::bind("0.0.0.0:8787")`，單一 v4 listener，v6 上無人聽。
2. Windows 的解析器對 `localhost` 依 RFC 6724 偏好 `::1`，落回 v4 前有秒級的連線失敗等待。

任一側改掉，縫就閉合。

## Proposed Solution

兩側都修，理由不同：

**一、Server 端（治本）**：當設定的監聽位址是 v4 萬用位址 `0.0.0.0` 或 v4 回環 `127.0.0.1` 時，盡力加綁同埠的 v6 對應位址（`[::]` / `[::1]`）作為伴聽器；綁不到（無 v6、埠被占）就照舊只聽 v4 並記一行日誌，不失敗。明確指定其他位址時完全照舊，不加任何東西。這讓任何客戶端——包括非 Fleety 的 ACP 編輯器——拼 `localhost` 都能立即連上。

**二、客戶端（防舊 Server）**：在傳輸層撥號的單一路口，主機名恰為 `localhost` 時改撥 `127.0.0.1`（埠與路徑不變，顯示與儲存的 URL 不變）。這保護連向尚未升級的舊 Server 的所有 Fleety 客戶端。撥號路口位於 CLI 一次性路徑與 Daemon 重連迴圈共同的下游，一處落地即涵蓋 AGENTS.md 所列的兩個 client-connection 平行介面，`init` 選擇器、設定面板、`fleety server` 子命令與共用 resolver 也因此無需各自改動。

## Non-Goals

- 不改 `FLEETY_ADDR` 明確指定值的語意：使用者指定什麼就綁什麼，伴聽器只作用於兩個 v4 預設形式。
- 不做 `localhost` 以外的主機名正規化，也不做 DNS 層面的 happy-eyeballs。
- 不改 mDNS 廣播內容（埠不變，v4 位址廣播照舊）。
- 不回頭改漫遊測試的 `[::1]` 伴聽器——那是測試環境自洽所需，留著。
- 撥往 v6-only Server 的使用者需自行拼 `ws://[::1]:8787`；此為文件化的既定行為，不做自動偵測。

## Success Criteria

- 預設啟動的 Server，`127.0.0.1` 與 `::1` 兩個家族的連線都立即被接受。
- v6 綁定失敗時 Server 照常以 v4 啟動，日誌說明伴聽器未建立。
- 客戶端撥 `ws://localhost:<port>` 到 v4-only 的舊 Server，不再有秒級延遲，sweep 內可達。
- URL 為 `[::1]`、其他主機名、IP 字面值時，撥號行為與現在逐字相同。
- `docs/env.md`、config registry 的 `FLEETY_ADDR` 說明與 `runtime-configuration` 規格一致描述伴聽器行為。
- 全 workspace 建置、測試、clippy 無新增錯誤或警告。

## Impact

- Affected specs:
  - Modified: `runtime-configuration`（FLEETY_ADDR 預設的伴聽器語意）, `connection-profiles`（新增 localhost 撥號偏好 v4 的要求）
- Affected code:
  - Modified:
    - crates/fleety-server/src/main.rs
    - crates/fleety-tools/src/transport.rs
    - crates/fleety-tools/src/config.rs
    - docs/env.md
    - AGENTS.md
  - New: (none)
  - Removed: (none)
