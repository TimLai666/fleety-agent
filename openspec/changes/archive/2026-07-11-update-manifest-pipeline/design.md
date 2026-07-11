## Context

更新機制的既有形狀：`crates/fleety-tools/src/update.rs` 以單一 env `FLEETY_UPDATE_MANIFEST` 解析 manifest URL（`{bin}` 佔位符替換 binary 名，純 URL 視為目前 binary 自己的 manifest），manifest 是 `{version, url, sha256}` 平面 JSON，`install()` 下載 `url` 的位元組、驗 sha256 後直接寫成執行檔（rename-aside 交換，無解壓）。消費端有四條路：`fleety update`（主機全元件鏈式更新）、`fleetyd update`（binary 加 sidecar 加服務重啟）、daemon 24 小時輪詢（notify 預設、apply 全自動）、以及 daemon 重連時的伺服器版本收斂（forward-only 釘選到 server 精確版本，需要 `{version}` 佔位符）。

三個事實讓它端到端不可用：release workflow 只發佈 tar.gz 與 zip 壓縮檔（僅 fleety-insyra sidecar 有裸資產），沒有任何 manifest 產生管道。latest 解析只替換 `{bin}` 不處理 `{version}`，所以照文件範例設了 `{version}` 模板後，輪詢與 update 指令會抓含字面 `{version}` 的網址。manifest schema 沒有平台維度，單一 url 無法同時服務 Linux x64、macOS ARM、macOS x64、Windows 的異質 fleet。另外收斂端 sibling 更新缺 `{bin}` 防護，在「有 {version}、無 {bin}」的設定下會把 fleetyd 的 manifest 內容寫進 fleety 與 fleety-server 的執行檔路徑。

限制條件：CLI 設定架構剛完成收斂，不擴充 env 面（不加第二個 manifest 變數）。`FLEETY_UPDATE_MANIFEST` 未設即不輪詢的 opt-in 姿態是既定 spec。forward-only 不降版是既定行為。binaries 以 workspace 單一版本 lockstep 出貨，`agent_core::VERSION` 是本地基準。GitHub 目前 0 release 0 tag，沒有已部署的舊 manifest 相容包袱。

## Goals / Non-Goals

**Goals:**

- 打完 tag 之後，主機只要設一行 `FLEETY_UPDATE_MANIFEST=https://github.com/TimLai666/fleety-agent/releases/latest/download/{bin}-manifest.json`，四條更新路徑（fleety update、fleetyd update、輪詢 notify 與 apply、伺服器版本收斂）全部可用，不需要自架任何伺服器。
- 單一 env 同時服務 latest 追蹤與精確版本釘選兩種解析。
- 自架 file server 的佈局（形如 https://host/dl/{bin}/{version}/manifest.json）仍受支援，latest 以 latest 別名目錄承接。
- 消除 sibling 更新以錯誤 binary 內容覆寫執行檔的可能性。

**Non-Goals:**

- 不動 install 腳本（安裝路徑繼續用壓縮檔資產，install-server.sh 的可寫性判斷落差另案處理）。
- 不動 fleety-insyra sidecar 的更新路徑（已有裸資產加 FLEETY_INSYRA_URL 機制）。
- 不把更新檢查改為預設開啟。不提供降版。不新增第二個 manifest env 變數。
- 不在這次加入下載內容的檔案格式 magic-bytes 檢查（操作者手寫 manifest 指向壓縮檔的誤用，靠文件與 sha256 門檻擋，列為未來強化）。

## Decisions

### 決策一：多 target manifest 解析

新 schema（v2）：

```json
{
  "version": "0.2.0",
  "versioned_manifest": "https://github.com/TimLai666/fleety-agent/releases/download/v{version}/{bin}-manifest.json",
  "targets": {
    "x86_64-unknown-linux-gnu": {
      "url": "https://github.com/TimLai666/fleety-agent/releases/download/v0.2.0/fleety-x86_64-unknown-linux-gnu",
      "sha256": "..."
    }
  }
}
```

