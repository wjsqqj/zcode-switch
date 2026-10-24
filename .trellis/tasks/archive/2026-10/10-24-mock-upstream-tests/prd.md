# E 上游 mock 测试体系

## Goal

参考 zcode2api 的 mock_upstream 思路，为 zcode-switch 建立零外部依赖的轻量 mock 上游（std TcpListener）+ claim/quota 故障注入测试，锁定协议语义，防止上游适配回归。

## Requirements

1. **URL 旁路**：claim.rs/quota.rs/changelog.rs 的上游基址支持 env 旁路 `ZCODE_API_BASE`（默认 https://zcode.z.ai），测试指向本地 mock 端口。
2. **轻量 mock**：tests 支持模块，监听 127.0.0.1 随机端口，按路径返回脚本化 JSON（支持按请求计数返回不同响应，实现故障序列）。
3. **测试矩阵**（至少覆盖）：
   - preview 解析：正常 plans / 空 plans / 非零 code / 401（凭证失效文案）
   - marketing/touch：内嵌 zcode_plan 提取 / 按钮 plan_id 无 payload 合成最小条目 / 重复 plan_id 去重 / 空 deliveries / code!=0 静默
   - merge：preview 优先去重 + priority 排序
   - 版本：version_ge 语义；changelog HTML 多版本取最大 / 非法 HTML 回退 / 缓存 TTL 内不重复请求
   - 验证码配置：成功缓存 / 失败回退默认
   - 3012 退避：首次 900s、连续命中指数、4 次升级 24h、7 天窗口衰减
   - claim 错误分类：3007/1001-1005/挑战 403
4. 测试不依赖网络（127.0.0.1）；`cargo test` 一键运行；不引入新第三方依赖。
5. 测试不打印/不依赖真实凭证（mock token 字符串即可）。

## Acceptance Criteria

- [ ] `cargo test` 全绿，覆盖上述矩阵核心断言（≥15 个用例）。
- [ ] 断网环境可运行（无外网请求）。
- [ ] cargo check 0 警告。

## Notes

- 复杂任务：design.md + implement.md 已备。
- 仓库历史上有意移除过单测（11496a8）；本次按用户要求重建测试资产，范围限定 claim/quota/version 链路。
