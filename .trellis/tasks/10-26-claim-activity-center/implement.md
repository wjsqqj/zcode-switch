# 执行计划：活动中心（子任务 B）

## 前置

1. 读 `.trellis/spec/backend/claim-plans.md`（§3 解析契约、§9 风控、§12 阻塞 I/O 纪律）。
2. 确认子任务 A 已完成（`refreshGate` / `invokeWithTimeout` / `#err-bar` 可用）。

## 步骤

### 1. 后端：活动元数据承载（R7）

- [ ] `claim.rs` 新增 `ActivityCampaign` / `ActivityAction`；`ClaimPlan` 增加可选 `campaign`。
- [ ] `walk_marketing_delivery` 在命中 plan 时收集 `campaign_id` / `resource_position` / 文案 / buttons。
- 验证：`cargo check`；新增 mock 用例 `marketing_payload_carries_campaign_meta`。

### 2. 后端：只读探测入口（R5）

- [ ] `claim.rs` 新增 `probe_marketing_only(...)`：只调 `marketing_touch_plans`，不写历史、不发 activation。
- [ ] `lib.rs` 新增 `activity_probe(ids)` 命令：经 `run_blocking`，跳过 `risk::is_cooling` 账号，返回 `{ items: [{accountId, name, plans}] }`。
- 验证：mock 用例 `activity_probe_only_touches_marketing`、`probe_skips_cooling_account`。

### 3. 后端：设置项（R4）

- [ ] `store.rs`：`Settings.activity_probe: Option<bool>` + getter（默认 false）、`AppState.activity_probe`。
- [ ] `lib.rs::set_behavior` 增加 `activity_probe` 参数；`cli.rs behavior` 增加同名 flag。
- 验证：`cargo check`；`--cli behavior --activity-probe on` 往返读写。

### 4. 前端：活动汇总数据与入口（R1/R2）

- [ ] `main.js` 新增 `activities`、`activityRows()`（合并 claimable / claimedState / activities，算出 `claimable|claimed|expired`）。
- [ ] 顶部新增「活动 (n)」按钮：`n=0` 时 `is-off` + 点击 toast；有值时打开弹框。
- [ ] 行内活动条点击 → `openActivityModal({ rows, focusKey })`，并补 `title` 提示。
- [ ] 时间展示 helper `timeRangeText`：开始优先 `current.starts_at`，缺失「未提供」。

### 5. 前端：探测调度与提醒（R4.1/R6/R6.1/R6.3）

- [ ] `maybeStartProbe()`：`state.activity_probe` 为真时启用调度；`probe.nextRoundAt = now + 20min + rand(0,5min)`，秒级随机。
- [ ] `pickProbeAccounts()`：过滤 `now-lastAt[id] >= 60min` 且 `done[id] < 20`；不足 2 个则放宽最小间隔；随机取 2 个。
- [ ] `scheduleProbeOne(id)`：每账号独立 `setTimeout(rand(0,5min))` 入队；`pumpProbe()` 串行执行，请求间隔 `rand(20s,90s)`；**不并发**。
- [ ] 非活跃时段（12:00–16:00）间隔取 3h；`done[id] >= 20` 后该账号降频 3h（不停止）；`done` 按本地日期重置。
- [ ] 后台（`document.hidden`）跳过；`visibilitychange` 回前台后 `rand(30s,120s)` 补一次。
- [ ] 结果写 `activities` 并 `render()`；失败静默（`logClientError`），不弹错、不打断。
- [ ] **账号级静默**：探测到有效期内活动 → 写 `localStorage["zsw-activity-known"]`（`quiesceUntil = max(endsAt)`），`pickProbeAccounts()` 跳过静默账号；`force` 可绕过（顶部「活动」按钮/手动刷新）。
- [ ] 全部账号静默时把 `nextRoundAt` 推到最近 `quiesceUntil`，不空转。
- [ ] 已见集合 `localStorage["zsw-activity-seen"]`（上限 200，FIFO）；新条目 → toast + 打开详情弹框。
- [ ] 到点提醒：由 `activityRows()` 的 `starts_at` / `effective_at` 注册 `reminders`，60s `reminderTicker` 比对触发（不用长 setTimeout），触发 toast + 打开详情；`localStorage["zsw-reminder-fired"]` 去重（上限 500）。

### 5.1 前端：到点自动领取（R6.2）

- [ ] 设置新增 `claim_at_effective`（默认 false），UI 注明「需要人工过验证码，非全自动」。
- [ ] `effective` 提醒触发且 `claim_at_effective && state.auto_claim` 时，`rand(5s,30s)` 后调 `autoClaimTick()`；进入前过 `refreshGate("global")`。
- [ ] `status` 仍为「待生效」时不得抢跑。

### 6. 前端：弹框（R3）

- [ ] `ui.js` 新增 `openActivityModal`：分组（可领/已领取/已过期）、可折叠条目、完整字段、无元数据降级文案。
- [ ] 设置页（`settings.js`）新增「活动探测」开关 + 风控说明文案。

### 7. 收口

- [ ] i18n 新增键（`act.*`、`s.activityProbe*`、`m.activityNew`、`m.activityNone`、`act.notProvided`）zh/en 同步。
- [ ] `cargo test` 全绿、`npm run check:i18n`、`vite build`。
- [ ] spec 沉淀：`claim-plans.md` 新增 §13「活动元数据与探测纪律」。
- [ ] 便携版重建并交用户真机验收（基准：`ZCODE GLOBAL BUILD` 19:15:53 → 10-27 09:00）。

## 回滚点

- 步骤 1-3 为后端新增字段/命令，可独立回退且不影响既有领取。
- 步骤 4-6 为前端新增路径，删除 `activities`/按钮/弹框即可回到现状。
