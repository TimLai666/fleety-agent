# Fleety — 開發路線與實作計畫

對照 [`STATUS.md`](STATUS.md)(已完成的能力清單)與 [`spec-v0.md`](spec-v0.md)(v0 規格)。本檔聚焦在「**還沒做、但該做**」的部分。已出貨項目的完整實作計畫保留在 git 歷史與 `openspec/changes/archive/`,不再佔用本檔。

> 撰寫原則:不為了湊清單而塞 nice-to-have;每個 must-have 都附拒絕條件(在什麼情況下這個方案應該改做別的)。

---

## 已出貨

### 舊版 roadmap §1-§5(自更早盤點)
- **Eval / regression harness** — `fleety-eval` crate + goldens,CI 必過。
- **Enrollment 端對端** — pairing code / token / `fleety pair`,整合測試齊。
- **Audit + rollback CLI** — `fleety audit list|show`、`fleety rollback list|apply`。
- **多模態輸入** — `Attachment` 進 `agent-core`,CLI `--image/--audio/--video/--file`,TUI Ctrl+V,OpenAI/Gemini provider 適配。
- **Sidecar / 自動更新檢測** — fleetyd 每日輪詢 `FLEETY_UPDATE_MANIFEST`;`fleety status` 顯示 sidecar 健康。
- **語音對話** — `fleety voice`(cpal 錄音 + whisper.cpp / server 端 STT),TTS 回覆。

### 2026-07 產品體驗稽核 backlog(72 項 confirmed → 全數修復或出貨)
高頻痛點於 2026-07-05 同日修復;結構性缺口拆成 11 個 Spectra 變更,於 2026-07-10 全部實作、測試、archive(見 `openspec/changes/archive/2026-07-10-*`):
- **turn-cancellation** — server 端 per-tool-call 取消 checkpoint + `CancelTurn` frame;TUI Esc 取消、ACP `session/cancel` 以 `stopReason=cancelled` 收尾。
- **restart-defer-until-idle**(舊 §1)— in-flight turn 計數 + marker 通道 + idle watcher;手動/update restart 走 defer,`--force` 即時。
- **schedule-run-notification**(舊 §2)— run outcome 記錄 + per-schedule 失敗隔離 + 連線時 owner-scoped 主動投遞。
- **tui-depth**(舊 §3)— markdown/程式碼區塊渲染、等待 spinner、多行輸入(Alt+Enter/Ctrl+J)、`/attach`、斷線指數退避重連、離開前確認。
- **config-value-validation**(舊 §5)— Setting validator(enum/bool/uint/URL),set 與互動編輯共用,壞值寫入前擋下。
- **provider-editor-usability** — 就地編輯 provider、group remove / role unset、逐欄輸入、刪除確認。
- **conversation-discovery** — `fleety conversations` 列表(id + 相對時間 + preview),owner-scoped。
- **voice-vad-barge-in** — 能量門檻 VAD 端點偵測取代固定秒數、TTS 播放中 barge-in。
- **grant-access-revoke** — `revoke_access` / `list_access` 工具,授權可收回。
- **deploy-hardening** — Windows 生命週期動詞 elevation 前置檢查、server/daemon sidecar 佈建對稱、Dockerfile 非 root(uid 10001)。
- **cli-clipboard-acp-polish** — 可讀配對錯誤、OAuth port fail-fast、install.sh 權限判斷、clipboard 大小上限 + 語言別 mime、ACP `session/load` 合規回應。

### CLI 設定架構重設計 Phase 1(2026-07-10 出貨,見 `docs/design-cli-config.md`)

三層徹底分離(連線 / 本機 CLI / server)的 Phase 1(全 additive、不動 wire),四個變更全部實作、測試、archive(`openspec/changes/archive/2026-07-10-{connection-profiles,provider-model-two-tier,auth-default-on,local-config-scope}`):
- **connection-profiles** — `~/.fleety/connections.toml` + CLI/daemon 共用 resolver(單一優先序、mDNS sticky + fingerprint guard)+ `fleety server` 命令群 + `init`/`pair` sugar + config.json/fleetyd.token 一次性冪等遷移(O_EXCL 閂);`FLEETY_AGENT_URL` 移出 registry,消除三處優先序陷阱。
- **provider-model-two-tier** — providers.toml 改兩層:type-tagged provider(api / oauth:codex,可擴展註冊)+ main/cheap member pool(stream/modalities/effort 下沉 member);混族 pool 能力取聯集;參照完整性寫前 validate;providers.toml 去重遷移;`FLEETY_MODEL_*` 降為 bootstrap seed、壞結構化設定硬啟動錯誤。
- **auth-default-on** — `FLEETY_REQUIRE_AUTH` 預設開(顯式 `0` 才關)+ 首啟配對引導 + 遠端寫入⇒認證必開(auth 關閉時拒 mutating config frame)。
- **local-config-scope** — `fleety config --target local` 只顯示/編輯 Cli/Shared;server/daemon key 導向正確主機。

