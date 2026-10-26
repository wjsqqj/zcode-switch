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

## 完成记录 · 批次 1（2026-10-26）

- 步骤 1 ✅ `ActivityCampaign` / `ActivityAction` + `ClaimPlan.campaign`（可选字段）；marketing delivery 解析时把 `campaign_id`/`position`/文案/buttons/时间窗写入。
- 步骤 2 ✅ `activity_probe(ids)` 命令：只调 `marketing_touch_plans`，跳过风控冷却账号（返回 `skipped`），经 `run_blocking`，不写历史、不发 activation。
- 步骤 3 ✅ 设置项 `activity_probe` / `claim_at_effective`（默认关）+ `AppState` 暴露 + `set_behavior` 参数。
- 步骤 4 ✅ 前端 `activities` / `activityRows()`（claimable / claimed / expired）/ 顶部「活动 (n)」按钮 / 行内活动条与已领取条点击打开详情。
- 步骤 5 ✅ 探测调度：20min+U(0,5min)、账号级 60min 间隔、每账号日配额 20（达限降 3h 不停止）、12:00–16:00 降频 3h、后台不探测、账号级静默到 `ends_at`、手动打开弹框触发一次强制探测（30s 节流）。
- 步骤 6 ✅ `openActivityModal`（三分组 + `<details>` 折叠 + 完整字段 + 无元数据降级）+ 设置页两个开关。
- **时间展示补全** ✅ `claimTimeRange()`：生效优先 `effective_at`（其次 `starts_at`），展示「生效 X → 至 Y」，未提供标「未提供」；待生效条目标黄。
- 验证 ✅ `cargo test`（lib 8 + integration 14，含新增 `marketing_payload_carries_campaign_meta`）、`check:i18n` 231 键、`vite build`、`npm run tauri build -- --no-bundle` 全部通过；便携版 14,461,440 字节（21:20，已验内嵌 dist）。

### 批次 2（2026-10-26 追加，按用户反馈修正）

- ✅ **开关开启立即探测**：`maybeStartProbe()` 不再延迟 60–180s，直接跑一轮 `probeRound()`，随后按 20min+U(0,5min) 排期；无候选时 5 分钟后复查（不再错误地把下一轮推到 3h）。
- ✅ **到点提醒**：`starts_at`（活动开始）与 `effective_at`（生效）各提醒一次，60s ticker 比对时间戳（非长 `setTimeout`），去重键 `accountId|planId|kind` 存 `localStorage["zsw-reminder-fired"]`（上限 500）。
- ✅ **到点自动领取改为独立开关**（用户指出原设计矛盾）：`claim_at_effective` **不再依赖 `auto_claim`**。到点后随机 5–30s 触发**一次** `claimForActivity()`，只在该账号当前仍有可领套餐时发起；无可领直接返回不发请求；不启动任何循环。`claim_start(auto:true)` 仍受风控冷却约束。
- ✅ **自动领取准入重写（用户第 3 轮反馈）**：`autoClaimTick()` 不再逐账号 `claim_refresh`。改为：
  - `autoTargets()` 只从已探测数据里挑「`status==="claimable"` + 已开始（`startsAt`/`effectiveAt` ≤ now）+ 未过期（`endsAt` > now）+ 本账号未标记已领 + 不在 hold/冷却」的条目；
  - 空集时直接返回并记 `lastAutoRound.noActivity`（顶部提示「未探测到可领活动，自动领取待命」），**零上游请求**；
  - 领取成功写标记 `claimMarks[accountId|planId]`（`localStorage["zsw-claim-marks"]`，上限 300），同一账号同一活动不再重复领、也不必反复查询是否领过；
  - 手动 `claim` / `claimAll` / 到点 `claimForActivity` 成功路径同样写标记；`claimForActivity` 领前先查标记。
- ✅ **调度补齐（用户核对后发现的缺口）**：
  - 每个账号独立 `U(0,5min)` 延迟（排序后串行），两次请求间再插 `rand(20s,90s)`；
  - `visibilitychange` → 回前台若距下次轮次 > 120s（或已过期），重排到 `rand(30s,120s)` 后补探；
  - `probe.roundRunning` 轮级互斥，避免两账号延迟间隙被 `refresh()` 重入导致两轮并发；
  - `probe.nextAt` 记录下次轮次时间，供前台补探判断。
