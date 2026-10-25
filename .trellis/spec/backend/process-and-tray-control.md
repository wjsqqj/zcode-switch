# Process & Tray Control — 关闭 ZCode 的正确姿势

## 1. Scope / Trigger

任何「结束 ZCode 进程」的代码路径都属于本契约：

- 切换账号（`store::switch_to` 非热切换分支）
- 托盘菜单「关闭 ZCode」（`lib::run_tray_action` 的 `kill`）
- Tauri 命令 `kill_zcode`（前端按钮）
- CLI `--cli kill`

**唯一入口**：`store::kill_zcode()`（`src-tauri/src/store.rs`）。新增关闭入口只允许调用它，不得另写 `taskkill` / `pkill`。

## 2. 为什么强杀会留下「幽灵图标」

ZCode 是 Electron 应用（`D:\java\ZCode`，`app.asar` + `resources\tray_icon.ico`），托盘图标由主进程通过 `Shell_NotifyIcon` 注册。`taskkill /F` 走 `TerminateProcess`，进程没有机会调用 `Shell_NotifyIcon(NIM_DELETE)`，图标所有权随进程死亡丢失 → 通知区域留下幽灵图标：

- 图标仍显示，点击无反应；
- 鼠标划过通知区域时由 explorer 回收（看起来「悬停就少一个」）；
- 每次强杀 +1，多次切换持续累加。

## 3. 实测结论：ZCode 没有可外部触发的优雅退出通道

在真机（Windows 11 / ZCode 14 个同名进程）逐一验证过：

| 手段 | 结果 |
|------|------|
| `taskkill /IM ZCode.exe`（不带 `/F`，投递 `WM_CLOSE`） | 无效，6s 内进程数不变（关闭 = 最小化到托盘） |
| `WM_QUERYENDSESSION` + `WM_ENDSESSION`（对全部顶层窗口 SendMessage） | 无效，进程数不变 |
| 主窗口特征 | `Chrome_WidgetWin_1`，`IsWindowVisible = false`（已藏到托盘） |

因此**不要在 Windows 上做「优雅关闭 + 超时回退」**：只会白等超时时间，最后仍然强杀并留图标（v1.7.0-beta.1 首版实现踩过，切换被拖慢 5s 且问题未解）。

## 4. 采用的方案：先摘图标，再强杀

托盘图标可按 `hwnd + uID` 用 `Shell_NotifyIconGetRect` 探测；**`Shell_NotifyIcon(NIM_DELETE)` 允许跨进程删除**（实测：替 ZCode 删除后 `Shell_NotifyIconGetRect` 立即解析失败，ZCode 进程仍在运行）。注意**不要按窗口类名筛选**：ZCode/Electron 的窗口类实测为单字符（`C`/`E` 等），托盘宿主没有稳定类名契约，按 PID + uID 探测即可。

流程（`store::kill_zcode()`，Windows）：

1. `zcode_pids()`：`tasklist /FI "IMAGENAME eq ZCode.exe" /FO CSV /NH` 取全部 PID（Electron 多进程同名）。
2. `EnumWindows` 遍历顶层窗口，凡 `GetWindowThreadProcessId` 命中上述 PID 的窗口都进入探测（不看类名）。
3. 对每个窗口遍历 `uID = 0..64`，`Shell_NotifyIconGetRect` 命中即调 `Shell_NotifyIcon(NIM_DELETE)` 摘除（Electron 实测 `uID = 3`，但不同版本可能变化，故按区间探测而非写死）。
4. `taskkill /F /IM ZCode.exe` 强杀，`wait_zcode_exit(8s)` 轮询确认退出。

关键点：

- 摘图标必须发生在强杀**之前**；杀完再摘要么 hwnd 已失效、要么幽灵已生成。
- 摘不到图标（`removed == 0`，窗口类变更或 ZCode 本来无图标）时 `eprintln!` 留痕，然后照常强杀。
- 非 Windows 保留「SIGTERM → 5s → SIGKILL」：macOS/Linux 的 SIGTERM 是真正的退出信号，Electron 会走 `will-quit` 销毁托盘图标。

## 5. Contracts

```rust
// src-tauri/src/store.rs
const ZCODE_IMAGE: &str = "ZCode.exe";               // Windows 映像名
fn zcode_pids() -> Vec<u32>;                         // tasklist CSV 第二列
pub fn zcode_running() -> bool;                      // = !zcode_pids().is_empty()
const FORCED_EXIT_TIMEOUT: Duration = 8s;            // 强杀后硬等待
const EXIT_POLL_INTERVAL: Duration = 200ms;          // 存活轮询
#[cfg(not(windows))] const GRACEFUL_EXIT_TIMEOUT: Duration = 5s; // 仅 SIGTERM 路径使用
fn wait_zcode_exit(timeout: Duration) -> bool;       // true = 已退出
fn remove_zcode_tray_icons() -> usize;               // Windows：强杀前摘图标，返回摘掉数量
pub fn kill_zcode() -> Result<bool, String>;         // Ok(true)=已退出 / Ok(false)=超时 / Err
```

- 返回语义与 `err.zcode.kill_timeout` 文案未变，调用方无需改动。
- 沙箱（`ZCODE_SWITCH_HOME` 已设置）直接 `Ok(true)` 短路，测试路径绝不触碰真实进程。

## 6. Known Limits

- 只能摘「当前还挂在 ZCode 进程窗口上」的图标；ZCode 升级后若注册方式变化（探测全部落空），`removed == 0` 会退化为旧行为（强杀 + 幽灵），并有 `eprintln!` 留痕可查。
- 历史遗留幽灵图标不处理：所有权已丢失，无法可靠删除，悬停即被 explorer 回收。
- 摘图标与强杀之间理论上存在竞态（ZCode 若在此窗口期重建图标则摘不到），实测窗口 < 50ms，可忽略。

## 7. Verification

```powershell
# 编译门禁
cd src-tauri; cargo check

# 沙箱冒烟：不触碰真实 ZCode，应输出 {"killed": true, "ok": true}
$env:ZCODE_SWITCH_HOME="$env:TEMP\zsw-sandbox"; cargo run --quiet -- --cli kill

# 真机（ZCode 运行中）：应 1~3s 内 14 → 0 个进程，且托盘不多出图标
.\portable\zcode-switch-1.7.0-beta.1-portable.exe --cli kill
```

真机验收（无法自动化）：ZCode 运行中连续切换 ≥3 次，右下角 ZCode 图标数量不随切换次数累加。

## 8. Portable 产物（踩坑：不要直接 cargo build --release）

`tauri` 2.x 用 `custom-protocol` **feature** 区分生产 / 开发模式（`tauri/build.rs`：`dev = !has_feature("custom-protocol")`）。仓库的 `Cargo.toml` 没有声明该 feature，只由 Tauri CLI 在非 dev 构建时注入。

- ❌ `cargo build --release` → 产物是 **dev 模式**：内嵌 `devUrl`，启动后浏览器里报 `127.0.0.1 拒绝连接 / ERR_CONNECTION_REFUSED`。
- ✅ 便携版正确构建（产物在 `src-tauri/target/release/zcode-switch.exe`，内嵌 `dist`，可直接拷走单文件运行）：

```powershell
npm run tauri build -- --no-bundle
```

自检：二进制里应能搜到 `dist/assets/main-*.js` 这类资源名；若搜不到、只有 `127.0.0.1:5173`，即为 dev 模式误产物。
