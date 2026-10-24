# D 风控退避与额度防线

## Goal

吸收 zcode2api 的风控处置与额度判定经验：3012「unusual activity」指数退避 + strikes 衰减窗口 + 冷却期零上游流量；额度判定排除一次性赠送池误判。

## Requirements

1. **3012 识别与退避**：billing/preview、marketing/touch、billing/claim、额度查询命中业务码 3012（或同义 unusual activity 文案）时，账号进入风控冷却：900s 起指数退避（×2，封顶 24h）。
2. **strikes 衰减窗口**：连续命中在 7 天窗口内累计；超窗重新起算；窗口内累计 4 次升级为「长冷却」（24h 封顶），冷却期内该账号跳过一切主动上游调用（额度轮询、claim 轮、marketing 查询）。
3. 冷却到期自动恢复 active，不要求人工介入；手动刷新（用户显式点击）可突破冷却。
4. **额度防线**：额度展示判定"耗尽"时，若该账号存在一次性赠送池（billing/balance 之外的赠送额度，或已知活动套餐在有效期内）则不显示/不判定为耗尽——具体以现有 quota.rs 数据结构核对后定实现（防白领 3 亿 token 被显示为 0）。
5. 冷却状态对前端可见（账号行小提示），i18n zh/en。

## Acceptance Criteria

- [ ] 模拟 3012（mock）触发退避，冷却期内自动轮询/轮次不再发起上游请求。
- [ ] strikes 7 天窗口衰减与 4 次升级逻辑有测试断言（E 子任务）。
- [ ] 存在有效赠送池时额度不误判耗尽。
- [ ] cargo check 0 警告；i18n 通过。

## Notes

- 复杂任务：design.md + implement.md 已备。
- zcode2api 实测教训原文：「上游对 messages 端点的 3012 是频道级瞬时频控：同号同刻 billing 全部正常、数小时自愈——硬禁用会让健康的 billing/claim 陪葬」。
