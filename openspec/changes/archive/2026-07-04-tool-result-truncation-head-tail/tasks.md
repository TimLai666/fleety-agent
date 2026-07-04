## 1. 長字串結構壓縮保留頭與尾

- [x] 1.1 測試先行:在 compress 測試加 `long_string_keeps_head_and_tail`(一個遠超 max_string 的字串經 SmartCrusher 後,開頭原始片段與結尾原始片段都在、中間有省略字元數標記)與 `short_string_unchanged`(≤ max_string 原樣);並把既有 `long_string_is_truncated` 的斷言由「只留頭 + …(+N chars)」更新為新的頭尾預期。先紅。驗證:`cargo test -p agent-core long_string_keeps_head_and_tail short_string_unchanged` 之一先紅(現況只留頭)。
- [x] 1.2 實作 SmartCrusher 對超過 max_string 的字串保留頭(約 3/4)+ 尾(約 1/4),中間插入「…(+N chars omitted)」標記,以 `chars()` 為單位切割(UTF-8 安全),並維持 `crush_tracked` 的 truncated=true(fetch id marker 行為不變)——落實「Requirement: Long strings are truncated head-and-tail」與 design「長字串結構壓縮保留頭與尾」。驗證:1.1 測試全轉綠。

## 2. fetch 一頁上限對齊 SmartCrusher 長字串門檻

- [x] 2.1 測試先行:加 `fetch_page_capped_to_threshold`(對 fetch 傳超過長字串門檻的 limit,回傳 content 長度被 clamp 到門檻)與 `fetch_page_survives_compression`(一頁 content 為門檻大小的字串,經 `compress_tool_result` 後 content 原樣、且不含 `fetch_tool_result` marker);若既有 fetch clamp 測試斷言的是 budget 上界,一併更新為門檻。先紅。驗證:`cargo test fetch_page_capped_to_threshold fetch_page_survives_compression` 之一先紅。
- [x] 2.2 實作:由 agent-core 以單一來源暴露 SmartCrusher 的長字串門檻(max_string),並把 `fetch_tool_result` 的 limit 預設值與 clamp 上界從 budget 對齊到該門檻,使一頁 content 不超過門檻、回饋壓縮時不再被 SmartCrusher 二次截、不冒出指向自身的 marker——落實「Requirement: Full tool results are retrievable in bounded segments」與 design「fetch 一頁上限對齊 SmartCrusher 長字串門檻」。驗證:2.1 測試全轉綠。

## 3. 全量驗證

- [x] 3.1 跑全 workspace 測試與 lint,確認無回歸(含既有 compress 與 fetch 分頁測試)且非測試碼無新 `unwrap_used`/`expect_used`。驗證:`cargo test` 全綠、`cargo clippy --all-targets` 無新警告。
