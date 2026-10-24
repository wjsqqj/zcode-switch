# Design: 已领取状态展示

## 后端

```rust
// claim.rs
pub struct ActivePlan { pub user_plan_id: String, pub plan_id: String, pub name: String,
                        pub status: String, pub starts_at: Option<i64>, pub ends_at: Option<i64>,
                        pub grant_items: Vec<ClaimGrant> }
pub fn fetch_active_plans(home, creds, config, mid) -> Result<Vec<ActivePlan>, String>;  // GET billing/current
// lib.rs
#[tauri::command] async fn claim_current(id: String) -> Result<Vec<ActivePlan>, String>;
// store.rs（或新 claimed.rs）：领取历史读写
//   {home}/claim-history.json: [{accountId, planId, planName, claimedAt}]
//   写入点：claim 成功事件（现有 claim 成功回调处，lib.rs 的 claim result 事件）
```

- fetch 失败 → Err 上抛，前端静默；历史文件损坏 → 空列表（flowlog 记一次）。

## 前端（main.js）

- `claimedState = { [accountId]: { plans: [...], history: [...], busy } }`。
- 触发点：refreshClaim 循环内、acctRefreshClaim 内（与 claim_refresh 并行调用 claim_current）；claim 成功回调后立即更新。
- 渲染（acctQuotaSlot / claimStripHtml 旁）：
  - 生效中：`已领·<name> · <grants> · 至 <date>`（绿色系小条）
  - 历史：`<name>（已过期）` 弱化灰条
  - 与可领条并存不互斥（可能同时有新活动可领 + 生效中套餐）。
- i18n：`claim.activeStrip`、`claim.expiredStrip` 等。

## 兼容

- 不改 claim_refresh 返回结构（新命令独立）；历史文件只追加。回滚 = revert 单 commit。
