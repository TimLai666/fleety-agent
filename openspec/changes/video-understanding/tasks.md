## 1. video_extract 工具(crates/fleety-tools/src/video.rs)

- [x] 1.1 依 design「決策一:video_extract 工具契約(crates/fleety-tools/src/video.rs)」與「決策五:whisper opt-in」與 spec「Native scene-aware video extraction tool」:新增 video.rs —— 純函式 build_crv_args(params)->Vec<String>(source/out/scene/fps_floor/max_frames/lang/transcribe → crv 參數;transcribe=false 或 whisper 缺 → `--no-transcribe`;預設 out 命名);`locate_crv()`(env FLEETY_CRV_BIN→which→bare);`struct VideoExtract{root}` + `impl Tool`(spec 名 video_extract、risk=Mutate、參數 schema;call 解析 out 於 workspace(sensitive guard)、spawn crv(可設 FLEETY_VIDEO_TIMEOUT_SECS 逾時)、讀輸出目錄回 {out_dir,manifest_path,manifest,frames[],frame_count,transcript_path?,notes?});crv 缺回可讀錯誤不 panic。`register_video(registry,root)`。先寫測試(tdd):build_crv_args 參數矩陣、locate_crv 在 env override 下回該路徑、crv 不存在(假 FLEETY_CRV_BIN)回錯不 panic。驗證:cargo test -p fleety-tools video 全綠。

## 2. 跨裝置註冊

- [x] 2.1 依 design「決策二:跨裝置註冊」與 spec「The extraction tool is available on every device」:lib.rs 匯出 register_video;在 crates/fleety-server/src/tools.rs 的 build_registry 與 crates/fleety-daemon/src/ondevice.rs 的 build_local_registry 各註冊 video_extract(與 register_insyra 並列)。驗證:cargo build -p fleety-server -p fleety-daemon 乾淨、video_extract 出現在兩處 registry(可加測試或 device_show 廣播清單檢查)。

## 3. crv 依賴自動佈署(仿 ddgs)

- [x] 3.1 依 design「決策三:crv 自動佈署(仿 ddgs)」與 spec「Extraction dependencies are auto-provisioned on the server」:新增 crv 的 auto_install_enabled(FLEETY_CRV_AUTO_INSTALL)、async crv_runs(`crv --version`)、async try_install_crv(有序 pipx/pip --user/python -m pip;FLEETY_VIDEO_WHISPER=on 時 package 用 claude-real-video[whisper] 否則 claude-real-video)、crv_dependency()->deps::Dependency(Strategy::UserPackage);加入 deps::server_default_deps()。驗證:cargo test -p fleety-tools -p fleety-server 全綠、boot ensure_dependencies 會含 crv(程式碼審閱/測試)。

## 4. ffmpeg 自動安裝(仿 Chrome OS-pkg)

- [x] 4.1 依 design「決策四:ffmpeg 自動安裝(仿 Chrome OS-pkg)」與 spec「Extraction dependencies are auto-provisioned on the server」:新增 ensure_ffmpeg() —— find_ffmpeg(env FLEETY_FFMPEG_BIN→which ffmpeg/ffprobe)→ 缺且 FLEETY_FFMPEG_AUTO_INSTALL 啟用 → try_package_manager_install_ffmpeg()(winget Gyan.FFmpeg / brew ffmpeg / apt ffmpeg,首個成功者勝,best-effort)→ 再偵測;server 開機與 crv 同批 ensure(best-effort 不阻塞)。純函式:per-OS attempts 表可測。驗證:cargo test -p fleety-tools 全綠、attempts 表對三平台正確(單元測試)。

## 5. 設定與 builtin skill

- [x] 5.1 [P] 依 design「決策六:設定與 skill」:config.rs registry 新增 FLEETY_VIDEO_WHISPER / FLEETY_FFMPEG_AUTO_INSTALL / FLEETY_CRV_AUTO_INSTALL(scope Server、on/off、v_onoff)並同步 setting_choices()。驗證:cargo test -p fleety-tools 全綠、三者出現在 config list。
- [x] 5.2 [P] 依 design「決策六:設定與 skill」與 spec「A builtin skill guides video extraction」:新增 crates/fleety-server/builtin-skills/fleety-real-video/SKILL.md(YAML frontmatter name+description;MIT 授權附註指向上游;導向 video_extract 工具、轉錄取捨、read_file_bytes 讀影格、device_exec 跨裝置);在 builtin_skills.rs 的 SKILLS 陣列以 include_dir! 加入。驗證:cargo build -p fleety-server 乾淨、skill_validate 通過、seed 後該 skill 存在且指向工具(程式碼/內容審閱)。

## 6. 文件

- [x] 6.1 [P] 依 design 與 spec:docs/tools.md 新增 video_extract 工具正典條目(any device、風險、參數、輸出);docs/env.md 補 FLEETY_VIDEO_WHISPER / FLEETY_FFMPEG_AUTO_INSTALL / FLEETY_CRV_AUTO_INSTALL / FLEETY_CRV_BIN / FLEETY_FFMPEG_BIN / FLEETY_VIDEO_TIMEOUT_SECS。驗證:內容審閱與 spec 用語一致。

## 7. 整體驗證

- [x] 7.1 全 workspace 驗證:cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all -- --check 乾淨。驗證:三道指令輸出乾淨。
