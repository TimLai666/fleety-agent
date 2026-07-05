# Fleety — 開發路線與實作計畫

對照 [`STATUS.md`](STATUS.md)(已完成的能力清單)與 [`spec-v0.md`](spec-v0.md)(v0 規格)。本檔聚焦在「**還沒做、但該做**」的部分。舊版本檔內已出貨項目的完整實作計畫(eval harness、enrollment、audit CLI、多模態)保留在 git 歷史,不再佔用本檔。

> 撰寫原則:不為了湊清單而塞 nice-to-have;每個 must-have 都附拒絕條件(在什麼情況下這個方案應該改做別的)。

---

## 已出貨(自上次盤點)

舊版 roadmap 的 §1-§5 全數或大半出貨,對應細節見 `STATUS.md`:

- **Eval / regression harness** — `fleety-eval` crate + goldens,CI 必過。
- **Enrollment 端對端** — pairing code / token / `fleety pair`,整合測試齊。
- **Audit + rollback CLI** — `fleety audit list|show`、`fleety rollback list|apply`。
- **多模態輸入** — `Attachment` 進 `agent-core`,CLI `--image/--audio/--video/--file`,TUI Ctrl+V,OpenAI/Gemini provider 適配。
- **Sidecar / 自動更新檢測(大半)** — fleetyd 每日輪詢 `FLEETY_UPDATE_MANIFEST`(`FLEETY_AUTO_UPDATE` notify/apply),sidecar 隨 `fleetyd update` 與 CI release 流程同步;`fleety status` 顯示 sidecar 健康。殘餘:status 尚未顯示「本機版本 vs 最新版」對照。
- **語音對話** — `fleety voice`(cpal 錄音 + whisper.cpp 預設 / `FLEETY_STT_CMD` 模板 / server 端 STT),TTS 回覆。

## 必做(must-have)

主要來自 2026-07-05 的產品體驗稽核(72 項 confirmed;高頻痛點已同日修復,以下是留下的結構性缺口)。

### 1. defer-until-idle 接上手動 restart 與 fleety-server

**現況** `fleety-tools/src/restart.rs` 的 PendingRestart/decide 只有 fleetyd 的自我更新路徑使用;手動 `restart` 動詞(兩個 binary)與 `fleety update` 觸發的 server 重啟都是即時 sc/systemctl,會打斷 in-flight turn(靠 journal recovery 續跑,不遺失,但體驗中斷)。文件已改為照實描述。

**設計** server 維護 in-flight turn 計數;`restart` 與更新觸發的重啟改走 PendingRestart,idle 才真正呼叫 manager restart;`--force` 保留即時路徑。daemon 的「收斂到 server 版本」改為請 server 自行 idle 重啟,而非外部硬砍。

**拒絕條件** 若使用情境幾乎都是單人互動(打斷自己剛好知道),成本效益可能不如把力氣花在 §2。

### 2. 排程結果可見性

**現況** 排程在 unattended 下執行,結果寫進 `schedule-<id>` 對話。`schedule_list` 已帶出 `conversation_id`(2026-07-05),但仍無主動通知:排程失敗只留在 log。

**設計** 最小版:scheduler 把每次 run 的 outcome(成功/失敗+一句摘要)寫回排程檔,`schedule_list` 帶出;理想版:使用者下次連線時把新完成的排程結果當 proactive 訊息投遞。

### 3. TUI 深度

游標輸入編輯(含 CJK 寬度、橫向捲動)、底部錨定捲動、核可 y/n 流程已於 2026-07-05 出貨。剩餘:

- markdown / 程式碼區塊渲染(目前純文字)
- 斷線自動重連(目前斷線即退出)
- 取消進行中的生成(需 server 端 turn cancellation,與 §4 共用)
- 多行輸入(Shift+Enter 換行)
- 等待中 spinner / tool 執行進度顯示

### 4. Server 端 turn cancellation(供 TUI 取消與 ACP session/cancel)

