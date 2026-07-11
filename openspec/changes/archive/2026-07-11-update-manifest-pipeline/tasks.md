## 1. 共用 target triple 與 manifest 解析（fleety-tools）

- [x] 1.1 依 design「決策六：target_triple 提升為 deps 共用」把 target triple 對照表提升到 deps 模組：crates/fleety-tools/src/deps.rs 提供 crate 內共用的 target_triple 查詢，crates/fleety-tools/src/deps/insyra.rs 改用共用版且 sidecar URL 行為不變，保留與 release.yml target 清單同步的註解。驗證：cargo test -p fleety-tools 既有測試全綠（含 insyra URL 測試不變）。
- [x] 1.2 依 spec 的 Update manifest schema 要求與 design「決策一：多 target manifest 解析」，於 crates/fleety-tools/src/update.rs 實作多 target manifest 解析：targets map 依本機 triple 選取、平面舊格式向後相容、未知欄位忽略、可選 versioned_manifest 欄位、缺本機 triple 時版本仍可探測而安裝報出含 manifest 版本與本機 triple 的錯誤。先寫單元測試再實作（tdd）：涵蓋選中本機 triple、缺 triple、平面格式、未知欄位忽略、sha256 小寫正規化。驗證：cargo test -p fleety-tools 新增測試全綠。
- [x] 1.3 依 spec 的 Manifest URL templating 要求與 design「決策二：latest 解析替換 {version} 為 latest」，讓 manifest_url_for 同時替換 {bin} 與 {version}（{version} 換為固定字 latest），純 URL 與純 {bin} 模板行為不變、manifest_url_for_versioned 既有行為不變。先寫測試：{version} 模板的 latest 解析結果與替換矩陣各列。驗證：cargo test -p fleety-tools 全綠。
- [x] 1.4 依 spec 的 Fleet convergence version resolution 要求與 design「決策三：收斂解析鏈」，於 crates/fleety-tools/src/update.rs 實作釘選決策純函式（輸入 latest manifest 與目標版本 V，輸出三分支：直接用 latest、去抓 versioned_manifest 替換後的 URL、無法釘選附原因）以及「取回釘選 manifest 後驗證其 version 等於 V、不符拒用且不安裝」的檢查。先寫測試：三分支各至少一例、版本不符拒用一例。驗證：cargo test -p fleety-tools 全綠。

## 2. daemon 收斂與輪詢接線（fleety-daemon）

- [x] 2.1 依 design「決策三：收斂解析鏈」把 crates/fleety-daemon/src/main.rs 的 converge_to_server_version 改走收斂解析鏈（env {version} 模板優先，否則抓 latest manifest 直接用或經 versioned_manifest 欄位釘選），行為符合 spec 的 Fleet convergence version resolution 四個 scenario，無路徑可走時的警告文字指出兩條出路（manifest 加 versioned_manifest 欄位、或 env 換 {version} 模板）。驗證：cargo test -p fleety-daemon 全綠，決策邏輯以 fleety-tools 純函式的單元測試覆蓋，cargo clippy 無新警告。
- [x] 2.2 依 spec 的 Manifest URL templating 要求與 design「決策四：sibling 更新的 {bin} 防護」補上防護：env 模板缺 {bin} 時跳過 fleety 與 fleety-server 的 sibling 更新並以警告指名缺 {bin} 佔位符，daemon 自我更新不受影響（與 fleety update CLI 端既有防護對齊）。先寫測試：防護判斷抽為可測函式，涵蓋「有 {version} 無 {bin}」組合不再解析出錯誤 binary 的 manifest。驗證：cargo test 全綠。
- [x] 2.3 確認輪詢路徑（crates/fleety-daemon/src/poll_updates.rs）在新解析下語義不變，符合 MODIFIED spec 的 Release-manifest update polling 要求：未設 FLEETY_UPDATE_MANIFEST 不輪詢、notify 只警告、apply 走完整更新。此任務為驗證性，預期不改碼；若發現需要改碼則先回報再動。驗證：cargo test -p fleety-daemon 既有測試全綠，poll 路徑對 {version} 模板 env 的解析結果以測試斷言為 latest 形式。

## 3. release workflow fan-in（.github/workflows/release.yml）

- [x] 3.1 [P] 依 design「決策五：release fan-in job 產生 manifest」在既有 matrix build job 追加：把 fleety、fleety-server、fleetyd 的裸二進位以「bin 名-target triple」命名（Windows 加 .exe）附掛到 release，並同時以 workflow artifact 上傳供 fan-in job 使用；壓縮檔資產與 fleety-insyra 裸資產維持不變。驗證：workflow YAML 以本地解析器檢查無語法錯誤，資產命名與 spec 的 Release publishes update artifacts and manifests 要求逐項人工比對一致。
- [x] 3.2 依 design「決策五：release fan-in job 產生 manifest」與 spec 的 Release publishes update artifacts and manifests 要求，新增 manifests fan-in job（needs: build）：下載全部裸資產 artifact、逐一計算 sha256、為每個 bin 產出一份多 target 的 <bin>-manifest.json（url 指 releases/download/<tag>/ 釘選資產、含 versioned_manifest 模板欄位）、以 jq 驗證欄位完整、僅 tag push 附掛到 release；tag 版本（去 v 前綴）與 Cargo.toml 的 [workspace.package] version 不符即讓 job 失敗（以節掃描取值，不用整檔 grep）。驗證：manifest 產生與版本守門的 shell 片段抽成可本地執行的步驟並以樣本輸入實跑（正常、版本不符兩案），workflow YAML 解析檢查通過。
- [x] 3.3 workflow_dispatch dry-run 行為對齊 spec 的 dispatch dry-run scenario：不附掛 release、manifest 以 workflow artifact 產出並通過 jq 驗證（以 Cargo 版本代作 tag）。同時查證 softprops/action-gh-release 對同名資產重跑的覆蓋語義並在 workflow 註解記錄結論（若非覆蓋則改為顯式刪後再傳）。驗證：條件守門與既有 build job 的 dry-run 姿態一致（人工檢視 diff），合併後首次 dispatch 的 artifact 檢視列入 PR 描述的上線後驗證清單。

## 4. 文件

- [x] 4.1 [P] docs/env.md 自我更新章節改寫：GitHub 直連一行建議值（releases/latest/download/{bin}-manifest.json 形式）、多 target schema 與 versioned_manifest 欄位、latest 解析的 {version} 替換為 latest 慣例與自架 latest 別名承接、收斂解析鏈順序、sibling 更新的 {bin} 要求、ARM 與 RISC-V 源碼自建平台的行為（notify 可用、安裝報無本平台資產）。驗證：內容審閱——讀者可複製一行設定讓四條更新路徑全通，用語與 spec 一致。
- [x] 4.2 [P] docs/STATUS.md 的 self-update 條目更新：manifest 發佈管線、多 target manifest、收斂解析鏈列為已出貨能力。驗證：內容審閱與實作現況相符。

## 5. 整體驗證

- [x] 5.1 全 workspace 驗證：cargo test --workspace 與 cargo clippy --workspace 無新警告（unwrap/expect 禁用姿態不變），並確認 fleety-cli 的 fleety update 鏈在 manifest_url_for 新行為下不需改碼即符合預期（sibling 更新仍以 manifest_is_templated 守門）。驗證：兩道指令輸出乾淨，fleety-cli 既有測試全綠。
