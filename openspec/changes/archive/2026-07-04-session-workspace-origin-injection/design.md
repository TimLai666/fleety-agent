## Context

session-workspace 目前的實作:conn.rs 在對話第一則訊息呼叫 `resolve_binding`,用 origin 的 `cwd` / `hostname` 決定工具的實體 root(同主機時 root = cwd),並把 `WorkspaceBinding { root, device }` 持久化;此外只寫一行 tracing log。origin context 從未以模型讀得到的文字呈現。

`prompts/protocol.md` 的「Origin Awareness」段落已假設「When the runtime attaches origin context to a message ... target = origin device, operating in its cwd」,並在動檔案前要 agent 逐層讀 origin 專案的 AGENTS.md / CLAUDE.md。但這個前提沒被兌現:runtime 沒有 attach origin 給模型。後果是跨裝置對話時,agent 不知道 origin 在哪台、cwd 是什麼,不會 device_exec 過去,也讀不到 origin 專案的逐層指示檔(那些檔在別台、server 本地沒有)。

每輪重放、不入對話歷史的 ephemeral system preamble 機制已經存在:conn.rs 組 turn 時,先 push 核心記憶(ME/USER/TODO)與當前時間兩個 `Message::system`,再 `storage.load` 持久歷史。origin 提示可搭同一班車,不需新建防洗機制。

## Goals / Non-Goals

**Goals:**

- 每個 turn 把 origin context(device id、hostname、os、cwd,可選 git 狀態)以 ephemeral、不寫入對話歷史的 system message 注入,長對話下不被壓縮摘要洗掉。
- origin 存進 WorkspaceBinding,使 resume 與後續 turn(未必再帶 origin)仍能一致重放。
- 讓 agent 依 protocol.md 既有指示,在跨裝置時自行 device_exec 到 origin device(含逐層讀 origin 的 AGENTS.md / CLAUDE.md)。
- 同主機與跨裝置的注入措辭區分,避免誤導。

**Non-Goals:**

- 不在工具分派層把裸 file/command/git 呼叫自動路由到 origin device。full-access agent 已能用 device_exec 顯式路由,自動搬工具是過度設計,且牽涉把 cwd 語意傳到目標裝置的額外複雜度。
- 不改動 device_exec / Hub / RunTool 路由機制本身。
- 不把 origin device 的 online/offline 探測列為硬需求(至多 best-effort,見 Open Questions)。
- 不動 session-workspace 既有的 fallback、origin-untrusted、resume 一致性三條 requirement,只改「自動路由」那一條的方向。
- 不處理稽核清單的其他項目。

## Decisions

### 以 ephemeral system preamble 每輪注入 origin,而非寫入對話歷史

沿用核心記憶 / 當前時間同款做法:在組 turn 的 system preamble 區 push 一個 origin `Message::system`,每輪重建、不 append 進 conversation。這樣它不是歷史的一部分,壓縮摘要(SmartCrusher / rolling summary)不會把它洗掉,也不會每輪堆積。
替代方案:(a) 放進第一則 user message —— 會隨對話變長被壓縮摘要吃掉,正是要避免的;(b) 只放進持久 system prompt —— resume 沒問題,但長對話中對開頭段落的注意力衰減,且無法反映「這個對話綁定的具體 origin」。故選 ephemeral 每輪重放。

### 將 origin 原始欄位存入 WorkspaceBinding

WorkspaceBinding 目前只有 `root` 與 `device`。新增 `origin_cwd` / `origin_hostname` / `origin_os`(皆 Option),在綁定時一併存下,之後每輪從 binding 讀出來組注入文字。
替代方案:每輪直接從當前 message 的 origin 讀 —— 後續 turn 與 resume 的訊息未必帶 origin(或帶了不同裝置的 origin),會讓提示消失或漂移。origin 是「對話綁定一次」的屬性,應隨 binding 持久化。

### 同主機與跨裝置採不同注入措辭

同主機時工具已 root 在 cwd,提示說明「origin 即本機,工具已在此目錄」,避免 agent 多此一舉包 device_exec。跨裝置時裸工具在 server 執行,提示明確指出「origin 在 device X 的 D,要動它的檔案用 device_exec(device=X)」——這是 agent 知道該路由到 X 的唯一線索。
替代方案:單一通用措辭 —— 跨裝置時會讓模型誤以為裸工具就落在 origin,導致在 server 上讀寫錯目錄。

### 交由 agent 自行 device_exec,不在分派層自動路由

