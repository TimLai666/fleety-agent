## Context

Fleety 的指令檔目前只由 `prompts/protocol.md` 指示 agent 自己 `read_file` 逐層讀 AGENTS.md / CLAUDE.md,runtime 不參與。缺口:跨裝置對話讀不到發起端的檔、沒有 user 全域層、且不保證被讀。

本 change 建在 session-workspace-origin-injection 之上:那個 change 讓 runtime 知道對話 origin 的 device 與 cwd,並提供「每輪 ephemeral system preamble、不入歷史、不被壓縮洗掉」的注入通道(與核心記憶 / 當前時間同款)。本 change 沿用同一通道,注入的是指令檔內容而非 origin 身分。跨裝置讀檔沿用既有 device_exec / bridge 路由,不新增傳輸機制。

主要張力是 context 成本:指令檔(尤其 CLAUDE.md)可能很大,而「逐層 + user 全域 + 每輪重放」若不控制會爆 context。設計核心就是在「保證餵到」與「不吃爆預算」之間取平衡。

## Goals / Non-Goals

**Goals:**

- runtime 自動、保證地注入「專案根 → origin cwd 逐層」與「發起裝置 user 全域」的 AGENTS.md / CLAUDE.md 到單一對話。
- 跨裝置時經 device_exec 從發起裝置讀回;同主機直接讀。
- 走 ephemeral 每輪重放通道防洗;去重與大小上限控制 context。
- 作用域僅限該對話,不外洩。

**Non-Goals:**

- 不載入 skills(Fleety 有獨立 skills tier,屬後續 change)。
- 不引入 plugin / hook 執行框架(Fleety 無此概念,需先 discuss)。
- 不做「每次 read_file 都重掃全樹」的昂貴版本。
- 不取代 agent 主動 read_file(仍可讀更深 / 更新內容)。
- 不修改 origin-injection 的行為,只依賴它。

## Decisions

### 純函式決定指令檔蒐集集合與逐層順序

以一個純函式從 (project_root, cwd, user_home) 算出要蒐集的指令檔路徑清單:從最上層往目標目錄,每一層各取 AGENTS.md 與 CLAUDE.md,末端加上 user 全域(~/.claude/CLAUDE.md、~/.agents/AGENTS.md)。清單有序(淺→深,越深越後、優先度越高,呼應 protocol.md「deeper 覆寫 root」),並在集合層級去重。純函式易測、與 I/O 分離。
替代方案:直接在 conn.rs 內嵌蒐集邏輯 —— 難測、易與注入耦合。故抽純函式。

### 綁定時注入初始樹與 user 全域,離開初始樹再按需補注

對話綁定時蒐集並注入「初始樹(root → cwd)+ user 全域」一次;之後 agent 讀到初始樹以外的目錄時,才補注該目錄鏈尚未注入的指令檔。避免每次 read_file 全樹重掃的成本,同時覆蓋離開初始工作區的情況。
替代方案:(a) 只注入 cwd 當層 —— 漏掉祖層 root 指令;(b) 每次 read_file 重掃全樹並注入 —— context 與 I/O 成本高、重複嚴重。故採「綁定一次 + 離開初始樹按需補」。

### 跨裝置經 device_exec 讀回發起端指令檔

當 origin 在別台裝置,指令檔在該裝置上,以既有 device_exec 路由到該裝置讀取檔案內容;同主機則直接讀本機。蒐集純函式產出路徑後,讀取層依 origin device 決定走本機或 device_exec。
替代方案:把指令檔納入裝置級 skill_sync 同步 —— 那是全域、非 per-conversation,且與「發起端當下狀態」語意不符。故用 device_exec 即時讀。

### 去重與大小上限避免 context 爆量

同一路徑的檔只注入一次(集合去重);設每檔位元組上限與單次注入總量上限,超過則裁切並標示已截斷。上限為具名常數,可經環境變數覆寫。
替代方案:無上限全量注入 —— 大型 CLAUDE.md 會吃爆每輪預算(尤其每輪重放)。故硬性上限兜底。

### 注入走 ephemeral 每輪重放且作用域僅限該對話

注入內容掛在該對話每輪重建的 ephemeral system preamble(不 append 進 conversation history),故不被壓縮摘要洗掉、且每輪冪等不累加。蒐集到的指令檔集合屬該對話狀態,不進入全域或其他對話。
替代方案:注入一次寫進歷史 —— 長對話會被壓縮吃掉,違背「保證餵到」。故每輪重放,並靠上限控制成本。

