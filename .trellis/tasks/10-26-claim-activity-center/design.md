# 设计：活动中心（子任务 B）

## 1. 边界

- 后端：`claim.rs`（活动元数据解析 + 探测入口）、`store.rs`（设置字段）、`lib.rs`（命令）、`risk.rs` 只读复用。
- 前端：`main.js`（状态/定时器/按钮/汇总数据）、`ui.js`（`openActivityModal`）、`settings.js`（开关）、`locales/{zh,en}.js`。
- 不改：领取协议、`collect_claimable_plans` 现有语义、`claimHold`、风控参数。

## 2. 数据承载（不破坏既有契约）

```rust
// claim.rs
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCampaign {
    pub campaign_id: String,
    pub position: String,          // "banner" | "popup" | ""
    pub title: String,             // 取 banner/popup 的 title/text 字段，缺失则空
    pub body: String,              // 描述文案（宽松扫描首个长字符串字段）
    pub buttons: Vec<ActivityAction>,
    pub starts_at: Option<i64>,    // ms；payload 未提供则 None
    pub ends_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityAction { pub kind: String, pub plan_id: Option<String> }

pub struct ClaimPlan {
    // ... 既有字段保持不变
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<ActivityCampaign>,
}
```

- 解析位置：`walk_marketing_delivery` 已递归扫描 delivery，命中 `plan_id` 时把当前 delivery 的 `campaign_id` / `resource_position` / banner/popup 文案 / buttons 一并塞进 `campaign`。
- `billing/preview` 通道产生的 plan 无 `campaign`（None），前端按「无详情元数据」降级展示。
- 兼容性：新增可选字段，老前端忽略；反序列化端不加 `deny_unknown_fields`，安全。

### 2.1 时间语义（实测修正，2026-10-26）

`billing/current` 的 `data.plans[]` 里三类时间必须分开：

| 字段 | 含义 | 样本 |
|---|---|---|
| `plan.starts_at` | 活动/计划开始 | 2026-10-26 19:15:53 |
| `entitlements[].effective_at` | **真实生效时间** | 2026-10-26 23:00:00 |
| `plan.ends_at` | 结束 | 2026-10-27 09:00:00 |

- 因此 `ClaimGrant` 需增加 `effective_at: Option<i64>`（秒×1000 转 ms），`ActivePlan` 增加 `effective_at`（取 `entitlements` 中最早的非空值）供 UI 直接用。
- UI 展示三段：`活动开始` / `待生效`（`effective_at > now` 时高亮）/ `结束`；缺哪个就标「未提供」。
- 禁止再用 `starts_at` 当「生效时间」——这是本任务修正的既有认知错误（spec `claim-plans.md` §10 已同步）。

## 3. 探测通道

```rust
// lib.rs
#[tauri::command]
async fn activity_probe(ids: Vec<String>) -> Result<ActivityProbeResult, String>;
// claim.rs
pub fn probe_marketing_only(home, creds, config, mid) -> Vec<ClaimPlan>;   // 只走 marketing/touch
```

- `activity_probe` 对每个 id：跳过 `risk::is_cooling` 的账号；调用 `probe_marketing_only`；**不**调用 `report_activation_events`，**不**写 `claim-history.json`。
- 失败静默：沿用 `marketing_touch_plans` 的 `flowlog::log("claim","marketing-fail",..)` + 返回空。
- 账号选择在前端做（随机 2 个），后端只执行传入的 ids，便于测试与复用。
- 全部经 `run_blocking`（§12.1 纪律）。

## 4. 前端状态

```js
let activities = {};                 // accountId -> ClaimPlan[]（含 campaign 元数据）
let probeTimer = null;
const PROBE_INTERVAL_MS = 30 * 60 * 1000;
const PROBE_FIRST_DELAY_MS = 3 * 60 * 1000;
const PROBE_ACCOUNTS = 2;
const SEEN_KEY = "zsw-activity-seen";   // localStorage: ["acct|campaignId|planId", ...]
```

