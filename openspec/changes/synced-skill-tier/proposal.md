## Why

內建 skill 目前全部編進 binary(`builtin_skills::seed`),要更新就得發 fleety 新版。使用者維護一個 skill repo(預設 `TimLai666/skills`,~45 個 skill,每個頂層資料夾一個),希望這些 skill 不必等發版就能更新——fleety-server 執行期定時從 repo 的 main 同步,且 repo 裡 skill 的增減要在本地 mirror 同步增減。這是一個與「編進 binary」本質不同的 skill 來源:**執行期、可變、定時同步**。

## What Changes

- 新增第 4 層 skill tier **`skills/synced`**(`home/skills/synced`),precedence 最低(installed > authored > builtin > synced):由同步模組獨佔管理,binary 不碰。`skills.rs` 的 tier 合併、`register` 與 storage 各加這一層。
- 新背景模組 **`skill_sync.rs`**:fleety-server 開機 spawn,開機同步一次 + 每 `FLEETY_SKILLS_SYNC_INTERVAL_SECS`(預設 3600)同步。
- **commit-SHA 條件同步**:每 tick 先取 repo main 最新 commit sha,與本地存的上次 sha 比;相同就跳過(只花一次輕量 API),不同或本地尚無才下載 main.zip(reqwest)、用既有 zip 解壓、mirror,並存新 sha。
- **mirror = clean replace**:repo 裡「頂層含 SKILL.md 的資料夾」寫入/更新,synced 裡 repo 已無的移除;忽略 repo root 的零散非 skill 檔。
- **never-crash**:下載/解壓/網路/API 任何失敗都保留上次成功副本、log warn、不崩。
- **env**:`FLEETY_SKILLS_SYNC`(開關)、`FLEETY_SKILLS_SYNC_REPO`(預設 TimLai666/skills)、`FLEETY_SKILLS_SYNC_INTERVAL_SECS`。server-only。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `synced-skill-tier`: 一個執行期定時從外部 repo 同步的 skill tier(`skills/synced`,precedence 最低)+ commit-SHA 條件下載 + mirror 增減 + never-crash 背景同步迴圈;skill 不必等 fleety 發版即可更新。

### Modified Capabilities

(none)

## Impact

- Affected specs: synced-skill-tier(新)
- Affected code:
  - New:
    - crates/fleety-server/src/skill_sync.rs(背景同步:SHA 條件檢查、下載解壓、mirror clean-replace、純函式 + 迴圈 spawn)
  - Modified:
    - crates/fleety-server/src/skills.rs(新增 synced 第 4 層 tier:Tiers + collect 合併,precedence 最低;register 多收 synced dir)
    - crates/fleety-server/src/storage.rs(新增 skills_synced_dir())
    - crates/fleety-server/src/main.rs(開機 spawn skill_sync,對齊 scheduler/gc 模式)
    - crates/fleety-server/Cargo.toml(若需 tar/額外解壓;目前 reqwest + zip 已足夠,視實作而定)
    - docs/env.md(FLEETY_SKILLS_SYNC / _REPO / _INTERVAL_SECS)