- 平面舊格式 `{version, url, sha256}` 繼續解析（向後相容，也留給單一平台自架者最簡形式）。未知欄位一律忽略（向前相容）。
- 解析結果：`version` 必有。url 加 sha256 成對變成可選（平面形式直接給、targets 形式取本機 triple 的項目）。`versioned_manifest` 可選。manifest 沒有本機 triple 項目時，安裝路徑報明確錯誤（訊息含 manifest 版本與本機 triple），但版本探測（notify 輪詢）仍成功。
- manifest 內的 url 一律指版本釘選資產（releases/download/TAG/ 路徑），不指 latest/download。資產不可變，sha256 在新版釋出後仍有效。
- 否決替代案「URL 模板加 {target} 佔位符」：env 契約要多一個佔位符、每 release 十二份 manifest、文件面翻倍。target 維度收進 manifest 內，updater 用自身編譯期 triple 選取即可。

### 決策二：latest 解析替換 {version} 為 latest

`manifest_url_for`（輪詢、fleetyd update、fleety update 共用的 latest 解析）除 `{bin}` 外，把 `{version}` 替換為字面 `latest`。自架佈局 https://host/dl/{bin}/{version}/manifest.json 的 latest 解析變成 https://host/dl/fleetyd/latest/manifest.json，操作者以 latest 目錄或 symlink 承接（文件明載此慣例）。GitHub 推薦模板不含 `{version}`，不受影響。

- 否決「latest 解析遇 {version} 直接報錯」：把可運作的常見佈局慣例變成死路，操作者被迫二選一。
- 否決「辨識 GitHub URL 形狀做路徑手術（download/v{version} 換成 latest/download）」：供應商特定的字串魔法進 updater，測試與心智負擔都不值。

### 決策三：收斂解析鏈

裝置要收斂到 server 版本 V 時，對每個 binary 依序：

1. env 模板含 `{version}`：走現行 `manifest_url_for_versioned`（行為不變）。
2. 否則抓該 bin 的 latest manifest：
   - 其 `version` 等於 V：直接以這份 manifest 安裝（最常見情境——server 剛更新到最新版，一次 fetch 完事）。
   - 含 `versioned_manifest` 模板：替換 `{bin}` 與 `{version}` 為 V 後取回釘選 manifest。取回後驗證其 `version` 欄位等於 V，不符即拒用並警告（發佈端錯誤不得靜默安裝）。
   - 都不行：警告無法釘選（訊息指出兩條出路：manifest 加 versioned_manifest 欄位，或 env 換 {version} 模板）。

發佈端最了解自己的 URL 佈局，由 manifest 自我描述版本釘選路徑，updater 不內建任何供應商知識。「latest 版本比對與走哪條路」的判斷實作為純函式（輸入 latest manifest 與 V，輸出使用 latest、去抓某 URL、或無法釘選加原因），可完整單元測試。

- 否決「收斂只抓 latest」：latest 超前 server 時裝置會越過 server 版本，違反 fleet 精確跟隨 server 的既定設計。
- 否決「第二個 env 變數放 versioned 模板」：設定面剛收斂完，不再擴。

### 決策四：sibling 更新的 {bin} 防護

收斂端更新 sibling binary（fleety、fleety-server）前，先要求 env 模板含 `{bin}`（決策三的兩條路徑都適用）。沒有就跳過該 sibling 並警告，警告文字指名補 `{bin}` 佔位符。與 fleety update CLI 端既有的防護對齊。理由：無 `{bin}` 的模板解析出來是執行中 daemon 自己的 manifest，拿它更新 fleety-server 等於把 fleetyd 的 binary 寫進 fleety-server 的執行檔路徑。自我更新（bin 等於目前行程）不受影響，純 URL 設定照舊可用。

### 決策五：release fan-in job 產生 manifest

