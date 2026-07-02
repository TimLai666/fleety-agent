## Why

Fleety 的所有持久狀態(對話、記憶、wiki、設定、模型金鑰、裝置配對 token…)都在 server host 的 `~/.fleety` 底下。目前**沒有備份**:主機掛了、磁碟壞了、或誤刪,這些都沒了,且金鑰/配對得全部重來。使用者要一個「設定一個私人 GitHub repo,Fleety 自動把全部東西備份上去(含金鑰),需要時能從備份恢復」的功能。repo 由**使用者設定,程式不寫死**。

## What Changes

- 新模組 `backup.rs`(fleety-server):把不可重生的狀態複製進本地 git mirror(`~/.fleety/backup-mirror`),`git add -A && commit && push` 到使用者設定的私人 repo。git 天然給增量(沒變不 commit/push)、歷史版本、restore = clone。
- **備份範圍**(不可重生):conversations/history、memory、wiki、devices、sites、schedules、skills/installed、skills/authored、auth.json、mcp installed 設定、cookies、`config.toml`、`providers.toml`(含 API key,明文,依使用者要求)。**排除**(可重生/超大):models、skills/builtin、skills/synced、rollback backups、workspace。
- **不寫死**:`FLEETY_BACKUP_REPO`(使用者設定)+ `FLEETY_BACKUP_TOKEN`(PAT,secret)。沒設 repo → 完全不備份。
- **排程 + 手動**:開機 spawn 背景迴圈每 `FLEETY_BACKUP_INTERVAL_SECS`(預設 3600)備份;server 子指令 `fleety-server backup now` 手動觸發。
- **私密驗證**:push 前用 GitHub API 確認該 repo 是 private,否則拒推(防把明文金鑰推上公開 repo)。
- **恢復**:`fleety-server backup restore`(server 停機時跑)clone repo,先把現有 home 改名為 `<home>.pre-restore-<時間戳>` 保留(可反悔),再放回備份內容,提示重啟。
- **never-crash**:任何失敗 log warn、保留本地 mirror、不崩。加密 MVP 不做(明文放使用者私人 repo,文件警告)。server-only。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `fleet-backup`: server 端把不可重生的 Fleety 狀態(含明文金鑰)mirror 進本地 git、自動 commit + push 到使用者設定的私人 GitHub repo(推前驗證 private)、定時 + 手動觸發、以及從備份恢復(先保留現有 home)。

### Modified Capabilities

(none)

## Impact

- Affected specs: fleet-backup(新)
- Affected code:
  - New:
    - crates/fleety-server/src/backup.rs(備份 include/exclude 清單 + mirror 複製 + git argv + private 驗證 + 排程 spawn + restore;純函式抽出可測)
  - Modified:
    - crates/fleety-server/src/main.rs(開機 spawn backup 迴圈;`backup now` / `backup restore` 子指令分派)
    - crates/fleety-tools/src/config.rs(registry 新增 FLEETY_BACKUP_REPO、FLEETY_BACKUP_TOKEN(secret)、FLEETY_BACKUP_INTERVAL_SECS)
    - crates/fleety-server/src/storage.rs(若需 backup-mirror 目錄路徑 helper)
    - docs/env.md(備份 env + 安全警告 + restore 用法)
