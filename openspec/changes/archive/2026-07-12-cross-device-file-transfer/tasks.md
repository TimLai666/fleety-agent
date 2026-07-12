## 1. 位元組讀寫(fleety-tools)

- [x] 1.1 依 design「決策一:位元組讀寫抽為純函式 + 薄工具」與 spec 的 Byte-level file read and write for binary and cross-device transfer 要求:crates/fleety-tools/src/lib.rs 新增純函式 read_file_bytes_at(root, rel)→{content_b64,sha256,bytes} 與 write_file_bytes_at(root, backups, rel, content_b64, overwrite)→{sha256,bytes,backup?},沿用 resolve_in_root/resolve_for_write/guard_sensitive/backup_existing;transfer_max_bytes() 讀 FLEETY_TRANSFER_MAX_BYTES(預設 64 MiB),讀/寫超限報含大小與上限的錯誤且無副作用。先寫測試(tdd):binary round-trip(bytes→b64→寫→讀回一致、sha256 相符)、超限拒(不寫)、敏感路徑仍擋。驗證:cargo test -p fleety-tools 全綠。
- [x] 1.2 依 design「決策一」把 read_file_bytes / write_file_bytes 作為薄工具在 register_workspace 註冊(隨 Hello 廣播、device_exec 可分派);config registry 加 FLEETY_TRANSFER_MAX_BYTES(可 config set)。驗證:cargo test -p fleety-tools 全綠、工具出現在 register_workspace 的清單、FLEETY_TRANSFER_MAX_BYTES 在 config list。

## 2. transfer_file 中繼工具(server)

- [x] 2.1 依 design「決策二:transfer_file 中繼工具(server)」「決策三:失敗與校驗」與 spec 的 transfer_file relays a file between two endpoints 要求:crates/fleety-server/src/bridge.rs 新增 TransferFile 工具(hub、pending、server workspace root、backups),參數 from/from_path/to/to_path/overwrite?;端點 server/空→本機純函式,device_id→route_run_tool_via 分派 read_file_bytes/write_file_bytes;讀→寫→sha256 比對,不符報損毀不算成功,回 {bytes,sha256,from,to};risk=Mutate。端點解析與 sha256 比對的可測部分抽純函式。先寫測試:端點解析(server vs device)、sha256 不符判定、server↔server 本機路徑 round-trip。驗證:cargo test -p fleety-server 全綠。
- [x] 2.2 依 design 在 crates/fleety-server/src/conn.rs 的 build_connection_stack 註冊 transfer_file(與 device_exec 同層/慣例對齊);確認 device_exec 描述或工具清單提及位元組工具可經其分派(如適用)。驗證:cargo test -p fleety-server 全綠、既有 device_exec 測試不回歸。

## 3. 文件

- [x] 3.1 [P] docs/env.md:FLEETY_TRANSFER_MAX_BYTES(預設 64 MiB、用途);工具區補 read_file_bytes / write_file_bytes / transfer_file(端點=裝置或 server、sha256 校驗、binary 支援)。驗證:內容審閱與 spec 用語一致。

## 4. 整體驗證

- [x] 4.1 全 workspace 驗證:cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證:指令輸出乾淨。
