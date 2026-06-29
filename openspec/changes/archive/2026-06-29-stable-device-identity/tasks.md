<!-- 每項含交付行為 + 驗證目標。tdd:true → 測試先寫。[P] = 可平行（不同檔案、無相依）。讀真實 OS 機器 id 與跨機撞名行為為環境相依，需手動驗證。 -->

## 1. 機器 id

- [x] 1.1 [P] 在 crates/fleety-cli/src/main.rs 與 crates/fleety-daemon/src/main.rs 讓 `device_id()` 改讀 OS 機器穩定 id（`machine-uid` crate：Windows MachineGuid / Linux /etc/machine-id / macOS IOPlatformUUID），`FLEETY_DEVICE_ID` 為覆蓋，讀不到時有明確 fallback（不可悄悄退回 hostname 撞名）；並另外提供 hostname 當 label——交付 "Device identity is machine-derived and stable"（決策「device_id is the OS machine id, overridable, with hostname as a label」）。驗證:單元測試 `FLEETY_DEVICE_ID` 覆蓋優先、fallback 決定性;cargo build -p fleety-cli -p fleety-daemon 綠;真實機器 id 讀取標手動驗證。新增依賴 `machine-uid`。

## 2. 協定 + 認證 + 連線身分解析

- [x] 2.1 [P] 在 crates/fleety-protocol/src/lib.rs 給 Hello 加 optional `hostname: Option<String>`（additive，PROTOCOL_VERSION 不變，舊端送 None）——交付 "Existing device data migrates losslessly, once" 所需的連線期 label（決策「Hello carries an optional hostname label (additive protocol change)」）。驗證:Hello 帶/不帶 hostname 的序列化 round-trip 測試;cargo test -p fleety-protocol 綠。
- [x] 2.2 在 crates/fleety-server/src/auth.rs 讓 `redeem` 綁機器 id、`verify` 解析機器 id（API 形狀不變、語意改）；crates/fleety-server/src/conn.rs 在 authenticated 時以 `verify(token)` 解析的 id 為準（忽略自報 Hello id），未開 auth 用 Hello 機器 id——交付 "Authenticated identity comes from the token" 與 "Devices are registered under a stable machine-derived id"（決策「Authenticated connections resolve the id from the token, not the Hello field」）。驗證:測試「authed 連線 resolved id = token 綁定 id ≠ 自報 id」「redeem 綁機器 id、verify 取回」;cargo test -p fleety-server 綠。

## 3. 一次性無損遷移

- [x] 3.1 在 crates/fleety-server/src/storage.rs 加 `migrate_device(hostname, machine_id) -> Result<bool>`：若 `fleet/devices/<hostname>/` 存在且 `<machine_id>/` 尚無，搬移 conversations/、history.jsonl、device.json（verify-before-delete）、idempotent；crates/fleety-server/src/conn.rs 在連線用 id 前呼叫、authed 時一併 rebind token→機器 id——交付 "Existing device data migrates losslessly, once"（決策「One-time, per-device, verify-before-delete migration keyed by hostname→machine-id」「Pre-existing collisions are not un-merged」）。驗證:遷移測試（搬移後資料完整、idempotent 第二次 no-op、目的已存在→skip、來源不存在→no-op）;cargo test -p fleety-server 綠。

## 4. 文件

- [x] 4.1 [P] docs/env.md：說明 device_id 現為機器穩定 id（machine-uid、FLEETY_DEVICE_ID 覆蓋、hostname 當 label）、authed 以 token 解析身分、一次性 hostname→機器id 遷移、pre-existing 同名合併不可逆——交付:文件與行為一致。驗證:內容審查。

## 5. 整體驗收

- [x] 5.1 全工作區綠且核心 host-free——交付關鍵驗收。驗證:cargo fmt、cargo clippy --workspace --all-targets -- -D warnings、cargo test --workspace 全綠;cargo tree -p agent-core 確認無 fleety-*;記錄真實機器 id 與跨機行為需手動驗證。
