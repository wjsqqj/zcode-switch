# Design: 风控退避与额度防线

## 风控冷却（新 risk.rs 或并入 store.rs）

```rust
pub struct RiskState { pub strikes: Vec<i64>, pub cooling_until: i64 }  // strikes: 命中时间戳(ms)
// 持久化 {home}/risk-state.json（按账号）
// 命中：record_strike(id) → 窗口内(7d)清理过期 strikes → push(now)
//   cooling = min(900 * 2^(len-1), 24h)；len>=4 → cooling = 24h
// 查询：is_cooling(id, now)；手动刷新传 force=true 跳过
```

- 注入点：claim.rs（preview/claim/marketing 的 code==3012 响应与 HTTP 403+unusual 文案）、quota.rs（额度查询 3012）。
- 跳过点：额度轮询 sweepTick（后端 store::live_quota/account_quota 的自动路径）、claim_refresh、marketing_touch_plans、auto claim 轮次。前端手动刷新直接调用不受限（后端不动，由前端 force 语义约定：命令加 force 参数）。

## 额度防线（quota.rs）

- 核对现有"耗尽"判定路径（balance 剩余为 0 的展示与状态推导）。
- 防线：若该账号存在生效中的一次性赠送（C 子任务的 claim-current 数据，或 balance 数据中 grant_items period=one_time 且 ends_at 未过）→ 不判定/不显示耗尽。
- 若 zcode-switch 现状并无"耗尽自动禁用"逻辑，则降级为展示层防线：赠送池有效时剩余额度展示 max(balance_remaining, 赠送池)或加"含赠送"标注——以实现时核实的真实数据形态为准，PRD 验收以"不误判"为准。

## 兼容

- risk-state.json 损坏视为空；冷却不持久化敏感信息（仅时间戳）。
- 回滚 = revert 单 commit。
