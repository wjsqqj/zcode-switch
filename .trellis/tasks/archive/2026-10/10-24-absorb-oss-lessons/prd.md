# 吸收开源项目经验：版本保鲜/领取加固/已领取状态/风控防线/mock 测试（父任务）

## Goal

从 `D:/IdeaProjects2025/zcode` 下四个开源项目（zcode-proxy、zcode2api、zcode-plan-claim、zcode-butler）吸收实战经验并落地到 zcode-switch。调研结论：zcode-proxy 与 zcode2api 本身大量移植自本项目，吸收重点是它们之上的实战增量。

## 任务地图（子任务）

| 子任务 | 交付物 | 来源 |
|---|---|---|
| 10-24-changelog-version | A 版本保鲜：changelog 拉最新版 + 缓存 + 三方 max | 用户新思路（已实测可行），zcode2api 版本门槛实证 |
| 10-24-claim-hardening | B 领取链路加固：验证码挑战排除 / 3007 重试一次 / 终态不重试 / 1005 记忆 / 验证码配置缓存兜底 | zcode2api 实测教训 + zcode-plan-claim CLAIM-LOGIC.md |
| 10-24-claimed-state | C 已领取状态展示：billing/current + 本地历史 + UI 四态 | zcode-plan-claim status 设计 |
| 10-24-risk-quota-guard | D 风控退避 + 额度防线：3012 指数退避/strikes 衰减；额度排除赠送池误判 | zcode2api risk_penalty + _bonus_active |
| 10-24-mock-upstream-tests | E 上游 mock 测试体系：轻量 mock + 故障注入测试 | zcode2api tests/mock_upstream |

## 执行顺序

A → B → C → D → E（E 的 mock 需要前序任务引入的 URL env 旁路；B/C/D 的错误分类互相依赖，按序收敛）。

## 跨子任务验收

- [ ] 全部子任务 `cargo check` 0 警告、i18n 键检查通过。
- [ ] 每个子任务独立 commit、独立归档。
- [ ] 不破坏既有行为：全量刷新、单账号刷新、自动领取、验证码窗口流程回归可用。

## 关键调研结论存档（子任务 PRD 的共同上下文）

- 版本门槛：服务端按客户端版本过滤活动下发；3.11.2 拿不到新活动（zcode2api constants.py:44-51 独立印证）。
- changelog 页面 SSR：初始 HTML 含版本号，`3\.\d+\.\d+` 正则可提取（2026-10-24 实测 100KB、8 个版本、最新 3.16.3@2026-10-23）；客户端 app-update.yml 指向 localhost 不可用。
- 验证码参数一次性：同 param 二次 claim 必 3007；仅允许换新参数重试一次；1001~1005 终态。
- marketing/touch：账号领取后该账号 deliveries 返回空（plan-claim 2026-10-11 实测）。
- 403 可能是验证码挑战而非凭证失效（zcode2api：错杀过健康账号）。
- billing/claim 只认 zcodejwttoken，oauth access_token 同为 JWT 格式但 401（plan-claim §6.1，本项目 claim_token 已正确）。
- platform 参数官方客户端发 `${process.platform}-${process.arch}`（如 win32-x64），本项目一致。
