# Claim Plans Discovery — billing/preview + marketing/touch

## 1. Scope / Trigger

ZCode 服务端把「可领套餐」分两条通道下发，工具必须双通道合并才能看到全部可领套餐：

- `GET /api/v1/zcode-plan/billing/preview` — 常规可领套餐。
- `GET /api/v1/marketing/touch?seq=0` — 营销活动套餐（如 2026-10 "ZCode 1024 Dev Day"，plan_id `zcode-v3-start-plan-1024`）。**billing/preview 不返回此类活动**；漏掉该通道 = 「刷新资格查不到新活动」。

任何新增可领套餐查询入口（GUI 命令、CLI、自动领取）都必须走 `claim::collect_claimable_plans()`，不要直接调 `preview_plans()`。

## 2. Signatures

```rust
// src-tauri/src/claim.rs
pub fn collect_claimable_plans(home, creds, config, device_mid: String) -> Result<Vec<ClaimPlan>, String>;
    // preview 错误上抛（保持既有报错体验）；marketing 失败静默为空
pub fn marketing_touch_plans(home, creds, config, device_mid: Option<String>) -> Vec<ClaimPlan>;
pub fn merge_marketing_plans(base: Vec<ClaimPlan>, extra: Vec<ClaimPlan>) -> Vec<ClaimPlan>;

// src-tauri/src/quota.rs
pub(crate) fn zcode_app_version() -> String; // = max(注册表探测版本, CLIENT_APP_VERSION)
pub const CLIENT_APP_VERSION: &str;          // 最新已知 ZCode 客户端版本（当前 3.16.3）
```

## 3. Contracts

### marketing/touch 请求

- Method: `GET https://zcode.z.ai/api/v1/marketing/touch?seq=0`
- Headers（对齐客户端，`zai_billing_headers_with_mid` 为其超集）：`X-Device-Mid`（UUID）、`X-Client-Language: zh-CN`、`X-ZCode-App-Version`、`Authorization: Bearer <zcodejwttoken>`（可匿名，但工具总是带账号 token）。
- **版本门槛**：服务端按 `X-ZCode-App-Version` 过滤活动下发（实测 3.11.2 → 空 deliveries，3.16.3 → 返回活动）。上报版本必须取 `max(本机探测版本, CLIENT_APP_VERSION)`，见 `quota::pick_app_version`。

### marketing/touch 响应

```jsonc
{ "code": 0, "data": { "server_time": u64, "language": "zh-CN",
    "deliveries": [ {
      "campaign_id": "...", "priority": 50,
      "resource_position": "banner" | "popup",
      "banner"/"popup": {
        "buttons": [ { "action": { "type": "claim_zcode_plan", "args": { "plan_id": "..." } } } ],
        "background"/"hero": { "args": { "zcode_plan": { /* 与 preview plans 同构：
          plan_id/name/priority/entitlements[show_name/meter/unit_type/grant_units/period] */ } } } } } ] } }
```

- 解析为**递归宽松扫描**（`walk_marketing_delivery`）：任意层级含非空 `plan_id` 且带 `entitlements` 数组或 `name` 字符串的对象 → `parse_plan()`；按钮 `action.type ∈ {claim_zcode_plan, claim_plan}` → 可领 plan_id（兜底 `action.planId`）。不做严格 schema——服务端 payload 位置可能变化。
- 按钮有 plan_id 但无内嵌 payload → 合成最小条目（name=plan_id），保证可见可领。

### 领取

活动套餐领取复用 `billing/claim`（`submit_claim` 不变）：`POST {plan_id}` + 阿里云验证码头。`claim_start` 的 plan 存在性校验必须基于 `collect_claimable_plans` 结果。

## 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| preview 网络错误/HTTP 非 0 | `Err` 上抛（claim_preview/claim_refresh/claim_start 现有报错链路） |
| preview **HTTP 404** | `Ok(vec![])` 静默（活动未上线属正常态，2026-11-03 起；见 §10.4） |
| preview 报错 + marketing 有货 | **降级成功**：返回 marketing 结果，`diag.preview_error` 记录原错误（2026-11-03 起） |
| preview 报错 + marketing 也空 | `Err` 上抛（不把故障吞成「没有活动」） |
| marketing 网络/code!=0/JSON 解析失败 | `flowlog::log("claim", "marketing-fail", detail)` + 返回 `[]`，**不弹 toast、不影响 preview 结果** |
| 本机无 ZCode 客户端（注册表探测失败） | 上报 `CLIENT_APP_VERSION`（3.16.3） |
| 本机客户端版本 < 常量 | 上报常量；> 常量 → 上报真实版本 |
| 活动已领取 / 不在有效窗口 | 服务端不下发或 claim 返回错误码，前端走现有错误提示 |

