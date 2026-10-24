# Implement: changelog 版本保鲜

## 1. 新增 src-tauri/src/changelog.rs
- [ ] `CHANGELOG_URL`、`pub fn latest_known_version(home: &Path) -> Option<String>`
  - 读缓存（{home}/changelog-cache.json：{version, fetched_at}，TTL 24h）
  - 未命中/过期 → GET（agent 10s 超时）→ 正则 `3\.\d+\.\d+`（手动扫描，不引 regex 依赖：匹配模式固定简单）→ semver 最大 → 写缓存
  - 全失败 → flowlog("version","changelog-fail") + None
  - env `ZCODE_CHANGELOG_URL` 旁路（mock 测试用，E 子任务断言依赖）
- [ ] quota.rs：`pick_app_version` 纳入 changelog 结果（三方 max）；需要 home 参数——zcode_app_version() 无 home，改从调用方传入或用 Paths 探测（设计：zcode_app_version 保持无参，changelog 缓存放 exe 同级/数据目录，由 quota::zcode_app_version 内部用 Paths::detect().home）
- 验证：cargo check；CLI 冒烟（临时加 debug 输出或依赖 E 的测试）

## 2. 质量检查
- [ ] cargo check 0 警告；缓存文件损坏注入测试（手工改坏 json 后运行不崩）
