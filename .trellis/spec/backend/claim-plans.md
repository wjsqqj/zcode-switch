# Claim Plans Discovery — billing/preview + marketing/touch

## 1. Scope / Trigger

ZCode 服务端把「可领套餐」分两条通道下发，工具必须双通道合并才能看到全部可领套餐：

- `GET /api/v1/zcode-plan/billing/preview` — 常规可领套餐。
- `GET /api/v1/marketing/touch?seq=0` — 营销活动套餐（如 2026-10 "ZCode 1024 Dev Day"，plan_id `zcode-v3-start-plan-1024`）。**billing/preview 不返回此类活动**；漏掉该通道 = 「刷新资格查不到新活动」。

任何新增可领套餐查询入口（GUI 命令、CLI、自动领取）都必须走 `claim::collect_claimable_plans()`，不要直接调 `preview_plans()`。

## 2. Signatures

```rust
// src-tauri/src/claim.rs
pub fn collect_claimable_plans(home, creds, config, device_mid: String) -> Result<Vec<ClaimPlan>, String>;
    // preview 错误上抛（保持既有报错体验）；marketing 失败静默为空
pub fn marketing_touch_plans(home, creds, config, device_mid: Option<String>) -> Vec<ClaimPlan>;
pub fn merge_marketing_plans(base: Vec<ClaimPlan>, extra: Vec<ClaimPlan>) -> Vec<ClaimPlan>;

// src-tauri/src/quota.rs
pub(crate) fn zcode_app_version() -> String; // = max(注册表探测版本, CLIENT_APP_VERSION)
pub const CLIENT_APP_VERSION: &str;          // 最新已知 ZCode 客户端版本（当前 3.16.3）
```

## 3. Contracts

### marketing/touch 请求

- Method: `GET https://zcode.z.ai/api/v1/marketing/touch?seq=0`
- Headers（对齐客户端，`zai_billing_headers_with_mid` 为其超集）：`X-Device-Mid`（UUID）、`X-Client-Language: zh-CN`、`X-ZCode-App-Version`、`Authorization: Bearer <zcodejwttoken>`（可匿名，但工具总是带账号 token）。
- **版本门槛**：服务端按 `X-ZCode-App-Version` 过滤活动下发（实测 3.11.2 → 空 deliveries，3.16.3 → 返回活动）。上报版本必须取 `max(本机探测版本, CLIENT_APP_VERSION)`，见 `quota::pick_app_version`。

### marketing/touch 响应

```jsonc
{ "code": 0, "data": { "server_time": u64, "language": "zh-CN",
    "deliveries": [ {
      "campaign_id": "...", "priority": 50,
      "resource_position": "banner" | "popup",
      "banner"/"popup": {
        "buttons": [ { "action": { "type": "claim_zcode_plan", "args": { "plan_id": "..." } } } ],
        "background"/"hero": { "args": { "zcode_plan": { /* 与 preview plans 同构：
          plan_id/name/priority/entitlements[show_name/meter/unit_type/grant_units/period] */ } } } } } ] } }
```

- 解析为**递归宽松扫描**（`walk_marketing_delivery`）：任意层级含非空 `plan_id` 且带 `entitlements` 数组或 `name` 字符串的对象 → `parse_plan()`；按钮 `action.type ∈ {claim_zcode_plan, claim_plan}` → 可领 plan_id（兜底 `action.planId`）。不做严格 schema——服务端 payload 位置可能变化。
- 按钮有 plan_id 但无内嵌 payload → 合成最小条目（name=plan_id），保证可见可领。

### 领取

活动套餐领取复用 `billing/claim`（`submit_claim` 不变）：`POST {plan_id}` + 阿里云验证码头。`claim_start` 的 plan 存在性校验必须基于 `collect_claimable_plans` 结果。

## 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| preview 网络错误/HTTP 非 0 | `Err` 上抛（claim_preview/claim_refresh/claim_start 现有报错链路） |
| marketing 网络/code!=0/JSON 解析失败 | `flowlog::log("claim", "marketing-fail", detail)` + 返回 `[]`，**不弹 toast、不影响 preview 结果** |
| 本机无 ZCode 客户端（注册表探测失败） | 上报 `CLIENT_APP_VERSION`（3.16.3） |
| 本机客户端版本 < 常量 | 上报常量；> 常量 → 上报真实版本 |
| 活动已领取 / 不在有效窗口 | 服务端不下发或 claim 返回错误码，前端走现有错误提示 |