- `activityRows()`：把 `activities` + `claimable` + `claimedState` 合并成统一条目 `{accountId, name, planId, campaignId, title, startsAt, endsAt, grants, status}`，`status ∈ claimable | claimed | expired`（`ends_at < now` 或 current 无该 plan 且历史有 → expired）。
- 顶部按钮 `活动 (n)`：`n = status==='claimable'` 的条目数；`n===0` 加 `is-off`，点击 toast 说明（沿用 A 的「不用 disabled」纪律）。
- 定时器：`maybeStartProbe()` 在 `state.activity_probe` 为真时挂 30 分钟 ticker（首次 3 分钟后）；账号来自 `state.accounts` 随机抽样 2 个；请求走 `invokeWithTimeout("activity_probe", { ids })`；结果写 `activities` + `render()`。
- 提醒：新条目（`SEEN_KEY` 未命中）→ `toast(...)` + `openActivityModal({ focusKey })`；随后写入已见集合（`Set` 上限 200 条，FIFO 淘汰）。

### 4.1 探测调度（防脚本特征）

```js
const PROBE = {
  baseMs: 20 * 60_000,       // 活跃窗口内轮次基准 20min
  jitterMs: 5 * 60_000,      // 轮次抖动：20min + U(0,5min)
  acctDelayMs: 5 * 60_000,   // 每账号独立随机延迟 0–5min
  acctMinGapMs: 60 * 60_000, // 账号级最小间隔（6 账号轮转 ≈ 67min/账号）
  reqGapMs: [20_000, 90_000],// 串行请求间隔
  activeWin: [16, 12],       // 活跃窗口 16:00 → 次日 12:00（跨天）
  lowGapMs: 3 * 3600_000,    // 非活跃时段（12:00–16:00）间隔
  perAcctDaily: 20,          // 每账号每日配额（全局 ≈106 请求/天）
  quotaGapMs: 3 * 3600_000,  // 达配额后降频间隔（不停止）
};
let probe = { nextRoundAt: 0, queue: [], running: false, lastAt: {}, done: {} };
```

- **轮次**：`nextRoundAt = now + base + rand(jitter)`，秒级随机；若下一轮落在非活跃时段（12:00–16:00）则 `max(该值, now + lowGapMs)`。
- **账号选择**：从 `state.accounts` 过滤 `now - lastAt[id] >= acctMinGap` 且 `done[id] < perAcctDaily`；不足 2 个则放宽 `acctMinGap`（下限 60min）；随机取 2 个。
- **下发**：每个账号各自 `setTimeout(rand(0, acctDelayMs))` 进队列，`pumpProbe()` 串行执行，两次请求间 `rand(reqGapMs)`；保证不并发。
- **配额**：`done[id] >= perAcctDaily` 时该账号改为 `quotaGapMs` 间隔（即 3h），**不停止**；`done` 按本地日期重置。
- **前台判定**：`document.hidden` 为真时跳过本轮；`visibilitychange` 回前台后随机 30–120s 补一次。
- **账号数影响**：账号 < 3 个时，每轮都会抽中同一批账号 → 被 `acctMinGapMs` 挡住，实际退化为 60min 一次（预期行为，更安全）。
- 所有请求走 `invokeWithTimeout("activity_probe", { ids: [one] })`，失败仅 `logClientError`，不弹错。

### 4.2 到点提醒

```js
let reminders = [];   // { key, accountId, planId, kind: 'start'|'effective', at }
const FIRED_KEY = "zsw-reminder-fired";  // localStorage，上限 500
```

- 数据源：`activityRows()` 里的 `starts_at` 与 `effective_at`；`at > now` 的才注册。
- 触发：复用 60s ticker（`refreshTicker` 之外独立 `reminderTicker`），比对 `Date.now()`；**不用长 `setTimeout`**，避免休眠后漂移。
- 触发动作：`toast` + `openActivityModal({ focusKey })`；写入 `FIRED_KEY` 去重（key = `ackId|planId|kind`）。
- 提醒文案区分：`act.startReminder`（活动开始，可去抢）/ `act.effectiveReminder`（额度已生效）。

