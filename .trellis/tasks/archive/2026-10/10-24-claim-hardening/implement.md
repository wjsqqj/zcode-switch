# Implement: 领取链路加固

## 1. claim.rs
- [ ] ClaimError 语义方法（is_captcha_rejected/is_terminal/is_challenge）
- [ ] submit_claim 401/403 挑战识别
- [ ] fetch_captcha_config 600s 缓存 + 默认兜底
- 验证：cargo check

## 2. 前端 captcha.js / main.js
- [ ] 3007 → 窗口内重建验证码自动重试一次
- [ ] claim 失败回调写 claimHold（1005→nextAt/1h；1001-1004→Infinity）
- [ ] autoClaimTick 跳过 hold 中组合
- [ ] 核对 jwt_expired 判定链路
- 验证：node --check；新 i18n key 补 zh/en

## 3. 质量检查
- [ ] cargo check 0 警告；check-i18n；E 子任务补 3007/挑战矩阵断言
