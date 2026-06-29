## 1. 能力型別與來源(純函式)

- [x] 1.1 在 crates/agent-core/src/model.rs 定義 `ModelCapabilities`(image/audio/pdf 旗標,text 恆真)+ `ModelProvider::capabilities(&self)`(有預設實作回全支援以維持相容),並實作純函式 `parse_modalities`、`mime_modality`、`supports`,交付 "Providers report their modality capabilities";對應設計「ModelProvider 暴露模態能力查詢」與「能力資訊隨 provider 攜帶,不改 trait 既有方法簽章」。先寫失敗測試:`parse_modalities` 對 "text,image" / "" / "text,bogus"(用 spec example 表),`mime_modality` 對 image/* / audio/* / application/pdf / 其他,`supports` 判定。

## 2. 送出前依能力路由或降級

- [x] 2.1 [P] 在 crates/agent-core/src/openai.rs 讓 provider 帶 `capabilities` 欄位,並改 wire_content:支援的模態照路由、不支援的模態改插入文字註記(取代只對未知 MIME 降級),交付 "Unsupported attachments degrade gracefully instead of failing the turn"(OpenAI 路徑);對應設計「送附件前依能力路由或優雅降級」。先寫失敗測試:能力僅 text 時圖片附件 → 產出不含 image_url、改為文字註記;能力含 image 時仍產生 image_url(既有測試)。
- [x] 2.2 [P] 在 crates/agent-core/src/gemini.rs 讓 provider 帶 `capabilities` 欄位,並改 build_parts 同樣依能力路由/降級,交付 "Unsupported attachments degrade gracefully instead of failing the turn"(Gemini 路徑);對應設計「送附件前依能力路由或優雅降級」。先寫失敗測試:能力僅 text 時音訊/圖片附件 → 不含 inline_data 影像、改文字註記;支援時仍 inline_data(既有測試)。

## 3. 注入能力 + 設定 + 文件

- [x] 3.1 在 crates/fleety-server/src/providers.rs 的 build_provider 為 main/cheap 各注入能力:先讀 `FLEETY_MODEL_MODALITIES` / `FLEETY_CHEAP_MODEL_MODALITIES`,未設定則用既有 looks_multimodal 推導,交付 "Providers report their modality capabilities" 的來源面(設定優先、啟發式預設);對應設計「能力來源:設定優先,啟發式為預設」。驗證:設定指定 text 時該 tier 能力不含 image;未設定且名稱為多模態家族時為多模態(單元或整合測試)。
- [x] 3.2 把 `FLEETY_MODEL_MODALITIES`、`FLEETY_CHEAP_MODEL_MODALITIES` 登記到 typed config registry(crates/fleety-tools/src/config.rs)並更新 docs/env.md。驗證:`config list` 顯示這兩鍵;docs/env.md 說明逗號分隔語意與未設定時的啟發式預設。
