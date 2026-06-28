## Why

voice-conversation 已上線（協定 voice/speech 欄位、core 雙通道、終端 OS 原生 TTS），但封存時誠實標注兩個缺口仍待補：(1) **mac/Linux 沒有真正的語音輸入**——headless OS STT 只在 Windows best-effort，mac/Linux 一律退回打字，所以「語音對話」目前只有「語音輸出」全平台可用；(2) **device deixis 只有行為描述、無實作**——`prompts/protocol.md` 教 agent 在口語通道把使用者注意力導向某裝置並附結構化 attention hint，但協定沒有承載該 hint 的欄位、終端也不會 surface/open。補這兩塊，語音對話才在三平台真正成立。

## What Changes

- **跨平台 STT（fleety-cli）**：終端錄麥克風到暫存 WAV，餵一個可設定的轉寫命令（預設指向本地 **whisper.cpp**）取回文字，送 `voice: true` 的 `UserMessage`。引擎/模型以 env 設定（`FLEETY_STT_CMD`、`FLEETY_STT_MODEL`）；命令或麥克風不可用時優雅退回現有行為（Windows System.Speech；其餘退回打字），永不 crash。選本地 whisper.cpp 是為離線、跨平台、不依賴雲、最貼合 fleety「終端做」哲學。
- **device deixis（attention hint）**：voice on 的終止回合，模型除了 display + speech 再可附一個結構化 attention hint（device_id + 看什麼 + 可選 url/path）；core 比照 speech 哨符的作法解析出來，`ServerMsg::Assistant` 以向後相容的 optional 欄位承載，終端收到後呈現/開啟對應目標。
- **文件**：docs/env.md 增 STT env；docs/tools.md／prompts 視需要補 attention hint 與 STT 行為。

## Non-Goals

（細節取捨見 design.md 的 Goals/Non-Goals。）

## Capabilities

### New Capabilities

- `cross-platform-stt`: 終端用本地 whisper.cpp（可設定命令）做跨平台語音輸入——錄音→轉寫→送 voice 訊息，缺引擎或失敗則優雅退回 OS STT／打字；server 仍只進出文字。
- `device-deixis`: voice on 終止回合可附結構化 attention hint（哪台裝置、看什麼、可選 url/path），core 解析、協定向後相容承載、終端 surface/open。

### Modified Capabilities

（無。voice-conversation 既有規格不改；本變更新增兩個獨立能力補其缺口。）

## Impact

- 受影響 specs：新增 cross-platform-stt、device-deixis。修改：無。
- 受影響程式：
  - 修改：crates/fleety-cli/src/voice.rs（whisper.cpp 錄音+轉寫的 listen 路徑與退回）、crates/fleety-cli/src/main.rs（voice 迴圈呈現 attention hint）、crates/fleety-protocol/src/lib.rs（ServerMsg::Assistant 加 optional attention 欄位）、crates/agent-core/src/agent.rs（voice on 時解析 attention，比照 speech；維持 host-free）、crates/fleety-server/src/conn.rs（把 attention 從 TurnOutcome 帶到 Assistant）、docs/env.md、docs/tools.md、prompts/protocol.md
  - 新增依賴：`cpal`（fleety-cli 的 Cargo.toml，跨平台麥克風擷取）
  - 移除：無
- 關鍵驗收：mac/Linux 在 cpal+whisper.cpp 可用時能語音輸入、不可用時優雅退回不 crash；attention hint 協定向後相容（舊端忽略）、voice off 不產 attention；agent-core 維持 host-free；workspace fmt + clippy -D + test 全綠。
- 已拍板：麥克風擷取採 **cpal**（新增 cpal 音訊相依，開箱即用、不要求使用者另裝錄音程式）；外部錄音命令為未採的替代方案。
