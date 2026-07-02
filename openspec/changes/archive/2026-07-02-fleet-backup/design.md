## Context

Fleety 的持久狀態在 server host:`agent_home()`(預設 ~/.fleety/agent;可被 FLEETY_AGENT_HOME 覆寫)底下有 fleet/(conversations/history、grants…)、memory、wiki、devices、sites、schedules、skills/{builtin,installed,authored,synced}、auth.json、mcp installed、cookies、models、fleet/backups(rollback store)。另外 config_path()(fleety_tools::config,預設 ~/.fleety/config.toml)與 providers_config::providers_path()(~/.fleety/providers.toml,含模型 API key)在 agent home 的上一層。server 已 shell out 系統 git(subagent.rs),git CLI 存在是既有假設;已有 reqwest;已有 gc/scheduler/skill_sync 開機 spawn 背景迴圈;main.rs 已有 `config` 這類「子指令跑完就退」的分派;config registry 支援 secret 標記與遮罩。

## Goals / Non-Goals

**Goals:**

- 把不可重生的 Fleety 狀態(含明文金鑰,依使用者要求)自動備份到**使用者設定的**私人 GitHub repo,程式不寫死 repo。
- 定時 + 手動;沒變不推;推前確認 repo private。
- 能從備份恢復,且不毀掉現有資料(先保留)。
- never-crash;沒設 repo 就完全不啟用。

**Non-Goals:**

- 對稱加密(MVP 明文);備份 device 端狀態;經連線遠端觸發 restore;GitHub API 造 tree/blob(用 git CLI);選擇性單檔還原;多 repo/多目的地。皆列後續。

## Decisions

### 機制:本地 git mirror + push,不用 GitHub API 造物件

`backup.rs` 維護一個 git repo `~/.fleety/backup-mirror`(= agent_home().parent()/backup-mirror)。每次備份:把該備份的內容複製/更新進 mirror → `git -C mirror add -A` → 有 diff 才 `commit` → `push`。首次:若 mirror 沒有 .git 就 `git init` + 設 remote。git 天然給增量、歷史、restore=clone。理由:server 已依賴系統 git;git 免自己造 blob/commit,且本地就有完整版本歷史。

### 範圍:copy agent_home 減去 exclude-set + 兩個 config 檔(不是寫死 include 清單)

備份 = 遞迴複製 agent_home,**排除**這些頂層/子路徑:`models`、`skills/builtin`、`skills/synced`、`fleet/backups`、以及 mirror 自身(若在其下);**外加** config_path() 與 providers_path() 兩個檔複製進 mirror(分別為 `config.toml`、`providers.toml`)。用 exclude-set 而非 include 清單:未來 agent home 長出新狀態目錄會自動被納入,不會漏備份(呼應「不要寫死清單」)。判定 `fn is_excluded(rel_under_home: &Path) -> bool` 純函式可測。

### repo/認證不寫死 + 沒設就不啟用

env(config registry):`FLEETY_BACKUP_REPO`(owner/repo 或完整 https URL,使用者設定)、`FLEETY_BACKUP_TOKEN`(PAT,secret 遮罩)、`FLEETY_BACKUP_INTERVAL_SECS`(預設 3600)。remote URL = `https://x-access-token:<PAT>@github.com/<owner/repo>.git`(owner/repo 形式時組裝;完整 URL 則注入 token)。**未設 FLEETY_BACKUP_REPO → 不 spawn 迴圈、backup now 回明確訊息**。純函式 `fn remote_url(repo, token) -> Result<String>`(驗證 repo 形狀、防注入)。

### 推前私密驗證(防明文金鑰上公開 repo)

push 前 GET `api.github.com/repos/<owner/repo>` 讀 `private`;`private != true`(或查不到)→ **拒絕 push**、log warn、保留本地 mirror(已 commit,下次仍可推)。純函式 `fn is_private_repo(api_json: &str) -> bool`(解析 JSON 的 private 欄)。理由:明文金鑰,推上公開 repo 是災難級誤操作,零成本防呆。

### 排程 + 手動

main 開機:若有 FLEETY_BACKUP_REPO 就 `backup::spawn(...)`(對齊 gc/skill_sync),每 interval 跑一次備份。手動:server 子指令 `fleety-server backup now`(對齊 main.rs 既有 `config` 子指令分派,跑完即退)。

### Restore:先保留現有 home,再放回

server 子指令 `fleety-server backup restore`(server 停機時人工跑)。流程:clone(用 repo+token)到暫存 → **把現有 agent_home 改名為 `<agent_home>.pre-restore-<timestamp>`(不刪,可反悔)、config.toml/providers.toml 同樣先改名保留** → 把備份內容放回 agent_home 對應位置與 config 檔 → 印出「已還原,請重啟 server;舊資料保留在 …pre-restore-…」。不做開機自動 restore(太魔法)。純函式 `fn pre_restore_path(orig: &Path, ts: &str) -> PathBuf`。

### never-crash + 安全警告

