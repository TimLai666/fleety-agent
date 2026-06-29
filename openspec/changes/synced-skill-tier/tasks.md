## 1. synced tier 接進 skills runtime

- [x] 1.1 在 crates/fleety-server/src/storage.rs 新增 `skills_synced_dir()`(home/skills/synced);在 crates/fleety-server/src/skills.rs 的 `Tiers` 加 `synced` 欄、`collect()` 併入第 4 層(synced 最低、被 builtin/authored/installed 覆蓋)、`register` 簽章多收 synced dir,並更新兩處呼叫端(crates/fleety-server/src/conn.rs 與 crates/fleety-server/src/scheduler.rs)傳 `storage.skills_synced_dir()`,交付 "A synced skill tier updates from a repo at runtime" 的 tier/precedence 面;對應設計「新增第 4 層 `skills/synced`,precedence 最低」。先寫失敗測試:synced 目錄放一個 skill、同名也放進 builtin → collect 回 builtin 版(precedence);synced 不存在 → collect 與現況三層相同。

## 2. 純函式(skill_sync.rs 的可測核心)

- [x] 2.1 [P] 在 crates/fleety-server/src/skill_sync.rs 實作純函式:`skill_dirs_from_extracted(root) -> Vec<String>`(repo 根的**頂層**且含 SKILL.md 的子目錄名,sorted)、`should_sync(remote_sha, local_sha: Option) -> bool`(local 無或不同→true),交付 "Syncing is conditional on the repo's latest commit" 與 "The synced tier mirrors the repo's skills" 的判定面;對應設計「commit-SHA 條件同步(沒變不抓)」「mirror = clean replace 該層(只認頂層含 SKILL.md 的目錄)」。先寫失敗測試:用 spec example 表驗 should_sync;造臨時樹(a/SKILL.md、b/SKILL.md、c 無、root 散檔)→ skill_dirs_from_extracted = [a,b]。

## 3. 同步流程(下載 + mirror + never-crash)

- [x] 3.1 在 skill_sync.rs 實作一次同步 `sync_once(synced_dir, repo, ...)`:取 repo main 最新 commit sha(GET api.github.com/repos/<repo>/commits/main)→ should_sync 為 false 就回;否則用 reqwest 下載 codeload main.zip → 既有 zip 解壓到暫存 → 取解壓後的 repo 根(GitHub archive 的單一 wrapper 目錄)→ 把 skill_dirs_from_extracted 列出的頂層含 SKILL.md 目錄複製進一個 **staging 目錄** + 寫 `.synced-sha`(此檔不算 skill)→ **原子替換** synced(移除舊 synced、rename staging 進去),repo 已移除的目錄因整層重建而消失;下載/解壓/API/IO 任何錯 → log warn、保留現有 synced、回 Ok(不崩),交付 "The synced tier mirrors the repo's skills, additions and removals" 與 "Syncing never crashes and is configurable" 的同步/失敗面;對應設計「mirror = clean replace 該層(只認頂層含 SKILL.md 的目錄)」「never-crash + 原子替換」。先寫失敗測試(無網路):抽出 `rebuild_into(staging, repo_root)` + swap 邏輯,以「含 a/b 的 repo 根」→ synced 出現 a/b + `.synced-sha`;再以「只含 a 的 repo 根」→ synced 只剩 a(removal inherent);重跑結果穩定。

## 4. 背景迴圈 + 啟動接線

- [x] 4.1 在 skill_sync.rs 實作 `spawn(synced_dir)`:讀 env(FLEETY_SKILLS_SYNC 預設開、"0" 關 → 不 spawn;FLEETY_SKILLS_SYNC_REPO 預設 TimLai666/skills;FLEETY_SKILLS_SYNC_INTERVAL_SECS 預設 3600),tokio task 開機同步一次後每 interval 呼叫 sync_once;在 crates/fleety-server/src/main.rs 開機處(對齊 scheduler/gc 的 spawn)呼叫 `skill_sync::spawn(storage.skills_synced_dir())`,交付 "A synced skill tier updates from a repo at runtime" 的「開機 + 定時」面與 "Syncing never crashes and is configurable" 的開關面;對應設計「背景同步模組 skill_sync.rs,開機 spawn + 定時」。驗證:FLEETY_SKILLS_SYNC=0 時 spawn 不啟動同步(單元/邏輯測試:讀 env 的開關判定);真開機定時同步手動驗證。

## 5. 文件

- [x] 5.1 [P] 在 docs/env.md 記錄 FLEETY_SKILLS_SYNC / FLEETY_SKILLS_SYNC_REPO / FLEETY_SKILLS_SYNC_INTERVAL_SECS 與 synced tier 行為(執行期定時從 repo 同步、precedence 最低可被覆蓋、mirror 增減、SHA 條件、never-crash、server-only),交付各 requirement 的文件面。驗證:內容審查涵蓋三個 env、precedence、mirror 增減、SHA 條件、server-only。
