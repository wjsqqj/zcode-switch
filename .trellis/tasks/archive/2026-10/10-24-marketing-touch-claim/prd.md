# 刷新资格查不到新版 ZCode 营销活动套餐

## Goal

修复「刷新资格」查不到新版 ZCode 活动套餐的问题：接入 `/api/v1/marketing/touch` 活动下发通道，把营销活动携带的可领套餐合并进可领列表，使其可显示、可手动/自动领取；并为每个账号提供单独的「刷新」入口。

## Background（根因，已实证）

- 新版活动（如 2026-10 的 "ZCode 1024 Dev Day"，plan_id `zcode-v3-start-plan-1024`）只通过营销触达接口 `GET /api/v1/marketing/touch?seq=0` 下发，响应的 `data.deliveries[]` 里 banner/popup 的按钮带 `claim_zcode_plan` action（`args.plan_id`），并在 `background/hero` 的 `args.zcode_plan` 内嵌完整套餐结构（字段与 `billing/preview` 的 plans 同构：plan_id/name/priority/entitlements[show_name/meter/unit_type/grant_units/period]）。
- 工具目前只查 `GET /api/v1/zcode-plan/billing/preview`，该接口不返回此类营销活动套餐 → 查不到、不能自动领。
- 领取链路一致：客户端弹窗领取就是 `POST /api/v1/zcode-plan/billing/claim {plan_id}` + 阿里云验证码头，工具现有 claim 流程可复用，但 `claim_start` 目前要求 plan 必须出现在 preview 结果里。
- 服务端按客户端版本做活动分发门槛：实测 app_version=3.11.2 返回空 deliveries，3.16.3 返回活动。工具上报的版本取自注册表探测本机客户端版本，探测失败回退硬编码 `CLIENT_APP_VERSION=3.11.2`（已过时）。
- marketing/touch 请求头（对齐客户端实现）：`X-Device-Mid`（UUID）、`X-Client-Language`、`X-ZCode-App-Version`、`Authorization: Bearer <token>`（工具的 `zai_billing_headers_with_mid` 为其超集，可直接复用）。

## Requirements

1. **活动套餐发现**：`claim_refresh` / `claim_preview` / `claim_start` 统一改为「billing/preview + marketing/touch」双通道合并查询，按 plan_id 去重（preview 结果优先），合并后按 priority 降序排序。
2. **版本上报策略**：上报版本取 `max(本机探测到的客户端版本, 内置最新已知版本常量)`；内置常量从 3.11.2 升级为 3.16.3。
3. **领取支持**：`claim_start` 的 plan 查找范围包含活动套餐，使活动套餐可走现有验证码窗口领取流程（含自动领取轮次——活动套餐合并进可领列表后自动被自动领取覆盖）。
4. **每账号刷新按钮**：账号行 `row-actions` 新增「刷新」图标按钮，仅刷新该账号的资格与可领套餐（调用现有 `claim_refresh` 单账号命令），不触发其他账号。
5. **异常分支（已确认）**：marketing/touch 查询失败（网络/接口变更/解析失败）时静默降级——不弹提示、不影响 preview 结果，仅写本地日志（复用 flowlog，flow 标签 `claim`）。
6. **展示（已确认）**：活动套餐直接合并进现有可领列表渲染，不加来源标记、不改前端渲染逻辑。
7. 前端错误提示复用现有 `m.refreshClaimAcctErr` 链路；成功时无需 toast（claim strip 自然出现）。

## Constraints

- 不改动领取接口协议（billing/claim + 验证码头）与验证码窗口流程。
- marketing/touch 请求头对齐客户端（X-Device-Mid/X-Client-Language/X-ZCode-App-Version/Authorization），不新增遥测、不调用 `marketing/touch/action` 上报。
- 不提取、不落盘任何凭据明文到日志。

## Acceptance Criteria

- [ ] 对返回活动的账号执行「刷新资格」或账号行「刷新」后，可领列表包含 marketing/touch 下发的活动套餐（如 zcode-v3-start-plan-1024），名称/额度正常渲染。
- [ ] 点击活动套餐的「领取」能进入现有验证码窗口流程；`claim_start` 不再因 plan 不在 preview 中而报 gone。
- [ ] 「自动领取」开启时，活动套餐纳入自动轮次。
- [ ] 本机无 ZCode 客户端（注册表探测失败）时，上报版本为 3.16.3；本机客户端版本更高时上报真实版本。
- [ ] marketing/touch 请求失败时刷新流程不报错、可领列表仍展示 preview 结果，`logs/oauth.log` 有对应失败记录。
- [ ] 每个账号行有独立「刷新」按钮，点击仅刷新该账号；全量「刷新资格」按钮行为不变。
- [ ] `cargo check` 通过；中英文文案齐全。

## Notes

- 决策记录（grill-me 已确认）：版本策略=max(真实, 最新已知 3.16.3)；展示=直接合并；自动领取=纳入；异常=静默降级+本地日志。
- 活动有 starts_at/ends_at（秒级时间戳），服务端自行控制下发窗口，工具不做本地有效期过滤。
