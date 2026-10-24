# Implement: 已领取状态展示

## 1. 后端
- [ ] claim.rs fetch_active_plans（billing/current + 解析 grant_items 复用 parse 逻辑）
- [ ] lib.rs claim_current 命令 + invoke_handler 注册
- [ ] 领取历史文件读写（写入点：claim 成功）
- 验证：cargo check

## 2. 前端
- [ ] claimedState 状态与刷新触发（全量/单账号/领取成功后）
- [ ] 四态渲染 + i18n zh/en
- 验证：node --check；check-i18n；CLI 冒烟 claim-current

## 3. 质量检查
- [ ] cargo check 0 警告；E 子任务补 billing/current mock 断言
