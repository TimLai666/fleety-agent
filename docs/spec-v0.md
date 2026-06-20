# Fleety — v0 施工規格（Walking Skeleton）

狀態：Draft　目標：先打通「origin-aware + full-access + 可回滾 + 斷線恢復」的最小骨架，能 demo 一條真實任務鏈，不追求覆蓋完整願景。

完整產品願景見 ChatGPT 的 v0.3 規格（尚未收進 repo，建議放 `docs/vision.md`）。本檔只定義 v0 要做什麼、刻意不做什麼，並與 `prompts/`（system prompt）、`docs/tools.md`（工具面）對齊。

## 0. v0 一句話

單機 Agent + 一支 CLI，使用者在自己機器的專案目錄叫 Agent 讀／改／跑，Agent 全自動執行、全程留 audit、可回滾，CLI 斷線後任務不消失、重連續上。

## 1. 範圍

### 1.1 v0 要做

1. **Agent Server**：單機、單使用者、檔案儲存（YAML + JSONL）。長駐 process。
2. **fleety CLI**：`init`、無參數互動模式（精美 TUI，框架見 §9.4）、`ask`、`status`。
3. **Connector：只做 `client_session`**。CLI 開著才在線。不碰 daemon／自動安裝／updater。
4. **Origin context**：每則訊息帶 `origin_device_id`、`cwd`、`os`、`shell`、git 狀態。
5. **本機工具**（對 origin 裝置）：`workspace_read_file`、`workspace_list_files`、`workspace_search`、`workspace_write_file`、`workspace_apply_patch`、`workspace_replace_lines`、`terminal_run`、`git_status`、`git_diff`。
6. **Model 層：只接 `openai_compatible`**（OpenAI / OpenRouter / LM Studio / Ollama）。`GET /models` 自動發現。**先不做 Codex OAuth**。
7. **對話持久化 + 任務恢復**：對話與任務狀態存在 Server，不依賴 CLI process 存活；CLI 重連用 `last_seen_event_id` 回放 missed events。
8. **Per-device 記憶**：`fleet/devices/{id}/` 含 `device.yaml`、`NOTES.md`、`history.jsonl`；Agent 任務後自動維護 NOTES.md。device.yaml 支援**多連接（`connectors[]`）+ `mobility` + `site`**（見 §3.1），co-location 安全規則的資料基礎。
9. **存取政策：`full_access` 預設**。mutate 直接執行但留 audit + rollback；critical 動作先確認。實作見 §6。

### 1.2 v0 刻意不做（往後排）

- fleetyd 背景服務、開機自啟動、自動安裝
- fleety-updater、整包更新與回滾
- SSH / HTTP / serial / adb / mqtt 等其他 connector（v0.1 先加 SSH）
- Capability Router、跨裝置 task graph、artifact manager、resource lock
- GPU / CUDA / ML 分派
- Skills / MCP runtime（prompt 已預留協定，runtime 先 stub）
- Codex OAuth backend
- Web UI、多使用者、RBAC
- 裝置判重 / 合併、degraded 偵測
- 語音對話的 STT / TTS 引擎與音訊處理（在終端做，不在 server）。**協定先留 speech 輸出通道 + voice mode 旗標**，引擎延後（見 §11 M7）
- headroom 的 ML prose 壓縮模型（階段二）。v0 只做 §10.1 階段一的演算法類原生壓縮（已內建 agent-core，非外掛）
- 自管排程 / cron（§10.2）。設計與工具先定，scheduler 實作 post-v0（M8）
- 瀏覽器自動化（§12）。以 skill 在目標裝置本機用該機 Chrome（CDP）驅動，post-v0（M9）
- 電腦操作 computer-use（§13）。內建 MCP（computer-use-mcp）在裝置本機控制螢幕/鍵鼠，隨 client runtime 自動裝，post-v0（M10）
- 知識 Wiki（§14）。Obsidian 格式第二大腦，agent-core 子系統、專屬 vault + 管理機制，post-v0（M11，實作輕量）

> Skills 與 MCP 在 `prompts/protocol.md` 已寫入協定。v0 runtime 對 `list_skills` / `use_skill` / `mcp_*` 回「未啟用」即可，不需實作，避免 prompt 與 runtime 對不上時 Agent 亂猜。

## 2. 架構（v0）

```
fleety CLI ──ws──> Agent Server ──> Model Provider (openai_compatible)
   │  (client_session)      │
   │  本機工具 bridge <──────┘  Agent 透過 session 回呼 CLI 執行本機工具
   └─ origin context: device_id / cwd / os / shell / git
```

- CLI 與 Server 走 **WebSocket**。
- 本機工具的實際執行在 **CLI 端**（client_session bridge）：Server 要讀檔／跑指令時，透過該裝置的 live session 下指令、CLI 在本機執行、回傳結果。v0 沒有 daemon，所以工具只能在「CLI 還開著」時跑。
- Server 是 stateful：對話、任務、事件序列、audit、記憶都在 Server 落地。
- **Device-scoping 不變式**：所有 handle（tab id、pid、serial port、workspace ref、session…）都綁 `device_id`，沒有全域 handle。handle 設計成綁裝置的不透明 token，**runtime 強制拒絕跨裝置使用**（筆電的 tab id 不能丟給桌機 Chrome），不靠 agent 自律。tool 回傳一律帶 `device_id`。是「對話隔離」的推廣。被拒時錯誤是 **actionable**：回 handle 的擁有 `device_id` + 兩條補救（①指錯目標→帶擁有裝置重發；②要另一台的對應資源→先在那台拿新 handle 再用），不是只回「rejected」。

## 3. 儲存佈局（v0）