### 4.3 到点自动领取

- 新设置 `claim_at_effective`（默认 false）；UI 注明「需要人工过验证码，非全自动」。
- 触发条件：`claim_at_effective && state.auto_claim` 且收到 `effective` 提醒 → `setTimeout(rand(5s,30s))` 后调 `autoClaimTick()`。
- 进入前先过 `refreshGate("global")`；失败路径复用自动领取既有逻辑（验证码窗口、`claimHold`）。
- 不在 `effective_at` 之前抢跑（`status` 仍为待生效时不发起领取）。

### 4.4 账号级静默（quiesce）

```js
const KNOWN_KEY = "zsw-activity-known";
// { [accountId]: { planId, campaignId, endsAt, effectiveAt }[] }  仅存仍在有效期的条目
```

- **进入静默**：某账号本次探测结果里存在 `endsAt > now || effectiveAt > now` 的条目 → 记录该账号的 `quiesceUntil = max(endsAt)`（若只有 `effectiveAt` 无 `endsAt`，则 `quiesceUntil = effectiveAt + 24h` 兜底）。
- **选择账号时过滤**：`pickProbeAccounts()` 跳过 `now < quiesceUntil[id]` 的账号（除非 `force=true`）。
- **自动恢复**：`quiesceUntil` 到期后条目自动失效（读取时过滤 `endsAt > now`），无需额外定时器。
- **手动优先**：顶部「活动」按钮打开弹窗时、以及用户点手动刷新时，传 `force` 绕过静默（用户显式意图）。
- **持久化**：写 `localStorage[KNOWN_KEY]`，上限 50 条（按 `endsAt` 淘汰最旧）；重启后据此继续静默，避免启动即全量探测。
- **与提醒的关系**：静默只影响「探测」，不影响已登记的 `starts_at` / `effective_at` 提醒——提醒由 `reminders` 独立驱动。
- **全部账号静默时**：调度器直接把 `nextRoundAt` 推到最近的 `quiesceUntil`，不空转。

## 5. 弹框

```js
// ui.js
export function openActivityModal(m)  // m: { rows, focusKey }
```

- 结构：标题「活动」+ 三个分组（可领 / 已领取 / 已过期，空分组隐藏）+ 每条为可折叠项；展开后显示 R3 全字段；无 `campaign` 元数据的条目显示「活动详情未提供（仅可领信息）」。
- 时间窗格式：`2026-10-26 19:15 → 10-27 09:00`；任一缺失显示「未提供」。
- 关闭：点击遮罩 / Esc / 关闭按钮，参考 `openProviderModal` 实现，不阻塞主界面。
- 行内活动条点击：`openActivityModal({ rows: activityRows(), focusKey })` 并自动展开对应项。

## 6. 设置

- `Settings` 增加 `activity_probe: Option<bool>`（`#[serde(default)]`），`set_behavior` 增加同名可选参数；默认 `false`。
- `AppState` 增加 `activity_probe: bool` 供前端读取。
- 设置页新增开关行，文案含风控说明（30 分钟 / 2 个账号 / 只读探测）。

## 7. 测试

- `mock_upstream.rs`：
  - `activity_probe_only_touches_marketing`：mock 只挂 `/marketing/touch`，断言无 `/billing/claim` 命中、无 history 文件写入。
  - `marketing_payload_carries_campaign_meta`：payload 含 `campaign_id` + banner 文案 + buttons → 解析出的 `campaign` 字段完整。
  - `probe_skips_cooling_account`：预置 risk 冷却 → `activity_probe` 不发请求（hits==0）。
- 前端：`check:i18n` + `vite build`；`openActivityModal` 手工走查（无单测框架）。

## 8. 回滚

- 前端：`activities` / 按钮 / 弹框为新增路径，回滚删新增代码即可，不影响可领列表。
- 后端：`ClaimPlan.campaign` 为可选字段，回滚只需停用 `activity_probe` 命令与设置项。
