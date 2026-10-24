# Design: 领取链路加固

## 错误分类（claim.rs，不改 ClaimError 结构避免跨层破坏）

```rust
impl ClaimError {
    pub fn is_captcha_rejected(&self) -> bool { self.code == 3007 }
    pub fn is_terminal(&self) -> bool { (1001..=1005).contains(&self.code) }
    pub fn is_challenge(&self) -> bool { self.code == -2 }  // HTTP 401/403 + 挑战特征
}
```
- submit_claim 的 ureq::Error::Status(401|403) 分支：读 headers/body，含挑战特征（header x-aliyun-captcha-verify-param 存在或 body 含 captcha 关键词）→ ClaimError{code:-2}；否则维持原语义。
- jwt_expired 核对：store 侧 jwt_expired 来自 token 解析而非 claim 错误（实现时确认无耦合）。

## 3007 重试一次（captcha.js 窗口内状态机）

- 提交结果回调 code===3007 → 销毁并重建阿里云验证码实例，自动再过一次无感验证，拿新 param 直接重提；局部 retried 标志保证只重试一次。

## 1005 记忆 + 终态（main.js）

- `claimHold = { ["acctId|planId"]: untilMs }`（会话级内存）。
- 失败回调写 hold：1005 → nextAt（无则 now+1h）；1001-1004 → Infinity。
- autoClaimTick 选 plan 跳过 hold 未到期组合；手动领取不拦（用户显式意图优先）。

## 验证码配置缓存（fetch_captcha_config）

- 进程内 Mutex<(Instant, Option<CaptchaConfig>)> 缓存 600s；失败用上次成功值；再无 → 内置默认（来源：zcode2api constants.py 与 plan-claim §3.4 观测一致）。

## 兼容与回滚

- ClaimError serde 结构不变；captcha.js 只加内部状态。回滚 = revert 单 commit。
- 风险：SDK 实例重建行为差异 → 仅重试一次且失败即止，最坏退化为现状。