```
~/.fleety/                      # CLI 端
  config.yaml                   # agent_url, device_id
  identity.yaml                 # device_id, runtime_version（token 進 keychain）

<agent-home>/fleet/            # Server 端
  ME.md                        # 自我認同（預設叫 Fleety）─┐
  USER.md                      # 使用者是誰                ├ 每回合自動注入（核心記憶）
  TODO.md                      # agent 待辦              ─┘
  TOOLS.md                     # 工具/skill 使用心得（按需 memory_read/write）
                               # 見 §5.2，對齊 TimLai666/agent 的記憶模型
  devices/
    {device_id}/
      device.yaml
      NOTES.md
      history.jsonl             # audit log（每個 mutate 一行）
      conversations/
        {conversation_id}.jsonl # 事件序列，含 event_id 單調遞增
  tasks/
    {task_id}.json             # 任務狀態（status, last_event_id, conversation_id）
  backups/                     # 非 git 目錄改檔前的還原點
  skills/installed/            # 使用者安裝的 skill（更新時保留）
  mcp/installed.yaml           # 使用者新增的 MCP servers（更新時保留）
  wiki/                        # 知識 Wiki（Obsidian vault，見 §14）；與 workspace、device 記憶分開

<runtime>/                     # 隨 runtime 發布、更新時整包換掉
  skills/builtin/              # 內建 skill（唯讀、更新覆蓋）
  mcp/builtin.yaml             # 內建 MCP servers（唯讀、更新覆蓋）
```

- **原則：workspace = dirty work。** 使用者的專案目錄只是 Agent 做髒活的地方，不放任何珍貴或持久資料。記憶、audit、rollback 還原點、事件序列**全部在 Fleety store**（上面 `~/.fleety/` 或 `<agent-home>/fleet/`），與 workspace 分開。`backups/` 放在 store 裡，**絕不**寫進被編輯的目錄。砍掉 workspace 不會丟記憶。
- **憑證一律存 Agent 端 secret store**：所有節點的憑證（device token、平台/服務的 API key、OAuth token、MCP server 的 key）放在 Agent 的 secret store（keychain／secret manager），**絕不明文寫進 config／記憶／workspace**，由節點引用。存取留 audit。
- `history.jsonl` 是 audit 真相，欄位對齊 `prompts/policy.md`（device/origin/target/connector/tool/command summary/exit code/risk/result/rollback ref）。
- 對話事件用單調遞增 `event_id`，這是斷線回放的依據。
- **內建 vs 安裝分開放**：內建 skill／MCP 隨 runtime 發布、放在 `<runtime>/`、**更新時整包覆蓋、視為唯讀**；使用者安裝的放在 user data（`<agent-home>/...installed`）、**更新時保留**。兩者實體分開，避免：更新覆蓋掉使用者裝的、或使用者改動弄壞內建。`mcp_add`／安裝 skill 一律寫到 installed，**絕不寫進 builtin**。id 衝突時 **installed 覆蓋 builtin 並回報**（讓使用者能覆寫內建，但留痕跡）。

### 3.1 device.yaml v0 欄位

即使 v0 只用 `client_session`、只有一台筆電，資料模型也要從一開始就支援多連接與位置，避免日後 migration：

```yaml
id: tingzhen-laptop
mobility: mobile            # stationary | mobile | unknown
site: unknown              # 固定裝置=所在地；行動裝置=目前所在地（會變）
connectors:                # 一台可多個；v0 通常只有一個
  - id: session-current
    type: client_session
    scope: local           # local（同 LAN）| remote（relay/internet）— co-location 訊號
    status: available_when_cli_running
```

- `connectors` 是 list，protocol.md 的優先順序與 co-location 判定都假設它可以多個。
- `mobility` / `site` 是「能連到 ≠ 人在旁邊」這條安全規則的依據（見 `prompts/policy.md` → Physical-Presence Actions、`prompts/memory.md` → Connectors, Location & Mobility）。
- **co-location guard 在 v0「先建模、enforcement 延後」**：v0 沒有實體致動裝置（電風扇之類），所以沒有實體動作要擋；但欄位與規則先就位，等 v0.1+ 接入 HTTP/MQTT 致動裝置時直接生效，不必回頭改 schema。

### 3.2 節點模型：裝置與平台/服務一起管

Fleety 連到的不只實體裝置，也包含透過 API／MCP 連的平台或軟體（Home Assistant、某 SaaS、GitHub、資料庫、某 MCP server）。**統一成一個 node registry 管，用 `kind` 鑑別**，不另起一套。

- **`kind`**：`host`／`target`／`tool`（實體裝置的角色，沿用願景）＋ `service`（API/MCP 平台）。一個 node 可身兼多角。
- **共用**：每個 node 都有 per-node 記憶（NOTES.md／capabilities／history）、connector、能力路由、audit——service 跟 device 一視同仁。service 就是「用 `http`／`mcp` connector 連的 node」。
- **實體專屬屬性只對實體 kind 生效**：`mobility`／`site`／co-location、螢幕／UI（browser、computer-use）只適用實體裝置；`service` 節點沒有這些，但有 endpoints、auth、capabilities。
- **憑證**：service 的 API key／OAuth token 跟 device token 一樣存 Agent secret store（見 §3 原則），由 node 引用。
- v0 範圍：v0 主要是實體裝置（client_session）。node `kind` 與 service 節點先入模型，service connector（http/mcp）的完整實作隨 §1.2 的 connector 排程（v0.1+）。

## 4. 工具面（v0 子集）

以 `docs/tools.md` 為命名正典。v0 啟用：

- 入口：`harness`（回 session_id + policy）、`device_list`、`device_show`、`project_current`
- 記憶：`memory_read`、`memory_write`
- 工作區：`workspace_*`（六個）
- 終端：`terminal_run`
- git：`git_status`、`git_diff`
- 歷史：`history_list`、`history_show`、`history_restore_preview`、`history_restore`

