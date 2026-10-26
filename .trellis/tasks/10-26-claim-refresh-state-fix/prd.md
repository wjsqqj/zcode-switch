# 修复领取刷新卡死与静默无反馈（子任务 A）

> 父任务：`.trellis/tasks/10-26-claim-refresh-activity-overhaul`

## Goal

让「刷新领取资格」在任何情况下都具有确定的可观测终态：要么给出结果，要么给出原因；永不出现「按钮灰着不动、整窗无响应、只能重启」的状态。

## 症状 → 根因假设

| 症状 | 根因 |
|---|---|
| 行内 ↻ 变灰后再点无提示 | `refreshClaim()` L388 / `acctRefreshClaim()` L443 / `claimAll()` L355 在全局忙时 `return`，无 toast |
| 存在 60s「再也点不动」 | `acctRefreshCooldown` / `refreshClaim.cooldownUntil` 只写不显示，用户无法知道还有多少秒 |
| 顶部刷新点击无反应 | 同上（全局 `refreshClaim.running` 为真时静默 return；冷却期才弹 toast） |
| 额度区一直「查询中…」 | `acctQuota[id].busy` 置位后依赖后续 render 清除；一旦 render 抛异常，界面永远停在 busy 帧 |
| 整个窗口按钮全部失效 | `render()` 无 try/catch：任何一次渲染异常都会让 `$app.innerHTML` 不再更新，视觉上整页冻结（事件委托仍在，但无任何可见变化） |
| 并发点击后窗口真卡死 | 后端 `async fn` 内直接跑阻塞 ureq，占满 Tauri 异步运行时工作线程，其余命令排队 |

## Requirements

- R1 **消除静默 return**：`refreshClaim` / `acctRefreshClaim` / `claimAll` / `claim` / `toggleAutoClaim` 所有提前返回分支必须给出可见反馈（区分「正在刷新，请等待」「冷却中，还需 N 秒」「需人工验证」等语义）。
- R2 **冷却可视化**：行内刷新按钮在冷却期显示剩余秒数（而非仅变灰）；顶部刷新按钮同理；冷却结束自动恢复且不再依赖点击才发现。
- R3 **render 异常兜底**：`render()` 包 try/catch；异常时保留可交互的外层框架并显示可关闭的错误条（含错误摘要），同时把错误写入本地日志文件，便于用户回传。
- R4 **前端请求看门狗**：每个刷新请求设置前端超时（默认 60s），超时后强制清 `busy`、提示「请求超时，已恢复」，避免无限 busy。
- R5 **后端阻塞 I/O 移出异步运行时**：`claim_preview` / `claim_refresh` / `claim_current` / `claim_start` / `claim_captcha_config` / `get_account_quota` / `get_live_quota` 等阻塞命令改为 `spawn_blocking` 执行，保证并发时 `get_state`、`set_behavior` 等命令不被饿死。
- R6 **全局刷新节奏可感知**：`refreshClaim` 账号间隔 5s 期间显示进度（已完成/总数 + 当前账号名），不再出现长时间无更新。
- R7 **3012 风控 mock 矩阵**：`src-tauri/tests/mock_upstream.rs` 增加 `provider_code=3012` / `request has been blocked due to unusual activity` / `status=405` 报文用例，断言：被识别为风控 strike、写入退避、后续自动路径零上游流量、手动 `force=true` 仍放行。
- R8 **零语义回归**：领取成功/失败/挑战/终态语义、`claimHold`、`risk` 退避时长、i18n key 既有文案不变。

## Constraints

- 不新增前端框架/状态库；沿用现有模块级状态与 `document` 级事件委托。
- 不引入新的网络库；`ureq` 超时保持不变。
- 中文/英文文案必须同步（`src/locales/zh.js` + `en.js`），并通过 `npm run check:i18n`。

## Acceptance Criteria

- [x] ✅ 代码中不存在无反馈的 `return`：`refreshClaim`/`acctRefreshClaim`/`claimAll` 改为 `refreshGate` + `toastGate`（`claim`/`toggleAutoClaim` 原有 toast 保留）。
- [x] ✅ 冷却期反馈：行内按钮角标显示剩余秒数，顶部按钮显示 `刷新 (Ns)`；`refreshTicker` 每秒重绘。
- [x] ✅ render 兜底：`render()` → try/catch + `renderInner()`；异常显示 `#err-bar` 且按钮仍可点；异常写 `client-error.log`。
- [x] ✅ 前端 60s 看门狗 `invokeWithTimeout`：超时清 `busy` 并提示 `m.refreshTimeout`。
- [x] ✅ 后端 8 个阻塞命令改 `spawn_blocking`；`cargo check` 0 新增 warning。
- [x] ✅ 3012 mock 用例：`risk::tests::observe_hits_3012_blocked_payload`、`mock_upstream::preview_blocked_unusual_activity_3012_surfaces_error` 均通过；`cargo test --lib` 8 passed、`cargo test --test mock_upstream` 11 passed。
- [x] ✅ `npm run check:i18n` 通过（zh/en 各 205 键）；`vite build` 通过。
- [x] ✅ 吸收调研补充项：`claim_preview`/`claim_current` 冷却期零上游流量；3007 不自动重试；赠送池按 `effective_at` 判定；mock 请求规格断言（头九件套 / body 仅 `plan_id`）；`claim-plans.md` §13 坑位表。
- [ ] ⏳ 真机：连续点 3 个账号刷新 + 顶部刷新 + 自动领取开关，全程无卡死、无静默无响应（需用户实跑便携版）。

## Notes

- 症状「已领取过 ZCode 1024 Dev Day」是历史条文案 `claim.historyStrip`，不是错误；本任务不改变其语义。
- 若真机复现后确认另有前端异常点，以 R3 的日志文件定位，不猜测修复。