- 既有 matrix build job 追加兩件事：把 fleety、fleety-server、fleetyd 的裸二進位（命名為 bin 名加 target triple，Windows 加 .exe，比照 fleety-insyra 慣例）附掛到 release，並同時以 workflow artifact 上傳給 fan-in job。壓縮檔資產照舊（install 腳本不動）。
- 新增 fan-in job（needs: build）：下載全部裸二進位 artifact、逐一算 sha256、為每個 bin 產一份多 target manifest（url 指 releases/download/TAG/ 的釘選資產，含 versioned_manifest 模板欄位），以 jq 驗證欄位完整性後，僅在 tag push 時附掛到 release（沿用既有 refs/tags 前綴守門）。
- 版本一致性檢查：以節掃描方式從 Cargo.toml 的 [workspace.package] 區段取 version（不可用整檔 grep，避免撞到其他 version 行），與去掉 v 前綴的 tag 比對，不一致即讓 job 失敗。manifest 版本必須等於 binaries 編譯進去的 agent_core::VERSION，否則 needs_update 的不等比較會永遠判定要更新，形成安裝迴圈。
- workflow_dispatch dry-run：以 Cargo 版本代作 tag 產生並驗證 manifest、上傳為 workflow artifact 供人工檢視，不附掛 release（沿用既有 dry-run 姿態）。

### 決策六：target_triple 提升為 deps 共用

把 target triple 對照表從 sidecar 專用模組提升到 deps 模組層級（crates/fleety-tools/src/deps.rs）供 insyra 與 update 兩處共用，保留「與 release.yml 的 target 清單保持同步」的註解。否決在 update.rs 複製一份：兩份清單必然漂移。

## Implementation Contract

**行為（操作者視角）：**

- 主機設定 FLEETY_UPDATE_MANIFEST 為 GitHub releases/latest/download/{bin}-manifest.json 形式後：fleety update 與 fleetyd update 下載本機 triple 的裸 binary、sha256 驗證、rename-aside 交換、服務重啟流程不變。輪詢 notify 記錄可用新版本，apply 等同定期 fleetyd update。daemon 重連遇 server 較新時精確釘選到 server 版本。
- {version} 模板的 latest 解析範例：https://host/dl/{bin}/{version}/manifest.json 對 fleetyd 解析為 https://host/dl/fleetyd/latest/manifest.json。
- manifest 缺本機 triple：更新報錯，錯誤訊息含 manifest 版本與本機 triple 字串。版本探測與 notify 照常運作。
- 釘選 manifest 版本不符：拒用並警告，訊息含期望版本與實得版本。
- sibling 跳過警告指名缺 {bin} 佔位符。

**介面與資料形狀：**

- manifest JSON 兩形式：平面（version、url、sha256）與 v2（version、targets map（triple 對 url 加 sha256）、可選 versioned_manifest）。未知欄位忽略。sha256 一律小寫十六進位比對（沿用現行 lowercase 正規化）。
- fleety_tools::update 既有公開函式簽名全部保留（manifest_url_for、manifest_url_for_versioned、probe_latest_for、probe_latest、install、update_named、update_to_version、self_update、self_update_to_version、needs_update_str、is_newer、manifest_is_templated、manifest_supports_version、sibling_exe）。manifest_url_for 的行為擴充為同時替換 {version} 為 latest。新增的釘選決策純函式與收斂解析入口由 apply 時命名，daemon 的 converge_to_server_version 改走新解析鏈。
- deps 模組提供 crate 內共用的 target_triple 查詢，insyra 模組改用共用版，行為不變。
- 每個 release tag 的資產集合：四個 Rust target 各有 fleety、fleety-server、fleetyd 三份裸 binary（Windows 帶 .exe）。既有 tar.gz 與 zip 壓縮檔不變。fleety-insyra 裸資產不變。新增 fleety-manifest.json、fleety-server-manifest.json、fleetyd-manifest.json 三份。

**失敗模式：**

