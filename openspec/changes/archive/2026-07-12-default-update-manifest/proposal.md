## Problem

在 server 本機跑 `fleety update` 直接報錯:`set FLEETY_UPDATE_MANIFEST to the update manifest URL (may contain {bin})`。我們這個 session 已經把 manifest 發佈管線建好(每個 release 都有 `fleety-manifest.json` 等三個 manifest),但 **client 端沒有內建預設 manifest URL** —— `manifest_url_for` 直接讀 `FLEETY_UPDATE_MANIFEST`、unset 就報錯,安裝腳本也沒設它。結果 stock 安裝的 `fleety update` 開箱即壞。

## Root Cause

`crates/fleety-tools/src/update.rs` 的 `manifest_url_for` / `manifest_is_templated` / `manifest_supports_version` 全部只從 `FLEETY_UPDATE_MANIFEST` env 取值,沒有 fallback。專案自己的 release manifest URL(`https://github.com/TimLai666/fleety-agent/releases/latest/download/{bin}-manifest.json`)只寫在 docs,沒編進 binary。

## Proposed Solution

把專案自己的 release manifest URL 編為內建預設常數 `DEFAULT_UPDATE_MANIFEST`;`FLEETY_UPDATE_MANIFEST` 未設時用它,設了就覆蓋(給 fork/私有鏡像)。manual `fleety update`(走 `manifest_url_for` 的 latest 形式)因此開箱可用;per-version 收斂仍靠 manifest 內的 `versioned_manifest` 欄位,故內建預設用 latest 形式即可。**daemon 的無人值守 auto-poll 維持 opt-in**(仍檢查 env 是否設),不因內建預設而自動開啟。

## Non-Goals (optional)

- 不改 daemon 無人值守 auto-poll 的 opt-in 語意(`poll_updates`/`daemon` 仍要求顯式設 `FLEETY_UPDATE_MANIFEST`)。
- 不改 manifest schema、sha256 校驗、forward-only 收斂或 `versioned_manifest` 機制。
- 不改 `manifest_url_for_versioned`(pinned 解析仍要求 env 模板含 `{version}`;預設走 manifest 欄位)。

## Success Criteria

- 未設 `FLEETY_UPDATE_MANIFEST` 時,`manifest_url_for("fleety")` 回 `https://github.com/TimLai666/fleety-agent/releases/latest/download/fleety-manifest.json`,`manifest_is_templated()` 為 true。
- 設了 `FLEETY_UPDATE_MANIFEST` 時,行為與原本一致(env 覆蓋)。
- daemon auto-poll 在 env 未設時仍略過(opt-in 不變)。
- 單元測試涵蓋未設→內建預設、已設→覆蓋。

## Impact

- Affected specs: `self-update`(modified — Manifest URL templating)
- Affected code:
  - Modified:
    - crates/fleety-tools/src/update.rs — 新增 DEFAULT_UPDATE_MANIFEST + manifest_template();manifest_url_for / manifest_is_templated / manifest_supports_version 改用它
    - docs/env.md — FLEETY_UPDATE_MANIFEST 說明改為「內建預設,env 覆蓋」
  - New: (none)
  - Removed: (none)
