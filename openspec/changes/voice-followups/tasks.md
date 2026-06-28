<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同 crate、無相依）。 -->

## 1. 協定 attention 欄位

- [x] 1.1 [P] 在 crates/fleety-protocol/src/lib.rs 新增 AttentionHint { device, look_at, url: Option<String> } 與 ServerMsg::Assistant 的 attention: Option<AttentionHint>（serde default/skip_serializing_if、向後相容、不 bump）——交付 "Attention hints are carried backward-compatibly"（決策「device deixis：attention hint 的協定形狀與向後相容」）。驗證:serde round-trip + 「缺 attention 欄位的舊 JSON 仍反序列化、視為無 hint」測試;cargo test -p fleety-protocol 全綠。

## 2. agent-core attention 解析

- [x] 2.1 [P] 在 crates/agent-core/src/agent.rs 新增 host-free 的 AttentionHint { device, look_at, url }、TurnOutcome 加 attention: Option<AttentionHint>，voice on 的系統 prompt 追加 ⟦ATTENTION⟧ 哨符說明，回合結束在切出 display/speech 後再解析 attention 區塊（device=…; look=…; url=…）——交付 "The terminal turn may carry an attention hint"（決策「attention hint 的模型輸出格式與 core 解析（比照 speech 哨符）」）。驗證:cargo build -p agent-core 綠;cargo tree -p agent-core 無 fleety-*。
- [x] 2.2 agent-core 單元測試——交付驗收:voice on 且模型輸出含 ⟦SPEECH⟧+⟦ATTENTION⟧→display/speech/attention 三者正確切分;無 attention 哨符→attention=None;voice off→attention=None。驗證:cargo test -p agent-core 全綠（值取自 spec Example 表）。

## 3. conn 帶出 attention

- [x] 3.1 在 crates/fleety-server/src/conn.rs 把 TurnOutcome.attention 經 TurnReply 帶出，只有終止回合把 agent_core::AttentionHint 對映成 fleety_protocol::AttentionHint 填入 ServerMsg::Assistant.attention;voice off／反思回合不帶——交付 attention 的終止回合 emit（決策「與既有 voice/goal/skill-learning-loop 的互動邊界」）。驗證:單元測試斷言「voice on 終止回合 Assistant 帶 attention;voice off 為 None」;cargo test -p fleety-server conn:: 全綠。

## 4. fleety-cli STT 與 attention 呈現

- [x] 4.1 在 crates/fleety-cli（Cargo.toml 加 cpal 依賴）把 voice.rs 的 listen 改為用 cpal 擷取麥克風→寫 16 kHz mono WAV 暫存檔→以可設定轉寫命令（FLEETY_STT_CMD，預設 whisper.cpp;模型 FLEETY_STT_MODEL;錄音秒數 FLEETY_STT_SECONDS）轉文字;無裝置/權限/命令缺失/失敗→退回（Windows System.Speech;其餘打字）、永不 crash、暫存檔即刪——交付 "Terminal transcribes speech via a configurable engine" 與 "Speech-to-text degrades gracefully"（決策「跨平台 STT：終端錄音 + 可設定轉寫命令（預設 whisper.cpp）」「麥克風擷取：採 cpal 跨平台音訊擷取（使用者已拍板）」「STT 退回鏈與 never-crash」）。驗證:測試以 bogus 轉寫命令模擬引擎缺失時 listen 回 None 不 crash;cargo test -p fleety-cli voice:: 全綠（真實麥克風+whisper 轉寫為執行期/硬體相依，需手動驗證）。
- [x] 4.2 在 crates/fleety-cli/src/main.rs 的 voice 迴圈，收到 ServerMsg::Assistant.attention 時呈現（印出「看 <device> 的 <look_at>」，有 url 則提示/開啟）——交付 "The terminal surfaces the attention hint"。驗證:cargo build -p fleety-cli 綠;手動確認呈現。

## 5. 文件

- [x] 5.1 docs/env.md 增 STT env（FLEETY_STT_CMD/MODEL/RECORD_CMD 與錄音長度）、docs/tools.md 與 prompts/protocol.md 補 attention hint 用法與跨平台 STT 行為——交付:文件與實作一致。驗證:內容審查,env 列／行為描述與規格一致。

## 6. 整體驗收

- [x] 6.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*。
