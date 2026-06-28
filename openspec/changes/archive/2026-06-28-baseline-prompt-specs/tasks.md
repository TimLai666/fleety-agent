<!-- 不寫程式:每項是「規格 ↔ prompts/ 內容 / storage.system_prompt() 機制」一致性驗收。
     行為 = capability 規格與實際組裝/提示內容相符;
     驗證 = 對照 crates/fleety-server/src/storage.rs 與 4 份 prompt + spectra validate。 -->

## 1. 系統提示組裝機制驗收

- [x] 1.1 [P] system-prompt-assembly 規格 "Assemble the system prompt from embedded docs and core memory"、"Preserve the system prompt across compaction"、"Minimal mode drops the static docs" 與現況相符(build 時 include_str protocol→rules→memory→policy 以 --- 串接 + # Core Memory(ME/USER/TODO);置 index 0 由 compaction 保留、不每回合重送;FLEETY_SYSTEM_PROMPT=minimal 只留核心記憶)。驗證:對照 crates/fleety-server/src/storage.rs 的 system_prompt()/core_memory() 與 PROTOCOL_MD/RULES_MD/MEMORY_MD/POLICY_MD 常數。

## 2. 存取與安全準則驗收

- [x] 2.1 [P] agent-conduct-policy 規格 "Full access by default"、"Physical-presence actions require co-location"、"Intrusive UI control prefers the least-intrusive tier"、"Unattended scheduled runs are mandate-bounded"、"Mutating actions are audited and reversible" 與 prompts/policy.md 五節一致,且與實作行為相符(full_access 預設、co-location、API/MCP>browser>computer-use 排序且 screenshot 豁免、schedule mandate/allowed_tools、稽核+rollback)。驗證:對照 prompts/policy.md 與 device-registry-and-routing/scheduling/audit-history/computer-use 既有規格及實作。

## 3. 人格與知識經營驗收

- [x] 3.1 [P] agent-persona-and-curiosity 規格 "Curiosity-driven investigation"、"Wiki-keeping discipline"、"Self-editable core memory" 與 prompts/memory.md + DEFAULT_ME 一致(好奇追溯源頭、依 LLM-wiki 規則經營並持續整理、ME/USER/TODO 可自編輯自我模型)。驗證:對照 prompts/memory.md 與 storage.rs 的 DEFAULT_ME/DEFAULT_USER/DEFAULT_TODO 及 knowledge-wiki/agent-memory 既有規格。

## 4. 整體驗收

- [x] 4.1 全 3 份能力規格通過 Spectra 結構與 scenario 檢核。驗證:spectra validate baseline-prompt-specs 零錯誤。
- [x] 4.2 確認 3 能力的 normative 主張無一與現況 prompts/ 內容或 storage 組裝衝突;rules.md 的通用工作風格散文未逐字規格化(只經「被嵌入」治理),protocol.md 中已由工具面能力管轄的部分未重複規格。驗證:逐項對照 proposal Non-Goals 與既有 baseline-tool-surface-specs 能力。
