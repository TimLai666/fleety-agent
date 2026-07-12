## 1. 難度分類器與 set_effort auto(crates/fleety-server/src/effort.rs)

- [x] 1.1 依 design「決策二:難度自動選 effort(turn 起始前的分類器)」與 spec「Effort is auto-selected by task difficulty」:新增純函式 parse_effort(text)->Option<Effort>(low/medium/high 映射;雜訊、空字串→None)與 assess_effort(new_msg, provider)->Option<Effort>(仿 triage,一次便宜模型呼叫;呼叫失敗或不可解析→None,不阻斷)。先寫測試(tdd):parse_effort 映射表含 None cases。驗證:cargo test -p fleety-server parse_effort 全綠。
- [x] 1.2 依 design「決策三:優先序,且自動結果不寫入 slot」與 spec「The main agent sets its own effort dynamically」:set_effort 工具 level enum 增加 auto —— 傳 auto 時把 session_effort 設 None(清除釘選),low/medium/high 維持寫入 Some;工具描述改為明講「不影響當前這一步、從下一個 turn/連續 turn 起生效並持久、傳 auto 交還自動」。測試:set_effort auto 後 slot 為 None、high 後為 Some(High)。驗證:cargo test -p fleety-server 全綠。

## 2. 重讀時序與 turn 起始套用(crates/fleety-server/src/conn.rs)

- [x] 2.1 依 design「決策一:重讀粒度 = goal 連續 turn(drive_turn 邊界)」與 spec「mid-request change applies to the next continuation turn」:drive_to_goal 簽章新增 session_effort: &SessionEffort 與 turn_baseline: Option<Effort>,provider 語意改為基礎 provider;在 goal 連續迴圈每次 drive_turn 之前計算 manual_pin.or(turn_baseline) 並以 provider.with_effort(...) 重選該連續 turn 的 provider。先寫行為測試:以記錄每次 drive_turn 收到 effort 的測試替身 provider,驗證連續 turn 間會 pick up slot 新值(釘選壓過 baseline、auto 回 baseline)。驗證:cargo test -p fleety-server 該測試全綠。
- [x] 2.2 依 design「決策二/三」:conn.rs turn 起始(現行讀 session_effort 處)在 FLEETY_AUTO_EFFORT on 且無手動釘選且訊息非空白時呼叫 assess_effort(優先 cheap tier provider,無則主 provider)得 turn_baseline,連同 session_effort 傳入 drive_to_goal;兩條 drive_to_goal 呼叫點(可中斷路徑與 require-approval 路徑)一併更新。驗證:cargo build -p fleety-server 乾淨、既有 effort/turn 相關測試不回歸。

## 3. 設定(crates/fleety-tools/src/config.rs)

- [x] 3.1 [P] 依 design「決策四:設定 FLEETY_AUTO_EFFORT」與 spec:型別化 registry 新增 FLEETY_AUTO_EFFORT(scope Shared、預設 on、布林驗證器),off 時 turn_baseline 恆為 None、跳過分類。驗證:cargo test -p fleety-tools 全綠、FLEETY_AUTO_EFFORT 出現在 fleety config list。

## 4. 文件與提示

- [x] 4.1 [P] 依 design 與 spec:docs/env.md 補 FLEETY_AUTO_EFFORT(用途、預設 on、每 top-level turn 一次便宜分類的成本取捨);docs/tools.md 修正 set_effort 描述,講清「不影響當前步、下一 turn/連續 turn 起生效並持久、auto 交還自動、含 auto 檔位」。驗證:內容審閱,與 per-task-effort spec 用語一致。
- [x] 4.2 [P] 依 design:prompts/protocol.md 補 effort 使用指引(依難度先設 effort、runtime 也會依難度自動選、set_effort auto 交還自動、跨 turn 持久),prompts/rules.md 補一句 effort 紀律。驗證:內容審閱,與 set_effort 工具描述一致、不與既有規則衝突。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證:cargo test --workspace、cargo clippy --workspace --all-targets -- -D warnings、cargo fmt --all -- --check 全乾淨。驗證:三道指令輸出乾淨。