- 说明：因每轮含两个账号的 0–5min 独立延迟，实际轮次间隔 ≈ 20–31min（比设定更抖，抗脚本特征更好）。
- ✅ **启动基线 + 已领活动补展示（用户第 4 轮反馈）**：
  - 服务端对已领活动不再下发（marketing 返回 0 条），因此基线对每个账号额外拉一次 `billing/current`，把「已领取 / 待生效」补进列表；
  - 基线每次启动都跑：当天未探过 → 探测 + current；当天已探过（多半是重启）→ 只拉 current，不重复打 marketing；
  - 新增右下角活动弹窗 `activityToast()`（独立 `act-toast-zone`，8s 自动消失，点击直达详情），不再强制弹框打断操作；
  - 弹窗按**活动**去重（`campaign_id`，缺失用 `plan_id`），同一活动多账号只提醒一次；「活动」按钮只要有任何条目（可领/已领/已过期）就不再置灰，角标仍只统计可领数；
  - `probe.log` 增加 `new <id> +n (claimable|claimed)` 记录。
- ✅ mock 用例：`probe_only_touches_marketing_and_writes_no_history`（只碰 marketing、不写历史）。
- ⏳ 未覆盖：「跳过风控冷却账号」没有独立 mock 用例（判断在 tauri 命令层、依赖 `Paths` + 私有 `risk` API），仅由 risk 单测与代码路径覆盖。

### 批次 3（2026-10-26 追加，活动 ID 与系统通知）

- ✅ **活动元数据全局缓存**：`campaign_id` / 展示位 / 文案 / 时间窗只存在于 marketing delivery；「已领取」条目来自 `billing/current`（无这些字段）。新增 `planMeta`（`localStorage["zsw-plan-meta"]`，plan_id → 元数据），任一账号探到即全账号复用。实测真实 payload：`campaign_id=2101141259813175296`、`resource_position=banner`、文案在 `banner.success_popup.{title,description}`。
- ✅ **后台系统通知**：接入 `tauri-plugin-notification`（Rust 2.4.0 / JS 2.4.0，`notification:default` 权限）。`document.hidden` 时发系统通知，前台仍用应用内右下角 `activityToast`。注意：进程完全退出时无法通知。
- ✅ **版本对齐**：`@tauri-apps/api` 钉到 `2.11.1`（对应 Rust `tauri 2.11.5`），否则 `tauri build` 报 major/minor 不匹配；`npm install` 会擅自升级，故用精确版本。
- ✅ **活动 ID/展示位「未提供」真因（2026-10-26 定位）**：`billing/preview` 现在也会下发同一 plan（实测 `plans=1`），而 preview 的 plan **不含活动元数据**；`merge_marketing_plans` 按 plan_id 去重时「preview 优先」把 marketing 那份丢了。已改为「保留 preview，但 campaign/description 缺失时从 marketing 补」。新增回归用例 `merge_adopts_campaign_meta_from_marketing`。
- ✅ **文案解析**：真实 payload 的文案是 `{content, format}` 结构（title 在 `banner.success_popup.title`、正文是含 HTML 的 `description`）。新增 `text_of` / `strip_html` / `first_text`，正文去 HTML 后展示；时间窗兜底取 `banner.background.args.zcode_plan.{starts_at,ends_at}`。实测解析结果：`campaignId=2101141259813175296`、`position=banner`、`title=ZCODE GLOBAL BUILD 领取成功`、`body=GLM-5.3-Flash 将于 2026年10月26日 23:00 生效。`、`starts=19:00 / ends=10-27 09:00`。
- ✅ **探测结果未入口列表**：`activityRows()` 原来只读 `claimable`/`claimedState`，没读 `activities`（探测结果），导致探测到的可领活动不显示、也不触发弹窗。已把 `activities[id]` 并入「可领」来源。
- ✅ **基线每次启动都做完整探测**：原先「当天已探过只拉 current」会让重启后 `activities` 为空（表现为活动 ID 缺失、无弹窗），已取消该优化。
- ✅ **活动目录持久化（2026-10-26 第 5 轮反馈）**：`probe.log` 证实 22:22 基线只探了 4 个账号——`tcaiq772`/`旅行者1423` 已被 22:17 那次写入「静默到 ends_at」，而探测结果只存内存，重启即丢 → 行内空白、按钮无角标、无弹窗。新增 `localStorage["zsw-activity-catalog"]`（探测/刷新/current 三路都写入，上限 300），`activityRows()` 以目录兜底渲染。
- ✅ **静默语义收窄**：静默只约束「20 分钟周期的自动探测」，**启动基线强制探测全部账号**（每账号 1 请求/次启动），保证展示与提醒数据完整；静默跳过也会写 `skip <id>（静默期）` 日志。
- ✅ **通知双通道**：窗口隐藏时发系统通知，同时**始终**放一条应用内右下角弹窗（回到窗口仍可见），`probe.log` 记录 `notify os` / `new ...`。
- ✅ **系统级可见性兜底（2026-10-26 第 6 轮反馈：便携版 Toast 不显示）**：
  - 新活动时窗口标题改为 `Z·SWITCH · 新活动：<活动名>`（Windows 任务栏悬停即显示窗口标题）；
  - 托盘 tooltip 追加 `· 新活动：<活动名>`（新增命令 `set_activity_tip` + `rebuild_tray` 拼接），悬停托盘图标可见；
  - 打开「活动」弹框或点击弹窗后清除上述提示（标题回到 `Z·SWITCH`、托盘提示清空）；
  - 窗口无焦点时 `requestUserAttention(2)` 让任务栏按钮闪烁（钉钉式提醒）。
  - 说明：窗口「关到托盘」后没有任务栏按钮，闪烁无效；此时靠托盘 tooltip + 系统通知（若系统允许）+ 恢复窗口时的应用内弹窗。
  - `tauri build`（带 NSIS 安装包）因 GitHub 下载 NSIS 超时失败，安装版未产出（Toast 需要安装版才可靠）。
