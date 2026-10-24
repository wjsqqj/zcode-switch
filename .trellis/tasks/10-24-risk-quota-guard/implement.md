# Implement: 风控退避与额度防线

## 1. 风控状态
- [ ] risk.rs：record_strike / is_cooling / 持久化 / 窗口衰减
- [ ] claim.rs / quota.rs 3012 命中注入
- [ ] 自动路径跳过冷却账号（quota sweep、claim_refresh、marketing、auto 轮）
- 验证：cargo check

## 2. 额度防线
- [ ] 核对 quota.rs 耗尽判定与赠送池数据形态
- [ ] 实现不误判（展示或判定层，按核实结果）
- 验证：cargo check

## 3. 前端
- [ ] 冷却提示 + i18n
- 验证：node --check；check-i18n；E 子任务补 3012 退避矩阵断言
