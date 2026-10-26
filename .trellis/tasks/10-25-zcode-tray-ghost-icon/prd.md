# 修复切换账号后 ZCode 托盘幽灵图标堆积

## Goal

切换账号时 `taskkill /F` 强杀 ZCode.exe，进程来不及调用 `Shell_NotifyIcon(NIM_DELETE)` 摘除托盘图标，导致 Windows 通知区域出现幽灵图标：每次切换 +1，鼠标悬停才被 explorer 回收。改为优雅关闭优先（WM_CLOSE / SIGTERM）→ 轮询 → 超时兜底强杀，并统一所有关闭入口。

## 现象与根因

- 现象：点某账号「切换」，账号切换成功；右下角多出一个 ZCode 图标，悬停后消失；多次切换持续累加。
- 根因：`store::kill_zcode()` 在 Windows 上直接 `taskkill /F /IM ZCode.exe`（强制终止），ZCode 无机会执行托盘清理；随后 `restart` 分支再 `launch_zcode()` 拉起新实例 → 幽灵 + 新图标共存。
- 已排除：本应用自身托盘图标只在 `setup` 中 `TrayIconBuilder::with_id("main")` 创建一次，`rebuild_tray()` 仅 `NIM_MODIFY`（tooltip/menu），不具备新增图标能力。

## Requirements

> 方案在实测后调整：原计划的「WM_CLOSE 优雅关闭优先」被证明对 ZCode 无效（见「实测否决」），改为「先摘托盘图标 → 强杀」。

- R1 **强杀前摘图标（Windows）**：`kill_zcode()` 先用 `tasklist` 取 ZCode 全部 PID，`EnumWindows` 找出属于这些 PID 且类名为 `Electron_NotifyIconHostWindow` 的窗口，对 `uID = 0..64` 用 `Shell_NotifyIconGetRect` 探测存在的图标并调 `Shell_NotifyIcon(NIM_DELETE)` 摘除。
- R2 **随后强杀并确认**：立刻 `taskkill /F /IM ZCode.exe`，`zcode_running()` 每 200ms 轮询，最长 8s，返回是否已退出。Windows 不再做优雅关闭尝试（无效果且白等 5s）。
- R3 **非 Windows 保持优雅语义**：`pkill -x zcode/ZCode` 先发 SIGTERM，5s 轮询超时后仍存活才 `pkill -9`，再 8s 硬等待。
- R4 **全局统一入口**：切换流程（`store::switch_to`）、托盘菜单「关闭 ZCode」（`run_tray_action` 的 `kill`）、`kill_zcode` Tauri 命令、CLI `kill` 子命令必须共用同一 `store::kill_zcode()` 实现，不得各自实现关闭逻辑。
- R5 **不清理历史残留**：幽灵图标所有权随进程死亡丢失，外部进程无法可靠删除；不新增相关清理逻辑、不重启 explorer、不新增 UI 噪音。
- R6 **零行为回归**：`kill_zcode()` 的返回语义保持 `Ok(true)=已退出 / Ok(false)=超时未退出 / Err=失败`；`err.zcode.kill_timeout` 仍用于超时提示；不新增 i18n key。

## 实测否决（2026-10-25，真机 Windows 11 / ZCode 14 进程）

| 手段 | 结果 |
|------|------|
| `taskkill /IM ZCode.exe`（投递 WM_CLOSE） | 无效，6s 后 14 进程不变（关闭=最小化到托盘） |
| `WM_QUERYENDSESSION` + `WM_ENDSESSION` 全部顶层窗口 | 无效，进程数不变 |
| 跨进程 `Shell_NotifyIcon(NIM_DELETE)`（hwnd=0xC0A7C, uID=3） | **有效**，GetRect 立即解析失败，进程仍在 |
| 摘图标 → `/F` 强杀 → 重启 | **图标总数不增**（用户目视确认） |

## Constraints

- 不改变 `switch_to` 的 `force` / `restart` / `hot` 语义与返回值结构。
- 不改变 ZCode 启动方式、不引入新依赖。
- 沙箱（`ZCODE_SWITCH_HOME` 已设置）下保持直接 `Ok(true)` 短路，不触碰真实进程。

## Acceptance Criteria

- [x] ✅ `src-tauri/src/store.rs`：Windows 分支 `kill_zcode()` 先 `remove_zcode_tray_icons()` 摘图标，再 `taskkill /F` 强杀，不再做无效的优雅关闭尝试。
- [x] ✅ 摘图标实现：`tasklist` 取 PID → `EnumWindows` 过滤 `GetWindowThreadProcessId` 命中 PID 且类名 `Electron_NotifyIconHostWindow` → `uID 0..64` 用 `Shell_NotifyIconGetRect` 探测 → `Shell_NotifyIcon(NIM_DELETE)` 删除。
- [x] ✅ 非 Windows 分支：先 SIGTERM（`pkill -x`，无 `-9`），5s 超时后才 `pkill -9`。
- [x] ✅ 全仓 `taskkill` / `pkill` 仅出现在 `store::kill_zcode()` 一处；托盘菜单 / CLI / Tauri 命令均只调用 `store::kill_zcode()`。
- [x] ✅ 轮询间隔 200ms、强杀后等待上限 8s 为常量（`FORCED_EXIT_TIMEOUT` / `EXIT_POLL_INTERVAL`）；`GRACEFUL_EXIT_TIMEOUT` 仅保留给非 Windows。
- [x] ✅ `cargo check` 通过，无新增 warning。
- [x] ✅ 沙箱自测：`ZCODE_SWITCH_HOME=$env:TEMP\zsw-sandbox` + `--cli kill` 输出 `{"killed": true, "ok": true}`，不触碰真实进程。
- [x] ✅ 真机单次实测（ZCode 运行中）：`--cli kill` 14 进程 → 0，耗时 2.91s；用户目视确认图标总数未增加。
- [ ] ⏳ 真机整链路实跑（需用户）：在便携版界面里连续切换账号 ≥3 次，右下角 ZCode 图标数量不再随切换次数累加。

## 验证记录（2026-10-25）

| 项 | 命令 | 结果 |
|----|------|------|
| 编译 | `cd src-tauri; cargo check` | exit 0，无 warning |
| 沙箱冒烟 | `$env:ZCODE_SWITCH_HOME="$env:TEMP\zsw-sandbox"; cargo run --quiet -- --cli kill` | `{"killed": true, "ok": true}` |
| 入口唯一性 | `rg "taskkill\|pkill" src-tauri/src` | 命中仅 `store.rs` 的 `kill_zcode()` |
| 真机 kill（Rust 实现） | `.\portable\...exe --cli kill` | 14 → 0 进程，2.91s，图标不增 |
| 便携版产物 | `npm run tauri build -- --no-bundle` → 拷到 `portable/` | 已校验内嵌 `dist/assets/main-*.js`（非 dev 模式） |
| 界面整链路 | 切换 3 次观察托盘 | ⏳ 待用户实跑 |


## Notes

- 幽灵图标只止损不清理属产品决策（已与用户确认）。
- 方案从「优雅关闭优先」调整为「先摘图标再强杀」，依据是实测 ZCode 不响应任何优雅退出信号；取舍是放弃「让 ZCode 自己收尾」，不要那个 5s 空等。
- 若未来 ZCode 升级导致 `Electron_NotifyIconHostWindow` 类名或注册方式变化，`remove_zcode_tray_icons()` 会返回 0 并留痕 `[zsw] 未找到 ZCode 托盘图标…`，行为退化为旧版（强杀 + 幽灵）。