## 5. Good/Base/Bad Cases

- **Good**: preview 3 个常规套餐 + marketing 1 个活动套餐 → 合并 4 个，按 priority 降序，无重复 plan_id。
- **Base**: marketing 返回空 deliveries（版本过旧/无活动）→ 仅 preview 结果，正常展示。
- **Bad**: 营销活动只在 marketing 通道有 payload，却只调 `preview_plans` → 活动对用户不可见（本任务修复前的 bug）。

## 6. Tests Required

仓库无单测框架；验证路径：

- `cargo check --manifest-path src-tauri/Cargo.toml` 0 警告。
- CLI 冒烟：`zcode-switch.exe --cli claim-preview` → 未领取活动的账号应出现 `zcode-v3-start-plan-1024` 类条目（grant_items 渲染 GLM-5.3-Flash 3亿 等）。
- 失败降级：断网/改 URL 常量后 CLI 仍应正常输出 preview 结果且 `logs/oauth.log` 出现 `[claim] marketing-fail`。

## 7. Wrong vs Correct

### Wrong

```rust
// 只查 preview —— 营销活动永远不可见
let plans = claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))?;
```

### Correct

```rust
// 双通道合并：preview（错误上抛）+ marketing/touch（静默降级）
let plans = claim::collect_claimable_plans(&paths.home, &acc.credentials, acc.config.as_ref(), mid)?;
```

---

## 8. 领取错误语义与重试纪律（2026-10-24 补充，任务 10-24-claim-hardening）

### ClaimError 语义方法

```rust
pub const CLAIM_ERR_CHALLENGE: i64 = -2;
impl ClaimError {
    pub fn is_captcha_rejected(&self) -> bool { self.code == 3007 }      // 可换新参数重试一次
    pub fn is_terminal(&self) -> bool { (1001..=1005).contains(&self.code) } // 不再重试
    pub fn is_challenge(&self) -> bool { self.code == CLAIM_ERR_CHALLENGE }  // 人机校验拦截，非凭证失效
}
```

- HTTP 401/403 携带 `x-aliyun-captcha-verify-param` 头或 body 含 captcha 字样 → `-2` 挑战（zcode2api 教训：当登录过期处理会错杀健康账号）。`failure_payload` 结构化输出 `terminal`/`challenge`，前端不得用 code 区间反推。
- 3007 **不自动重试**（2026-10-26 变更，吸收 zcode2api 2.6.5 整改结论）：拒码本身是上游风险信号，换码连打只会加剧。`claim_captcha_submit` 收到 3007 直接走终态失败并记 `flowlog` `captcha-rejected-3007`，由用户重新发起领取（重新取参数）。`captcha_attempt` 字段与 `{retry:true}` 协议已移除。
- 前端自动轮 (账号,套餐) 级 `claimHold`：终态 Infinity、1005 至 nextAt（无则 1h）、挑战 10min。

### 验证码配置

`fetch_captcha_config()` 进程内缓存 600s；失败回退上次成功值；再无 → 内置默认 `prefix=no8xfe / sceneId=11xygtvd / region=cn`（zcode2api 与 plan-claim 观测一致）。测试用 `reset_captcha_config_cache_for_tests()` 清缓存。

## 9. 风控退避（2026-10-24 补充，任务 10-24-risk-quota-guard）

`risk.rs`：3012/unusual activity 命中记 strike；900s×2ⁿ⁻¹ 指数退避封顶 24h；7 天窗口内累计 4 次升级 24h；窗口外 strike 衰减不累计；到期自动恢复。持久化 `risk-state.json`（仅时间戳）。

- 冷却期内自动路径零上游流量：`get_account_quota` / `claim_refresh` / `claim_start(auto)` 直接拒绝；手动操作经 `force=true` 放行（用户显式意图优先）。
- 观测注入点：所有上游 Err 经 `risk::observe_upstream_failure` 过滤（仅 3012/unusual/风控字样记 strike，网络抖动不误伤）。

## 10. 已领取状态（2026-10-24 补充，任务 10-24-claimed-state）

- `fetch_active_plans` → `GET billing/current`（`data.plans[{user_plan_id,plan_id,name,status,starts_at,ends_at,entitlements}]`，秒级时间戳×1000 转 ms）。
- **诊断命令**：`--cli claim-current --id` 查已生效套餐 + 本地历史；`--cli claim-raw --id [--only preview|marketing|current]` 原样打印上游 JSON（排查「为什么这个账号看不到活动」）。实测：服务端对无资格账号两通道均返回 `code=0` 且数组为空，**不提供原因字段**，本地不得据此编造原因。
- preview 为空 ≠ 无活动（可能已领取）——用 current + 历史区分。

