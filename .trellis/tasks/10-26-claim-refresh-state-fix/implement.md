# 执行计划：刷新状态机与阻塞 I/O（子任务 A）

## 前置

1. 读 `.trellis/spec/backend/claim-plans.md`（§8-§11 错误语义、风控退避、已领取状态、mock 旁路）。
2. 确认 `src-tauri/tests/mock_upstream.rs` 现有测试全绿（作为回归基线）。

## 步骤

### 1. 后端：阻塞 I/O 移出 async worker（R5）

- [ ] `src-tauri/src/lib.rs`：给 `claim_preview` / `claim_refresh` / `claim_current` / `claim_start` / `claim_captcha_config` / `get_account_quota` / `get_live_quota` 加 `spawn_blocking` 包装，逻辑体原样搬入闭包。
- [ ] 新增 i18n key `err.internal`（zh/en）用于 join 失败。
- 验证：`cd src-tauri; cargo check` → 0 新增 warning。

### 2. 后端：客户端错误日志命令（R3 配套）

- [ ] 新增 `log_client_error(message: String)` 命令：写入 `{home}/.zcode-switch/client-error.log`，按 200 行滚动。
- 验证：`cargo check`；手动 `--cli` 无法覆盖，靠前端自测。

### 3. 前端：统一准入与冷却可视化（R1/R2/R6）

- [ ] `src/main.js` 新增 `refreshGate(scope)` / `cooldownRemaining(scope)`；替换 5 处入口的静默 return。
- [ ] 行内按钮与顶部按钮渲染冷却倒计时；全局刷新显示 `done/total · 当前账号`。
- [ ] 新增 i18n key：`m.refreshBusyRunning`、`m.refreshCooldownSec`、`m.refreshTimeout`、`m.renderError`、`btn.refreshClaimProgress`。
- 验证：`npm run check:i18n`。

### 4. 前端：render 兜底与日志（R3）

- [ ] `render` 拆为 `render()`（try/catch）+ `renderInner()`。
- [ ] 新增独立 `#err-bar` 节点与 `showRenderErrorBanner` / `clearRenderError`。
- [ ] 注册 `window.onerror` / `unhandledrejection` → `invoke("log_client_error")`（节流，1s 最多 1 条）。

### 5. 前端：请求看门狗（R4）

- [ ] 新增 `invokeWithTimeout`，替换 `claim_preview` / `claim_refresh` / `claim_current` / `get_account_quota` 的调用。
- [ ] 超时分支清 `busy`、写 `claimable[id].err`、toast `m.refreshTimeout`。

### 6. 测试：3012 风控矩阵（R7）

- [ ] `src-tauri/tests/mock_upstream.rs` 增加 `blocked_unusual_activity_3012` 用例：405 + `provider_code=3012` + `request has been blocked due to unusual activity`。
- [ ] 断言：strike 落盘、`is_cooling` 为真、非 force 的 `claim_refresh` 返回冷却错误、force 放行。
- 验证：`cd src-tauri; cargo test` 全绿（含既有用例）。

### 7. 收口

- [ ] 每步完成即更新本文件复选框。
- [ ] `cargo test` + `npm run check:i18n` + `npx vite build` 三绿。
- [ ] 更新 `.trellis/spec/backend/claim-plans.md`（新增 §12「刷新状态机与阻塞 I/O 纪律」）与 `frontend` 相关规范（如有）。
- [ ] 父任务验收：交给用户真机跑「3 账号刷新 + 顶部刷新 + 自动领取开关」。

## 完成记录（2026-10-26）

- 步骤 1 ✅ `run_blocking` 包装 8 个命令（claim_preview / claim_refresh / claim_current / claim_start / claim_captcha_config / claim_captcha_submit / get_account_quota / get_live_quota），新增 `err.internal`。
- 步骤 2 ✅ 新增 `log_client_error` 命令 → `{home}/.zcode-switch/client-error.log`（200 行滚动）。
- 步骤 3 ✅ `refreshGate` / `toastGate` / `cooldownLeft`；三处静默 return 全部替换；行内角标 + 顶部 `(Ns)` 倒计时；全局显示 `done/total · 当前账号`。
- 步骤 4 ✅ `render()` 拆 `renderInner()` + `#err-bar` + `window.onerror` / `unhandledrejection` 落盘。
- 步骤 5 ✅ `invokeWithTimeout`（60s）覆盖 claim_preview / claim_refresh / claim_current / get_account_quota。
- 步骤 6 ✅ `risk::tests::observe_hits_3012_blocked_payload` + `mock_upstream::preview_blocked_unusual_activity_3012_surfaces_error`；顺带修复 risk 单测并行覆盖 `ZCODE_SWITCH_HOME` 的既有偶发失败（加 `TEST_LOCK`）。
- 步骤 7 ✅ `cargo check` 0 warning；`cargo test --lib` 8 passed；`cargo test --test mock_upstream` 11 passed；`npm run check:i18n` 205 keys；`vite build` 通过；spec 已补 `claim-plans.md` §12。
- 追加 ✅ 诊断命令：`--cli claim-current --id`（已生效套餐 + 本地历史）、`--cli claim-raw --id [--only preview|marketing|current]`（原样上游 JSON）。二者用于区分「服务端没下发」与「已领过/解析丢失」，实测已定位 `entitlements[].effective_at` 才是生效时间。
- 追加 ✅ 吸收 zcode/proxy/2api 调研结论（2026-10-26 第二批）：
  - **冷却期零上游流量补齐**：`claim_preview` 直接返回 `err.risk.cooling`；`claim_current` 不发上游、只回本地历史（原先两者都不拦，冷却被反复刷新击穿）。
  - **3007 改为不自动重试**：移除 `captcha_attempt` / `{retry:true}` 协议，收到 3007 直接终态失败并记 `flowlog captcha-rejected-3007`（吸收 zcode2api 2.6.5 结论：拒码本身是风险信号）。
  - **赠送池判定改用 `effective_at`**：`ActivePlan` 新增 `effective_at`（取 `entitlements[].effective_at` 最小值，0=立即生效），前端 `hasActiveBonus` 不再只看 `status==="active"`（否则 23:00 才生效的套餐现在就被当成生效中）。
  - **mock 增加请求规格断言**：`tests/support` 记录每条请求 `path/headers/body`，新增 `preview_requests_carry_identity_headers`（九件套头 + GET 无 body）与 `claim_post_sends_plan_id_and_captcha_headers`（body 仅 `plan_id`、验证码参数与地区成套、凭证不入 body）。
  - **坑位表沉淀**：`claim-plans.md` §13（header→错误码对照 + 冷却门禁覆盖表 + 请求规格断言纪律）。
- ⏳ 待用户真机：3 账号连刷 + 顶部刷新 + 自动领取开关，确认无卡死与静默无响应。

## 回滚点

- 步骤 1 后可独立回退后端包装（不影响前端）。
- 步骤 3-5 为前端状态机改动，失败时 `git checkout -- src/main.js src/locales` 即回退。
