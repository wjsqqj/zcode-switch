# Implement: 上游 mock 测试体系

## 1. 前置铺路（若前序任务未覆盖）
- [ ] 核对 A/B/C/D 已引入 ZCODE_API_BASE / ZCODE_CHANGELOG_URL 旁路；缺则补
## 2. Mock 支架
- [ ] tests/support/mock.rs（TcpListener 最小 HTTP）
## 3. 测试矩阵
- [ ] 纯函数组：version_ge / merge_marketing_plans / walk_marketing_delivery / risk strikes
- [ ] HTTP 组：preview 解析矩阵 / marketing 矩阵 / changelog 提取+缓存 TTL / captcha 配置回退 / 3012 退避序列
## 4. 质量检查
- [ ] cargo test 全绿；断网可跑；cargo check 0 警告