### 10.1 生效时间契约（2026-11-03 修正，任务 11-03-claim-effective-time-display）

**时间语义（再次强调，2026-10-26 首次实测、2026-11-03 因线上回归重写）**：`plan.starts_at` 是活动/计划开始时间，**永远不是生效时间**；真正生效时间在 `entitlements[].effective_at`（样例：`ZCODE GLOBAL BUILD` 的 `starts_at=19:15:53`、`entitlements[0].effective_at=23:00:00`、`ends_at=次日 09:00`）。

**签名**

```rust
// claim.rs
pub struct ClaimPlan { /* … */ pub effective_at: Option<i64> }  // ms；0 = 立即生效；None = 未下发
pub(crate) fn entitlements_effective_at(p: &Value) -> Option<i64>; // entitlements[].effective_at 取 min ×1000
// store.rs
pub(crate) fn record_claim_history(paths, account_id, user_id: Option<&str>, plan_id, plan_name,
    starts_at: Option<i64>, ends_at: Option<i64>, effective_at: Option<i64>);
```

**契约**

- 三个入口（`billing/claim` 领取响应、`billing/current`、`marketing/touch` 内嵌 `zcode_plan`）**共用** `entitlements_effective_at()`，禁止任一处再内联 `min()`。
- `parse_entitlements()` 只负责额度展示（按 `meter/unit_type/show_name` 过滤），**不能**用它取生效时间。
- `claim-history.json` 每条：`{accountId, planId, planName, claimedAt, startsAt, endsAt, effectiveAt}`（全部 ms）。老文件只有前四个字段，读取端必须容忍缺失。
- 前端唯一语义实现：`src/claim-time.js`（`fmtDateTime` / `parseEffectiveFromCopy` / `resolveEffective` / `rangeOf` / `effectiveDetail`）。`main.js` 与 `ui.js` 禁止各写一份时间格式化或 `effectiveAt ?? startsAt`。
- 取值优先级：`effective_at` 字段 → 活动正文解析（仅含「生效」的句段、取「生效」前最近时间）→ `null`。文案兜底必须在 UI 标注「（据活动说明）」。
- 时间格式统一 `YYYY-MM-DD HH:mm`（带年份）。
- `claim-current` 在本机实测可返回 `{"code":0}` **无 `data`**：已领套餐可能尚未进入 current，此时唯一来源是本地历史/持久化目录。

**验证矩阵**

| 条件 | 行内/汇总条 | 详情「生效时间」 |
|---|---|---|
| `effective_at` 存在且 > now | `待生效 X → 至 Y` | `X` |
| `effective_at` 存在且 ≤ now / `0` | `生效 X → 至 Y` / `立即生效` | `X` / `立即生效` |
| 只有文案解析值且 > now | `预计生效 X → 至 Y` | `预计 X 生效（据活动说明）` |
| 都没有，但有 `starts_at` | `活动 A → 至 Y`（**不得写「生效」**） | `领取后确认` |
| 历史记录缺时间 | `已领取过 X（时间未记录）` | — |

**测试**

- `mock_upstream::marketing_plan_parses_entitlement_effective_at`：多 entitlement 取 min、×1000、与 `starts_at` 无关。
- `mock_upstream::marketing_plan_without_effective_at_stays_none`：缺失必须是 `None`，不得回退 `starts_at`。
- `mock_upstream::current_parses_active_plans`：current 的 `effective_at` 取自 entitlement。
- `store::tests::claim_history_records_times_and_keeps_legacy_entries`：老记录保留 + 新字段落盘。
- `scripts/test-claim-time.mjs`（`npm test`）：格式、文案解析边界（非法日期、开始/生效同句、无年份跨年）、取值优先级、三态判定。

**Wrong / Correct**

```js
// Wrong：把活动开始当生效时间，线上表现「生效 11/03 19:00」，而真实生效是 23:00
const startMs = p.effective_at ?? p.starts_at ?? null;
t("act.timeRange", { start: fmt(startMs), end: fmt(p.ends_at) });

// Correct：生效只认 effective_at，starts_at 单独走「活动」文案
const g = rangeOf({ startsAt, endsAt, effectiveAt: eff.at, effectiveSource: eff.source });
t(ACT_RANGE_KEYS[g.kind], { start: fmt(g.startMs), end: fmt(g.endMs) });
```

> **Warning**：`effective_at` 的 `0` 是合法值（立即生效），前端判断必须用 `!= null`，不能用真值判断。

### 10.2 领取历史归属与重登兼容（2026-11-03 补充）

**问题（实测）**：退出后重新登录会生成**新的 accountId**（同一用户 `52a7232d…` → `9907b4bb…`），历史只按 `accountId` 查会导致「已领取记录凭空消失」，表现为「重登后刷不到活动」。

