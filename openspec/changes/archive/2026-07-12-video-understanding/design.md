## Context

現有可重用積木(來自 codebase 盤點):

- **原生 shell-out 工具範式** `crates/fleety-tools/src/insyra.rs`:`register_insyra(registry, root)`、`locate_binary()`(env `FLEETY_INSYRA_BIN` → beside-exe → PATH)、`Command::new(bin)...spawn()`、`impl Tool`(`spec()` 給 `ToolSpec{name,description,parameters,risk}`、`async call()`)。跨裝置的關鍵:工具**不是**放進 `register_workspace`,而是**同時**註冊在 `fleety-server/src/tools.rs::build_registry` 與 `fleety-daemon/src/ondevice.rs::build_local_registry`,daemon 才會廣播它、server 才能 `device_exec` 分派。
- **pip 依賴自動佈署範式** `crates/fleety-server/src/builtin_mcp.rs`(ddgs):`auto_install_enabled()`(`FLEETY_*_AUTO_INSTALL != "0"`)、`resolve_*_command`(env→PATH→bare)、async `*_runs` 探測、async `try_install_*`(有序 `[(cmd,&[args])]` 首個成功者勝:pipx / pip --user / python -m pip)、`Dependency::new(name, Strategy::UserPackage, Some(env_key), probe, install)`、boot 於 `ensure_dependencies()`。Python 已是 `deps::server_default_deps()`(`["python","ddgs","node","insyra"]`)一員,crv 裝進該 Python。
- **系統二進位 OS-pkg 安裝範式** `crates/fleety-tools/src/chrome.rs`:`try_package_manager_install()` 的 per-OS `attempts: &[(&str,&[&str])]`(winget / brew / apt|snap,首個 `status.success()` 勝,best-effort)、`find_*_binary`(env→候選/PATH)、`resolve_or_install`(偵測→若啟用則裝→再偵測)。
- **builtin skill vendoring** `crates/fleety-server/src/builtin_skills.rs`:`SKILLS: &[(&str,&Dir)]` 以 `include_dir!` 內嵌整個 `builtin-skills/<name>/` 目錄;`seed()` 開機清空重寫,若有 `HEADER.md` 則折疊進該 skill 的 `SKILL.md` 前面(Fleety 註記騎在上游之上)。
- **型別化設定** `crates/fleety-tools/src/config.rs`:`registry()` 陣列加 `Setting{key,scope,default,description,secret,validator}`;`v_onoff`/`v_bool` 驗證器;enum/bool 要同步 `setting_choices()`。既有 `_BIN` 類旗標(`FLEETY_INSYRA_BIN`/`FLEETY_DDGS_BIN`/`FLEETY_CHROME_BIN`)是直接讀原始 env、不進 registry。

約束:所有佈署路徑 best-effort、開機永不阻塞;下載二進位用 staged `.new`→chmod→rename 原子替換;Windows 解析子行程輸出要 `tr -d '\r'`/strip CR。

## Goals / Non-Goals

**Goals:**

- 一個一級、跨裝置(device_exec 可分派)的 `video_extract` 工具,把影片(URL 或本機檔)轉成少量關鍵影格 + 選配 transcript + manifest,輸出落在 workspace 可讀處。
- crv 與 ffmpeg 開箱自動佈署(server 端),whisper 為 opt-in。
- 一個引導用 builtin skill,把 agent 導向原生工具而非直接開 CLI。

**Non-Goals:**

- 不 vendor crv Python 原始碼、不改 `--video` 多模態路徑、不做 crv 進階旗標(`--why`/`--kb`/`--grid`/`--report`)。
- 不在每台 daemon 開機自動裝 crv/ffmpeg(裝置端 opt-in / 按需)。

## Decisions

### 決策一:video_extract 工具契約(crates/fleety-tools/src/video.rs)

新增 `struct VideoExtract { root: PathBuf }` 與 `register_video(registry, root)`。與 insyra 不同,crv 是**一次性 CLI**,故不維持長駐 Proc —— 每次 `call` spawn 一次 `crv` 等結束。