- ✅ **弹框内直接领取（第 12 轮反馈）**：活动弹框「可领」条目展开后内置「领取」按钮（点击走 `actions.claim` → 验证码流程，无需关闭弹框）；顺带修正 `actions.claim` 数据源为 `claimablePlansFor(id)`（原先只读刷新缓存，探测发现的活动点领取会误报「无可领」）。
- ✅ **移除「测试活动提醒」按钮**：设置页按钮、`settings.js` 实现、`s.testNotify*` i18n 键全部清理（托盘闪烁已由真实链路验证）。
- 说明：账号行「可领条 + 已领历史条」并存是两个不同活动（当前可领 vs 过去领过已过期），非重复展示。
- ✅ **自动领取永远拿不到目标（第 17 轮，关键 bug）**：`autoTargets()` 里仍用 `claimable[accountId].plans` 找 plan，而探测发现的活动只存在 `activities` 里 → `plan` 为空 → 目标集恒空 → 静默 return（连日志都没有）。已改为 `claimablePlansFor(accountId)`（刷新 + 探测合并），并给 `effectiveClaimTick` 加 `effective tick targets=N ids=...` 日志便于核对。
- ✅ **到点自动领取对「已过点」活动不生效（第 16 轮）**：原实现只由 `reminders`（`startsAt`/`effectiveAt` 在未来 → 到点触发）驱动；活动时间已过时被 `syncReminders` 跳过，所以开关打开也不会有动作。改为独立调度：启动 30s 后首查、之后每 60s 检查「已到点且未领」的目标（`effectiveClaimTick` → `autoTargets()`），并抽出 `runClaimTargets(targets, source)` 让「自动领取」与「到点自动领取」共用同一套准入/防重复/冷却逻辑。无目标时不发任何上游请求。
- ✅ **领取后可领条/领取按钮不消失（第 15 轮）**：可领条来自探测缓存 `activities`，已领取来自 `billing/current`；领取成功后 current 更新但探测缓存未清，`claimablePlansFor` 合并后仍判为可领。修复：`claimablePlansFor` 排除「claimedState 中存在的 plan」与「已打标 plan」；`markClaimed` 同步从 `activities` / `claimable` 中移除该 plan。
- ✅ **弹框内「领取」点了没反应（第 14 轮）**：`openActivityModal` 在 `.act-panel` 上 `stopPropagation()`，导致面板内 `click="actions.claim(...)"` 的按钮事件无法冒泡到 document 级委托（所有属性式按钮全失效）。改为「仅当 `e.target === mask` 时关闭」，不再阻断冒泡；并在 `actions.claim` 里先移除 `.act-mask`，避免弹框挡住验证码窗口。
- ✅ **有可领活动时隐藏历史条**（第 13 轮要求）：`claimedStripHtml` 在 `!active.length && claimablePlansFor(id).length` 时直接返回，不再叠加旧活动的历史条。
- ✅ **自动领取链路审查（第 13 轮要求）**：
  - 已核对正确：准入（可领+已开始+未过期+未标记+hold/冷却到期）→ 空集零请求；`claim_start(auto)` 受 risk 冷却约束；成功写标记防重复；终态 `Infinity`、1005 `nextAt`；调度 = 启动 2min 首轮 + 10min 间隔，开关打开且距上次 >60s 立即跑一次。
  - 修复 1：`3007/挑战` 的 hold 从 10min 改为 **60min**（拒码是风控信号且 3007 不再自动重试，否则每 10min 重复弹验证码窗口）。
  - 修复 2：超时后无条件 `claim_cancel` 会清掉**当前任何**待领取（可能误取消用户手动发起的领取）。新增后端 `claim_pending()` + 前端 `cancelClaimIfMine(id, planId)`，仅在待领取仍是自己发起的那一笔时取消；替换 claimAll / autoClaimTick / claimForActivity / captcha-interactive 四处调用。
  - 已知次要项（未改）：`waitForClaimResult` 是全局单例等待者，自动与手动并发时互不感知（取消已收敛为「只取消自己的」，风险可控）；`autoTargets()` 会经 `activityRows()` 触发一次 plan-meta 缓存写入（量小）。