**契约**

- 落盘字段：`{accountId, userId, planId, planName, claimedAt, startsAt, endsAt, effectiveAt}`；`userId` 取自 `zcrypto::account_identity(creds, home).user_id`（oauth `user_info.id` 或 JWT `sub`）。
- 读取匹配：`accountId` **或** `userId` 命中即返回；任一侧为空/缺失时只走 `accountId`（老文件天然兼容）。
- 所有调用点（`claim_current`、`--cli claim-current`、冷却分支）都必须传 userId，漏传即退化回旧行为。

**为什么不做「按 planId 全局匹配」**：同一活动会被多个用户领取，按 planId 匹配会把别人的记录算到本账号头上。

**验证**：`store::tests::claim_history_survives_relogin_by_user_id`（同 userId 不同 accountId 必须命中；无 userId 的老记录不得被其他账号命中）。

### 10.2.1 余额推断「已领取」（2026-11-03 补充，任务 claimed-inference-by-tier）

**实测**（新账号 fbnmmsij vs 已领账号旅行者2151）：

| 账号 | `billing/current` | `billing/balance` |
|---|---|---|
| 未领取 | 空 | `is_empty=true, plan_tier=null, source=no_plan` |
| 已领取（权益未生效） | **空（无 data）** | `plan_tier="Start Plan"`, `tier_code="start"`, `plans[0].name="ZCODE GLOBAL BUILD"` |

- 领取后活动从 `marketing`/`preview`/`current` 全部消失，但 **balance 仍保留活动套餐名与 `start` 等级**；官方 3.17.2 客户端显示「已领取过」正是靠它（3.17.2 asar 的 API 路径与 3.16.3 完全一致，没有新接口）。
- 前端契约：`fetchClaimed` 在 `current.plans` 为空时调用 `get_account_quota`，只采纳 `tier_code == "start"`（或 tier 含 start）的套餐，存 `claimedState[id].quotaPlans`；10 分钟节流（`hintAt`），失败静默。
- 渲染：活动面板补 `status=claimed` 行（无时间），行内条显示「已领取（待生效）」；**不写入** `claim-history.json`（推断不得污染真实历史）。
- 禁止把非 start 套餐（Coding/Pro）当活动记录；也禁止用它阻断领取（服务端仍是唯一裁决者）。

---

### 10.3 活动下发可见性：版本与空态诊断（2026-11-03 补充）

**实测结论**（3 个账号 + 官方 token/deviceMid 对照）：`marketing/touch` 与 `billing/preview` 在「已领取 / 无资格」时都返回 `code=0` 且空数组，**不提供原因**；`billing/current` 在权益生效前也不返回待生效套餐。所以「刷不出来」必须在本地给出可解释信息，而不是静默空白。

**版本链路**

| 项 | 值 / 行为 |
|---|---|
| `CLIENT_APP_VERSION` | `3.17.2`（2026-11-03 官方最新；常量必须随官方发版更新） |
| changelog TTL | `6h`（原 24h；活动按客户端版本下发，版本越新越好） |
| 保鲜入口 | `claim_refresh` / `claim_preview` / `activity_probe` / `version_status` 调 `changelog::ensure_fresh(home)` |
| 保鲜纪律 | `ensure_fresh` **只 spawn 后台线程**（进程内 60s 节流），请求线程零网络 IO（见 §12.1） |
| 上报版本 | `max(本机探测, CLIENT_APP_VERSION, changelog 缓存)`；因此 reported 不可能小于 latest |

**`VersionStatus`（camelCase 序列化）**

```rust
pub struct VersionStatus {
    pub reported: String,        // 实际上报
    pub latest: Option<String>,  // changelog 缓存
    pub stale: bool,             // 缓存缺失/超 6h → reported 可能落后
    pub cache_age_secs: Option<u64>,
    pub client_constant: String,
    pub constant_behind: bool,   // 常量 < latest → 该发新版了
}
```

- `claim_refresh` 返回体新增 `version`；UI 在 `stale || constantBehind` 时**每会话提示一次**（不刷屏）。
- 空态文案必须包含：上报版本 / 官方最新 / `stale` 或 `constantBehind` 的具体原因 / 「官方客户端领取的记录本工具无法同步」。
- `--cli claim-raw` 输出 `_meta`：`appVersion / latestVersion / versionStale / constantBehind / userAgent / tokenSource(zcodejwt|oauth|none) / hasZcodeJwt / deviceMidMasked / accountName`，**禁止输出 token 明文**。

### 10.4 活动投放窗口与 404 语义（2026-11-03 补充，参考 zcode2api 2.7.10）

