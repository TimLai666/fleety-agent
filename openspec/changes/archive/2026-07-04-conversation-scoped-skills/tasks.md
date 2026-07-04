## 1. 純函式決定 project 與 user skill 來源目錄

- [x] 1.1 測試先行:加 `skill_sources_layers_and_dedupes`,斷言給定 origin_cwd 與 user_home,`skill_sources` 回傳「origin cwd 逐層往上每層的 `.claude/skills` 與 `.agents/skills`,加 user 全域 `~/.claude/skills`、`~/.agents/skills`」的有序(深→淺)去重來源清單,先紅。驗證:`cargo test -p fleety-server skill_sources_layers_and_dedupes` 先紅。
- [x] 1.2 在新模組實作 `skill_sources` 純函式產生上述來源目錄清單——落實 design「純函式決定 project 與 user skill 來源目錄」。驗證:1.1 測試轉綠。

## 2. skill 來源與回傳一路攜帶所在裝置

- [x] 2.1 測試先行:加 `collect_carries_device`(對話級來源的 skill 帶 `device`,同主機為 `None`)與 `list_and_use_report_device`(`list_skills` 每條目與 `use_skill` 回傳含 `device` 欄位),先紅。驗證:`cargo test -p fleety-server collect_carries_device list_and_use_report_device` 先紅。
- [x] 2.2 讓 skill 來源與 `SkillInfo` 攜帶 `device: Option<String>`,並在 `collect` / `list_skills` / `use_skill` 及 skill file 工具的 JSON 回傳新增 `device` 欄位(`null` = server),既有 `name`/`source`/`path`/`content` 不變——落實「Requirement: Three-tier skill store」的 device 契約與 design「skill 來源與回傳一路攜帶所在裝置」。驗證:2.1 測試轉綠。

## 3. 對話級 tier 疊加與 precedence

- [x] 3.1 測試先行:加 `conversation_scoped_skill_is_isolated`(兩個綁定不同對話級來源的 registry,各自只見自身對話級 skill、且不進全域)與 `conversation_tier_overrides_global`(同名 skill 在對話級與 installed 都有時對話級勝出),先紅。驗證:`cargo test -p fleety-server conversation_scoped_skill_is_isolated conversation_tier_overrides_global` 先紅。
- [x] 3.2 讓 skill 註冊 / `collect` 接受對話級 project + user 來源疊加,合併順序為 project > user > installed > authored > builtin > synced——落實「Requirement: Conversation-scoped project and user skill tiers」、design「對話級 tier 以 per-connection registry 疊加來源目錄」與「precedence:對話級疊在既有四層之上」。驗證:3.1 測試轉綠。

## 4. 首版同主機綁定對話級來源,跨裝置退回

- [x] 4.1 讓 `build_connection_stack` 依對話 origin 綁定對話級 skill 來源:同主機(device `None`)時用 `skill_sources` 蒐集並疊加;origin 在別台或無 origin 時對話級來源為空、退回既有四層——落實 design「首版同主機,跨裝置 skill 來源與 scripts 執行列後續」與 spec scenario「cross-device or absent origin falls back to global tiers」。驗證:`cargo test -p fleety-server cross_device_or_absent_falls_back_to_global`(跨裝置 / 無 origin 時 `list_skills` 只含全域四層)。

## 5. 全量驗證

- [x] 5.1 跑全 workspace 測試與 lint,確認無回歸且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
