## Context

Settings 已經持有一個由 `ratatui::init()` 建立的 full-screen alternate-screen terminal。現行流程在 `open_provider_now` 時先呼叫 `ratatui::restore()`，接著才連線、驗證 Server identity、抓取 Provider snapshot 與 credential status，最後 Provider editor 再次呼叫 `ratatui::init()`。本機 PTY 重播 Tab 至 Providers & Models、Enter、`a`，實際擷取到 `CSI ? 1049 l` 後接 `CSI ? 1049 h`；這會讓 Terminal.app 等圖形終端在交接空窗顯示 primary scrollback。先前只驗證路由狀態與按鍵消費的測試無法觀察控制序列。

## Goals / Non-Goals

**Goals:**

- Settings 與 Provider editor 的一般進入、編輯、返回流程共用一個連續的 alternate-screen session。
- Provider editor 收到 `a` 後在同一個 terminal backend 上繪出 Add Provider wizard。
- 保留獨立 `fleety provider edit` 命令與 OAuth plain-terminal 流程。
- 以可在 CI 執行的終端生命週期測試防止回歸，並以實際 PTY 重播驗證控制序列。

**Non-Goals:**

- 不改 Provider、model、credential 或 Server protocol 的資料模型。
- 不改 Chat 的 inline viewport 設計。
- 不在 Provider editor 加入滑鼠操作或重做輸入元件。
- 不把 OAuth 瀏覽器流程塞進 alternate screen。

## Decisions

### Settings passes its existing terminal into the Provider editor

將 Provider editor 的同步迴圈拆成可接受 caller-owned terminal 的核心入口。Settings 路徑傳入自己現有的 terminal，因此 Enter 與 `a` 都不觸發 LeaveAlternateScreen 或 EnterAlternateScreen。獨立命令保留一層 wrapper，自行 init、呼叫核心入口、再 restore。

替代方案是把 Settings 的 `restore()` 延後到 snapshot 載入後。這只能縮短 primary-screen 空窗，仍保留不必要的切換，也無法保證圖形終端不閃動，因此不採用。

### Plain-terminal transitions stay explicit and exceptional

只有 OAuth 瀏覽器／確認流程需要 plain terminal 時，embedded 路徑才暫停 alternate screen；OAuth 完成後重新建立 terminal，再載入新的 Provider snapshot。準備失敗、一般 Add/Edit/Save、返回 Settings 都不得切換 terminal。

替代方案是讓 Provider editor 永遠自行擁有 terminal。這會重現目前的巢狀生命週期，無法達成連續畫面，因此不採用。

### Regression coverage observes terminal ownership, not only route state

測試必須以 caller-owned backend 執行 Provider editor 的初始繪製與 `a` transition，確認 Add Provider wizard 出現在同一 backend。另以 PTY 控制序列檢查記錄正常 Settings → Provider → Add 流程沒有相鄰的 LeaveAlternateScreen／EnterAlternateScreen；測試不得只呼叫 `on_key` 後檢查 enum。

## Implementation Contract

**Behavior:** 使用者在 Settings 的 Providers & Models 頁面按 Enter，接著按 `a`，畫面 SHALL 保持在同一個 full-screen terminal session 並顯示 Add Provider 的 type picker。連線、snapshot 或 credential-status 準備期間 SHALL 保留 Settings 畫面，不得暴露 primary scrollback。準備失敗 SHALL 在原 Settings terminal 顯示既有錯誤狀態。

**Interfaces:** Provider editor SHALL 提供一個接受 caller-owned ratatui terminal 的核心執行入口；既有 standalone 入口 SHALL 保持對外行為並包裝 terminal init/restore。Settings 的 remote Provider 呼叫鏈 SHALL 把同一 terminal 傳到核心入口。Wire protocol、設定檔與命令列參數不變。

**Failure modes:** Provider 準備失敗時不得丟失 Settings terminal ownership，也不得再次 init 一個重疊 terminal。OAuth action 可明確 restore，完成或失敗後都 SHALL 恢復一個可繼續繪製的 Settings/Provider terminal。editor draw error SHALL 沿既有 `CoreError` 路徑回報。

**Acceptance criteria:**

- caller-owned terminal 測試從 Browse 送入 `a` 後能在同一 backend 讀到 `add provider` 與 type picker。
- lifecycle／PTY 測試在一般 Settings → Provider → Add 流程中找不到 LeaveAlternateScreen 後接 EnterAlternateScreen 的交接序列。
- standalone Provider editor、OAuth terminal suspend/resume、Settings 返回與既有 Provider 單元測試全部通過。
- `cargo fmt --all -- --check`、Fleety CLI 測試與 clippy gate 通過。

**Scope boundaries:** 僅調整 CLI 的 Settings、Provider editor 與相關終端測試。Daemon、Server、協定、installer、Chat inline viewport 與其他設定 owner 不在範圍內。

## Risks / Trade-offs

- [Risk] caller-owned terminal 的生命週期責任不清會造成 double restore 或錯誤後無法重繪 → 以 standalone wrapper 與 embedded core 明確區分 ownership，核心入口不得 init/restore。
- [Risk] OAuth 返回時 terminal 物件已失效 → OAuth 邊界由 embedded host 成對執行 restore/init，並測試成功與錯誤返回。
- [Risk] 泛型 terminal backend 讓函式簽章複雜 → 泛型只留在同步繪製核心，網路與 Provider domain 邏輯維持現有型別。