- **投放窗口**：官方活动/套餐按天投放，通常北京时间 **19:00 后放量**（zcode2api 用户侧情报；2026-11-03 实测 19:42 仍可 preview 到当天活动，领取后转空）。19:00 前刷空属正常态，活动面板空态必须说明。
- **preview 404**：活动未上线时上游返回 404 → `preview_once` 返回 `Ok(vec![])`，不得上抛。`raw_preview`（`--cli claim-raw`）保持原样，便于查看真实响应。
- **禁止扩散**：404 静默只允许存在于 preview 读路径；`submit_claim` 的任何失败必须报错。
- **渠道值**：`X-Release-Channel = production`（官方 bundle 实测）；`stable` 是 3.11.2 时代遗留值。

### 10.5 诊断日志与导出（2026-11-04 补充，任务 activity-diagnostics-export）

**为什么**：用户反馈「别人刷不出活动、我这边能刷出」。若只有失败日志，两端无法对比；必须把**成功路径**也落成可对比的结构化行。

**日志格式**（`flowlog`，`[claim]` 流）

```
2026-11-04 00:15:41 [claim] refresh acct=4b4d0629… ver=3.17.2 token=zcodejwt preview=200/0/0 marketing=200/0/1 transport=curl ms=1888 plans=1
2026-11-04 00:15:41 [claim] probe   acct=4b4d0629… plans=1 status=200 code=0 transport=curl
```

- 字段含义：`预览/活动 = HTTP状态/业务码/条数`；`token` = `zcodejwt|oauth|none`；`transport` = `curl|ureq`。
- 采集点：`claim_refresh`（GUI/CLI 刷新）、`activity_probe`（探测）、`--cli claim-preview`。
- **只写数字与枚举**，不写 URL、文案、令牌。

**导出**

```rust
pub fn diag::build_text(paths: &Paths, account: Option<&str>) -> String;
// Tauri: diagnostics_json(account) / export_diagnostics(app, account) → 保存对话框
// CLI:   --cli diag [--id <id>]  → 直接打印
```

内容：`{schema, notice, generatedAt, app{version,os,arch,language}, version{...}, accounts[{id(masked),name,history}], recentRefresh[], flowlogTail[], truncated}`；总量 >512KB 时清空 `flowlogTail` 并置 `truncated:true`。

**脱敏（双保险）**

1. 采集阶段：id 只写前 8 位，`deviceMid` 从不进入日志；
2. 导出阶段 `mask_secrets()`：`Bearer <token>` → `Bearer ***`，≥32 位 hex/uuid → 前 8 位 + `…`。

**测试**：`diag::tests::mask_hides_tokens_and_long_ids`、`mask_keeps_normal_text`。

**Wrong / Correct**

```rust
// Wrong：把令牌/完整 id 写进日志
flowlog::log("claim", "refresh", &format!("token={token} mid={mid} acct={id}"));

// Correct：只写枚举 + 掩码 + 数字
flowlog::log("claim", "refresh", &diag::refresh_line(&id, &ver, token_source, &diag, plans.len()));
```

## 11. 版本链与测试旁路（2026-10-24 补充，任务 changelog-version / mock-upstream-tests）

- 上报版本 = `max(注册表探测, 安装包内置快照, changelog 缓存最新, CLIENT_APP_VERSION 常量)`（四层，2026-11-03 增快照）。
  - 内置快照：`src-tauri/resources/known-latest.json`（`{version, generated_at}`），由 `scripts/dist.mjs` 在每次打包前刷新（优先本机 changelog 缓存，其次线上页面），随安装包分发；运行时按 `exe 同目录 → resources/` 查找，坏 JSON/缺失一律 None。用途：换机/缓存被清/首次离线时不再退回旧常量。
  - changelog 缓存 `{home}/.zcode-switch/changelog-cache.json`（**6h TTL**（2026-11-03 由 24h 收紧））。
  - 保鲜时机：启动线程内若缓存**缺失**则同步拉一次（3s 硬超时），否则走 TTL；每次刷新活动 `ensure_fresh` 在缓存**缺失**时同步等一次（2s 硬超时），**过期仍走后台**（60s 节流）——禁止每次刷新都等网络。
  - 版本提取只认 `Release v3.x.y` / `releases/3.x.y/` 锚点（页面 SVG path 坐标与千分位数字会产生 57.787.384 类假阳性）。
  - 诊断：`VersionStatus.bundled` 与 `--cli claim-raw` 的 `_meta.bundledVersion` 指明上报值是否来自快照。
