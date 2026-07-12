## Why

Fleety 目前處理影片只能把原始影片當多模態附件丟給模型(`fleety ask --video`),昂貴且對長片無效。`claude-real-video`(MIT)提供「場景感知抽格 + 轉錄」:ffmpeg 偵測真正的畫面變化(非固定取樣)+ 去重,產出少量關鍵影格 + transcript + `MANIFEST.txt`,讓模型能實際「看懂」影片而不燒爆 context。使用者要求把這個技術**內建**進 Fleety,並選定最深整合:原生 `video_extract` 工具 + skill,ffmpeg 自動安裝。

## What Changes

- **原生 `video_extract` 工具**:在 fleety-tools 新增一個像 `insyra_exec` 的工具,shell 出 `crv`(claude-real-video 的 CLI),參數 `source`(URL 或本機路徑)、`out?`、`scene?`、`fps_floor?`、`max_frames?`、`lang?`、`transcribe?`;回結構化 `{out_dir, manifest_path, manifest, frames[], frame_count, transcript_path?}`。註冊在 server 與每個 daemon 的 registry(隨 Hello 廣播、`device_exec` 可分派),讓 agent 能在有影片的那台抽格,再用 `read_file`(transcript/manifest)與 `read_file_bytes`(JPG 影格)讀回。risk=Mutate(寫檔 + 網路下載)。
- **crv 依賴自動佈署**:仿 ddgs,以 pip/pipx 自動裝 `claude-real-video`(yt-dlp 為其相依,一併帶入);Python 已是 server 既有 default dep。Whisper 轉錄為 opt-in(`FLEETY_VIDEO_WHISPER=on` 才裝 `[whisper]`,避免預設拉入 torch 重相依);無 whisper 時工具預設不轉錄。
- **ffmpeg 自動安裝**:仿 Chrome 的 OS 套件管理器路線,偵測 ffmpeg/ffprobe,缺就用 winget(Windows)/brew(macOS)/apt(Linux)裝;best-effort、可 opt-out。
- **builtin skill**:vendor 一個 `fleety-real-video` builtin skill(MIT 附註),內容指向**原生 `video_extract` 工具**(非直接開 CLI,如 insyra HEADER 的慣例):說明場景感知取徑、何時轉錄、如何以 read_file_bytes 讀影格、跨裝置經 device_exec。
- **設定**:型別化 registry 新增 `FLEETY_VIDEO_WHISPER`(on/off)、`FLEETY_FFMPEG_AUTO_INSTALL`(on/off)、`FLEETY_CRV_AUTO_INSTALL`(on/off);路徑覆蓋 `FLEETY_CRV_BIN` / `FLEETY_FFMPEG_BIN` 依既有 _BIN 慣例讀原始 env。

## Non-Goals (optional)

- **不 vendor crv 的 Python 原始碼**:依賴其 pip 套件並自動佈署(同 ddgs 精神),只 vendor 其 skill;避免維護一份 fork。
- **不把 crv/ffmpeg 加進 daemon 預設 deps**(不在每台裝置開機都裝重相依):v0 由 server 端自動佈署;裝置端經 `FLEETY_DEPS` opt-in 或後續按需佈署,工具本身在缺依賴時回可讀錯誤。
- **不改既有 `--video` 多模態路徑**:`video_extract` 是新增的前處理能力,不取代直接附件。
- **不做 crv 的 `--why` / `--kb` / `--grid` / `--report` 進階旗標**(v0 聚焦核心抽格+轉錄;之後可加)。

## Capabilities

### New Capabilities

- `video-understanding`: 原生 video_extract 工具(場景感知抽格 + 選配轉錄,shell 出 crv)、其跨裝置註冊與輸出契約、crv/ffmpeg/whisper 的自動佈署與設定、以及引導用的 builtin skill。

### Modified Capabilities

(none)

## Impact

- Affected specs: `video-understanding`(new)
- Affected code:
  - New:
    - crates/fleety-tools/src/video.rs — video_extract 工具 + crv 定位/spawn/輸出解析
  - Modified:
    - crates/fleety-tools/src/lib.rs — 匯出 register_video
    - crates/fleety-server/src/tools.rs — build_registry 註冊 video_extract
    - crates/fleety-daemon/src/ondevice.rs — build_local_registry 註冊 video_extract
    - crates/fleety-tools/src/deps.rs — crv 加入 server default deps
    - crates/fleety-server/src/builtin_mcp.rs 或新模組 — crv 的 pip 安裝/升級 + Dependency 工廠(仿 ddgs)
    - crates/fleety-tools/src/chrome.rs 附近或新模組 — ffmpeg 的 OS-pkg 偵測+安裝(仿 try_package_manager_install)
    - crates/fleety-tools/src/config.rs — 新增 FLEETY_VIDEO_WHISPER / FLEETY_FFMPEG_AUTO_INSTALL / FLEETY_CRV_AUTO_INSTALL 及 setting_choices
    - crates/fleety-server/src/builtin_skills.rs — SKILLS 陣列加入 fleety-real-video
    - crates/fleety-server/builtin-skills/fleety-real-video/ — 新 skill 目錄(SKILL.md + MIT 附註)
    - docs/env.md、docs/tools.md — 新 var 與 video_extract 工具正典
  - Removed: (none)