複製/git/API/網路任何錯 → log warn、保留現有本地 mirror、不崩、下個 tick 再試。文件明確警告:金鑰明文進 repo,`FLEETY_BACKUP_TOKEN` 或 repo 存取權外洩 = 全部密鑰外洩;務必用私人 repo + 最小權限 PAT。

## Implementation Contract

**行為(Behavior):**

- 未設 FLEETY_BACKUP_REPO:開機不 spawn;`backup now` 回「未設定備份 repo」訊息;不建 mirror。
- 設好後 tick / `backup now`:複製狀態進 mirror(排除 models/builtin/synced/rollback backups)、含 config.toml + providers.toml → add -A → 有變才 commit → private 驗證通過才 push。
- repo 非 private → 不 push、warn,mirror 仍有本地 commit。
- 沒有 diff(狀態沒變)→ 不 commit、不 push(git no-op)。
- `backup restore`:現有 agent_home + config 檔先改名 .pre-restore-<ts> 保留,再放回備份;印出重啟提示。
- 任何失敗 → warn、保留現況、不崩。

**介面 / 資料形狀:**

- storage:`pub fn backup_mirror_dir(&self) -> PathBuf`(agent_home.parent()/backup-mirror)。
- backup.rs 純函式:`is_excluded(rel_under_home: &Path) -> bool`(models / skills/builtin / skills/synced / fleet/backups);`remote_url(repo: &str, token: &str) -> Result<String>`;`is_private_repo(api_json: &str) -> bool`;`pre_restore_path(orig: &Path, ts: &str) -> PathBuf`;`commit_message(ts: &str) -> String`。
- backup.rs 動作:`async fn run_backup(agent_home, config_path, providers_path, mirror, repo, token) -> Result<BackupOutcome>`(BackupOutcome:NothingChanged / Pushed / RefusedNotPrivate);`pub fn spawn(...)`;`async fn restore(agent_home, config_path, providers_path, repo, token, ts) -> Result<()>`。
- main.rs:開機 spawn(有 repo 時);`backup` 子指令(`now` / `restore`)分派,跑完即退。
- config registry:FLEETY_BACKUP_REPO(Server)、FLEETY_BACKUP_TOKEN(Server, secret)、FLEETY_BACKUP_INTERVAL_SECS(Server,預設 3600)。

**失敗模式:**

- token/repo 未設 → 不啟用 / 明確訊息。
- repo 形狀非法 → remote_url 回 Err、不動作。
- API 查 private 失敗 / 非 private → 不 push、warn、留本地 commit。
- git 不存在 / push 認證失敗 / 網路失敗 → warn、留本地 mirror、不崩。
- restore 時 clone 失敗 → 不動現有 home(改名發生在 clone 成功後)、warn。

**驗收標準(Acceptance):**

- 單元測試:`is_excluded`(models/skills builtin/skills synced/fleet backups → true;memory/wiki/conversations → false);`remote_url`(owner/repo → 正確 https+token URL;非法 repo → Err;token 不出現在錯誤訊息);`is_private_repo`(private:true→true、false→false、缺欄→false);`pre_restore_path`(orig + ts → orig 帶 .pre-restore-<ts> 後綴)。
- 整合測試(無網路):以臨時 home 造出 memory/models/skills 等 → 執行「複製進 mirror(排除)」邏輯 → mirror 有 memory、無 models/builtin/synced/backups、有 config.toml/providers.toml;git init+add+commit 用真 git(server 本就依賴 git)在暫存 repo 驗證「沒變→no-op、變了→有新 commit」;push/clone/private-API 為手動驗證。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒。

**範圍邊界:**

- In scope:backup.rs(清單/URL/private/pre-restore 純函式 + mirror 複製 + git + 排程 spawn + restore)、main.rs(spawn + backup 子指令)、config registry 三條目、storage backup_mirror_dir、docs。
- Out of scope:加密、device 端、遠端 restore、GitHub API 物件、單檔還原、多目的地、agent-core/協定變更。

## Risks / Trade-offs

- [明文金鑰進 repo] → 強制推前 private 驗證 + 文件強警告;加密列後續(FLEETY_BACKUP_PASSPHRASE)。
- [repo 膨脹] → 排除 models/backups/builtin/synced;真的很大時使用者可再排除(後續可設定)。
- [PAT 進 remote URL 可能被 log] → 只在記憶體組裝、錯誤訊息不含 token;git remote 存 .git/config(在 mirror 內、mirror 不進備份)。
- [restore 破壞性] → 先改名保留現有 home,絕不直接刪;server 停機時人工跑。
- [git 未安裝] → 與現有 git 依賴一致;缺就 warn 不崩。

## Migration Plan

- 純加層:沒設 FLEETY_BACKUP_REPO 時完全不動作、零影響。無資料遷移。回滾:移除 spawn + 子指令 + backup.rs;backup-mirror 目錄可留可刪。

## Open Questions

- 可選對稱加密(FLEETY_BACKUP_PASSPHRASE)後再 push。
- 備份 device/daemon 端狀態。
- 經連線遠端觸發 backup/restore(目前 server host 本機子指令)。
- 選擇性還原單一對話/檔案、多 repo 目的地、可設定的額外 exclude。
