## 1. 鑄造 frame(protocol + server)

- [x] 1.1 依 design「決策一:MintPairingCode 請求/回覆 frame」與 spec 的 A pairing code can be minted over the connection 要求:fleety-protocol 新增 ClientMsg::MintPairingCode 與 ServerMsg::PairingCode { code: Option<String>, error: Option<WireError> }(additive);server session 迴圈處理:auth 開啟→create_pairing 回 code,關閉→回 error 說明。先寫測試(tdd):frame round-trip(additive、舊形容忍)、鑄造分支(auth 開回 code、auth 關回 error)。驗證:cargo test -p fleety-protocol -p fleety-server 全綠。

## 2. pair-code 命令(CLI)

- [x] 2.1 依 design「決策二:`fleety pair-code` 命令」與 spec 要求:新增 fleety pair-code 子命令,走 connect_hello 連目前 server、送 MintPairingCode、收 PairingCode:有 code 印碼與 fleety pair <code> 提示,有 error 可讀回報,舊 server(unsupported/未知回覆)給版本提示;help 補一行。驗證:cargo test -p fleety-cli 全綠、pair-code 出現在 help。

## 3. TUI 認證終止

- [x] 3.1 依 design「決策三:TUI 對 unauthenticated 終止」與 spec 的 The TUI surfaces an authentication rejection instead of reconnecting forever 要求:TUI 主迴圈 Error 分支對 error.kind == "unauthenticated" 設可讀 not-paired 訊息並 should_quit,不進重連;其他 Error 與暫時斷線重連不變;認證終止判定抽純函式。先寫測試:is_auth_rejection 純函式(unauthenticated→true、unsupported/invalid/其他→false)。驗證:cargo test -p fleety-cli 全綠、既有 TUI 測試不回歸。

## 4. 整體驗證

- [x] 4.1 全 workspace 驗證:cargo test --workspace 與 cargo clippy --workspace --all-targets -- -D warnings 乾淨、cargo fmt --all -- --check 通過。驗證:指令輸出乾淨。
