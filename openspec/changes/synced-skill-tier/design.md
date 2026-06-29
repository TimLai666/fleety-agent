## Context

skills runtime(crates/fleety-server/src/skills.rs)有三層 tier:`struct Tiers { builtin, authored, installed }`,`collect()` 依 installed > authored > builtin 合併,skill 以**目錄名**辨識(`entry.file_name`)。`register(registry, builtin, authored, installed)` 在兩處呼叫:conn.rs:1535、scheduler.rs:59。storage 提供 `skills_{builtin,authored,installed}_dir()` = `home/skills/{...}`。builtin tier 由 `builtin_skills::seed()` 每次開機 clean replace(binary-owned),所以執行期同步的 skill 不能放 builtin。fleety-server 已有 reqwest + zip + tokio;main.rs 已在開機 spawn 背景迴圈(scheduler、gc)。skills 是 server 端概念(agent loop 在 server;device 經 device_exec 只跑工具)。

## Goals / Non-Goals

**Goals:**

- 外部 repo(預設 TimLai666/skills)的 skill 不必等 fleety 發版即可更新:執行期定時同步。
- repo 裡 skill 增減 → 本地 mirror 增減。
- 沒更新時近乎零成本(SHA 條件)、失敗不崩、不污染 binary-owned 的 builtin。

**Non-Goals:**

- skill 內容簽章/來源驗證;多 repo;git 增量同步;repo allowlist;把 synced skill 鋪到 device。皆列後續。

## Decisions

### 新增第 4 層 `skills/synced`,precedence 最低

`Tiers` 加 `synced: PathBuf`;`collect()` 先放 synced(最低),再依序被 builtin、authored、installed 覆蓋(最終 installed > authored > builtin > synced);`register` 簽章多收一個 dir,兩處呼叫(conn.rs、scheduler.rs)傳 `storage.skills_synced_dir()`;storage 加 `skills_synced_dir()` = `home/skills/synced`。理由:synced 是外部來源,放最低 → 使用者 installed / agent authored / binary builtin 同名都能蓋過它;與 builtin 分開目錄,才不會被 `builtin_skills::seed()` 的 clean replace 清掉。

### 背景同步模組 skill_sync.rs,開機 spawn + 定時

新增 skill_sync.rs:`pub fn spawn(synced_dir, ...)` 在 main.rs 開機呼叫(對齊 scheduler/gc 的 spawn 模式),tokio task 迴圈:開機立即同步一次,之後每 `FLEETY_SKILLS_SYNC_INTERVAL_SECS`(預設 3600)一次。`FLEETY_SKILLS_SYNC=0` 則不 spawn。理由:server 端常駐迴圈;開機先同步避免首啟到第一 tick 的空窗。

### commit-SHA 條件同步(沒變不抓)

每 tick:先 `GET https://api.github.com/repos/<repo>/commits/main`(只取最新 commit sha),與 synced 旁存的 `.synced-sha` 比;**相同 → 跳過**(只花一次輕量 API);**不同或本地尚無 sha → 下載 `codeload.github.com/<repo>/zip/refs/heads/main`(reqwest)、用既有 zip 解壓到暫存、mirror、成功後把新 sha 寫入 `.synced-sha`**。理由:repo 多數時間沒變,條件檢查讓定時同步平時近乎零成本;sha 比 ETag 直觀好 debug。

### mirror = clean replace 該層(只認頂層含 SKILL.md 的目錄)

從解壓後的 repo 根挑出「**頂層**(repo root 的直接子目錄)且含 `SKILL.md` 的目錄」當 skill;忽略 repo root 的零散 .md/.py。寫入/覆寫這些 skill 目錄到 synced;synced 裡 repo 已無的 skill 目錄移除(repo 增減 → 本地增減)。`.synced-sha` 不算 skill、不被移除。理由:repo 的 skill 形狀是「頂層資料夾=skill」;clean replace 確保增減同步、不留髒檔。

### never-crash + 原子替換

下載/解壓先進**暫存目錄**,成功組好後再替換 synced 內容(避免半同步狀態被 skills 讀到);API/網路/解壓/IO 任何錯 → 記 warn、保留現有 synced 副本、本 tick 放棄,下個 tick 再試;首次就失敗 → synced 暫空。整個迴圈包在不會 panic 的錯誤處理裡。理由:never-crash;skills 隨時可能被 list/use 讀,half-written 會出錯。

## Implementation Contract

**行為(Behavior):**

