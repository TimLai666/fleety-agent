## Context

agent-core 的兩個 provider(OpenAI 相容、Gemini)在組請求時,wire_content / build_parts 會依 MIME 把附件路由成 image_url / input_audio / file / inline_data,只有未知 MIME 才降級成文字註記。執行期沒有任何「這個模型支不支援這種模態」的判斷;唯一的能力資訊是 fleety-server providers.rs 的 looks_multimodal(硬編字串清單),且只在開機發警告。ProviderTiers 有 main/cheap 兩層但 wire 格式相同。約束:agent-core 不得依賴任何 fleety crate;forbid unsafe;never-crash;env 測試可單執行緒。

## Goals / Non-Goals

**Goals:**

- 每個 provider 能回報自己支援哪些模態(text/image/audio/pdf)。
- 送附件前依能力決定:支援→照路由;不支援→丟附件 + 插入文字註記,讓 turn 仍能完成。
- 能力可由設定覆寫,未設定時用啟發式(looks_multimodal 家族)推導合理預設。
- 不改 wire 訊息形狀、不改 ModelProvider 既有方法語意。

**Non-Goals:**

- 不做「不支援時自動升級到有能力的 tier」的路由(列 Open Question,屬後續 / fleety-server 層)。
- 不引入新依賴、不改 wire 協定。
- 不新增模態種類的實際解碼(例如把圖轉文字描述);降級就是丟附件 + 註記。

## Decisions

### ModelProvider 暴露模態能力查詢

在 model.rs 定義 `ModelCapabilities`(一組布林/集合:text 恆真,image/audio/pdf 可有可無)並在 `ModelProvider` trait 加一個有預設實作的查詢方法 `capabilities(&self) -> ModelCapabilities`(預設回「全部支援」以保持既有行為相容,具體 provider 覆寫)。理由:加預設方法不破壞既有實作者(EchoProvider/MockProvider 等),且讓呼叫端能在送出前查詢。

### 能力來源:設定優先,啟發式為預設

provider 建構時帶入一個 `ModelCapabilities`:fleety-server 的 build_provider 先讀 `FLEETY_MODEL_MODALITIES` / `FLEETY_CHEAP_MODEL_MODALITIES`(逗號分隔,如 `text,image,audio`);未設定時呼叫既有 looks_multimodal 推導(多模態家族→text+image+audio+pdf;否則→text only)。解析設定字串做成純函式 `parse_modalities(&str) -> ModelCapabilities` 以利測試。理由:作業者可精確指定;沒指定時沿用既有啟發式當合理預設。

### 送附件前依能力路由或優雅降級

把 wire_content / build_parts 的附件處理改成:對每個附件,先依其 MIME 判斷模態,若 provider 能力不含該模態 → 不路由成媒體 part,改成插入一句文字註記(例如「[附了一個 <mime>,但此模型無法讀取]」);支援才照現有路由。未知 MIME 維持降級成文字。判斷「mime → 模態」與「是否支援」抽成純函式以利測試。理由:把「失敗點」從遠端端點(整個 turn 失敗)前移到本地、且以可讀註記讓模型知道有附件存在。

### 能力資訊隨 provider 攜帶,不改 trait 既有方法簽章

provider 結構體新增一個 `capabilities` 欄位;trait 的 `complete`/`complete_streaming` 簽章不變,只新增 `capabilities()` 查詢。wire_content / build_parts 改成讀 provider 自身的能力。理由:最小侵入,既有呼叫端與測試不需改動。

## Implementation Contract

**行為(Behavior):**

- provider 回報能力:多模態模型 → text+image+audio+pdf(或設定指定的子集);純文字模型 → 僅 text。
- 送含圖片附件給「不支援 image」的 provider:不送出 image part,改插入文字註記,turn 正常完成(不再因端點拒絕而失敗)。
- 送含圖片給「支援 image」的 provider:行為與現況相同(照路由)。
- 未知 MIME:維持降級成文字註記。
- `FLEETY_MODEL_MODALITIES` 指定時以其為準;未指定時由 looks_multimodal 推導。

**介面 / 資料形狀:**

- model.rs:`struct ModelCapabilities { image: bool, audio: bool, pdf: bool }`(text 恆支援),含建構輔助;`ModelProvider::capabilities(&self) -> ModelCapabilities`(預設回全支援)。
- 純函式:`parse_modalities(s: &str) -> ModelCapabilities`;`mime_modality(mime: &str) -> Modality`(image/audio/pdf/other);`supports(caps, modality) -> bool`。
- openai.rs / gemini.rs:provider 結構體加 `capabilities` 欄位;wire_content/build_parts 依之路由或降級。
- fleety-server providers.rs:build_provider 解析設定或啟發式,注入能力。
- fleety-tools config registry:`FLEETY_MODEL_MODALITIES`、`FLEETY_CHEAP_MODEL_MODALITIES`。

**失敗模式:**

- 設定字串無法解析的詞 → 忽略該詞(不 panic),其餘照常;空字串 → 退回啟發式預設。
- 降級永不失敗(只是少送一個 part + 多一行文字)。

**驗收標準(Acceptance):**

- 單元測試:`parse_modalities` 對 "text,image"、""、含未知詞 的解析;`mime_modality` 對 image/*、audio/*、application/pdf、其他的判定;`supports` 判定。
- 單元測試:wire_content/build_parts 在「不支援 image」能力下,圖片附件 → 文字註記而非 image part(可檢查產出的 JSON 不含 image_url/inline_data 影像);在「支援」下 → 仍產生媒體 part(既有測試)。
- 既有 provider 測試全綠(預設能力=全支援,行為相容)。
- clippy -D 乾淨、agent-core host-free、env 測試單執行緒可跑。

**範圍邊界:**

- In scope:能力型別 + 查詢、設定/啟發式來源、送出前依能力路由或降級、設定鍵、文件。
- Out of scope:tier 自動升級路由、wire 協定變更、trait 既有方法簽章變更、把附件轉描述。

## Risks / Trade-offs

- [啟發式名單過時,誤判某模型不支援圖] → 設定可明確覆寫;名單沿用既有 looks_multimodal(已維護)。
- [降級讓模型「看不到」使用者的圖] → 以明確文字註記告知模型有附件存在,避免無聲遺漏;作業者可改用多模態模型或設定能力。
- [預設能力=全支援,可能仍盲送] → 由 fleety-server 注入真實能力;只有未注入能力的測試樁才用「全支援」預設(行為相容)。

## Migration Plan

- 純加層:trait 新增有預設的查詢方法,既有實作不需改。fleety-server 注入能力後才真正 gating。
- 不設任何模態設定時,行為由啟發式決定;要完全沿用舊行為可把能力設為全支援。
- 無資料遷移。

## Open Questions

- 不支援某模態時,是否自動升級到有能力的 tier(而非降級)?本變更只做降級,升級路由留待後續(可能與 effort / tier 路由變更一起)。
- 是否需要更細的能力(例如 image 但不支援高解析、audio 僅特定格式)?初版只做粗粒度模態旗標。