- 參數:`source`(必填,URL 或 workspace 相對/絕對路徑)、`out?`(workspace 相對輸出目錄,預設 `crv-out/<sanitized-source-stem>`)、`scene?`(f64)、`fps_floor?`(f64)、`max_frames?`(u32)、`lang?`(字串)、`transcribe?`(bool)。
- 組指令:`crv <source> -o <abs_out> [--scene x] [--fps-floor x] [--max-frames n] [--lang x] [--no-transcribe]`。`transcribe=false` 或 whisper 不存在時加 `--no-transcribe`。`out` 以 workspace root 解析成絕對路徑(沿用 resolve_for_write 類邏輯,含 sensitive-path guard)。
- 定位 crv:`locate_crv()` = env `FLEETY_CRV_BIN`(is_file)→ `which("crv")` → bare `crv`/`crv.exe`。
- 回傳:讀輸出目錄後回 `{ out_dir(相對), manifest_path, manifest(文字內容), frames:[相對路徑], frame_count, transcript_path?(存在才給) }`,讓 agent 直接 `read_file`(manifest/transcript)與 `read_file_bytes`(影格)。
- risk = `Mutate`(寫檔 + yt-dlp 網路下載 + 跑 ffmpeg/whisper)。
- 逾時:影片處理可能很久(下載+ffmpeg+whisper),用可設逾時 `FLEETY_VIDEO_TIMEOUT_SECS`(預設較長,如 900s);逾時殺子行程回可讀錯誤。
- 缺 crv/ffmpeg:回可讀、可行動的錯誤(「安裝方式 / 設 FLEETY_CRV_BIN」),不 panic。
- 端點解析與指令組裝的可測部分抽純函式(如 `build_crv_args(params)->Vec<String>`)。

### 決策二:跨裝置註冊

`register_video` 同時加到 `fleety-server/src/tools.rs::build_registry`(server workspace)與 `fleety-daemon/src/ondevice.rs::build_local_registry`(裝置本地),與 `register_insyra` 並列。如此每台 daemon 廣播 `video_extract`,server 端 `device_exec(device, "video_extract", …)` 可在有影片的裝置抽格。

### 決策三:crv 自動佈署(仿 ddgs)

新增 crv 的 `auto_install_enabled`(`FLEETY_CRV_AUTO_INSTALL`)、async `crv_runs`(`crv --version`)、async `try_install_crv`(有序:`pipx install --force claude-real-video`、`pip3/pip install --user -U claude-real-video`、`python -m pip …`;`FLEETY_VIDEO_WHISPER=on` 時 package 改 `claude-real-video[whisper]`)、`crv_dependency() -> deps::Dependency`(Strategy::UserPackage)。加入 `deps::server_default_deps()`。yt-dlp 為 crv 相依,pip 一併帶入。（可放 builtin_mcp 旁或新模組 `deps/crv.rs`;沿用 Dependency 框架。）

### 決策四:ffmpeg 自動安裝(仿 Chrome OS-pkg)

新增 `ensure_ffmpeg()`:`find_ffmpeg`(env `FLEETY_FFMPEG_BIN` → `which("ffmpeg")`/`ffprobe`)→ 缺且 `FLEETY_FFMPEG_AUTO_INSTALL` 啟用 → `try_package_manager_install_ffmpeg()`(winget `Gyan.FFmpeg` / brew `ffmpeg` / apt `ffmpeg`,首個成功者勝,best-effort)→ 再偵測。開機在 server 端 ensure(與 crv 同批),best-effort 不阻塞。ffmpeg 無可靠 managed-download fallback,故缺且裝不起來時,`video_extract` 回可讀錯誤引導手動安裝。

### 決策五:whisper opt-in

轉錄需 openai-whisper(torch,重)。預設**不裝** whisper:crv 核心給場景感知影格、可用內嵌字幕。`FLEETY_VIDEO_WHISPER=on` 才在安裝時用 `[whisper]`。工具 `transcribe` 預設 false;true 但 whisper 不存在時,退回 `--no-transcribe` 並在回傳註記「transcription skipped (whisper not installed; set FLEETY_VIDEO_WHISPER=on)」。

### 決策六:設定與 skill

