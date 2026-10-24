# Implement: marketing/touch 活动套餐接入 + 每账号刷新

> 顺序执行；每步完成后勾选。验证命令见各步。

## 1. quota.rs 版本策略

- [ ] `CLIENT_APP_VERSION` "3.11.2" → "3.16.3"
- [ ] 新增 `fn version_ge(a: &str, b: &str) -> bool`（分段数值比较）
- [ ] `zcode_app_version()` 探测成功分支改为返回 `max(常量, 探测值)`
- 验证：`cargo check`（src-tauri）

## 2. claim.rs marketing/touch 通道

- [ ] `MARKETING_TOUCH_URL` 常量
- [ ] `pub fn marketing_touch_plans(home, creds, config, device_mid) -> Vec<ClaimPlan>`
  - claim_token → GET → 递归收集 `zcode_plan` payload（parse_plan 复用）+ 按钮 `claim_zcode_plan/claim_plan` plan_id
  - 无 payload 的按钮 plan_id 合成最小条目
  - 去重 + priority 降序
  - 全错误路径 `flowlog::log("claim", "marketing-fail", …)` + 返回空
- 验证：`cargo check`；`cargo build` 后 CLI `claim-preview` 冒烟（有活动账号）

## 3. lib.rs 合并层

- [ ] 新增 `fn collect_claimable_plans(paths, acc, mid) -> Vec<ClaimPlan>`：preview（Err 上抛语义由调用方保持）+ marketing 追加去重
- [ ] `claim_preview` / `claim_refresh` / `claim_start` 三处改用合并结果（preview 的 Err 语义不变）
- [ ] `claim_start` plan 查找基于合并结果
- 验证：`cargo check`

## 4. 前端每账号刷新

- [ ] `actions.acctRefreshClaim(id)`（守卫/冷却/写回/toast/scheduleNext）
- [ ] 账号行 `row-actions` 新增 refresh 图标按钮（disabled 联动忙碌与冷却态）
- [ ] `acctRefreshBusy` / 每账号冷却状态；render 停表条件复用现有 ticker 机制或依赖现有 render 触发点
- [ ] `src/locales/zh.js` + `src/locales/en.js` 增加 `btn.acctRefreshTitle`
- 验证：`npm run build`（或 vite build）无语法错误；人工核对模板字符串转义（id 单引号注入面与现有 `actions.claim('${id}')` 一致）

## 5. 质量检查（trellis-check 范围）

- [ ] `cargo check` 0 err 0 warn（新增代码）
- [ ] grep 确认 `CLIENT_APP_VERSION` 其余消费者语义未破坏（oauth 头）
- [ ] 对照 PRD Acceptance Criteria 逐条核对
- [ ] 前端改动 diff 自查：不引入全局状态泄漏、不破坏现有全量刷新/自动领取互斥

## 6. 收尾

- [ ] spec 更新评估（若 marketing/touch 协议要点值得沉淀 → trellis-update-spec）
- [ ] commit（feat(claim): 接入 marketing/touch 活动套餐 + 每账号刷新）
- [ ] task.py archive

## 回滚点

- 单 commit revert 即可整体回退；无数据迁移。
