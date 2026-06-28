<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同 crate、無相依）。 -->

## 1. 協定欄位（fleety-protocol）

- [x] [P] 1.1 在 crates/fleety-protocol/src/lib.rs 為 ClientMsg::UserMessage 加 voice: bool、ServerMsg::Assistant 加 speech: Option<String>，皆用 serde default / skip_serializing_if——交付 "Per-message voice flag" 與 "Backward-compatible protocol"（決策「協定欄位：UserMessage.voice 與 Assistant.speech，向後相容不 bump」）。驗證:新增 serde round-trip 測試 + 「缺 voice/speech 欄位的舊 JSON 仍可反序列化、voice 視為 false、speech 視為 none」的向後相容測試;PROTOCOL_VERSION 維持 0。

## 2. agent-core 雙通道輸出

- [x] [P] 2.1 在 crates/agent-core 為 LoopConfig 加 voice: bool、TurnOutcome 加 speech: Option<String>，run_turn/run_turn_streaming 於 voice on 時在系統 prompt 追加雙通道哨符說明、回合結束以哨符切分模型最終訊息為 (display, speech)——交付 "Dual-channel output on the terminal turn"（決策「雙通道輸出契約：模型在終止回覆附口語分隔區塊，core 解析切分」與「voice 旗標經 LoopConfig 傳入 core 並驅動系統 prompt」）。驗證:cargo build -p agent-core 綠;agent-core 仍不依賴任何 fleety crate（cargo tree -p agent-core 無 fleety-*）。
- [x] 2.2 agent-core 單元測試（MockProvider）——交付驗收:voice=false 時 TurnOutcome.speech=None 且系統 prompt 不含口語段;voice=true 且模型輸出含哨符→display 去除哨符及其後、speech=哨符後文字;模型未給哨符→speech=None、display 完整。驗證:cargo test -p agent-core 全綠（切分案例值取自 spec 的 Example 表）。

## 3. server 整合（fleety-server）

- [x] 3.1 在 crates/fleety-server/src/conn.rs 把 UserMessage.voice 經 drive_to_goal 傳入每回合的 drive_turn 並設 LoopConfig.voice;只有終止回合把 TurnOutcome.speech 填進 ServerMsg::Assistant.speech，中間續做回合不帶 speech;同步更新 crates/fleety-server/src/subagent.rs 的 drive_turn 呼叫點——交付 "Only the terminal turn speaks"（決策「server 僅在終止回合 emit speech」）。驗證:單元測試以 out 接收端斷言「voice on 多回合迴圈只有終止回合的 Assistant 帶 speech」與「voice off 回合 Assistant.speech 為 None」。

## 4. 終端 STT/TTS（fleety-cli）

- [x] 4.1 在 crates/fleety-cli 新增 voice 模組（crates/fleety-cli/src/voice.rs）以 OS 原生引擎實作 speak（TTS:macOS say／Windows SAPI／Linux spd-say）與 listen（STT;Linux 無原生則回報不支援、請打字），並接進 CLI 迴圈（聽到語音送 voice:true 的 UserMessage、收到 speech 即朗讀）;引擎缺失或失敗優雅退回純文字、永不 crash——交付 "Terminal OS-native speech with graceful fallback"（決策「終端 OS 原生 STT/TTS 與優雅退回」）。驗證:測試/模擬引擎缺失時退回純文字不 crash;手動於各平台確認朗讀與退回行為。

## 5. 文件與提示同步

- [x] 5.1 更新 prompts/protocol.md 的 Output Channels 段為實作後狀態（voice 旗標、speech 欄位、終止回合產出、fleety voice 子命令）、修正 docs/spec-v0.md「speech 欄位與 voice 旗標 M2 已先留」為「於本變更實作」並更新 M7 狀態——交付:文件與實作一致（決策「文件與既有契約同步」）。註:voice 不新增 server env（FLEETY_VOICE_INTERMEDIATE 經實作確認做不到而捨棄）、也不新增 agent 工具，故 docs/env.md、docs/tools.md 無需變更。驗證:內容審查,行為描述與規格一致。

## 6. 整體驗收

- [x] 6.1 全工作區綠且核心 host-free——交付向後相容與 host-free 關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*。
