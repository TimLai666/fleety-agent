## Why

prompts/(protocol/rules/memory/policy)是 Fleety 的行為契約,在 build 時經 include_str 組進系統提示(storage.system_prompt())。目前沒有規格描述「系統提示如何組裝」與其中「可驗證的安全準則與人格行為」,只有散文。把**組裝機制**與**關鍵 normative 行為**納入 Spectra,讓提示的結構與安全/人格契約有單一真相來源。

## What Changes

- 為系統提示的**組裝機制**與**可驗證的行為準則/人格**建立能力規格,以 storage.system_prompt() 與 4 份 prompt 的實際內容為準。
- 3 個 capability:`system-prompt-assembly`、`agent-conduct-policy`、`agent-persona-and-curiosity`。
- **不改任何程式、不改任何提示文字。**

## Non-Goals

- **不把 rules.md 的通用工作風格散文逐字轉成 SHALL**(理解需求/批判性判斷/微觀宏觀/回報格式等)。這些續存於 rules.md,經「被嵌入系統提示」治理,由 `system-prompt-assembly` 規格涵蓋其「被嵌入」這件事,而非逐條規格化。
- **不重複規格已由工具面能力管轄的 protocol.md 內容**:裝置 scoping、cross-device 分派、skills、scheduling、外部 MCP 已分別由 device-registry-and-routing、skills-management、scheduling、mcp-servers 規格管轄。
- `FLEETY_SYSTEM_PROMPT` 的開關行為在此規格(屬組裝機制),不在 baseline-config-specs 重複。
- 不改任何提示文字、組裝順序或預設人格。

## Capabilities

### New Capabilities

- `system-prompt-assembly`: 系統提示組裝機制 —— build 時嵌入 protocol→rules→memory→policy + 核心記憶(ME/USER/TODO),置於 message index 0、由 compaction 保留(不每回合重送),`FLEETY_SYSTEM_PROMPT=minimal` 只留核心記憶。
- `agent-conduct-policy`: 存取與安全準則(policy.md 五節)—— 預設 full access、需實體在場的動作要 co-location、侵入式 UI 控制取最小侵入層、無人值守排程受 mandate 約束、變動動作皆稽核且可 rollback。
- `agent-persona-and-curiosity`: 人格與知識經營(memory.md + DEFAULT_ME)—— 對世界好奇、遇異常/驚喜/可深掘處主動追溯源頭、依 LLM-wiki 規則經營並持續整理 wiki、核心記憶為可自編輯的自我模型。

### Modified Capabilities

(none)

## Impact

- Affected specs: 3 new capability specs under openspec/specs/.
- Affected code:
  - New: none (specs/documentation only)
  - Modified: none
  - Removed: none
- Source of truth: prompts/protocol.md, prompts/rules.md, prompts/memory.md, prompts/policy.md, and the assembly in crates/fleety-server/src/storage.rs (system_prompt / core_memory).