**Phase 2(remote-config-panel,2026-07-10 出貨)** — 動 wire、交付 G2(一個面板設定任何東西):`ConfigSnapshot`/`ConfigApply` frame + revision 樂觀鎖 + 真原子存檔(config.toml tmp+rename+mutex)+ 能力協商(`Welcome.config_protocol`)+ 未知 frame 容忍(`ClientMsg::Unknown` → unsupported、不斷線)+ secret tri-state(keep/set/clear)+ 遠端寫入認證閘 + 敏感 key 稽核;`PROTOCOL_VERSION` 0→1。裸 `fleety config`(TTY)開三區互動面板(連線 / 本機 / server),server 區經結構化通道遠端 edit、舊 server 退回 ConfigExec。**已知缺口**(minimal-viable,列後續):server 區為單值 edit,provider/model 的完整互動編輯待補;傳輸 wss 硬要求未做;敏感 key 面板告警為二次確認,更完整的分級稽核待強化。

## 剩餘(should-have)

- **remote-config-panel 收尾** — server 區 provider/model 完整互動編輯、傳輸 wss 要求、敏感 key 分級告警/稽核;§4「額外防線」(配對強化、snapshot 敏感欄位讀取分級)可一併做。
- **fleety status 顯示 sidecar 版本 vs 最新版** — 目前只顯示 sidecar 健康(路徑),未做「本機版本 vs release 最新版」對照。小項。

真正未做的多屬 milestone 深度(見下)與待決策略,已無高頻體驗缺口。

## Milestone 深度(既定 backlog,非稽核缺陷)

在 v0 已出貨的能力上仍可加深,已在 `STATUS.md` remaining gaps 追蹤:
- **M9 browser** — snapshot-ref acting(`browser_tabs`、accessibility-snapshot + ref-based act);目前是 `browser_eval` 原始 JS。
- **M10 computer-use** — presence gating(「人在用這台 → 警告/節流」),供無人看管控制。
- **M11 wiki** — raw/distilled/meta 三層結構、frontmatter/wikilink 強制、dedup/lint/MOC、矛盾偵測(目前 convention-only)。
- **M8 scheduling** — `schedule_show` / `schedule_update`(目前 edit = delete + recreate)。

## 明確延後或暫不做

| 項目 | 延後到 | 理由 |
|---|---|---|
| Mobile client | post-v0 | spec 明確排除;桌機優先 |
| Multi-user / RBAC | post-v0 | spec 明確排除;單使用者夠 |
| Web UI / REST API | post-v0 | TUI/CLI 已能完成所有操作 |
| Credential broker | post-v0 | 使用者已表態先不做(Codex OAuth 登入已出貨) |
| Encryption-at-rest / key rotation | post-v0 | 主機檔案系統權限已足夠 v0 |
| LLM 成本追蹤 | post-v0 | 監測類功能,非核心能力 |
| codebase-memory-mcp 整合 | 條件未滿足 | 需要 device→server file sync,v0 不做 |

## 待決策略(blocking)

1. **Presence inference 信號來源**(colocation 上報與 site 記錄已出貨,推論未做)
   - 選項:(a) daemon 主動上報 LAN 鄰居 vs (b) server 主動掃 vs (c) 混合
   - 待決點:回報頻率、假陽性容忍、隱私邊界

（`FLEETY_ADDR` 預設已於 2026-07-11 拍板出貨:裸機預設改 `0.0.0.0:8787`、綁 0.0.0.0 時自動偵測對外 IP 廣播 mDNS,配合 auth-default-on 的認證預設開。見 expose-server-by-default。）

## 建議下一動

高頻體驗缺口已清空,CLI 設定架構重設計 Phase 1 + Phase 2(含互動全包面板)皆已出貨(G1+G2),`FLEETY_ADDR` 預設對外亦已拍板出貨。建議依序:
1. **決定 presence 信號來源**——純產品決策,擋住 presence 推論這條線(頻率 / 假陽性 / 隱私邊界三點),適合走 `/spectra-discuss`。
2. **remote-config-panel 收尾**(server 區 provider/model 完整互動編輯、wss、敏感 key 分級)——設定體驗的最後打磨。
3. **milestone 深度**擇一開展(browser snapshot-ref act 對 agent 自動化價值最高;wiki 三層對知識沉澱價值最高)。
4. **fleety status 版本對照**小項可順手收尾。

---

_最後更新:2026-07-10,CLI 設定架構重設計 Phase 1(4 變更)+ Phase 2(remote-config-panel,結構化 wire + 互動全包面板 minimal-viable)全數出貨後重排。進度推進或現實對不上,直接改本檔。_
