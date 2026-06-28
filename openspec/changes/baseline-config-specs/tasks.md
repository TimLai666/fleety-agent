<!-- 不寫程式:每項是「規格 ↔ env.md / 程式讀取點」一致性驗收。
     行為 = capability 規格與實際讀取的變數/預設值相符;
     驗證 = 對照 docs/env.md 與 crates/ 的 "FLEETY_<NAME>" 讀取點 + spectra validate。 -->

## 1. 伺服器執行期設定驗收

- [ ] 1.1 [P] runtime-configuration 規格 "Server bootstrap configuration" 與 "Access policy and authentication" 與現況相符(ADDR 預設 127.0.0.1:8787、AGENT_HOME、WORKSPACE、SCHED_TICK 60;POLICY 預設 full_access 且 require_approval 閘控非讀工具;REQUIRE_AUTH/TOKEN)。驗證:對照 docs/env.md 與 crates/ 內 "FLEETY_ADDR"/"FLEETY_POLICY"/"FLEETY_REQUIRE_AUTH" 等讀取點與預設。
- [ ] 1.2 [P] model-provider 規格 "OpenAI-compatible model endpoint" 與現況相符(MODEL_BASE_URL/MODEL/KEY;未設則 echo provider;MODEL_STREAM 預設 0,1 啟用 SSE)。驗證:對照 "FLEETY_MODEL_BASE_URL"/"FLEETY_MODEL_STREAM" 讀取點與 echo 回退分支。
- [ ] 1.3 [P] retention-gc 規格 "Periodic retention sweep" 與現況相符(GC_DISABLED 跳過;GC_INTERVAL_SECS 預設 21600、60s 下限;BACKUPS_RETENTION_SECS 604800;HISTORY_ROTATE_BYTES 33554432 輪替)。驗證:對照 GC 背景迴圈讀取點與 clamp/輪替邏輯。

## 2. 探索、配對與自更新驗收

- [ ] 2.1 [P] service-discovery 規格 "mDNS service discovery" 與現況相符(宣告 _fleety._tcp.local.;MDNS_DISABLED 跳過 announce+browse;綁 0.0.0.0 時 MDNS_HOST_IP 必填;MDNS_HOST 實例名)。驗證:對照 mDNS 宣告/瀏覽程式與 "FLEETY_MDNS_*" 讀取點。
- [ ] 2.2 [P] device-enrollment 規格 "Daemon connection configuration" 與 "Token pairing and persistence" 與現況相符(AGENT_URL mDNS 2s→ws://127.0.0.1:8787;DEVICE_ID 路徑安全;DEVICE_ROOT;PAIRING_CODE 換 Welcome token 並寫 ~/.fleety/fleetyd.token;TOKEN 覆寫)。驗證:對照 fleetyd 連線/配對程式與 token 落盤路徑。
- [ ] 2.3 [P] self-update 規格 "Release-manifest update polling" 與 "Sidecar and install paths" 與現況相符(UPDATE_MANIFEST 才輪詢;POLL_SECS 86400、60s 下限;AUTO_UPDATE notify vs apply;INSYRA_BIN/URL;INSTALL_DIR)。驗證:對照 fleetyd 自更新背景迴圈與 sidecar 解析路徑、install-server.sh。

## 3. 整體驗收

- [ ] 3.1 全 6 份能力規格通過 Spectra 結構與 scenario 檢核。驗證:spectra validate baseline-config-specs 零錯誤。
- [ ] 3.2 確認 6 能力的 normative 主張(預設值、clamp、回退)無一與現況讀取點衝突;已由工具面能力管轄的變數(FLEETY_FS_SCOPE/DDGS_*/CHROME_*/WIKI_EMBED/MODELS_DIR/ALLOW_PRIVATE_NET)未重複規格,FLEETY_SYSTEM_PROMPT 留給 baseline-prompt-specs。驗證:逐項對照 docs/env.md 分組與 proposal Non-Goals。