v0 不啟用（runtime 回「未啟用」）：`capability_probe`、`project_add/create/clone`、`use_skill`、`mcp_*`、`git_log/show`。

## 5. 對話與恢復（v0 核心，要早做對）

- 每個對話綁 `conversation_id` + `origin_device_id`，**不同裝置不混**（v0 雖然多半單裝置，但隔離邏輯要從一開始就在）。
- 長任務寫 `tasks/{id}.json`，狀態：`running` / `waiting_for_origin_device` / `done` / `failed`。
- CLI 斷線：Server 上的對話與任務不消失。若任務下一步需要該裝置的本機工具（v0 幾乎都需要，因為只有 client_session），標 `waiting_for_origin_device`。
- CLI 重連送 `{ device_id, last_seen_event_id, active_conversation_id }`，Server 回放 missed events + 目前任務狀態 + 待執行步驟。

### 5.1 對話記憶（三層）

1. **工作記憶**：當前 context window（這一回合），靠 §10.1 壓縮控管。
2. **對話記憶（單一對話內）**：事件序列 `conversations/{id}.jsonl`，可重連回放、全程記得。v0 已有。
3. **長期/跨對話記憶**：靠提煉——把對話精華蒸餾進 per-device NOTES.md（裝置操作性事實）或知識 wiki（通用知識）。對話是 raw 來源，重要的才提煉留存（對齊 wiki 的 raw→distilled）。

跨對話召回機制（v0.1+，工具見 `docs/tools.md`）：

- `conversation_list`／`conversation_search`／`conversation_read`：召回過去對話。**預設 device-scoped**（守隔離）；使用者明確要跨裝置回想時可放寬，但要標來源。
- **對話摘要**：對話結束（或滾動）時產生摘要，存在該對話旁，召回時先給摘要、需要才取全文（也是一種壓縮，§10.1）。
- **蒸餾流程**：對話裡學到的 durable 東西，主動寫進 NOTES.md／wiki，不要只躺在對話記錄裡等召回。raw 對話 + 提煉記憶兩者並存。

### 5.2 核心記憶檔（對齊 TimLai666/agent）

