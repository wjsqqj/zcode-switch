# 设计：刷新状态机与阻塞 I/O 整改（子任务 A）

## 1. 边界

- 只改「刷新/领取」相关状态机与后端命令执行方式；不动协议、不动领取语义、不动切换逻辑。
- 前端文件：`src/main.js`（状态与渲染）、`src/ui.js`（仅当需要错误条组件）、`src/locales/{zh,en}.js`。
- 后端文件：`src-tauri/src/lib.rs`（命令包装）、`src-tauri/tests/mock_upstream.rs`（3012 用例）。

## 2. 前端状态机

### 2.1 唯一准入判据

现状是 3 个函数各自写一遍「忙不忙」判断，且判断为真时静默 return。改为单一函数：

```js
// 返回 { ok: true } 或 { ok:false, kind: 'running'|'cooldown'|'challenge', waitSec? }
function refreshGate(scope /* global | id */) { ... }
```

- `refreshClaim()` / `acctRefreshClaim()` / `claimAll()` / `claim()` / `toggleAutoClaim()` 统一调用它，`ok:false` 时按 `kind` 弹对应 toast（新增 i18n key：`m.refreshBusyRunning` / `m.refreshCooldown` / `m.claimBusy`）。
- 冷却剩余秒数由 `cooldownRemaining(scope)` 计算，渲染时直接读，不再靠点击才提示。

### 2.2 冷却可视化

- 行内按钮文案：冷却期显示 `↻ 58s`（保留 icon，不新增布局）。
- 顶部按钮：冷却期显示 `刷新 (58s)` 并 disabled。
- 倒计时复用现有 `refreshTicker`（1s tick 已存在），确保 ticker 在冷却期内不停止（现有 `stopRefreshTickerIfIdle` 已按冷却判断，保持）。

### 2.3 render 异常兜底

```js
function render() {
  try { renderInner(); clearRenderError(); }
  catch (e) { showRenderErrorBanner(e); logClientError(e); }
}
```

- 错误条是不进 `$app.innerHTML` 的独立 DOM 节点（`#err-bar`），保证 render 全挂时仍可见、可关闭。
- `logClientError` 通过新命令 `log_client_error` 写 `{home}/.zcode-switch/client-error.log`（滚动保留最近 200 行），避免用户看不到 DevTools 时无从诊断。
- 同时注册 `window.onerror` / `unhandledrejection` 写入同一日志。

### 2.4 请求看门狗

```js
const REQ_TIMEOUT_MS = 60_000;
async function invokeWithTimeout(cmd, args, ms = REQ_TIMEOUT_MS) { ... Promise.race([invoke, timeout]) }
```

- 超时抛 `err.timeout`，由调用方 catch 后清 busy + toast；后端请求仍在跑但前端不再无限 busy。

### 2.5 全局刷新进度

- `refreshClaim` 每账号请求前 render 一次，并在顶部按钮旁显示 `3/6 · tcaiq772`（复用现有 `done/total` 字段，新增当前账号名）。
- 账号间隔 5s 保持（避免瞬时打满上游），但因有进度展示，用户不再感知为卡死。

## 3. 后端阻塞 I/O

```rust
#[tauri::command]
async fn claim_refresh(...) -> Result<ClaimRefreshResult, String> {
    tauri::async_runtime::spawn_blocking(move || { /* 现有同步逻辑 */ })
        .await
        .map_err(|e| trf("err.internal", ...))?
}
```

- 覆盖：`claim_preview`、`claim_refresh`、`claim_current`、`claim_start`、`claim_captcha_config`、`get_account_quota`、`get_live_quota`、`kill_zcode`（已在阻塞线程的除外，逐一核对）。
- `store_guard()` 仍在阻塞段内使用，保持串行语义；`spawn_blocking` 任务数与 Tauri 的 blocking 池（默认较大）匹配，不再占用 async worker。

## 4. 测试

- `src-tauri/tests/mock_upstream.rs` 新增用例：上游返回 `status=405` + body 含 `request has been blocked due to unusual activity` + `provider_code=3012` → 断言 `risk::observe_upstream_failure` 记 strike、`is_cooling` 为真、`claim_refresh(force=false)` 返回冷却错误、`force=true` 放行。
- 既有 mock 用例回归不得减少。

## 5. 回滚

- 前端：状态机改动集中在 `refreshGate` 与 render 包裹，回滚只需还原 `main.js` 三处入口。
- 后端：`spawn_blocking` 包装为纯包裹层，逐个命令可独立回退。