## 5. Good/Base/Bad Cases

- **Good**: preview 3 个常规套餐 + marketing 1 个活动套餐 → 合并 4 个，按 priority 降序，无重复 plan_id。
- **Base**: marketing 返回空 deliveries（版本过旧/无活动）→ 仅 preview 结果，正常展示。
- **Bad**: 营销活动只在 marketing 通道有 payload，却只调 `preview_plans` → 活动对用户不可见（本任务修复前的 bug）。

## 6. Tests Required

仓库无单测框架；验证路径：

- `cargo check --manifest-path src-tauri/Cargo.toml` 0 警告。
- CLI 冒烟：`zcode-switch.exe --cli claim-preview` → 未领取活动的账号应出现 `zcode-v3-start-plan-1024` 类条目（grant_items 渲染 GLM-5.3-Flash 3亿 等）。
- 失败降级：断网/改 URL 常量后 CLI 仍应正常输出 preview 结果且 `logs/oauth.log` 出现 `[claim] marketing-fail`。

## 7. Wrong vs Correct

### Wrong

```rust
// 只查 preview —— 营销活动永远不可见
let plans = claim::preview_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))?;
```

### Correct

```rust
// 双通道合并：preview（错误上抛）+ marketing/touch（静默降级）
let plans = claim::collect_claimable_plans(&paths.home, &acc.credentials, acc.config.as_ref(), mid)?;
```

---

## 8. 领取错误语义与重试纪律（2026-10-24 补充，任务 10-24-claim-hardening）

### ClaimError 语义方法

```rust
pub const CLAIM_ERR_CHALLENGE: i64 = -2;
impl ClaimError {
    pub fn is_captcha_rejected(&self) -> bool { self.code == 3007 }      // 可换新参数重试一次
    pub fn is_terminal(&self) -> bool { (1001..=1005).contains(&self.code) } // 不再重试
    pub fn is_challenge(&self) -> bool { self.code == CLAIM_ERR_CHALLENGE }  // 人机校验拦截，非凭证失效
}
```

- HTTP 401/403 携带 `x-aliyun-captcha-verify-param` 头或 body 含 captcha 字样 → `-2` 挑战（zcode2api 教训：当登录过期处理会错杀健康账号）。`failure_payload` 结构化输出 `terminal`/`challenge`，前端不得用 code 区间反推。
- 3007 **不自动重试**（2026-10-26 变更，吸收 zcode2api 2.6.5 整改结论）：拒码本身是上游风险信号，换码连打只会加剧。`claim_captcha_submit` 收到 3007 直接走终态失败并记 `flowlog` `captcha-rejected-3007`，由用户重新发起领取（重新取参数）。`captcha_attempt` 字段与 `{retry:true}` 协议已移除。
- 前端自动轮 (账号,套餐) 级 `claimHold`：终态 Infinity、1005 至 nextAt（无则 1h）、挑战 10min。

### 验证码配置

`fetch_captcha_config()` 进程内缓存 600s；失败回退上次成功值；再无 → 内置默认 `prefix=no8xfe / sceneId=11xygtvd / region=cn`（zcode2api 与 plan-claim 观测一致）。测试用 `reset_captcha_config_cache_for_tests()` 清缓存。

## 9. 风控退避（2026-10-24 补充，任务 10-24-risk-quota-guard）

`risk.rs`：3012/unusual activity 命中记 strike；900s×2ⁿ⁻¹ 指数退避封顶 24h；7 天窗口内累计 4 次升级 24h；窗口外 strike 衰减不累计；到期自动恢复。持久化 `risk-state.json`（仅时间戳）。

- 冷却期内自动路径零上游流量：`get_account_quota` / `claim_refresh` / `claim_start(auto)` 直接拒绝；手动操作经 `force=true` 放行（用户显式意图优先）。
- 观测注入点：所有上游 Err 经 `risk::observe_upstream_failure` 过滤（仅 3012/unusual/风控字样记 strike，网络抖动不误伤）。

## 10. 已领取状态（2026-10-24 补充，任务 10-24-claimed-state）

