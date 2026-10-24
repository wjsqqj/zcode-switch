# Design: marketing/touch 活动套餐接入 + 每账号刷新

## 边界与数据流

```
前端 main.js                          Rust 后端
────────────────────────────────────────────────────────────────
actions.refreshClaim()  ──全量循环──▶ claim_refresh(id)   ┐
actions.acctRefreshClaim(id) ──单账号▶ claim_refresh(id)   ├─▶ collect_claimable_plans()  [lib.rs 新增]
claimAll / autoClaimTick ──────────▶ claim_start(id,pid)  │      1) claim::preview_plans()      （现有，billing/preview）
claim-preview CLI ─────────────────▶ claim_preview(id)    ┘      2) claim::marketing_touch_plans()（新增，marketing/touch）
                                                              3) 按 plan_id 去重（preview 优先）+ priority 降序
claim_start: 领取前用 collect_claimable_plans() 校验 plan 存在（替代仅 preview 校验）
```

- 前端渲染零改动（合并发生在后端 `Vec<ClaimPlan>` 层，字段同构）。
- 领取链路零改动（`submit_claim` 不动）。

## claim.rs 新增

```rust
pub const MARKETING_TOUCH_URL: &str = "https://zcode.z.ai/api/v1/marketing/touch?seq=0";

pub fn marketing_touch_plans(home, creds, config, device_mid) -> Vec<ClaimPlan>
```

- token：复用 `claim_token()`；无 token 直接返回空（记日志）。
- 请求：`agent().get(MARKETING_TOUCH_URL)` + `quota::zai_billing_headers_with_mid(token, mid)`（已含 X-Device-Mid / X-ZCode-App-Version / X-Client-Language / Authorization，为客户端所发头的超集）。
- 响应解析（宽松，全部 `serde_json::Value` 导航，不做严格 schema）：
  1. `code != 0` → 记日志、返回空。
  2. 遍历 `data.deliveries[]`，对每个 delivery：
     - 递归收集内嵌套餐：任意层级的对象若含字符串 `plan_id` 且含 `entitlements` 数组（或 `name`），喂给现有 `parse_plan()`（含 `zcode_plan` 现位于 `banner.background.args` / `success_popup.hero.args` / `popup.hero.args`，递归扫描对将来位置变化健壮）。
     - 收集可领 plan_id：`banner.buttons[]` / `popup.buttons[]` 的 `action.type ∈ {claim_zcode_plan, claim_plan}` → `action.args.plan_id`（兜底 `action.planId`）。
  3. 按钮 plan_id 无内嵌 payload 时合成最小条目（name=plan_id、空 grants），保证可领取可见。
  4. 按 plan_id 去重、priority 降序（与 preview_once 排序一致）。
- 任何网络/解析错误：`flowlog::log("claim", "marketing-fail", …)` 后返回 `Vec::new()`，绝不向上传播错误（PRD 异常分支决策）。

## quota.rs 版本策略

- `CLIENT_APP_VERSION`: `"3.11.2"` → `"3.16.3"`（语义：最新已知版本，同时是 oauth 头的兜底版本）。
- 新增 `fn version_ge(a: &str, b: &str) -> bool`（按 `.` 分段数值比较，长度不齐补 0）。
- `zcode_app_version()`：探测成功后 `return if version_ge(CLIENT_APP_VERSION, &detected) { CLIENT_APP_VERSION } else { detected }`；探测失败路径不变（返回常量，即新兜底 3.16.3）。
- 影响面：`zai_billing_headers_with_mid`（preview/claim/marketing）与 `zcode_app_version()` 的所有消费者统一获得 max 策略；`zai_oauth_headers_with_mid` 继续用常量（随常量升级到 3.16.3）。

## lib.rs 合并层

```rust
fn collect_claimable_plans(paths, acc, mid) -> Vec<claim::ClaimPlan> {
    let mut plans = claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid.clone()))
        .unwrap_or_default();          // preview 失败保留空，交由 marketing 兜底（错误向上抛的策略见下）
    ...
}
```

- 现有三处（`claim_preview` / `claim_refresh` / `claim_start`）的 preview 调用收敛到该函数；**preview 失败仍向上抛错**（保持现有报错体验），marketing 失败静默（在 claim.rs 内部消化）：
  - `claim_preview` / `claim_start`：`preview_plans` 出错时继续返回 Err（现状不变）。
  - `claim_refresh`：同上，preview 出错走现有 Err 分支；marketing 仅在 preview 成功后追加。
- 去重：以 preview 结果为基准，marketing 条目 plan_id 未出现才追加。

## 前端 main.js

- 新增状态：`const acctRefreshBusy = {};`（按账号的忙碌标记）+ 复用 `REFRESH_CLAIM_COOLDOWN_MS` 做每账号冷却 `acctRefreshCooldownUntil = {}`。
- `actions.acctRefreshClaim(id)`：
  1. 守卫：`refreshClaim.running || claimAllRunning || claimActive || autoClaimRunning || acctRefreshBusy[id]` → 直接 return（自动领取运行中不打断轮次，与全量刷新的 abort 语义区分）。
  2. 冷却中 → 复用 `btn.refreshClaimCooldownTitle` toast。
  3. `claimable[id] = { plans: 保留现有, busy: true }` → `invoke("claim_refresh", { id })` → 成功写回 `claimable[id]`；`r.activationError` 走现有 warn toast；异常走现有 err toast。
  4. `scheduleNext(id)` + render；冷却 60s。
- 账号行 `row-actions` 新增图标按钮（`refresh` 图标，16px，置于额度按钮后）：
  `title=btn.acctRefreshTitle`，disabled 条件与守卫一致 + 该账号 busy/冷却。
- i18n：`btn.acctRefreshTitle`（zh: "刷新此账号的活动资格与可领套餐（有频率限制）" / en 对应），加入 `src/locales/zh.js`、`src/locales/en.js`。

## 兼容与回滚

- 纯新增通道 + 常量升级，不改已有接口协议；回滚 = revert 单个 commit。
- 风险点：marketing/touch 未来 schema 变化 → 解析宽松 + 静默降级，最坏情况退回"查不到活动"的现状。
- 活动频率：每账号每次手动刷新/自动轮次各 1 次 GET，量级与现有 preview 相同。

## 日志

- 复用 `flowlog`（`logs/oauth.log`，256KB 轮转）：flow=`claim`，event=`marketing-fail` / `marketing-empty` 不记（避免噪音），仅失败记 detail（HTTP 状态/错误串，不含凭据）。
