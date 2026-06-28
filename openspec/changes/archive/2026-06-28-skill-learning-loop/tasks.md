<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔、無相依）。 -->

## 1. conn 反思迴圈

- [x] 1.1 在 crates/fleety-server/src/conn.rs：TurnReply 加 steps: usize（drive_turn 從 TurnOutcome.steps 帶出）、drive_to_goal 改回傳 Result<usize>（該訊息各回合步數累加）、新增 maybe_reflect（min_steps>0 且 steps>=min_steps 時跑「一次」反思 drive_turn:emit_terminal=true、voice=false、不遞迴）與 skill_reflect_min_steps_from_env（預設 5、0 關閉）、UserMessage arm 在 drive_to_goal 後呼叫 maybe_reflect，反思 seed 指示依落點判準存 skill／記 memory-or-wiki——交付 "Reflection fires after a complex task" 與 "Reflection is bounded and configurable"（決策「反思 nudge：goal 終止後依步數門檻跑一次有界反思回合」「複雜度啟發式：累加各回合的工具步數」「與 goal / voice 的互動與邊界」）。驗證:cargo build -p fleety-server 綠。
- [x] 1.2 conn 單元測試——交付驗收:步數達門檻→反思回合多跑一次（以 out 接收端／MockProvider 腳本消耗斷言）;步數未達或 min_steps=0→不跑反思;反思只跑一次不遞迴（腳本剛好夠一次,多跑會耗盡 provider 而失敗）。驗證:cargo test -p fleety-server conn:: 全綠。

## 2. use_skill 回傳 path

- [x] 2.1 [P] 在 crates/fleety-server/src/skills.rs 讓 use_skill 回傳 JSON 新增 path（skill 目錄絕對路徑，與 list_skills 一致），使 skill 內 scripts/ 自寫工具可由 agent 取該 path 後以 run_command 執行——交付 "Three-tier skill store"（修改）與 "Skill-held tools run via the command tool"（決策「use_skill 回傳加上 skill 絕對目錄 path」）。驗證:skills 單元測試斷言 use_skill 回傳含 path 且指向該 skill 目錄、既有欄位不變（向後相容）;cargo test -p fleety-server skills:: 全綠。

## 3. prompts 學習迴圈

- [x] 3.1 [P] 更新 prompts/protocol.md（Skills 段或新「學習迴圈」段）與 prompts/rules.md：教 agent 學習迴圈與記憶落點判準（可重用流程→authored skill、必要時 scripts/ 自寫工具於 SKILL.md 提到並以 run_command＋use_skill 的 path 執行;durable 使用者/專案事實→memory 或 wiki;只對當前對話有意義→不記;不重複 code/git 已知）——交付 "Reflection captures procedures, facts, or nothing"（決策「記憶落點判準（skill / memory / wiki / 不記）」「skill 自寫工具以 run_command 約定執行」）。驗證:內容審查,行為與落點判準與規格一致。

## 4. docs

- [x] 4.1 [P] docs/env.md 增 FLEETY_SKILL_REFLECT_MIN_STEPS（預設 5、0 關閉）、docs/tools.md 註明 use_skill 回傳含 path 且 skill 內 scripts/ 自寫工具以 run_command 執行的約定——交付:文件與實作一致（決策「use_skill 回傳加上 skill 絕對目錄 path」「skill 自寫工具以 run_command 約定執行」）。驗證:內容審查,env 列／工具回傳描述與規格一致。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心未受影響——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*（本變更不應動到 agent-core）。