- `fetch_active_plans` → `GET billing/current`（`data.plans[{user_plan_id,plan_id,name,status,starts_at,ends_at,entitlements}]`，秒级时间戳×1000 转 ms）。
- **时间语义（2026-10-26 实测补充）**：`plan.starts_at` 是活动/计划开始时间，**不是生效时间**；真正生效时间在 `entitlements[].effective_at`（样例：`ZCODE GLOBAL BUILD` 的 `starts_at=19:15:53`、`entitlements[0].effective_at=23:00:00`、`ends_at=次日 09:00`）。任何「生效时间」展示必须优先 `effective_at`，其次 `starts_at`，都没有标「未提供」。
- **诊断命令**：`--cli claim-current --id` 查已生效套餐 + 本地历史；`--cli claim-raw --id [--only preview|marketing|current]` 原样打印上游 JSON（排查「为什么这个账号看不到活动」）。实测：服务端对无资格账号两通道均返回 `code=0` 且数组为空，**不提供原因字段**，本地不得据此编造原因。
- 领取成功落 `claim-history.json`（200 条滚动，仅元数据）；`claim_current(id)` 命令返回 `{plans, history}`；current 失败前端静默。
- preview 为空 ≠ 无活动（可能已领取）——用 current + 历史区分。

## 11. 版本链与测试旁路（2026-10-24 补充，任务 changelog-version / mock-upstream-tests）

- 上报版本 = `max(注册表探测, changelog 缓存最新, CLIENT_APP_VERSION 常量)`；changelog 缓存 `{home}/.zcode-switch/changelog-cache.json`（24h TTL，启动后台线程保鲜，请求只读缓存）；版本提取只认 `Release v3.x.y` / `releases/3.x.y/` 锚点（页面 SVG path 坐标与千分位数字会产生 57.787.384 类假阳性）。
- 测试旁路：`ZCODE_API_BASE`（claim 全部端点）、`ZCODE_CHANGELOG_URL`；URL 由 `claim.rs` 的 `billing_*_url()/client_configs_url()/marketing_touch_url()` 函数拼出，禁止再引入硬编码 URL 常量。
- mock 测试：`src-tauri/tests/support/mod.rs`（TcpListener 最小 HTTP，非阻塞 accept + 轮询 shutdown，否则 Drop join 死锁）；用例在 `tests/mock_upstream.rs`，env 相关用例必须持 `ENV_LOCK` 串行。

---

**历史**: 2026-10-24 新增（marketing/touch 接入）。同日补充 §8-§11（吸收 zcode-proxy/zcode2api/zcode-plan-claim 经验的五任务批次）。2026-10-26 补充 §12（刷新状态机与阻塞 I/O 纪律）、§13（请求头坑位表与冷却门禁）。根因排查与协议还原记录见 `.trellis/tasks/archive/`。

---

## 13. 请求头坑位表与冷却门禁（2026-10-26 补充，吸收 zcode2api / zcode-plan-claim）

### 13.1 请求头 → 错误码对照（排障查表，别再猜）

| 症状 | 最可能缺失/错误的头 | 说明 |
|---|---|---|
| 3007 验证码失败 | `X-Aliyun-Captcha-Verify-Region` 缺失/与参数不同源 | 参数与地区必须成套；参数还要求「原样 JSON 字符串」，被二次序列化同样报 3007 |
| 3001 参数错误 | `X-Platform=win32` 之类平台值与客户端不一致 | `billing/preview` 容错、`client/configs` 直接拒 |
| preview 恒空 / 无活动 | `X-ZCode-App-Version` 落后 | 版本门槛会过滤活动；必须取 `max(本机探测, 最新已知)` |
| 401 空体 | 用了 oauth access_token 而非 `zcodejwttoken` | 领取链路只认 `zcodejwttoken` |
| 3012 / unusual activity | 出口 IP 与验证码求解不一致、请求过于规律 | 见 §9 退避；求解与 claim 必须同出口 |
| 幂等异常 | `X-Device-Mid` 每次变化 | 每账号必须稳定复用同一 mid |

必须成套发送（`preview_requests_carry_identity_headers` 用例锁定）：`Authorization`、`X-Device-Mid`、`X-ZCode-App-Version`、`X-Platform`、`X-Os-Category`、`X-Client-Language`、`X-Client-Timezone`、`X-Title`、`User-Agent`。

### 13.2 冷却期零上游流量（所有 billing 入口）

`risk::is_cooling` 必须在**每一个** billing 入口前判断，当前覆盖面：

