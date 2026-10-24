# C 已领取状态展示

## Goal

用 billing/current（已生效套餐）+ 本地领取历史，把账号的领取状态显式化：区分「无可领 / 已领取生效中 / 已领过已过期」，消除"手动领取后刷新显示空白"的困惑。

## Requirements

1. 新增 tauri 命令 `claim_current(id)`：GET /api/v1/zcode-plan/billing/current（Bearer zcodejwttoken + 公共头），返回已生效套餐列表（user_plan_id/plan_id/name/status/starts_at/ends_at/entitlements 摘要）。
2. 本地领取历史：领取成功后记录 {accountId, planId, planName, claimedAt} 到数据目录 json（不含凭证）；查询时并入展示。
3. 前端账号行 claim 区四态：`claimable`（现有可领条）/ `claimed-active`（生效中，显示套餐名+额度+到期时间）/ `claimed-expired`（历史，弱化展示）/ 无记录不显示。刷新资格/单账号刷新时一并拉取 current。
4. current 查询失败静默（有 preview 结果优先展示）；历史文件损坏视为空。
5. i18n zh/en 全覆盖。

## Acceptance Criteria

- [ ] 领取成功后账号行出现"生效中"条目（含到期时间），直到 ends_at 过期转为历史态。
- [ ] 手动在客户端领取过的账号，刷新后也能显示生效中（current 来自服务端）。
- [ ] current 失败不影响可领列表；历史文件损坏不崩。
- [ ] cargo check 0 警告；i18n 通过。

## Notes

- 复杂任务：design.md + implement.md 已备。
- billing/current 响应结构参考 plan-claim §3.3：data.plans[{user_plan_id, plan_id, name, status, starts_at, ends_at, entitlements[]}]；ends_at 为秒级时间戳。