- 测试旁路：`ZCODE_API_BASE`（claim 全部端点）、`ZCODE_CHANGELOG_URL`；URL 由 `claim.rs` 的 `billing_*_url()/client_configs_url()/marketing_touch_url()` 函数拼出，禁止再引入硬编码 URL 常量。
- mock 测试：`src-tauri/tests/support/mod.rs`（TcpListener 最小 HTTP，非阻塞 accept + 轮询 shutdown，否则 Drop join 死锁）；用例在 `tests/mock_upstream.rs`，env 相关用例必须持 `ENV_LOCK` 串行。

---

**历史**: 2026-10-24 新增（marketing/touch 接入）。同日补充 §8-§11（吸收 zcode-proxy/zcode2api/zcode-plan-claim 经验的五任务批次）。2026-10-26 补充 §12（刷新状态机与阻塞 I/O 纪律）、§13（请求头坑位表与冷却门禁）。根因排查与协议还原记录见 `.trellis/tasks/archive/`。

---

## 13. 请求头坑位表与冷却门禁（2026-10-26 补充，吸收 zcode2api / zcode-plan-claim）

### 13.1 请求头 → 错误码对照（排障查表，别再猜）

| 症状 | 最可能缺失/错误的头 | 说明 |
|---|---|---|
| 3007 验证码失败 | `X-Aliyun-Captcha-Verify-Region` 缺失/与参数不同源 | 参数与地区必须成套；参数还要求「原样 JSON 字符串」，被二次序列化同样报 3007 |
| 3001 参数错误 | `X-Platform=win32` 之类平台值与客户端不一致 | `billing/preview` 容错、`client/configs` 直接拒 |
| preview 恒空 / 无活动 | `X-ZCode-App-Version` 落后 | 版本门槛会过滤活动；必须取 `max(本机探测, 最新已知)` |
| preview 404（活动未上线） | 正常态 | 静默为空，**不要**渲染成「刷新失败」；见 §10.4 |
| 活动 19:00 前刷不到 | 正常态 | 官方按天投放，通常北京时间 19:00 后放量；见 §10.4 |
| 指纹与官方客户端不一致 | `X-Release-Channel` 值过时 | 官方为 `production`（`stable` 是 3.11.2 时代遗留） |
| 401 空体 | 用了 oauth access_token 而非 `zcodejwttoken` | 领取链路只认 `zcodejwttoken` |
| 3012 / unusual activity | 出口 IP 与验证码求解不一致、请求过于规律 | 见 §9 退避；求解与 claim 必须同出口 |
| 幂等异常 | `X-Device-Mid` 每次变化 | 每账号必须稳定复用同一 mid |

必须成套发送（`preview_requests_carry_identity_headers` 用例锁定）：`Authorization`、`X-Device-Mid`、`X-ZCode-App-Version`、`X-Platform`、`X-Os-Category`、`X-Client-Language`、`X-Client-Timezone`、`X-Title`、`User-Agent`。

### 13.2 冷却期零上游流量（所有 billing 入口）

`risk::is_cooling` 必须在**每一个** billing 入口前判断，当前覆盖面：

| 命令 | 冷却期行为 |
|---|---|
| `get_account_quota` | 非 `force` 直接返回 `err.risk.cooling` |
| `claim_refresh` | 非 `force` 直接返回 `err.risk.cooling` |
| `claim_preview` | 直接返回 `err.risk.cooling`（2026-10-26 补齐） |
| `claim_current` | **不发上游**，只返回本地 `claim_history_for`（2026-10-26 补齐；避免刷新击穿冷却） |
| `claim_start` | 仅 `auto=true` 时拦；手动领取是显式意图，放行 |

新增任何 billing 命令时，先对照此表决定冷却期行为，不得遗漏。

### 13.3 活动中心契约（2026-10-27 补充，任务 10-26-claim-activity-center）

**数据来源与合并**

- `billing/preview` 与 `marketing/touch` 可能下发**同一个 plan_id**；preview 优先去重，但 **preview 的 plan 没有活动元数据**（`campaign_id` / `resource_position` / 文案 / 时间窗）。
  → `merge_marketing_plans` 必须在 `b.campaign.is_none()` 时从 marketing 那份补齐，否则「活动 ID / 展示位」永远显示「未提供」（已有回归用例 `merge_adopts_campaign_meta_from_marketing`）。
- 真实 payload 的文案是 `{content, format}`（format=plaintext|html）：title 取 `banner.success_popup.title`，正文取 `banner.success_popup.description` 并去 HTML；时间窗兜底取 `banner.background.args.zcode_plan.{starts_at,ends_at}`。
- 前端可领来源唯一：`claimablePlansFor(id)`（刷新结果优先 + 探测结果兜底 + 按 plan_id 去重，**排除已在 `claimedState` 或已打标的 plan**）。行内条、活动弹框、自动领取都必须用它。

