# A changelog 版本保鲜

## Goal

上报版本自动跟随官方最新客户端版本：从 https://zcode.z.ai/cn/changelog 拉取版本号列表并本地缓存，上报版本改为 max(注册表探测版本, changelog 最新版, 内置常量)，免去"官方发新版就要重新构建工具"。

## Requirements

1. 新增 changelog 抓取：GET https://zcode.z.ai/cn/changelog（SSR 页面，无需 JS），正则提取全部 `x.y.z` 版本号，取 semver 最大值。
2. 本地缓存（数据目录下 json 文件），TTL 24h；过期后台刷新原则同现有代码风格（同步实现即可，10s 超时）。
3. 抓取失败/解析失败/无匹配 → 静默回退，不影响现有版本链路；记 flowlog。
4. 版本解析链：max(注册表探测, changelog 最新, CLIENT_APP_VERSION 常量)；比较复用 version_ge 语义（扩展为三方）。
5. 缓存文件损坏/版本字段非法 → 视为无缓存。

## Acceptance Criteria

- [ ] 有网络时首次运行后缓存文件生成，上报版本 ≥ 内置常量。
- [ ] 断网时回退常量，`logs/oauth.log` 有失败记录，流程不报错。
- [ ] mock 测试：HTML 内多版本号取最大；非法 HTML 回退；缓存 24h 内不重复请求（E 子任务覆盖断言）。
- [ ] cargo check 0 警告。

## Notes

- 轻量任务，设计要点并入 implement.md。
- changelog 页面 2026-10-24 实测：100KB SSR HTML，`3\.\d+\.\d+` 提取 8 个版本，最新 3.16.3。
