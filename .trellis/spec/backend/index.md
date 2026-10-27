# Backend Development Guidelines

> Best practices for the Tauri Rust backend (`src-tauri/`).

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Claim Plans Discovery](./claim-plans.md) | 可领套餐双通道发现契约（billing/preview + marketing/touch）与版本上报策略 | Active |
| [Process & Tray Control](./process-and-tray-control.md) | 关闭 ZCode 必须优雅优先（WM_CLOSE / SIGTERM），唯一入口 `store::kill_zcode()`；强杀 `taskkill /F` 会造成通知区域幽灵图标堆积 | Active |
| [z.ai Login Assist Injection](./zai-login-assist-injection.md) | 注入到外部登录页的脚本规则：按 origin 收敛、用可见输入框判表单归属、页内切换优先、轮询去重 | Active |

---

**Language**: All documentation should be written in **English** or bilingual when tied to Chinese domain terms.