**探测与静默**

- `activity_probe(ids)` 只走 `marketing/touch`，不领取、不写 `claim-history.json`、不发 activation；经 `run_blocking`；冷却账号返回在 `skipped`。
- 调度：活跃窗口 20min + U(0,5min)；每轮 2 账号、各自 U(0,5min) 延迟后**串行**（间隔 20–90s）；账号级最小间隔 60min；12:00–16:00 与达配额（每账号 20 次）后降频 3h（不停止）。
- 静默：探到有效期内活动 → 该账号静默到 `ends_at`（只约束周期探测）；**启动基线强制探测全部账号**（每账号 1 请求），并把结果写入持久化目录 `localStorage["zsw-activity-catalog"]`，用于重启后展示（否则静默 + 内存态会表现为「列表空白/无角标/无提醒」）。
- 提醒去重：可领 → 会话级（每次启动对未领活动再提醒一次）；已领取 → 持久级（只提醒一次）。

**通知可见性（Windows 便携版）**

- Toast 依赖安装版快捷方式/AppUserModelID，便携版不可靠；可靠通道为：托盘图标红点交替闪烁（持续到用户点开活动）+ 托盘右键菜单 `【新活动】…` + tooltip 前缀 + 窗口标题。
- 停止闪烁的唯一入口：打开活动详情（按钮 / 弹窗 / 托盘菜单）。

**自动领取准入**

- `autoTargets()` 的 plan 必须来自 `claimablePlansFor(accountId)`（只读 `claimable` 会让探测发现的活动永远进不了自动领取）。
- 准入四条件：`status==="claimable"` + 已开始（`startsAt/effectiveAt ≤ now`）+ 未过期（`endsAt > now`）+ 未被标记/`claimHold`/账号冷却拦住；空集 → **零上游请求**。
- 「自动领取」与「到点自动领取」共用 `runClaimTargets()`：前者 2min 首轮 + 10min 间隔，后者启动 30s 首查 + 每 60s 检查「已到点未领」。
- 成功后写 `localStorage["zsw-claim-marks"]`（`accountId|planId`）并从 `activities`/`claimable` 摘除该 plan。
- 超时取消必须用 `cancelClaimIfMine(id, planId)`（依赖 `claim_pending`），**不得**无条件 `claim_cancel`（会误取消用户手动的领取）。
- `3007/挑战` 持有期 60min（拒码是风控信号且 3007 不自动重试）。

**前端渲染纪律**

- 弹窗面板**禁止** `stopPropagation()`：所有 `click="actions.xxx()"` 依赖 document 级委托，被面板拦断后会「点了没反应」（活动弹框「领取」按钮踩过）。关闭逻辑应为 `if (e.target === mask) close()`。

### 13.4 测试侧请求规格断言

`tests/support` 的 mock 会记录每条请求的 `path/headers/body`（`MockUpstream::requests()`），用例应同时断言：

- 响应解析正确（业务语义）；
- **请求规格正确**：头成套、body 仅含允许字段（领取只有 `plan_id`）、GET 无 body；
- 安全不变量：凭证不出现在 body、只读命令不产生写请求。


---

## 12. 刷新状态机与阻塞 I/O 纪律（2026-10-26 补充，任务 10-26-claim-refresh-state-fix）

### 12.1 async 命令禁止直接跑阻塞 I/O

`claim_preview` / `claim_refresh` / `claim_current` / `claim_start` / `claim_captcha_config` / `claim_captcha_submit` / `get_account_quota` / `get_live_quota` 一律经 `lib::run_blocking(...)`（内部 `tauri::async_runtime::spawn_blocking`）执行。

**原因（实测）**：一次 `claim_refresh` 最长阻塞 2×10s（activation 事件）+ 25s（preview）+ 25s（marketing/touch）。直接在 async 命令里跑 `ureq` 会占满异步运行时 worker；用户连点 3 个账号后，`get_state` / `set_behavior` / `claim_cancel` 全部排队 → 整窗卡死、额度区永远「查询中…」。

新增任何网络/进程类命令时遵循同一纪律；`switch_to`、`kill_zcode` 因持有 `STORE_LOCK`（`MutexGuard` 非 `Send`）暂未包装，改动时须把 `store_guard()` 一并移入闭包。

### 12.2 前端刷新准入与反馈契约