- registry 新增 `FLEETY_VIDEO_WHISPER`/`FLEETY_FFMPEG_AUTO_INSTALL`/`FLEETY_CRV_AUTO_INSTALL`(scope Server、on/off、`v_onoff`),同步 `setting_choices()`;`FLEETY_CRV_BIN`/`FLEETY_FFMPEG_BIN`/`FLEETY_VIDEO_TIMEOUT_SECS` 依 _BIN 慣例讀原始 env。
- `builtin-skills/fleety-real-video/SKILL.md`(YAML frontmatter `name`+`description`;MIT 授權附註指向上游),導向 `video_extract` 工具用法、轉錄取捨、read_file_bytes 讀影格、device_exec 跨裝置。加入 `builtin_skills.rs::SKILLS`。

## Implementation Contract

**Behavior:**

- Agent 呼叫 `video_extract{source:"<url|path>", …}` → runtime 跑 crv → 回輸出目錄、manifest 內容、影格相對路徑清單、frame_count、（若有）transcript 路徑;agent 隨後 `read_file`/`read_file_bytes` 讀內容分析。
- server 開機自動確保 crv(pip)+ ffmpeg(OS-pkg)存在(best-effort);whisper 僅在 `FLEETY_VIDEO_WHISPER=on` 時裝。
- `device_exec(device,"video_extract",args)` 在該裝置抽格(前提:該裝置有 crv/ffmpeg)。

**Interface / data shape:**

- 工具名 `video_extract`;params 見決策一;回傳 JSON `{out_dir, manifest_path, manifest, frames[], frame_count, transcript_path?, notes?}`;risk `Mutate`。
- `register_video(registry:&mut ToolRegistry, root:&Path)`;純函式 `build_crv_args(params)->Vec<String>`。
- 新 env:`FLEETY_CRV_BIN`、`FLEETY_CRV_AUTO_INSTALL`、`FLEETY_FFMPEG_BIN`、`FLEETY_FFMPEG_AUTO_INSTALL`、`FLEETY_VIDEO_WHISPER`、`FLEETY_VIDEO_TIMEOUT_SECS`(前三個 on/off 者進 registry + config list)。

**Failure modes:**

- crv 缺(佈署失敗/離線)→ 工具回可讀錯誤含安裝指引,不 panic。
- ffmpeg 缺且裝不起來 → 工具回可讀錯誤引導手動裝。
- 逾時 → 殺子行程、回逾時錯誤。
- whisper 不存在但 transcribe=true → 退回不轉錄 + notes 註記。
- 佈署路徑全 best-effort,開機不阻塞、不 `?`-propagate。

**Acceptance criteria:**

- 單元測試:`build_crv_args` 對各參數組合(含 transcribe=false→`--no-transcribe`、預設 out 命名、sensitive 路徑擋)產出正確參數;crv/ffmpeg 定位在 env override 下回該路徑。
- 工具在 crv 不存在時回可讀錯誤(可用假 FLEETY_CRV_BIN 指向不存在檔驗證錯誤訊息,不 panic)。
- `video_extract` 出現在 server 與 daemon registry(build_registry / build_local_registry),`FLEETY_*_AUTO_INSTALL`/`FLEETY_VIDEO_WHISPER` 出現在 config list。
- `cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 乾淨。

**Scope boundaries:**

- In:video.rs 工具 + 兩處註冊、crv/ffmpeg/whisper 佈署、config keys、builtin skill、docs、新 spec。
- Out:vendor crv Python 碼、daemon 預設佈署 crv/ffmpeg、`--video` 多模態路徑、crv 進階旗標、devices 端開機自動佈署。

## Risks / Trade-offs

- **重相依**:whisper→torch 很大,故預設 opt-in;ffmpeg 是系統二進位,OS-pkg 安裝在 Linux 需 root、可能靜默失敗(best-effort,缺就回可讀錯誤)。
- **跨裝置佈署缺口**:v0 只在 server 自動佈署;裝置端要 crv/ffmpeg 才能在該裝置抽格,否則工具回可讀錯誤 —— 列為後續(daemon opt-in 佈署)。
- **crv 輸出契約耦合**:回傳 schema 綁 crv 的輸出目錄結構;上游若改結構需同步(以讀目錄+manifest 文字為主,降低耦合)。
- **網路egress / 成本**:yt-dlp 下載 + whisper 轉錄耗時耗資源;逾時可設、risk=Mutate 受稽核。
- **授權**:crv 為 MIT,vendor 其 skill 需保留授權附註;依賴其 pip 套件不涉散布其原始碼。