- 開機(FLEETY_SKILLS_SYNC≠0):skill_sync 立即同步一次,之後每 interval 一次。
- tick 且 repo main sha == 上次 → 不下載、synced 不變。
- tick 且 sha 不同(或無 .synced-sha)→ 下載 main.zip、解壓、在 staging 重建整層(只放 repo 頂層含 SKILL.md 的目錄)+ 寫 .synced-sha → 原子替換 synced:repo 的目錄出現在 `skills/synced/<name>/`,repo 已移除的 `<name>` 因重建而消失。
- synced 的 skill 可被同名 installed/authored/builtin 覆蓋(precedence 最低)。
- 同步失敗(離線、404、壞 zip)→ synced 保留上次內容、log warn、不崩。
- FLEETY_SKILLS_SYNC=0 → 完全不同步(不 spawn)。

**介面 / 資料形狀:**

- storage:`pub fn skills_synced_dir(&self) -> PathBuf`(home/skills/synced)。
- skills.rs:`Tiers { builtin, authored, installed, synced }`;`collect` 4 層合併(synced 最低);`register(registry, builtin, authored, installed, synced)`(兩處呼叫端更新)。
- skill_sync.rs:`pub fn spawn(synced_dir: PathBuf)`(讀 env 自決 interval/repo/開關);純函式:`fn skill_dirs_from_extracted(root: &Path) -> Vec<String>`(頂層含 SKILL.md 的目錄名);`fn should_sync(remote_sha: &str, local_sha: Option<&str>) -> bool`(local 無或不同→true)。移除靠「整層在 staging 重建後原子替換」inherently 達成,不需 diff 函式。
- env:FLEETY_SKILLS_SYNC(預設開,"0" 關)、FLEETY_SKILLS_SYNC_REPO(預設 "TimLai666/skills";接受 owner/repo)、FLEETY_SKILLS_SYNC_INTERVAL_SECS(預設 3600)。

**失敗模式:**

- API 取 sha 失敗 → 本 tick 跳過(視為沒變或稍後再試),不動 synced、warn。
- zip 下載/解壓失敗 → 保留現有 synced、warn,不替換。
- 某 skill 目錄缺 SKILL.md → 不視為 skill(略過)。
- synced 目錄不存在 → 首次同步時建立。

**驗收標準(Acceptance):**

- 單元測試:`skill_dirs_from_extracted`(造一個臨時樹:頂層 a/SKILL.md、b/SKILL.md、c/(無 SKILL.md)、root 散檔 → 回 [a,b]);`should_sync`(None→true、同→false、異→true)。
- 整合測試(無網路):給一個「已解壓好的 repo 根」呼叫 rebuild+swap → synced 出現 a/b、`.synced-sha` 寫入;再以「移除 b 的 repo 根」rebuild+swap → synced 只剩 a(removal inherent);重跑相同樹 → 結果穩定。
- 既有 skills 三層測試/行為不變(synced 不存在時 collect 與現況相同)。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒;真網路下載 + 真 GitHub 同步為手動驗證。

**範圍邊界:**

- In scope:synced tier(storage/skills.rs/register 兩處)、skill_sync.rs(SHA 條件 + 下載解壓 + mirror clean-replace + spawn 迴圈 + never-crash)、main.rs spawn、docs。
- Out of scope:簽章/驗證、多 repo、git 增量、allowlist、device 鋪設、agent-core/協定變更。

## Risks / Trade-offs

- [自動 pull 外部 repo 的 skill 指令 = 自動載入指示] → 單一可設定 repo(預設使用者自己的)+ 可關閉;簽章驗證列後續。
- [每次有新 commit 就整包重抓] → SHA 條件已避免「沒變也抓」;真增量(只抓改動的 skill)列後續。
- [half-written 被讀到] → 暫存組好再原子替換。
- [precedence 放最低,使用者可能想讓 synced 蓋過 builtin] → v0 固定最低、語意清楚;要可調再說。

## Migration Plan

- 純加層:不開(FLEETY_SKILLS_SYNC=0 或無網路)時行為與現況相同(synced 為空/不存在,collect 照舊三層)。
- 無資料遷移。回滾:移除 spawn 呼叫 + synced tier,`skills/synced` 目錄可留可刪。

## Open Questions

- skill 內容簽章 / 來源驗證(防 repo 被竄改後自動載入)。
- 多 repo 同步 + 命名衝突策略。
- git 增量 / 只抓改動的 skill(省頻寬,雖 SHA 條件已省掉大部分)。
- 把 synced skill 也鋪到 device(目前 skills 僅 server)。