- 唯一判据 `refreshGate(scope)`（`scope = "global" | accountId`），返回 `{ok}` 或 `{ok:false, kind:"running"|"cooldown", waitSec}`；`toastGate()` 负责把失败原因变成 toast。
- **禁止用 `disabled` 表达「忙/冷却」**：`disabled` 会吞掉 click，用户得不到任何提示（本次 bug 的直接成因）。改用 `is-off` class + 可点击，点击后由 gate 说明原因。
- 冷却必须可见：行内按钮角标显示剩余秒数，顶部按钮显示 `刷新 (Ns)`；`refreshTicker` 每秒重绘保证倒计时递减。
- 全局刷新显示 `done/total · 当前账号名`，账号间 5s 间隔期间不再出现「无任何更新」。
- 所有 `claim_*` / `get_account_quota` 前端调用走 `invokeWithTimeout`（60s 看门狗），超时清 `busy` 并提示，杜绝永久 busy。

### 12.3 render 异常兜底

`render()` 只做 try/catch，实际渲染在 `renderInner()`；异常时：
- 不清空页面，展示挂在 `body` 上的 `#err-bar`（可关闭），保证按钮仍可点；
- 调用 `log_client_error` 写 `{home}/.zcode-switch/client-error.log`（滚动 200 行）；
- `window.onerror` / `unhandledrejection` 同样落盘。

排障路径：用户反馈「界面卡住」→ 先读 `client-error.log`，不要再靠猜。

### 12.4 风控矩阵（3012 现场报文）

- `risk::tests::observe_hits_3012_blocked_payload`：`unusual activity` + `provider_code=3012` + `status=405` 报文 → 记 strike、首次 ~900s、二次 ~1800s。
- `mock_upstream::preview_blocked_unusual_activity_3012_surfaces_error`：同上报文经 405 返回时错误上抛且保留 `405` 与 `unusual activity` 文本（供 `observe_upstream_failure` 命中）。
- `risk` 单元测试会设置进程级 `ZCODE_SWITCH_HOME`，**必须持 `TEST_LOCK` 串行**，否则并行测试互相覆盖 home 导致偶发失败（本次一并修复）。

### 12.5 zcode.z.ai 请求层纪律（2026-11-03 补充，移植 PR#43）

**签名**

```rust
// quota.rs
pub(crate) struct HttpResp { pub status: u16, pub body: String, pub headers: Vec<(String, String)> }
pub(crate) fn curl_http(url, method, headers, body, timeout_secs) -> Result<HttpResp, String>;
pub(crate) fn ureq_http(url, method, headers, body, timeout_secs) -> Result<HttpResp, String>;
pub(crate) fn zcode_http(url, method, headers, body, timeout_secs) -> Result<HttpResp, String>;
pub(crate) fn http_status_error(prefix, &HttpResp) -> String;
```

**契约**

- 所有 `zcode.z.ai` 请求统一走 `zcode_http()`（curl 优先、ureq+native-tls 兜底）；**禁止**在 claim/quota 各写一份 `AgentBuilder` 直连该域。
- 传输层判定：2xx 但 body 为空 / 非 JSON → 视为传输失败并换通道；**4xx/5xx 属有语义响应**，交调用方（404=未投放静默、401/403=挑战）。
- 凭据只经 stdin（`curl -K -`）传递，禁止出现在 argv；`dump-header` 的临时文件读完即删。
- `open.bigmodel.cn` 额度接口保持 ureq 直连与原 429 退避，不经过弹性层。
- 版本号三级回退：注册表 `DisplayVersion` → `settings.zcode_path` 的 exe `ProductVersion` → `max(CLIENT_APP_VERSION, changelog 缓存)`。

**为什么**：WAF 会按 TLS 指纹/时段策略性返回 2xx 空 body；把「空」当「无数据」会制造假阴性（PR#43 的核心结论）。本机三栈对照未复现该差异（ureq/curl/node 同为空），弹性层属**跨环境保险**而非本地解药。

**测试**

- `quota::http_tests::curl_http_reads_local_mock`：curl 直连本地 mock，解析状态码/body/头。
- `quota::http_tests::zcode_http_rejects_empty_body`：2xx 空 body → 两通道都失败 → `Err`。
- `quota::http_tests::zcode_http_returns_404_as_response`：404 原样返回（preview 静默依赖）。
- 既有 `preview_404_is_treated_as_empty` / `preview_500_still_errors` 必须继续通过。

#### Wrong vs Correct

```rust
// Wrong：直接 ureq 打活动接口，空 body 被当成「没有活动」
let resp = ureq::AgentBuilder::new().build().get(&url).call()?;
let v: Value = serde_json::from_str(&resp.into_string()?).unwrap_or(Value::Null);

// Correct：统一弹性层，空 body 视为传输失败并换通道
let resp = quota::zcode_http(&url, "GET", &headers, None, 25)?;
if resp.status == 404 { return Ok(Vec::new()); }
if !resp.ok() { return Err(quota::http_status_error("preview", &resp)); }
let v = quota::zcode_http_json(&resp)?;
```
