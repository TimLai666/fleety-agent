## 1. 純函式(backup.rs 可測核心)

- [x] 1.1 [P] 在 crates/fleety-server/src/backup.rs 實作純函式:`is_excluded(rel_under_home: &Path) -> bool`(models、skills/builtin、skills/synced、fleet/backups → true;memory/wiki/conversations 等 → false);`remote_url(repo: &str, token: &str) -> Result<String>`(owner/repo → https://x-access-token:<token>@github.com/<repo>.git;非法 repo 形狀 → Err;token 不出現在 Err 訊息);`is_private_repo(api_json: &str) -> bool`(解析 private 欄:true→true、false/缺→false);`pre_restore_path(orig: &Path, ts: &str) -> PathBuf`(orig 加 .pre-restore-<ts> 後綴);`commit_message(ts: &str) -> String`,交付 "Back up non-regenerable state to a user-configured private repo" 的排除面、"Refuse to push to a non-private repo" 的判定面、"Restore from backup preserves existing data first" 的路徑面;對應設計「範圍:copy agent_home 減去 exclude-set + 兩個 config 檔(不是寫死 include 清單)」「推前私密驗證(防明文金鑰上公開 repo)」「repo/認證不寫死 + 沒設就不啟用」「Restore:先保留現有 home,再放回」。先寫失敗測試:用 spec example 表驗 is_private_repo;is_excluded 四個 true + 三個 false;remote_url owner/repo 正確、非法→Err;pre_restore_path 後綴。

## 2. 備份執行(複製 + git;server 端動作)

- [x] 2.1 在 backup.rs 實作 `run_backup(agent_home, config_path, providers_path, mirror, repo, token)`:把 agent_home 遞迴複製進 mirror(用 is_excluded 跳過排除路徑)+ 複製 config.toml/providers.toml 進 mirror;mirror 沒 .git 就 git init + 設 remote(remote_url);`git -C mirror add -A`;有 diff 才 commit(commit_message);push 前先 GET api.github.com/repos/<repo> 用 is_private_repo 判定,非 private → 不 push、warn、回 RefusedNotPrivate;private 才 push;回 BackupOutcome(NothingChanged/Pushed/RefusedNotPrivate);複製/git/API/網路任何錯 → 回 Err(caller log warn、不崩),交付 "Back up non-regenerable state to a user-configured private repo" 與 "Refuse to push to a non-private repo" 的執行面;對應設計「機制:本地 git mirror + push,不用 GitHub API 造物件」「推前私密驗證(防明文金鑰上公開 repo)」。先寫失敗測試(無網路,用真 git 在暫存 repo):造 home 有 memory + models + skills/builtin + fleet/backups → 複製進 mirror 後 mirror 有 memory、config.toml、providers.toml,無 models/builtin/backups;第一次 add+commit 有新 commit,狀態不變再跑 → 無新 commit(NothingChanged)。

## 3. 排程 spawn + 手動子指令 + 啟動接線

- [x] 3.1 在 backup.rs 實作 `spawn(agent_home, config_path, providers_path, mirror)`:讀 env(FLEETY_BACKUP_REPO 未設 → 不 spawn、log info;FLEETY_BACKUP_TOKEN;FLEETY_BACKUP_INTERVAL_SECS 預設 3600),tokio task 每 interval 呼叫 run_backup、依 outcome log(Pushed/NothingChanged/RefusedNotPrivate)、Err→warn;在 crates/fleety-server/src/main.rs 開機處(對齊 gc/skill_sync spawn)呼叫 `backup::spawn(...)`;並在 main.rs 的子指令分派(對齊既有 `config` 分派)加 `backup` → args[2]=="now" 跑一次 run_backup 後退出、其餘印用法,交付 "Runtime scheduling and manual trigger";對應設計「排程 + 手動」。驗證:未設 FLEETY_BACKUP_REPO → spawn 不啟動(讀 env 判定的單元測試);`backup now` 未設 repo → 明確訊息;真定時/手動 push 手動驗證。

## 4. Restore 子指令

- [x] 4.1 在 backup.rs 實作 `restore(agent_home, config_path, providers_path, repo, token, ts)`:clone repo 到暫存;成功後把現有 agent_home、config.toml、providers.toml 各自 rename 成 pre_restore_path(…, ts) 保留;把 clone 內容放回 agent_home 對應位置與兩個 config 檔;回 Ok 並讓 caller 印「已還原,請重啟;舊資料保留在 …」;clone 失敗 → 不改現有任何東西、回 Err;在 main.rs 的 `backup` 子指令加 `restore` 分派(server 停機時人工跑,跑完退出),交付 "Restore from backup preserves existing data first";對應設計「Restore:先保留現有 home,再放回」。先寫失敗測試(無網路):以「已 clone 好的暫存目錄」直接呼叫 restore 的放回邏輯 → 現有 home 被改名為 .pre-restore-<ts>(仍存在)、新內容就位;clone 前失敗不動現有 home(以 pre_restore 尚未發生驗證)。

## 5. config registry + 文件

- [x] 5.1 [P] 在 crates/fleety-tools/src/config.rs registry 新增 FLEETY_BACKUP_REPO(Server)、FLEETY_BACKUP_TOKEN(Server, secret=true)、FLEETY_BACKUP_INTERVAL_SECS(Server, 預設 3600);在 docs/env.md 記錄三者 + 備份範圍(排除 models/builtin/synced/rollback backups)+ 私密驗證 + `backup now` / `backup restore` 用法 + **安全警告**(金鑰明文進 repo,token/repo 權限外洩=全部密鑰外洩,務必私人 repo + 最小權限 PAT),交付各 requirement 的設定/文件面;對應設計「repo/認證不寫死 + 沒設就不啟用」「never-crash + 安全警告」。驗證:registry 測試涵蓋新鍵、FLEETY_BACKUP_TOKEN 遮罩(display_value);docs 內容審查涵蓋三 env、排除清單、private 驗證、restore、安全警告。
