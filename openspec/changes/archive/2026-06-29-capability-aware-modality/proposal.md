## Why

附件(圖片/音訊/PDF)目前一律照送給模型,不檢查該模型支不支援該模態。`openai.rs` 的 wire_content 與 `gemini.rs` 的 build_parts 會把 image→image_url、audio→input_audio、pdf→file 照路由,只有「未知 MIME」才降級成文字。能力判斷只有開機時 providers.rs 的 looks_multimodal(硬編字串)發一個警告,執行期不 gating。結果:純文字模型收到圖片 → 端點拒絕 → 整個 turn 失敗。tier 系統(main/cheap)已存在但兩層用相同 wire 格式、無能力差異。這也是「每任務 effort」與「音訊直送模型 STT」兩個後續變更的能力前提。

## What Changes

- ModelProvider 新增**自我能力查詢**(回傳支援的模態集合:text/image/audio/pdf 等),不改既有方法語意。
- 能力來源:優先讀設定(`FLEETY_MODEL_MODALITIES` / `FLEETY_CHEAP_MODEL_MODALITIES`,逗號分隔如 `text,image`);未設定時由 looks_multimodal 等啟發式推導為預設。
- 送附件前,若該 provider 不支援該附件模態 → **優雅降級**:丟掉該附件並插入一句文字註記(例如「[附了一張圖片,但目前模型無法讀取]」),讓 turn 仍能進行,而非讓端點報錯使整個 turn 失敗。
- 取代目前「只對未知 MIME 降級」的邏輯,改為「依該 provider 的能力決定路由或降級」。

## Non-Goals

(本變更會建立 design.md,Non-Goals 寫在 design 的 Goals/Non-Goals 一節。)

## Capabilities

### New Capabilities

- `capability-aware-modality`: provider 自我模態能力查詢(設定優先、啟發式為預設),以及送附件前依能力做路由或優雅降級(不支援的模態 → 丟附件 + 文字註記),讓不支援的模型不會因附件而整個 turn 失敗。

### Modified Capabilities

(none)

## Impact

- Affected specs: capability-aware-modality(新)
- Affected code:
  - Modified:
    - crates/agent-core/src/model.rs(ModelProvider 加 capabilities 查詢 + ModelCapabilities 型別)
    - crates/agent-core/src/openai.rs(wire_content 依能力路由/降級;provider 帶能力)
    - crates/agent-core/src/gemini.rs(build_parts 依能力路由/降級;provider 帶能力)
    - crates/fleety-server/src/providers.rs(建構 provider 時帶入設定/啟發式能力)
    - crates/fleety-tools/src/config.rs(新增模態設定鍵)
    - docs/env.md(記錄模態設定)
