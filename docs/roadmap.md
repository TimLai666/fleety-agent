# Fleety — 開發路線與實作計畫

對照 [`STATUS.md`](STATUS.md)(已完成的能力清單)與 [`spec-v0.md`](spec-v0.md)(v0 規格)。本檔聚焦在「**還沒做、但該做**」的部分,給出可執行的實作計畫——每項都含設計、步驟、驗收條件、與粗略工作量估計。

> 撰寫原則:不為了湊清單而塞 nice-to-have;每個 must-have 都附拒絕條件(在什麼情況下這個方案應該改做別的)。

---

## 必做(must-have)

擋下一個里程碑的東西。沒做完之前不該開新領域。

### 1. Eval / regression harness

**為什麼現在做**
- 目前 79 個單元測試覆蓋工具與協定,但**沒有任何 agent 迴圈級別的回歸測試**。換 model、改 system prompt、調 tool spec 都是手感。
- Fleety 已大到「沒 eval 不敢動」的階段:當前換 model provider 或刪除某個工具,沒有機制看會不會退步。
- 越晚做越貴:現在 79 個工具與 4 條恢復路徑,golden 資料不大就可以涵蓋核心場景。

**設計**

新增 crate `fleety-eval`,提供:

1. **golden conversation 格式**:JSONL,每行 `{ "input": "<user msg>", "expected": { "tools_called": [...], "final_contains": [...], "must_not_call": [...] } }`。寬鬆比對(不要求 token 精確匹配,只看工具序列與最終訊息片段)。
2. **runner**:`cargo run -p fleety-eval -- run <golden.jsonl>`,跑每條 golden,輸出 pass/fail 表。
3. **fixture provider**:接 echo provider + 一個簡單的「腳本式」provider(讀 JSONL 指定每回合該回傳什麼工具呼叫),讓測試可以離線跑、不打真 model。
4. **覆蓋面**(初版 10 條 golden):
   - 純讀:`list_dir` → `read_file` 回答
   - 寫入+rollback:`write_file` → `rollback` 反悔
   - 排程:`schedule_create` 後 tick 觸發
   - 中斷恢復:journal 留半截 → server 重啟 → recover_all_interactive 完成
   - MCP:`mcp_add` → `mcp_call` 走完 stdio JSON-RPC
   - skill:`list_skills` → `use_skill` → 後續按 skill 行動
   - 設備:`device_show` 跨裝置 handle 拒絕
   - 排程恢復:scheduler tick 撿起斷掉的 schedule turn

**步驟**

1. 新增 crate `crates/fleety-eval/`(`Cargo.toml`、`src/main.rs`、`src/runner.rs`、`src/scripted_provider.rs`、`src/golden.rs`)
2. 設計 `Golden` struct + 序列化格式;`Verdict` 含 pass/fail + 第一個差異點
3. 把 echo provider 抽到 `agent-core` 暴露(目前在 `fleety-server` 私有),evaluation 共用
4. 寫 ScriptedProvider:讀 `[{step:0, content:"", tool_calls:[...]}, {step:1, content:"done"}]`
5. 在 `crates/fleety-eval/goldens/` 放 10 個 fixture
6. CI:`.github/workflows/ci.yml` 加 `cargo run -p fleety-eval -- run crates/fleety-eval/goldens/*.jsonl`,任一條 fail → CI 紅
7. 文件:在 `docs/` 加 `eval.md` 說明 golden 格式 + 如何新增

**驗收**

- `cargo run -p fleety-eval -- run` 在主分支 10/10 綠
- 故意把 `tools::write_file` 換成 noop,eval 紅
- CI 將 eval 列為必過

**預估** 3-4 個工作日

**這個方案會錯的情況**
- 如果使用者要的是「上線後監測」的線上 eval(打真 model、量延遲與成本),就不該先做 offline harness。但 offline 是線上 eval 的前置。

---

### 2. Enrollment 端對端完成

**為什麼現在做**
- `auth.rs` 有 token / pairing code 骨架,但**沒有走完一次的整合測試**。多裝置願景的地基。
- 現在的 `fleety init` 在「無 token」時直接 `Welcome`,使用者體驗對但安全模型沒收緊。
- 後期加 RBAC / 多 user 時,如果 enrollment 還是半套,要回頭重做基礎,成本翻倍。

**設計**

完成這條路徑:

```
裝置 A(首次)
  → fleetyd 沒 token → 直接連 server,server 印出 pairing code
  → 使用者把 code 帶到既有信任裝置(或 server CLI)輸入
  → server 驗 code → 發 token → 回傳
  → fleetyd 存 token 到 ~/.fleety/fleetyd.token

裝置 B(後續)
  → fleetyd 帶 token → server 直接 Welcome
  → token 不對 → ConnError("auth.invalid_token"),fleetyd 刪掉本機 token 重來
```

token 設計:
- 32 bytes 隨機 → base64url
- server 端存 `auth.json`(已有)
- token revoke:CLI `fleety auth revoke <prefix>` 從 server 刪掉
- 過期:v0 不做(明確標)

**步驟**

1. `fleety-protocol::ClientMsg::Hello` 已有 `pairing_code` 欄位,確認 server 端 `auth.rs` 完整處理(目前是骨架,要走 pairing → mint → return 完整流程)
2. server `auth.rs`:`mint_pairing_code() -> String`、`consume_pairing_code(code) -> Result<Token>`
3. fleetyd `main.rs`:首次無 token 時,從 server 拿 pairing instruction(`ServerMsg::AwaitPairing { code, instructions }`),print 出來,等待使用者
4. CLI `fleety pair <code>`:把 code 送回 server 換 token(或 server 印的 instruction 就是 `fleety pair <code>`)
5. fleetyd 拿到 token,寫到 `~/.fleety/fleetyd.token`
6. 整合測試 `crates/fleety-server/tests/enrollment.rs`:模擬兩條 ws 連線,一條當 fleetyd 拿 pairing code,一條當 CLI 完成 pairing,驗證 token 之後重連能 Welcome
7. 文件:`docs/enrollment.md` 含序列圖

**驗收**

- 全新裝置:無 token → pairing → token 寫入 → 重連 OK
- 偽造 token:server 拒絕 + fleetyd 收到後清掉本機檔
- 兩台 fleetyd 同時 pair 不會搶到同一條 token
- 整合測試覆蓋上述三條

**預估** 2-3 個工作日

**這個方案會錯的情況**
- 如果決定走 mTLS 而不是 token,整套要砍掉重做。但 mTLS 對手機端不友善,v0 不選。

---

### 3. Audit + rollback 使用者介面

**為什麼現在做**
- `history.jsonl` 和 `backups/` 全在,但**沒有「列、看、復原」的入口**。full-access 安全模型的核心承諾(能看、能驗、能復原)現在只有「能看」一半。
- 加這層成本低、信任度提升大。

**設計**

CLI:

```
fleety audit list [--device X] [--since 1h] [--limit 50]
  → 列每筆 ToolCall / ToolResult / Approval 事件,含 seq + ts + tool + 摘要

fleety audit show <seq>
  → 展開那筆事件的完整 JSON

fleety rollback list
  → 列所有可還原的快照(從 backups_dir 掃)

fleety rollback apply <backup-id>
  → 把該快照覆寫回工作區檔案(本身也記一筆 audit)
```

server 端不開新 endpoint:CLI 用既有 ws ResumeStream + 新增 `Request { kind: "audit_*"|"rollback_*", ... }` 訊息類型(輕量)。

**步驟**

1. `fleety-protocol`:新增 `ClientMsg::AuditList { device_id, since, limit }` / `AuditShow { device_id, seq }` / `RollbackList { device_id }` / `RollbackApply { device_id, backup_id }`,對應 `ServerMsg::AuditResult` / `RollbackResult`
2. server `conn.rs`:新增 handler 路由到 `storage.rs` 上方法
3. server `storage.rs`:`list_audit_after(device, since, limit)` / `read_audit_event(device, seq)` / `list_backups(device)` / `apply_backup(device, backup_id)` — 後兩個會檔案系統操作,要記新 audit 並 backup 當前狀態(rollback 自己也可 rollback)
4. CLI `crates/fleety-cli/src/audit.rs` + `rollback.rs`:輸出表格(`comfy-table` 或自寫)
5. 測試:`storage::tests` 加 backup-then-rollback 往返 + audit 來回

**驗收**

- `write_file` 後 `audit list` 看得到那筆
- `audit show <seq>` 印出完整 diff
- `rollback list` 顯示對應 backup
- `rollback apply` 還原檔案,且**自己**也產生一筆 audit
- rollback 失敗(backup 已刪)有可行動錯誤訊息

**預估** 2 個工作日