- env 未設：一切照舊跳過（opt-in 不變）。
- manifest 取得或解析失敗：輪詢單 tick 隔離警告（不變），update 指令回報錯誤訊息（不變）。
- fan-in job 失敗（含版本不一致）：release 上有 binaries 與壓縮檔但沒有 manifest，updater 端 fetch 失敗只留警告，不可能發生錯誤安裝（fail closed）。修正後重跑 workflow 須能覆蓋同名資產。
- 收斂鏈全部不可行：警告並跳過，裝置停在原版本（forward-only 安全側）。

**驗收準則：**

- cargo test -p fleety-tools 新增單元測試全綠：targets 形式選中本機 triple、缺 triple 時 version 可讀而安裝報錯、平面形式向後相容、未知欄位忽略、versioned_manifest 模板替換、manifest_url_for 的 {version} 換 latest、釘選決策純函式三分支、釘選版本不符拒用。
- cargo test 全 workspace 綠。cargo clippy 全 workspace 無新警告（unwrap/expect 禁用姿態不變）。
- daemon 收斂與 sibling 防護的可測部分抽為純函式並有單元測試。網路與檔案交換路徑維持專案既有的手動驗證姿態（與 update.rs 現行 doc 註解一致）。
- workflow：workflow_dispatch dry-run 產出三份通過 jq 驗證的 manifest artifact。版本一致性守門在 tag 與 Cargo 版本不符時讓 job 失敗（守門邏輯可由 dry-run 記錄檢視）。首個真實 tag 後，releases/latest/download/fleety-manifest.json 可直接 curl 取得且欄位完整（上線後人工驗證項）。
- docs/env.md 讀者可複製一行 GitHub 設定值讓四條更新路徑全通。自架 latest 別名慣例、v2 schema、收斂解析鏈、sibling {bin} 要求皆有記載。

**範圍邊界：**

- 範圍內：.github/workflows/release.yml、crates/fleety-tools/src/update.rs、crates/fleety-tools/src/deps.rs、crates/fleety-tools/src/deps/insyra.rs、crates/fleety-daemon/src/main.rs、docs/env.md、docs/STATUS.md。
- 範圍外：install 腳本三支、sidecar 更新路徑、fleety-cli 的 update 指令流程（其行為經 manifest_url_for 修正間接受益，程式碼不動）、任何新 env 變數、輪詢預設值。

## Risks / Trade-offs

- [重跑 release workflow 時同名資產覆蓋行為依賴 gh-release action 的語義] → apply 時實際確認 softprops/action-gh-release 對既存同名資產的處置。若非覆蓋，改為顯式刪後傳。此點列為 fan-in 任務的驗收之一。
- [tag 與 Cargo 版本不符的人為失誤] → fan-in 硬性失敗，manifest 不上架。binaries 已附掛但 updater 因無 manifest 而 fail closed，修正 tag 或版本後重跑即可。
- [自架 {version} 模板者若無法提供 latest 別名] → 輪詢從「抓字面 {version} 網址 404」變成「抓 latest 別名 404」，可見性相同（每 tick 警告一次）且文件明載出路，風險不高於現況。
- [manifest 只列四個 Rust target，ARM 與 RISC-V 上源碼自建的 fleetyd 輪詢] → notify 照常報新版本。apply 或手動 update 報「無本平台資產」的明確錯誤，不會裝壞東西。文件註明這類平台走源碼更新。
- [收斂經 versioned_manifest 需兩次 fetch] → 僅發生在重連且 latest 超前 server 的窗口，頻率可忽略。
- [操作者手寫 manifest 指向壓縮檔資產] → sha256 必須自算所以多半知道自己在做什麼。文件明載 url 必須指裸 binary。magic-bytes 檢查列為未來強化，不在本次範圍。

## Migration Plan

程式碼與 workflow 同一變更出貨。合併後第一個真實 tag 產出完整資產集合，之後在各主機設定 env 值即可啟用。目前 0 release 0 部署，無舊 manifest 相容遷移。回滾即 revert 本變更。已附掛在既有 release 上的 manifest 對當時出貨的程式碼仍然有效，不需清理。

## Open Questions

- 無阻斷項。gh-release action 的同名資產覆蓋語義在 apply 時確認（見 Risks 第一條）。