注入 origin 後,路由決策交給 agent + protocol.md 既有指示,不在 server 分派層攔截裸工具。這是最小改動,且與 Fleety「full-access + 顯式 device_exec」的既有定位一致。
替代方案:分派層把裸工具自動轉成 device_exec —— 需要把 root=cwd 的語意傳到目標裝置的 on-device registry、處理目標離線、與顯式 device_exec 行為重疊,複雜度高且偏離定位。列為 Non-Goal。

## Implementation Contract

**Behavior:** 每個 turn 的 model context system preamble 區,除既有核心記憶與當前時間外,多一段 origin 說明,標明對話 origin 的 device id、hostname、os、cwd(git 狀態可選)。同主機與跨裝置措辭不同。該段每輪重建、不寫入 conversation。resume 一個已綁定的對話後,同一段 origin 說明仍存在且內容一致。跨裝置對話中,agent 能據此段用 device_exec 存取 origin device(含逐層讀其 AGENTS.md / CLAUDE.md)。

**Data shape:** `WorkspaceBinding` 新增三個可選欄位:`origin_cwd: Option<String>`、`origin_hostname: Option<String>`、`origin_os: Option<String>`。序列化需向後相容:既有已持久化的 binding 缺這些欄位時,反序列化以 `None` 載入,退化為現行「僅 device + root」行為。`resolve_binding` 的簽章擴充以接收並回傳 origin os(cwd / hostname 已是入參),同主機與跨裝置兩分支都填入這些欄位。

**注入文字(格式契約,非逐字):** 一段簡短 system 文字,含 device id、hostname、os、cwd;跨裝置版本額外含「用 device_exec(device=<id>) 在該裝置操作」的指引。維持一至二句,git 狀態預設精簡。

**Failure modes:** origin 缺失(舊 CLI 不送 OriginContext,或 cwd 空/相對)→ 不注入 origin 段(或注入指向 server workspace 的 fallback 說明),對話照常進行,對應既有 fallback requirement。binding 反序列化缺新欄位 → 三欄位為 None,注入退化或省略,不報錯。

**Acceptance criteria:**
- 單元測試:`resolve_binding` 在同主機、跨裝置兩情境都回傳含 origin_cwd / origin_hostname / origin_os 的 binding。
- 單元測試:純函式的注入文字產生器對「同主機」「跨裝置」「無 origin」三情境各產生預期文字(跨裝置版本含 device_exec 指引、無 origin 版本不含 origin 段)。
- 單元測試:WorkspaceBinding 序列化往返保留三新欄位;缺欄位的舊 JSON 反序列化為 None。
- 手動驗證:跨裝置對話中,agent 於回應/工具選擇反映 origin device 與 cwd,並在需要動 origin 檔案時使用 device_exec。

**Scope 邊界:** in scope —— workspace.rs(binding 欄位 + resolve_binding)、conn.rs(每輪注入 + 綁定時存 origin)、storage.rs(binding 持久化欄位)、session-workspace spec 改寫、上述測試。out of scope —— device_exec/Hub/RunTool 機制、online/offline 探測硬需求、分派層自動路由、其他稽核項目。

## Risks / Trade-offs

- [注入文字過長,反而吃掉 context 預算] → 固定一至二句精簡格式;git 狀態預設只帶 branch + dirty 旗標或省略。
- [每輪重放造成堆積] → 走 ephemeral 通道、不入歷史,天然冪等(與當前時間注入同理)。
- [resume 後 origin 失真] → 以 binding 持久化 origin 欄位解決,並用序列化往返測試覆蓋。
- [跨裝置時 origin device 離線,agent 仍嘗試 device_exec] → 注入僅標身分不保證線上,device_exec 失敗沿用既有錯誤路徑;是否標 online 列 Open Questions。
- [spec 改寫與既有 requirement 衝突] → 只改「自動路由」一條,保留 fallback、origin-untrusted、resume 三條不動。

## Migration Plan

純新增欄位與注入邏輯,無資料遷移:舊 WorkspaceBinding JSON 以 None 相容載入。部署後新對話立即獲得 origin 注入;既有對話 resume 時若 binding 無 origin 欄位則退化為現行行為(下次重新綁定才會補上,或視實作在 resume 時以當前 origin 補寫)。Rollback:還原 workspace.rs / conn.rs / storage.rs 三檔即可,持久化的多餘欄位對舊程式無害(被忽略)。

## Open Questions

- 注入是否帶 git 狀態(branch + dirty)?建議首版帶 branch + dirty 旗標,best-effort,取不到就省略。
- 是否在注入中標示 origin device 目前 online / offline?需查 Hub live 狀態、增加耦合;建議首版不做,列為後續增強。
- resume 一個「綁定於舊格式(無 origin 欄位)binding」的對話時,是否用當前訊息的 origin 回填 binding?建議:若當前訊息帶 origin 且與 binding.device 一致則回填,否則維持退化行為。
