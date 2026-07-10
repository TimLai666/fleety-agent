## Why

三個部署/安全打磨缺口讓實際上線體驗變差：Windows 生命週期動詞在無管理員權限時要等 `sc` 失敗才報錯（且錯誤只走 tracing，服務情境下不易看到）；fleety-server 的 install/up 不像 fleetyd 那樣在安裝時佈建 insyra sidecar，安裝路徑不對稱且使用者無從得知 insyra_exec 可能不可用；Docker 容器以 root 執行，寫進 bind-mount 的 /workspace 檔案在宿主端變成 root 所有。

## What Changes

- 在 fleety-tools 新增一個無 `unsafe`、無新依賴的權限偵測（Windows 用 `net session` 之類 exit code 探測），對「會變更服務狀態」的 Windows 生命週期動詞（install/uninstall/start/stop/restart/enable/disable/up/down）在呼叫 SCM 之前先偵測是否 elevated；未 elevated 就在動手前中止，於 stderr 印出可執行指引，不留下半套狀態。status 這類唯讀查詢不需權限、不做檢查。
- 讓 fleety-server 的 install/up 也在動作完成後 best-effort 佈建 fleety-insyra sidecar（呼叫既有 `fleety_tools::deps::insyra::ensure_insyra`），與 fleetyd 對齊；佈建失敗只印 console 提示（insyra_exec 在下次 update 成功前不可用），不讓 install/up 失敗。
- Dockerfile 改以專屬非 root 使用者執行 fleety-server，並確保其對 /data、/workspace 可寫、ddgs 仍在 PATH 上，讓容器寫入 volume 的檔案不再是 root 所有。

## Non-Goals

- 不改 Linux（systemd `--user`）/ macOS（launchd LaunchAgent）的權限模型——它們本就不需 root。
- 不解決 bind-mount 的宿主 UID 對應問題：只保證容器進程非 root；宿主端若需特定 owner，仍由使用者以對應 UID 掛載處理。
- 不新增 Docker Compose / Kubernetes securityContext 等額外編排設定。
- 不改 sidecar 的下載來源、版本或更新策略。
- 不為權限偵測引入新 crate 或 `unsafe`（違反 workspace 規則）。

## Capabilities

### New Capabilities

- `container-deployment`: 定義 Fleety server 容器映像的執行姿態（以非 root 使用者執行、volume 可寫、內建工具仍可用），此前無 spec 涵蓋容器打包。

### Modified Capabilities

- service-lifecycle: 新增「Windows 變更型生命週期動詞在動手前 pre-flight 偵測 elevation」的規範，補足目前只在 sc create 失敗後被動提示的缺口。
- startup-dependencies: 新增「install/up 對 insyra sidecar 的佈建在 fleety-server 與 fleetyd 之間對齊」的規範。

## Impact

- Affected specs: service-lifecycle, startup-dependencies, container-deployment
- Affected code:
  - Modified: crates/fleety-tools/src/service.rs
  - Modified: crates/fleety-server/src/service.rs
  - Modified: crates/fleety-server/src/main.rs
  - Modified: crates/fleety-daemon/src/service.rs
  - Modified: Dockerfile
