# Backend Development Guidelines

> Best practices for the Tauri Rust backend (`src-tauri/`).

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Claim Plans Discovery](./claim-plans.md) | 可领套餐双通道发现契约（billing/preview + marketing/touch）与版本上报策略 | Active |
| [Process & Tray Control](./process-and-tray-control.md) | 关闭 ZCode 必须优雅优先（WM_CLOSE / SIGTERM），唯一入口 `store::kill_zcode()`；强杀 `taskkill /F` 会造成通知区域幽灵图标堆积 | Active |

---

**Language**: All documentation should be written in **English** or bilingual when tied to Chinese domain terms.