**這個方案會錯的情況**
- 如果之後做 Web UI,CLI 表格會變雞肋。但 CLI 的 storage handler 仍然會被 Web UI 直接重用,不浪費。

---

## 該做(should-have)

不擋里程碑,但 1-2 個 sprint 內值得排進。

### 4. Sidecar / built-in MCP 自動更新檢測

**現況** 跟 fleetyd 一起更新是被動的:使用者要跑 `fleetyd update`。沒有背景檢查機制。

**設計** fleetyd 啟動 + 每 24h 背景查 GitHub release latest tag,若不同於本機快取版本,記 log 並選擇性自動下載(由 `FLEETY_AUTO_UPDATE` 開關控制,預設 `notify`)。

**步驟**

1. `provision.rs` 加 `query_latest_tag(repo) -> Result<String>`(GET `https://api.github.com/repos/<repo>/releases/latest`,只讀 `tag_name`)
2. `provision.rs` 加 `installed_version(binary_path) -> Result<String>`(spawn binary `--version`、parse)
3. fleetyd main 啟動時 spawn 24h interval task,比對,記 log 或下載
4. CLI `fleety status` 加一行顯示 sidecar 版本與最新對照

**預估** 1-2 個工作日

---

### 5. 多模態輸入(圖片附件)

**現況** 對話只接文字。`Message` 沒有附件欄位。

**設計** `Message::user` 加 `attachments: Vec<Attachment>`,Attachment 是 `{ kind: "image"|"file", mime: String, bytes_b64: String }`。CLI `fleety ask --image foo.png "..."`;ws protocol 把 bytes 嵌進 frame(超過 1MB 走分塊)。Provider 端只有支援 multimodal 的 model 才送圖,echo provider 忽略。

**預估** 3-4 個工作日(`agent-core` 訊息結構 + 各 provider 適配 + CLI)

---

## 明確延後或暫不做

下列項目刻意不放在當前 plan,理由列出來,避免日後爭論:

| 項目 | 延後到 | 理由 |
|---|---|---|
| 語音 / 喚醒詞 | 後續里程碑(M7 預留) | 引擎選型未定;不擋 v0 |
| Mobile client | post-v0 | spec 明確排除;桌機優先 |
| Multi-user / RBAC | post-v0 | spec 明確排除;單使用者夠 |
| Web UI / REST API | post-v0 | TUI/CLI 已能完成所有操作 |
| Credential broker / OAuth | post-v0 | 使用者已表態先不做 |
| Encryption-at-rest / key rotation | post-v0 | 主機檔案系統權限已足夠 v0 |
| LLM 成本追蹤 | post-v0 | 監測類功能,非核心能力 |
| codebase-memory-mcp 整合 | 條件未滿足 | 需要 device→server file sync,v0 不做 |

---

## 待決策略(blocking)

下面任一未拍板,就無法把對應功能放進 plan:

1. **Presence inference 信號來源**
   - 選項:(a) daemon 主動上報 LAN 鄰居 vs (b) server 主動掃 vs (c) 混合
   - 影響:[`fleety-presence-roadmap`](../memory/fleety-presence-roadmap.md) 的具體實作
   - 待決點:回報頻率、假陽性容忍、隱私邊界

2. **Model routing / fallback 策略**
   - 選項:(a) 單一 provider hard-fail vs (b) 多 provider 排序 fallback vs (c) cost-aware routing
   - 影響:`OpenAiCompat` 是否該升級成 `MultiProvider`
   - 待決點:fallback 觸發條件、成本/品質權衡規則

3. **Voice 引擎選型(等到要做 M7 時)**
   - 選項:Whisper local / Whisper API / Azure speech / 本機 TTS
   - 待決點:離線優先還是品質優先;預算

---

## 建議下一動

**第一順位** Eval harness(§1)。理由:Fleety 已大到沒它就不敢動,且其他所有功能都會受益。3-4 天,獨立 crate,風險低。

**第二順位** Audit + rollback CLI(§3)。理由:full-access 承諾的最後一塊;事件已備齊,只缺入口;2 天即可交付。

**第三順位** Enrollment 完成(§2)。理由:不擋當下的單裝置開發,但開新功能前要把基礎收緊。2-3 天。

照 1 → 3 → 2 走,7-9 天能讓 Fleety 進入「能放心動、能放心交、能放心讓使用者用」的階段。

---

_最後更新:由本次盤點決定的計畫,不是定稿;進度推進或現實對不上,直接改本檔。_