## Implementation Contract

**Behavior:** 對話綁定後,其每輪 context 的 system preamble 區含「專案根 → cwd 逐層」與「發起裝置 user 全域」的 AGENTS.md / CLAUDE.md 內容,淺→深排序、去重、受大小上限。跨裝置對話的內容來自發起裝置(經 device_exec 讀回)。agent 讀到初始樹以外的目錄後,該目錄鏈尚未注入的指令檔也會出現在後續輪次。注入只在該對話可見,不影響其他對話。

**Interface / data shape:** 蒐集純函式輸入 (project_root, cwd, user_home),輸出有序去重的候選路徑清單(每層 AGENTS.md 與 CLAUDE.md + user 全域兩檔)。讀取層輸出 (path, source_device, content) 清單。每檔位元組上限與總量上限為具名常數(可經環境變數覆寫)。對話狀態記錄「已注入路徑集合」以支援去重與按需補注。

**Failure modes:** 檔不存在 → 略過該路徑。跨裝置來源讀取失敗或裝置離線 → 略過該來源並在注入中留一則簡短註記,不阻斷對話。超過大小上限 → 裁切內容並標示已截斷。無 origin / 舊 CLI → 只注入 server 端可及的路徑(退化),不報錯。

**Acceptance criteria:**
- 純函式測試 `collect_instruction_paths_layers_and_dedupes`:給定 root/cwd/user_home,回傳淺→深逐層 + user 全域、且去重的路徑清單。
- 純函式測試 `collect_skips_missing_and_caps_size`:缺檔略過;超過每檔 / 總量上限時裁切並標記截斷。
- 測試 `injection_is_per_conversation`:一個對話的指令檔注入不出現在另一個對話的 context。
- 測試 `on_demand_appends_out_of_tree_dir`:讀到初始樹外目錄後,該目錄鏈指令檔補進後續輪次且不重複已注入者。
- 手動 / 整合:同主機與跨裝置對話的 preamble 各含預期指令檔內容(跨裝置來自發起裝置),不重複。

**Scope 邊界:** in scope —— 蒐集純函式(新模組)、conn.rs 的綁定時注入與按需補注、workspace binding 攜帶蒐集來源、本機與 device_exec 讀取整合、上述測試。out of scope —— skills / plugins / hooks 載入、plugin/hook 框架、每次 read_file 全樹重掃、origin-injection 本身的行為。

## Risks / Trade-offs

- [指令檔大 + 每輪重放吃爆 context] → 每檔與總量硬上限、去重;必要時只注入頭部並標截斷;上限可經環境變數調。
- [每輪重放造成累加] → 走 ephemeral 通道、不入歷史,冪等重建(與核心記憶注入同理)。
- [跨裝置讀取延遲 / 失敗拖慢或中斷對話] → best-effort:失敗略過該來源、留註記,不阻斷。
- [「專案根」界定不明] → 見 Open Questions;預設往上逐層直到 user_home 或檔系統根,全部納入(去重),以上限兜底。
- [與 agent 主動 read_file 內容重疊] → 注入是補強不是取代;去重僅在注入集合內,agent 仍可主動讀更深 / 更新內容。

## Migration Plan

純新增蒐集與注入邏輯,無資料遷移。部署後新對話與 resume 的對話在下一輪即獲得指令檔注入(resume 依賴 origin-injection 已持久化的 origin 來源重建蒐集)。Rollback:移除注入呼叫與新模組即可,無持久化結構變更(蒐集集合為對話執行期狀態)。

## Open Questions

- 「專案根」如何界定:一路往上到 user_home / 檔系統根逐層蒐集(簡單、涵蓋廣、靠上限兜底),或偵測 git root 為界?建議首版採「逐層往上直到 user_home 或檔系統根」,避免對非 git 專案失效。
- user 全域層涵蓋範圍:僅 ~/.claude/CLAUDE.md 與 ~/.agents/AGENTS.md,或也含 ~/AGENTS.md、~/CLAUDE.md?建議首版限前兩者。
- 大小上限預設值與是否對超大檔改注入摘要而非裁切:建議首版硬裁切 + 截斷標記,摘要列後續增強。