- ✅ **闪烁视觉与去重策略修正（第 11 轮反馈）**：
  - 提醒态图标从「整图标染红」改为**原图标 + 右下角红点徽标**（`badged_icon`，红点出现/消失交替，钉钉式）；
  - 可领提醒去重从「持久」改为**会话级**：每次启动对未领取的活动再提醒一次（含持续闪烁），直到领取；已领取活动保持持久去重（只提醒一次）；
  - 实测依据：22:24 的可领提醒（旧构建闪 6 下即停）被用户错过，持久去重导致后续启动永远静默。
- ✅ **行高抖动修复（第 7 轮反馈）**：冷却倒计时原由 `refreshTicker` 每秒 `render()` 整页重建 innerHTML，导致行高/滚动跳动。改为 `tickCooldowns()` 只更新角标文本（`[data-cd-id]`）与顶部文案（`#refresh-claim-label`），仅当倒计时归零或刷新进行中才整页重绘。
- ✅ **任务栏闪烁修复**：判断由 `document.hasFocus()`（Tauri 下不可靠）改为 `getCurrentWindow().isFocused()`，结果写 `probe.log`（`attention flash` / `attention skip(focused)` / `attention error`）。注意：闪烁只在「首次发现新活动」时触发，已提醒过的活动不会重复。
- ✅ **自测入口**：设置页新增「测试活动提醒」按钮，一次性触发 窗口标题 → 托盘 tooltip → 强制任务栏闪烁 → 系统通知 → 应用内弹窗，便于快速判断哪一环失效。
- ✅ **托盘图标闪烁（第 8 轮：用户要的是托盘闪，不是任务栏）**：新增命令 `tray_flash(times)`——Windows 托盘无闪烁 API，实现为「提醒态图标（红色提亮）与常态交替 420ms，默认 6 次后恢复」，全部经 `run_on_main_thread` 设置，失败写 `probe.log`。
- ✅ **托盘 tooltip 刷新**：设置活动提示后额外 `set_icon` 一次强制 shell 重新读取 szTip（Windows 上仅 `NIM_MODIFY` 有时不刷新），并写日志 `tray tip -> Some("...")` 便于确认命令送达。
- ✅ 新活动与「测试活动提醒」都会触发托盘闪烁（`flashTray(6/8)`）。
- ✅ **测试按钮点了无反应的两个根因（第 9 轮定位）**：
  - 设置窗口是独立入口 `src/settings.js`，有自己的 `actions`；`testActivityNotify` 只加在 `main.js`，因此点击后什么都没发生（连 invoke 都没有）。已在 `settings.js` 内实现同样逻辑。
  - `client-error.log` 记录 `plugin:window|set_title not allowed by ACL` → `capabilities/default.json` 补 `core:window:allow-set-title` / `allow-is-focused` / `allow-request-user-attention`；否则窗口标题、焦点判断、任务栏闪烁全被 ACL 静默拒绝。
  - 测试路径现在会写 `test tray tip ok` / `test tray flash ok` / `test notify granted=…` 到 `probe.log`。
- ✅ **托盘持续闪烁 + 菜单入口（第 10 轮：闪几秒太弱、tooltip 不起眼）**：
  - `tray_flash(times)`：`times=None/0` 改为**持续闪烁**（后台线程按 `TRAY_FLASH_ACTIVE` 循环，420ms 交替），不再自动停止；`Some(n)` 仅测试用。
  - 新增 `tray_flash_stop()`：恢复常态图标并停止闪烁；前端在 `openActivity`、点击活动弹窗、托盘菜单点击时调用。
  - 托盘菜单顶部新增显眼条目 `【新活动】<活动名>`（点击 → `emit("tray-activity")` → 前端 `actions.openActivity()` 打开详情并停止闪烁），比 tooltip 小字更不易被忽略。
  - tooltip 文案改为 `【新活动】<活动名> · <base>`，把关键信息前置。
- ✅ 编译坑：`run_on_main_thread` 要求 `'static` 闭包，`app.default_window_icon().cloned()` 借用了 `app`；必须先构造 owned `Image::new_owned(...)` 再移入闭包。

## 回滚点

- 步骤 1-3 为后端新增字段/命令，可独立回退且不影响既有领取。
- 步骤 4-6 为前端新增路径，删除 `activities`/按钮/弹框即可回到现状。