| 命令 | 冷却期行为 |
|---|---|
| `get_account_quota` | 非 `force` 直接返回 `err.risk.cooling` |
| `claim_refresh` | 非 `force` 直接返回 `err.risk.cooling` |
| `claim_preview` | 直接返回 `err.risk.cooling`（2026-10-26 补齐） |
| `claim_current` | **不发上游**，只返回本地 `claim_history_for`（2026-10-26 补齐；避免刷新击穿冷却） |
| `claim_start` | 仅 `auto=true` 时拦；手动领取是显式意图，放行 |

新增任何 billing 命令时，先对照此表决定冷却期行为，不得遗漏。

### 13.3 测试侧请求规格断言

`tests/support` 的 mock 会记录每条请求的 `path/headers/body`（`MockUpstream::requests()`），用例应同时断言：

- 响应解析正确（业务语义）；
- **请求规格正确**：头成套、body 仅含允许字段（领取只有 `plan_id`）、GET 无 body；
- 安全不变量：凭证不出现在 body、只读命令不产生写请求。


---

## 12. 刷新状态机与阻塞 I/O 纪律（2026-10-26 补充，任务 10-26-claim-refresh-state-fix）

### 12.1 async 命令禁止直接跑阻塞 I/O

`claim_preview` / `claim_refresh` / `claim_current` / `claim_start` / `claim_captcha_config` / `claim_captcha_submit` / `get_account_quota` / `get_live_quota` 一律经 `lib::run_blocking(...)`（内部 `tauri::async_runtime::spawn_blocking`）执行。

**原因（实测）**：一次 `claim_refresh` 最长阻塞 2×10s（activation 事件）+ 25s（preview）+ 25s（marketing/touch）。直接在 async 命令里跑 `ureq` 会占满异步运行时 worker；用户连点 3 个账号后，`get_state` / `set_behavior` / `claim_cancel` 全部排队 → 整窗卡死、额度区永远「查询中…」。

新增任何网络/进程类命令时遵循同一纪律；`switch_to`、`kill_zcode` 因持有 `STORE_LOCK`（`MutexGuard` 非 `Send`）暂未包装，改动时须把 `store_guard()` 一并移入闭包。

### 12.2 前端刷新准入与反馈契约

- 唯一判据 `refreshGate(scope)`（`scope = "global" | accountId`），返回 `{ok}` 或 `{ok:false, kind:"running"|"cooldown", waitSec}`；`toastGate()` 负责把失败原因变成 toast。
- **禁止用 `disabled` 表达「忙/冷却」**：`disabled` 会吞掉 click，用户得不到任何提示（本次 bug 的直接成因）。改用 `is-off` class + 可点击，点击后由 gate 说明原因。
- 冷却必须可见：行内按钮角标显示剩余秒数，顶部按钮显示 `刷新 (Ns)`；`refreshTicker` 每秒重绘保证倒计时递减。
- 全局刷新显示 `done/total · 当前账号名`，账号间 5s 间隔期间不再出现「无任何更新」。
- 所有 `claim_*` / `get_account_quota` 前端调用走 `invokeWithTimeout`（60s 看门狗），超时清 `busy` 并提示，杜绝永久 busy。

### 12.3 render 异常兜底

`render()` 只做 try/catch，实际渲染在 `renderInner()`；异常时：
- 不清空页面，展示挂在 `body` 上的 `#err-bar`（可关闭），保证按钮仍可点；
- 调用 `log_client_error` 写 `{home}/.zcode-switch/client-error.log`（滚动 200 行）；
- `window.onerror` / `unhandledrejection` 同样落盘。

排障路径：用户反馈「界面卡住」→ 先读 `client-error.log`，不要再靠猜。

### 12.4 风控矩阵（3012 现场报文）

- `risk::tests::observe_hits_3012_blocked_payload`：`unusual activity` + `provider_code=3012` + `status=405` 报文 → 记 strike、首次 ~900s、二次 ~1800s。
- `mock_upstream::preview_blocked_unusual_activity_3012_surfaces_error`：同上报文经 405 返回时错误上抛且保留 `405` 与 `unusual activity` 文本（供 `observe_upstream_failure` 命中）。
- `risk` 单元测试会设置进程级 `ZCODE_SWITCH_HOME`，**必须持 `TEST_LOCK` 串行**，否则并行测试互相覆盖 home 导致偶发失败（本次一并修复）。
