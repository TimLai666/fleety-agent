## Why

模型 provider 目前寫死成兩個 tier(ProviderTiers { main, cheap }),各一個實例,由扁平 env 設定;subagent 用 resolve("main"/"cheap") 二選一。無法有第三、第四個實例,因此無法配置「好幾個 Codex 帳號」或「好幾個 OpenAI 相容端點」,也沒有把額度分散到多個帳號、或某帳號額度滿時自動換另一個的能力。使用者要能擁有任意多個同類型 provider,並以群組做 round-robin 分散與故障轉移。

## What Changes

- 新增 `~/.fleety/providers.toml`(獨立檔,金鑰與一般設定隔離):`[[provider]]` 陣列定義任意數量具名 provider(name、base_url、model、key、stream、modalities、effort;type 沿用現有 base_url 啟發式自動判 openai 相容 vs gemini)。
- `[[group]]` 定義群組(name、members=provider 名稱清單、strategy=`round_robin`|`failover`)。
- 角色映射:`main`/`cheap`/任意 subagent tier 名 → 引用某個 provider 或 group 名。
- 新增 `PoolProvider`(在 fleety-server,實作既有 `ModelProvider`,**不改 agent-core trait**):持有多個成員 + 策略;呼叫某成員回 Err(成員內部已含 resilient-model-calls 的退避重試)就**換下一個成員**,全部失敗才回最後的錯。`round_robin` 每次呼叫原子遞增起點分散;`failover` 固定從頭、壞了才往後。
- `resolve(name)` 改查具名池;**沒有 providers.toml 時 fallback 既有 `FLEETY_MODEL_*`/`FLEETY_CHEAP_MODEL_*` env**(建出名為 main/cheap 的 provider),零設定行為不變。

## Non-Goals

(本變更會建立 design.md,Non-Goals / 後續寫在 design 的 Goals/Non-Goals 與 Open Questions。)

## Capabilities

### New Capabilities

- `provider-pool`: 具名 provider 池(providers.toml)+ 群組策略(round_robin / failover,任何錯換下一個成員)+ 角色引用名稱(單一 provider 或 group)+ 與 env 的相容 fallback;以 fleety-server 的包裝 provider 實作,agent-core trait 不變。

### Modified Capabilities

(none)

## Impact

- Affected specs: provider-pool(新)
- Affected code:
  - New:
    - crates/fleety-server/src/pool.rs(PoolProvider:成員 + 策略 + 輪替/換手;providers.toml 的 schema 與載入/解析、純函式策略選擇)
  - Modified:
    - crates/fleety-server/src/providers.rs(ProviderTiers / resolve 改查具名池;無 providers.toml 時 fallback 既有 env build)
    - crates/fleety-server/Cargo.toml(若需 toml/serde 解析 providers.toml — server 已有 serde/serde_json;toml 解析依賴視需要)
    - docs/env.md(providers.toml 格式與 env fallback 說明)