**現況** ACP 的 session/cancel 是 no-op;TUI 送出後只能等。server 沒有中止 in-flight turn 的機制。

**設計** turn loop 每步檢查 cancellation token;收到 cancel 把已完成的工具結果落地、turn 標記 cancelled、回覆 stop_reason=cancelled。ACP 與 TUI(Esc 或 Ctrl+C 一次=取消、兩次=離開)共用。

### 5. config 值驗證

**現況** `config set` 只驗 key 不驗 value;寫壞的值在下次 boot 被 silent fallback 吃掉(providers.toml 壞檔已改為 error 級日誌)。

**設計** registry 的 Setting 加 validator(enum 白名單 / 數字範圍 / URL scheme),set 與互動編輯共用;無法驗證的 key 保持放行。

## 該做(should-have)

- **provider 編輯器就地編輯** — 目前只能刪掉重建(被 group/role 引用時要連環解綁);TUI 缺 group remove / role unset;逗號分隔單行輸入改欄位式表單。
- **conversation 列表** — `fleety resume` 已可從 ask/TUI 印出的 conversation id 接續(2026-07-05);補 `fleety conversations`(或 status 帶最近對話)讓列表可發現。
- **voice 體驗** — 錄音提示已加(2026-07-05);剩 VAD(自動斷句取代固定秒數)與 TTS 播放中打斷(barge-in)。
- **Windows 服務前置權限檢查** — install/up 在動手前先偵測是否系統管理員,而非 sc create 失敗才報。
- **容器非 root** — Dockerfile 改 non-root user,避免 bind mount 的 workspace 檔案在宿主端變 root 所有。
- **fleety status 顯示 sidecar 版本 vs 最新版**(§已出貨殘餘)。

## 明確延後或暫不做

| 項目 | 延後到 | 理由 |
|---|---|---|
| Mobile client | post-v0 | spec 明確排除;桌機優先 |
| Multi-user / RBAC | post-v0 | spec 明確排除;單使用者夠 |
| Web UI / REST API | post-v0 | TUI/CLI 已能完成所有操作 |
| Credential broker / OAuth broker | post-v0 | 使用者已表態先不做(Codex OAuth 登入已另行出貨) |
| Encryption-at-rest / key rotation | post-v0 | 主機檔案系統權限已足夠 v0 |
| LLM 成本追蹤 | post-v0 | 監測類功能,非核心能力 |
| codebase-memory-mcp 整合 | 條件未滿足 | 需要 device→server file sync,v0 不做 |

## 待決策略(blocking)

1. **Presence inference 信號來源**(colocation 上報與 site 記錄已出貨,推論未做)
   - 選項:(a) daemon 主動上報 LAN 鄰居 vs (b) server 主動掃 vs (c) 混合
   - 待決點:回報頻率、假陽性容忍、隱私邊界
2. **`FLEETY_ADDR` 預設值** — 預設 `127.0.0.1` 讓「跨裝置」開箱即不可達(啟動時已加提示)。改 `0.0.0.0` 是安全取捨:配合 `FLEETY_REQUIRE_AUTH` 預設值一起決定。

已拍板(從舊版待決清單移出):model routing/fallback → `providers.toml` 的 round_robin / failover pool 已出貨;voice 引擎 → whisper.cpp 預設 + `FLEETY_STT_CMD` 模板已出貨。

## 建議下一動

**第一順位** §4 turn cancellation。理由:同時解 TUI 取消與 ACP cancel 兩個對外承諾,是唯一需要動 agent loop 的基礎件,越晚做越貴。

**第二順位** §2 排程可見性。理由:無人看管會動手改東西的功能,結果不可見是信任缺口;最小版一天內可交付。

**第三順位** §1 defer-until-idle。理由:文件已照實,承諾債不再誤導;做完後把文件改回「不打斷」。

---

_最後更新:2026-07-05,依產品體驗稽核(72 項 confirmed)重排;進度推進或現實對不上,直接改本檔。_