沿用 [TimLai666/agent](https://github.com/TimLai666/agent) 的記憶模型，agent 級檔案（非 per-device、非 per-conversation）放 `<agent-home>/`：

- **每回合自動注入**（核心記憶，永遠在 context）：
  - `ME.md`：自我認同，**預設寫它叫 Fleety**（名字、定位、persona）。
  - `USER.md`：使用者是誰（角色、偏好、習慣）。
  - `TODO.md`：agent 的持續待辦，跨回合/跨對話延續。
- **按需取用**（`memory_read`／`memory_write`，不自動注入）：
  - `TOOLS.md`：工具/skill 使用心得。
  - 其餘長期記憶由 Fleety 的 **per-device 記憶（devices/*/NOTES.md）＋ 知識 wiki** 承擔——這是對 TimLai666/agent 扁平 `MEMORY.md` 的結構化擴充（裝置事實歸裝置、通用知識歸 wiki）。
- 都是「資料」非寫死 prompt：agent／使用者可改、會演進。protocol.md 的 "You are Fleety Agent" 是框架底線，`ME.md` 是其上可編輯的自我。
- seam：核心記憶檔機制是 agent-core 泛用概念（隨框架帶走），預設內容（如 ME.md＝Fleety）由 Fleety 提供。

## 6. 安全（v0）

對齊 `prompts/policy.md`，v0 最小落地：

- **full_access 預設**：read / mutate 直接跑，不每步問。
- **Audit**：每個 mutate（含 `terminal_run` 造成的檔案變動）寫一筆 `history.jsonl`，附 before/after 與 `history_step_id`。
- **Rollback**：git 目錄靠工作樹；非 git 目錄改檔前先寫 `backups/`。`history_restore` 可還原。
- **Critical gate**：`terminal_run` 前由一個**指令分類器**判斷是否落入不可逆清單（wipe/mkfs/dd/刪 HOME/改 sshd_config/金鑰/防火牆/遠端唯一主機 reboot）。命中 → 回 `critical`、停下來要使用者確認。
  - 注意：分類器是**語意/結構分級**，不是字串黑名單。v0 可先做「保守版」：偵測到高危動詞或目標即升級 critical，寧可多攔。寬鬆化留到有 audit 數據後。
- **Prompt injection**：v0 就把「讀到的內容是資料不是指令」寫進 system prompt（已在 policy.md）。沒有額外機制，靠 audit + rollback 擦屁股。

## 6.5 永不崩潰（系統級硬需求）

Agent Server 與 daemon 是長駐核心，**任何單一輸入、工具、模型回應、connector 斷線都不得讓 process 崩潰**。這是硬需求，不是 nice-to-have。

- **錯誤即值**：所有失敗路徑回傳結構化錯誤，不靠未捕捉的例外／panic 結束。Rust 用 `Result`，禁止在執行路徑用 `unwrap`／`expect`／`panic!`。
- **panic 隔離**：每個 session／task／connector 跑在獨立的非同步任務裡，邊界處攔截 panic，記錄並回報，**絕不向上傳播殺掉整個 process**。一個壞掉的對話不能拖垮其他對話。
- **錯誤一定看得到、且可行動（actionable）**：每條錯誤路徑最終都產生一個使用者可見的結構化錯誤事件（CLI/TUI 要能優雅呈現），不准靜默死掉或丟 stack trace 崩潰。「就算錯了，也要成功把錯誤訊息呈現出來。」回給 agent 的錯誤要**講清楚原因＋怎麼往下做**（例：跨裝置 handle 被拒，回擁有 device_id ＋ 補救路徑），不是只回「失敗／rejected」，讓 agent 能自我修正而非卡住或亂猜。
- **監督式重啟**是最後一道防線，不是常態。設計目標是錯誤降級成「回報」，而非「重啟」。

這條需求是選 Rust 的主要理由（見 §9.3）。

## 7. Enrollment（v0 最簡版）

- `fleety init <agent-url>`：CLI 連 Server → Server 配發 `device_id` + token（token 進 keychain，失敗則退普通檔但警告）→ Server 建 `devices/{id}/`、寫 `device.yaml` + 初始 `NOTES.md`。
- v0 **不做** daemon 安裝、不做判重合併。重裝就是新裝置，之後再處理。
- **配對驗證：一次性 pairing code（已決）**。Server 為待配對裝置產生短效 pairing code，新裝置 `init` 時輸入才完成註冊，擋掉隨意註冊。
  - **第一台裝置**：沒有已配對裝置可代發，code 印在 Server console（或啟動 log）。
  - **後續裝置**：使用者可從**已配對的信任裝置**叫 Agent 把目前的 pairing code 顯示出來（Agent 透過該裝置的 live session 回傳 code），不必去翻 Server console。對應一個只開放給已配對裝置的工具（暫名 `enrollment_pairing_code`，回傳目前 pending 的 code）。
  - code 短效（例如數分鐘）、用完即失效；pending enrollment 逾時自動清除。

## 8. 驗收（Demo 即測試）

v0 完成的定義是這三條 demo 能跑：

1. **加入**：`fleety init http://localhost:8787` → Server 出現該裝置 online，`devices/{id}/` 建好。
2. **本機改 bug**：在專案目錄 `fleety` → 「幫我修這個 bug」→ Agent 知道 origin + cwd → 讀檔 → 套 patch → 跑測試 → 顯示 diff → 寫 history → 更新 NOTES.md。每個 mutate 在 `history.jsonl` 有對應筆、可 `history_restore`。
3. **斷線恢復**：發一個多步任務 → 中途關掉 CLI → 重開 `fleety` → 送 `last_seen_event_id` → 看到 missed events 回放 + 任務續上。

## 9. 關鍵決策

### 9.1 指令分類器策略（已決：偏寬）

只攔真正不可逆的 critical 清單（policy.md 那組：wipe/mkfs/dd/刪 HOME/改 sshd_config/金鑰/防火牆/遠端唯一主機 reboot），其餘 mutate 一律放行（靠 audit + rollback 兜底）。寧可少攔、保留體驗，不做保守多攔。critical 清單之外若日後發現該攔的，再個案加入。

### 9.2 配對驗證（已決：pairing code + 已配對裝置代發）

見 §7。第一台用 console code，後續可由已配對裝置叫 Agent 顯示 code。

### 9.3 語言與 agent loop（已決：Rust + 自寫 loop，不採用 vercel/eve）

- **語言：Rust**。理由是 §6.5 的「永不崩潰」硬需求——Rust 的 `Result` 錯誤模型、無 GC、可在邊界攔 panic，最直接服務這個目標；單一 workspace 共用型別，比跨語言更好維護。
- **不採用 vercel/eve 當基座**：eve 是 2026-06-19 才發表的 beta TS 框架，雖然 durable workflow（崩潰可續）、approvals、skills、MCP 都跟我們重疊，但 (a) 它是 TS，跟 Rust 偏好衝突；(b) **beta，官方明說 API/行為 GA 前會變**，跟「永不崩潰／穩定」直接矛盾；(c) 它解的是「單一 agent 的耐久與工具」，**沒有處理 Fleety 真正的難點：跨裝置 mesh、connector、origin-aware、現場能力探索、co-location**。結論：不依賴它，但**借鑑它的設計**（agent 即目錄、durable workflow 以 checkpoint 續跑——正好對應我們的事件序列 + 重連回放）。日後可把 eve 當「可選後端／skill」，不當核心。
- **agent loop：自寫最小 tool-calling loop**（Rust），完全掌控 connector／approval／audit 接縫，不被框架綁架。openai_compatible 就是 HTTP + SSE，Rust 直接做（`reqwest` + SSE）。

### 9.4 互動 TUI 框架（已決：ratatui）

全 Rust，Ink（React/TS）不適用。用 **ratatui**——成熟（21k★、33M 下載、4400+ crates）、60fps、`gitui`／`atuin`／`yazi` 都是它做的精美 TUI，達 claude-code 等級質感沒問題。若日後 TUI spike 覺得撐不起美感，退路是 hybrid（Rust 核心 + TS/Ink CLI，以 WebSocket 協定當跨語言契約），但成本較高、非預設。

## 10. 框架抽取策略與 workspace

決策：**Fleety 優先，agent 框架後抽**。通用 agent 核心從第一天就放進獨立 crate（`agent-core`），但不先抽成獨立專案；等 v0 可用、出現第二個使用者後，再把 `agent-core` 搬成獨立 repo，**以 git submodule 掛回 Fleety**。

讓「後抽成 submodule」無痛的鐵律：

- **`agent-core` 只依賴外部 crate，絕不依賴任何 Fleety 專屬東西**（裝置、connector、記憶、co-location 都不准進去）。依賴方向永遠是 Fleety → agent-core，反向禁止。
- `agent-core` 是完整、可單獨 build 的目錄（自己的 `Cargo.toml`），這樣搬成獨立 repo 仍能編譯，掛成 submodule 後 workspace 以路徑成員引用、路徑不變。
- API 維持 0.x、可隨時破壞，直到正式抽取才談穩定。

cargo workspace：

```
fleety/ (workspace)
  crates/
    agent-core/      # 未來的框架（後抽成 submodule）：model provider 抽象、tool trait+registry、
                     # tool-calling loop、durable event log+resume、approval/policy hook、audit hook。
                     # 零裝置概念、零 Fleety 依賴。
    fleety-protocol/ # CLI↔server wire types、tool schema（跨語言契約留口）
    fleety-server/   # device registry、memory、connector、co-location、capability → 依賴 agent-core
    fleety-daemon/   # fleetyd（後期 milestone）
    fleety-cli/      # ratatui TUI → 依賴 fleety-protocol
```

借鑑 eve 但不依賴它：agent 即目錄（對應 per-device 記憶資料夾）、durable workflow checkpoint 續跑（對應事件序列+重連回放）、approvals、skills/MCP——當設計參考。

agent-core 從一開始要留好的 seam（都是 trait，零 Fleety 依賴）：

- **儲存抽象**：event log / memory / 還原點透過 trait 注入，**絕不假設寫在 cwd**。呼應「workspace = dirty work」——core 不該知道 workspace 在哪，由 Fleety 提供 store。
- **`ContextCompressor`**：進 model 前壓縮 context 的 hook，後面掛原生壓縮模組（見 §10.1）。
- **知識 Wiki 子系統**：Obsidian 格式的長期知識庫基元（vault 位置由 Fleety 注入，見 §14）。generic、隨框架帶走。
- **Scheduler**：自管排程的原生基元——存排程、評估觸發、fire→spawn 一個 agent run。generic（學 openclaw 的 cron 機制），Fleety 在上層綁裝置／conversation context 與無人值守政策（見 §10.2、`prompts/policy.md`）。
- **輸出通道**：core 支援多通道輸出（至少 `display` + `speech`），通道是泛用概念；Fleety 在上層加「device deixis」（叫使用者看某裝置的儀表板之類）。voice mode 是 per-session/per-message 旗標，旗標關閉就不產 speech、不花 token。
- **model provider / tool / approval / audit** hook：如前述。

### 10.1 Token 節省 / context 壓縮（headroom 技術原生內建）

決策：**把 headroom 的技術用 Rust 原生重寫進 agent-core**，不接它的 Python 本體。這樣未來抽出框架時，這些壓縮能力一起帶走，是框架的內建差異化。全部放在 `ContextCompressor` trait 後面。

階段一（演算法類，無 ML，M1 起就做進 agent-core）：

1. 工具輸出預算化——大 stdout／檔案／diff 進 context 前截斷＋摘要。
2. JSON 壓縮（對應 SmartCrusher）——陣列／巢狀物件結構化壓縮。
3. AST-aware 程式碼壓縮（對應 CodeCompressor）——用 tree-sitter（Rust 綁定）依語言壓。
4. 可逆壓縮（對應 CCR）——完整原文存 Fleety store，context 只放摘要＋handle，需要時用工具取回。我們儲存層本來就有。
5. cache-stable 前綴（對應 CacheAligner）——system prompt／tool defs 固定順序，吃 provider KV cache。
6. 記憶即壓縮——讀 NOTES.md／capabilities 索引，不重播完整歷史。
7. **context 視窗壓實（compaction，參考 [TimLai666/agent](https://github.com/TimLai666/agent) `internal/compaction.py`）**——context 逼近預算（**約 75%**）時 runtime 自動觸發，**兩階段、便宜先行**：
   - **A 修剪舊工具輸出（便宜）**：只留最近約 8 筆工具輸出 inline，更舊的截到小預算（約 700 tokens）。若降到門檻以下就**跳過摘要**，省一次 LLM 呼叫。
   - **B 摘要舊回合（較貴）**：保留最近約 20 則訊息逐字，更舊的交 LLM 摘要（依時序萃取：使用者意圖、技術決策、檔案變更、錯誤、待辦；輸出結構化 analysis+summary，多個 prompt fallback）。摘要呼叫本身**輸入預算化**（壓縮輸入上限、前次摘要上限），免得摘要又爆。仍**保留所有 user 訊息**（高訊號）。
   - 保留穩定前綴（前面不動、壓中後段，對齊 §10.3 KV cache）。
   - **可逆**：事件序列 `conversations/{id}.jsonl` 是真相（比 TimLai666 的 transcriptPath 更完整），壓實只影響「送進模型的視圖」，`conversation_read`／召回可還原全文。
   - 參數（75% 觸發、留 8 工具輸出／20 訊息、輸入預算）是起點，依實測調整；與 §5.1 對話摘要共用同一套摘要器。

階段二（ML prose 壓縮，對應 Kompress-base，較重、延後、可選）：

- 需要 ML 模型。要原生就用 **Rust ML runtime（candle 或 onnxruntime）在 process 內跑，不開 Python sidecar**。模型載入失敗就降級回階段一（守永不崩潰）。
- 評估成本後再決定自訓／轉換現成模型或先用非 ML 啟發式。先不做，trait 留好位置。

授權注意：依「技術概念」用 Rust clean-room 重寫，不抄原始碼；若要參考 headroom 原始碼，先確認其授權相容。

### 10.2 自管排程 / cron（學 openclaw）

讓 agent 依需求自己建立與管理排程。學 openclaw 的 cron 機制，但補上 Fleety 缺的安全層。

- **scheduler 在 Server**（長駐、stateful），排程持久化在 **Fleety store**（與 workspace 分離）。即使沒有 CLI 連著也會照時間 fire。
- **觸發型態**：cron 表達式（週期）、`at`（一次性指定時間）、`every`（間隔）。
- **排程內容**：要跑的 prompt／指令、context 綁定（origin／target 裝置、cwd/workspace、開新 conversation 還是續接哪個）、**mandate（授權範圍，建立時談好，含這個 job 可做的 critical 動作）**、enabled、created_by、next_run／last_run／last_result。
- **fire 行為**：Server 起一個 agent run（task + 事件序列），帶存好的 prompt + context。結果落地、可被使用者重連後看到（接 §5 resume）。
- **無人值守靠「建立時授權」而非「fire 時核准」**（見 `prompts/policy.md` → Unattended Runs）：
  - mandate **從使用者語意推斷**，不要他另外宣告授權範圍。推**最小覆蓋範圍、寧窄不寬**，以具體動作存下來。只有推斷涉及 critical／不可逆或有歧義時，用一句話覆述確認；純 routine 直接做並告知。
  - **推斷在建立時做（人在、能糾正）；fire 時只嚴格比對已記錄的具體 mandate、不鬆散重推**——無人值守時鬆散重推正是 injection／幻覺擴權的破口。
  - fire 時在 mandate 內**全自動跑完、含 critical、不再問人**，full_access + audit + rollback。只有**超出 mandate**的動作（尤其 critical）才停泊回報。使用者不會被 routine 工作打擾。
- **離線錯過**：Server 曾停機錯過的排程，重啟預設 **不補跑、只回報**（避免一次灌爆）；可改 per-schedule 補跑一次。
- **永不崩潰**：單一排程失敗只記錄＋回報，不拖垮 scheduler 或 server，各 job 隔離。
- **agent 自管**：`schedule_create／list／show／update／delete`（見 `docs/tools.md`）。prompt 指引：週期或延後的任務才建排程、清掉過期／重複排程、別建失控排程、context 綁清楚。
- **seam**：scheduler 基元在 agent-core（generic）；裝置 context 綁定與無人值守政策在 Fleety 上層。

### 10.3 Cache 一致性與失效（每個 cache 都要有更新機制）

核心原則：**任何 cache 的 key 都要含「會影響其值的所有東西」的版本／指紋；來源一變（改檔、skill/MCP 熱重載、設定變更）就 bump 指紋、讓下游 cache 失效。正確性優先於命中率——寧可 miss 重算，絕不端過期內容給 model。**

逐個 cache：

- **KV-cache 前綴（§10.1 CacheAligner）**：前綴 = system prompt + tool registry + 啟用的 skills + MCP tool defs + 早期 context，給它算指紋。
  - **為何是一條前綴、不能分開 cache**：provider KV cache 物理上就是線性前綴，無法切成獨立可重用的塊。transformer 注意力讓每個 token 的 K/V 依賴前面所有 token，所以同一段文字在不同位置／前面內容變了，KV 就不同、不能重用。命中條件是「從第 0 token 逐字相同到某點」。因此唯一槓桿是排序，不是分段。（能分開的內容快取——檔案、壓縮、skill 原文、MCP schema——見下面各項，那是 app 層、跟位置無關，本來就各自 cache。）
  - **排序：最穩定在前、最易變在後**（system prompt → 工具 defs → 啟用中的 skill 內容）。KV cache 從第一個變動 token 之後全失效，把易變的往後排，skill 熱重載只失效後半段、保住前半。
  - skill/MCP 熱重載或 system prompt 改 → 重算指紋；變了就重建前綴、放棄舊 provider cache（這個 miss 是對的，端舊 tool defs 才是 bug）。
- **檔案內容 / 壓縮後檢視**：用 content hash 或 mtime+size 當 key。agent 自己改檔（apply_patch 等）後 runtime 知道檔變了 → 失效該檔的快取／壓縮表示。外部改檔靠「用前先讀」+ 重讀，跨可能變動的邊界不信舊讀值。
- **可逆壓縮原文（CCR store）**：原文不可變、用 handle 定址，本身無失效問題；但「某檔的壓縮摘要」要跟著檔案 hash 一起失效。
- **capability / facts cache**：已有 stale 狀態模型，用前現場驗證（見 `prompts/memory.md`）。
- **models.cache.json**：provider 模型清單，手動 refresh 或 TTL。

熱重載機制（skills / MCP）：

- 監看來源，變更時重載、重算 schema、bump tool-registry／skill 指紋、失效前綴 cache。
- **在 turn 邊界套用**，不在一次 model call 中途換 tool defs，避免污染進行中的回合。
- 重載失敗保留上一個可用版本、回報，不崩（接「永不崩潰」）。

seam：cache 與失效是 agent-core 的 framework 關注；skill／MCP／檔案來源變更要發事件 bump 指紋。Fleety 提供來源，agent-core 管 cache 與失效協定。

v0 範圍：前綴指紋 + 檔案 hash 失效屬 M1／M3；skills/MCP 熱重載失效等 skills/MCP 真的實作（post-v0）才生效，協定先定。

## 11. Milestones（vertical slice，每個可獨立驗證）

- **M0 骨架**：cargo workspace + 上述 crates、CI、永不崩潰底座（`Result`/`thiserror`、panic 邊界隔離、結構化錯誤事件）。能編譯能跑、無功能。
- **M1 agent-core loop（無裝置）**：model provider trait + openai_compatible（`reqwest`+SSE）、tool trait+registry、tool-calling loop、in-memory event log。用假工具跑通對話+工具呼叫，驗證 loop 與錯誤處理。**這就是未來框架，先單獨驗證。**
- **M2 Server + session + CLI 連線**：fleety-server WebSocket、harness/session、fleety-protocol wire types、CLI 連上、對話 round-trip、事件落地 JSONL。
- **M3 client_session 工具橋 + 本機工具**：CLI 端執行本機工具（read/list/search/write/patch/replace/terminal/git）、送 origin context、audit + rollback（git/backup）+ 偏寬 critical 分類器。**改檔/寫程式前先讀專案各層級 `AGENTS.md`／`CLAUDE.md`／`README`（root→目標目錄，deeper 覆蓋 root）並遵循**，現讀現用不靠記憶。→ 命中 demo #2（本機改 bug）。
- **M4 斷線恢復**：task record、event_id 回放、CLI 重連帶 last_seen_event_id。→ 命中 demo #3。
- **M5 Enrollment + 裝置模型 + 記憶**：`fleety init`、pairing code（console + 已配對裝置代發）、device.yaml（connectors[]/mobility/site）、per-device 記憶 + NOTES.md 自動維護。→ 命中 demo #1。
- **M6 精美 TUI（ratatui）**：把 plain CLI 升級成 header／conversation／activity／approval／input／status 的精美 TUI。
- **M7 語音對話（post-v0）**：終端做 STT／TTS（server 仍只進出文字）；啟用 speech 輸出通道 + device deixis；voice mode 旗標。協定欄位 M0–M2 就先留，引擎這裡才接。
- **M8 自管排程 / cron（post-v0）**：agent-core scheduler（cron/at/every、fire→spawn run）、`schedule_*` 工具、無人值守政策（critical 停泊回報）、離線不補跑。學 openclaw、補安全層（§10.2）。
- **M9 瀏覽器自動化（post-v0）**：browser skill，目標裝置本機 CDP 驅動該機 Chrome（managed/user profile）、snapshot-ref 動作、SSRF 防護、user-profile 綁 co-location/核准、登入態敏感 act＝critical（§12）。
- **M10 電腦操作 computer-use（post-v0）**：內建 computer-use-mcp 隨 runtime 自動裝在裝置端、版本隨 runtime 更新；screenshot 自由用、UI 控制節制使用且使用者活躍時先提醒；偏好順序 API/MCP > browser > computer-use（§13）。
- **M11 知識 Wiki（post-v0，輕量）**：agent-core wiki 子系統、`<agent-home>/wiki/` Obsidian vault、三層結構、`wiki_*` 工具（強制寫進 vault）、dedup/index/log/lint 管理機制、矛盾不靜默覆寫（§14）。

順序刻意把最難、最該驗證的（loop、永不崩潰、工具橋）放前面；enrollment 與 TUI 美化放後面。M7 語音是 post-v0。Skills/MCP v0 stub，不列 milestone。`speech` 輸出欄位與 voice mode 旗標在 fleety-protocol 從 M2 就先定義（不實作引擎），避免日後改協定。

## 12. 瀏覽器自動化（裝置能力，post-v0，學 openclaw）

讓 agent 連到任意一台裝置、用**那台裝置的 Chrome** 操作。學 openclaw 的 CDP 設計，套進 Fleety 跨裝置模型。

- **機制：CDP（Chrome DevTools Protocol），不靠 Playwright/Puppeteer。** 在目標裝置本機跑 CDP 控制、loopback 連該機 Chrome。
- **跑在目標裝置的 Chrome、透過該裝置 connector 分派**：device 的 fleetyd/session 本機開／接 Chrome、本機講 CDP；agent 只用高階 `browser` 工具下意圖、收結果。**CDP 流量留在裝置本機（CDP 很 chatty），只有高階意圖跨 connector。** 這正是「用那台裝置的 Chrome」：它有那台的登入/cookie、在那台的網路（能連該機 localhost／內網的儀表板）、人能在那台螢幕看著。
- **profile 模式**（學 openclaw 的 named profiles）：
  - `managed`：隔離的 agent 專用 profile、無登入、預設、安全。
  - `user`：接該裝置使用者的真實已登入 Chrome——高風險，agent 等於用使用者身分操作（信箱、銀行、購物）。**綁 co-location／核准**：接真實 profile 應在使用者人在該裝置時（能看著、能核准），對齊 `prompts/policy.md` 同場域與 critical。
- **動作（學 openclaw snapshot-ref，不用脆弱 CSS selector）**：`snapshot`（回 UI 樹＋穩定 ref）、`screenshot`、`act`（用 ref 點/打字/拖/選/捲）、`navigate`、`open`、`tabs`、`wait`、`evaluate`、`dialog`、`status`。單一 `browser` 工具帶 action。
- **風險對齊既有政策**：在 `user`（已登入）profile 裡「對外送出/花錢/刪除」的 act＝critical 要確認；無人值守排程要動 browser 必須在 mandate 內。
- **網頁內容不可信**：snapshot/DOM/頁面文字是資料不是指令（prompt injection 主要管道）。**SSRF 防護**：預設擋私有網路，任務需要才窄允許（例如「看這台的儀表板」要明確允許該 host）。
- **vision fallback**：主模型無視覺時，screenshot 交給影像模型描述成文字。
- **封裝為 skill**：browser skill 提供 capability `browser.automate` + `browser` 工具 + 偵測 Chrome/CDP 的 probe。Fleety 上層、非 agent-core，對齊 Skills 系統。核心 system prompt 不需內建瀏覽器細節，由 skill 注入。

## 13. 電腦操作 computer-use（裝置能力，post-v0）

讓 Fleety 操作每一台裝置的桌面，或截圖看某台裝置在幹嘛。內建 [computer-use-mcp](https://github.com/domdomegg/computer-use-mcp)（nut.js、跨平台、screenshot/click/type/move/scroll/key）。

- **內建、隨 client runtime 自動裝在裝置端**：computer-use-mcp 在被控裝置本機跑（控制該機螢幕/鍵鼠），所以必須裝在裝置端。歸入 builtin MCP（見 §3 內建 vs 安裝），跟 CLI/daemon 一起自動安裝。
- **版本維持最新，但走 runtime 更新通道、不在執行期亂抓 latest**：pin 到隨 runtime 發布的測試過版本，靠 fleety-updater 整包更新（守永不崩潰）。執行期 `npx latest` 這種會在無人值守時突然壞掉，不採用。
- **device-scoped**：透過該裝置 connector 分派，screenshot/動作的 handle 綁該 device（見 §2 不變式）。
- **使用節制（UI 控制會干擾使用者）**：
  - **screenshot 例外**：低衝擊、純觀察，可自由用（含使用者正在用時）——「看某台在幹嘛」就靠它。
  - **UI 控制（click/type/move/scroll/key）會搶使用者的滑鼠鍵盤**：節制使用、非高頻；且**偏好順序 API/MCP > browser(CDP) > computer-use**，computer-use 是沒有更好介面時的最後手段（pixel 點擊又脆又擾人）。
  - **使用者活躍時先提醒**：動作前看該裝置的「使用者活躍/idle」訊號（最近輸入時間）；若使用者正在用，先提醒再接管。需要裝置回報活躍狀態。
- **安全**：作者明言「給模型完整控制權、可能大破壞」。沿用 audit + critical gate；高破壞性桌面動作走 policy.md critical；無人值守排程要 computer-use 必須在 mandate 內。
- **封裝**：builtin MCP，agent 透過 `mcp_call` 分派到該裝置；非 agent-core。

## 14. 知識 Wiki（Obsidian 格式，學 Hermes llm-wiki）

讓 agent 把所學整理成 Obsidian 格式的互聯筆記，當長期「第二大腦」。**專屬位置 + 管理機制，不是給個 skill 讓它隨處建。**

**範圍是通用知識，不限裝置**：上網研究的主題、某技術怎麼運作、debug 心得、拼湊出的概念——任何學到的都提煉進來，裝置只是來源之一。**而且是活的知識**：不斷**擴充**（新知）、**提煉**（raw 來源 → 乾淨概念頁）、**精進**（回頭加深/重構/合併既有頁）、**修正**（更好資訊到了就更新、調 confidence、解矛盾），不是寫一次就擺著、也不是只 append。raw→distilled 的提煉與持續精進靠 lint＋可掛排程的 consolidation 維持。

- **專屬 vault**：`<agent-home>/wiki/`，與 workspace、per-device 記憶分開。可直接用 Obsidian 開（wikilink/graph/dataview）。
- **三層結構（學 Hermes）**：
  - `raw/`（articles｜papers｜transcripts｜assets）：原始來源、唯讀不改、frontmatter 帶 `source_url`／`ingested`／`sha256`（偵測 re-ingest drift）。
  - type 淺資料夾（只一層）：`concepts/`、`entities/`、`howto/`、`comparisons/`、`summaries/`、`queries/`、`moc/`。agent 維護的筆記。
  - `SCHEMA.md`（領域範圍、慣例、tag taxonomy）、`index.md`（依 type 編目）、`log.md`（時序動作）。
- **筆記格式**：YAML frontmatter（title, created, updated, type, tags, sources；選配 confidence、contradictions）；檔名 lowercase-with-hyphens；用 `[[wikilinks]]`、**每頁至少 2 條外連**（防孤兒）。
- **分類模型（四層、由硬到軟，刻意淺資料夾）**：
  1. **Layer**：raw（原料）／distilled（成品）／meta。結構分，非主題分。
  2. **Type**：每頁**一個** type（concept／entity／howto／comparison／summary／query／moc），對應淺資料夾＋frontmatter `type`。**只一層、不深層巢狀。**
  3. **Tags（主題/領域分類，真正的主軸）**：多值，定義在 `SCHEMA.md` 的 tag taxonomy，**是活的**——隨知識成長、被 lint/consolidation 重構。主題分類靠 tag，不靠資料夾。
  4. **Wikilinks ＋ MOC**：連結圖才是真正的組織。跨主題入口用 **MOC（Map of Content）**筆記（`moc/moc-rust.md` 連起所有 rust 頁），比深資料夾彈性。`index.md`＝依 type 的全域目錄；MOC＝依主題的入口。
  - **取捨**：刻意「淺資料夾（只分 type）＋重 tag/連結/MOC」。知識天生多維（一頁可同屬 rust＋esp32＋embedded），硬塞單一資料夾樹會打架；Obsidian/Zettelkasten 共識也是資料夾淺、分類靠 tag 與連結。這也讓分類本身能隨「活的知識」演進，不被一開始釘死的架構綁住。
- **管理機制（你強調的重點）**：
  - **orient-first**：每次動作前先讀 SCHEMA／index／log，避免重複與漏連。
  - **dedup**：建立前先 search ＋ 查 index，已有就更新、不新建。
  - **建立門檻**：實體/概念出現在 2+ 來源、或在單一來源居核心才開頁；>200 行拆分；棄用內容**封存不刪**。
  - **矛盾處理**：新資訊衝突比日期、新的通常蓋舊；真矛盾就**兩種立場都記** ＋ frontmatter 標 contradictions ＋ flag 使用者，**絕不靜默覆寫**。
  - **index／log 維護**：新頁加進 index（依 type、字母序、一行摘要、計數＋更新日）；動作 append 到 log（>500 輪替）。
  - **lint**：掃孤兒、斷連、index 缺漏、stale（>90 天）、矛盾、單來源無 confidence、超長頁。可掛**排程（§10.2）**定期跑（mandate＝維護 wiki）。
- **工具化、不散落**：透過 `wiki_*` 工具寫，**runtime 強制只寫進 vault、套慣例**；不讓 agent 在 workspace 或別處亂建筆記。
- **與 per-device 記憶的界線**：device 記憶＝某台節點的操作性事實（現場驗證、device-scoped）；wiki＝跨裝置的 durable 概念/研究知識（互聯、Obsidian）。「ESP32 燒錄怎麼運作」→ wiki concept；「pi-a 有 espflash 在 /usr/bin」→ pi-a 的 capabilities/NOTES。
- **seam**：wiki 是 agent-core 的泛用知識子系統（隨框架帶走），vault 位置由 Fleety 注入。
- 選配（後期）：server 環境用 obsidian-headless ＋ Obsidian Sync 背景同步。

## 15. 與其他檔的關係

- `prompts/protocol.md` `memory.md` `policy.md` `rules.md`：Agent 的 system prompt，描述 v0 Agent 的行為基準。
- `docs/tools.md`：工具命名正典，v0 實作的工具須對齊。
- 本檔：v0 範圍與驗收。三者任一改動牽涉介面時，要同步另外兩者。
