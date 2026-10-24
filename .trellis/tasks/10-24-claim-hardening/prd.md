# B 领取链路加固

## Goal

把 zcode2api / zcode-plan-claim 用真实领取验证出的失败处置纪律移植到 zcode-switch 的 claim 链路：区分验证码挑战与凭证失效、3007 换新参数重试一次、终态不重试、1005 领取记忆、验证码配置缓存兜底。

## Requirements

1. **验证码挑战排除**：HTTP 401/403 响应携带验证码挑战特征（响应头 x-aliyun-captcha-verify-param 或 body 含 captcha 挑战文案）时，归类为「需过验证码」而非「凭证失效」，不触发 jwt_expired 误报。
2. **3007 重试一次**：验证码窗口提交流程收到 3007 → 窗口内自动重新过一次验证码并重提，仅一次。
3. **终态不重试**：1001/1002/1003/1004/1005 失败后自动领取轮次不再对该账号该套餐发起领取（1003 已领取亦为终态）。
4. **1005 next_at 记忆**：nextAt 时间内自动领取轮次跳过该账号该套餐（省验证码求解与上游写流量）；到期自动恢复；nextAt 不可用回退现有 1h 冷却。
5. **验证码配置缓存**：client/configs captcha 配置缓存 600s；请求失败回退内置默认（enabled=true, region=cn, prefix=no8xfe, sceneId=11xygtvd）。
6. 既有错误文案/i18n 不删除；新增分类走新增 key。

## Acceptance Criteria

- [ ] 3007 后自动重试一次链路可用（窗口内完成，无需用户重新点领取）。
- [ ] 自动领取轮次对 1005 组合在 nextAt 前跳过（不发 claim 写请求）。
- [ ] 验证码配置接口失败时流程不中断（走默认配置）。
- [ ] 挑战类错误不影响 jwt_expired 判定。
- [ ] cargo check 0 警告；i18n 键检查通过。

## Notes

- 复杂任务：design.md + implement.md 已备。
- 现状：claim_start → open_captcha_window → captcha.js 过码 → claim_captcha_submit；claim.rs 已有 next_at 解析（1005）与 i18n 错误映射。
