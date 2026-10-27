pub mod cipher;
pub mod cli;
pub mod i18n;
pub mod changelog;
pub mod claim;
mod flowlog;
mod oauth;
mod probe;
mod quota;
mod risk;
mod store;
mod zcrypto;

use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use store::*;
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::AutoLaunchManager;
use tauri_plugin_dialog::DialogExt;

const TRAY_ID: &str = "main";

static STORE_LOCK: Mutex<()> = Mutex::new(());

/// 托盘 tooltip 上附加的「新活动」提示（None = 无提示）。
/// Windows 便携版拿不到 Toast，悬停托盘图标看提示是最可靠的可见通道。
static ACTIVITY_TIP: Mutex<Option<String>> = Mutex::new(None);
/// 托盘闪烁开关：true 时后台线程持续交替图标，直到用户点开活动详情。
static TRAY_FLASH_ACTIVE: AtomicBool = AtomicBool::new(false);

fn activity_tip_guard() -> std::sync::MutexGuard<'static, Option<String>> {
    match ACTIVITY_TIP.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Clone)]
struct PendingClaim {
    account_id: String,
    account_name: String,
    plan_id: String,
    plan_name: String,
    credentials: Value,
    config: Option<Value>,
    device_mid: String,
}

static PENDING_CLAIM: Mutex<Option<PendingClaim>> = Mutex::new(None);

fn pending_guard() -> std::sync::MutexGuard<'static, Option<PendingClaim>> {
    match PENDING_CLAIM.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn store_guard() -> std::sync::MutexGuard<'static, ()> {
    match STORE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 在阻塞线程池里跑必须同步等待的工作（ureq 网络请求、进程操作等）。
///
/// 纪律：`async` 命令里**禁止**直接执行阻塞 I/O。Tauri 的 async 命令跑在异步运行时的
/// worker 线程上，一次 `claim_refresh` 最长阻塞 2×10s（激活事件）+ 25s（preview）+
/// 25s（marketing）；并发点几个账号就会把所有 worker 占满，`get_state` / `set_behavior`
/// 等命令全部排队 → 界面整体卡死、额度区永远「查询中…」。
async fn run_blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| i18n::trf("err.internal", &[("e", &e.to_string())]))?
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect()
}

fn tray_menu_inner(app: &AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let paths = Paths::detect();
    let state = match store::get_state(&paths) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tray state error: {e}");
            return MenuBuilder::new(app)
                .item(&MenuItem::with_id(app, "show", &i18n::tr("tray.show"), true, None::<&str>)?)
                .item(&MenuItem::with_id(app, "quit", &i18n::tr("tray.quit"), true, None::<&str>)?)
                .build();
        }
    };

    let mut b = MenuBuilder::new(app);
    // 有新活动时，托盘菜单顶部给出醒目入口（tooltip 小字太弱，菜单更不易被忽略）
    if let Some(extra) = activity_tip_guard().clone().filter(|t| !t.trim().is_empty()) {
        let label = format!("【新活动】{extra}");
        b = b
            .item(&MenuItem::with_id(app, "activity", &label, true, None::<&str>)?)
            .separator();
    }
    let b = b
        .item(&MenuItem::with_id(app, "show", &i18n::tr("tray.show"), true, None::<&str>)?)
        .item(&MenuItem::with_id(app, "capture", &i18n::tr("tray.capture"), state.live_logged_in, None::<&str>)?)
        .separator()
        .item(&MenuItem::with_id(app, "launch", &i18n::tr("tray.launch"), state.zcode_path_ok && !state.zcode_running, None::<&str>)?)
        .item(&MenuItem::with_id(app, "kill", &i18n::tr("tray.kill"), state.zcode_running, None::<&str>)?)
        .separator()
        .item(&MenuItem::with_id(app, "quit", &i18n::tr("tray.quit"), true, None::<&str>)?);
    b.build()
}

pub fn rebuild_tray(app: &AppHandle) {
    let app2 = app.clone();
    let res = app.run_on_main_thread(move || {
        if let Some(tray) = app2.tray_by_id(TRAY_ID) {
            let paths = Paths::detect();
            let tip = match store::get_state(&paths) {
                Ok(s) => {
                    let cur = s
                        .accounts
                        .iter()
                        .find(|a| a.is_active)
                        .map(|a| a.name.clone())
                        .or_else(|| s.live_identity.as_ref().and_then(|i| i.label()))
                        .unwrap_or_else(|| if s.live_logged_in { i18n::tr("tray.unsaved") } else { i18n::tr("tray.logged_out") });
                    format!("Z·SWITCH · {cur}")
                }
                Err(_) => "Z·SWITCH".into(),
            };
            let extra = activity_tip_guard().clone().filter(|t| !t.trim().is_empty());
            let has_extra = extra.is_some();
            let tip = match &extra {
                // 把「新活动」放最前面：tooltip 前面部分更容易被看见
                Some(e) => format!("【新活动】{e} · {tip}"),
                None => tip,
            };
            let _ = tray.set_tooltip(Some(&tip));
            // Windows：仅 NIM_MODIFY 更新 szTip 有时不刷新，重设一次当前图标可强制 shell 读新提示
            if has_extra {
                if let Some(ic) = app2.default_window_icon().cloned() {
                    let _ = tray.set_icon(Some(ic));
                }
            }
            if let Ok(menu) = tray_menu_inner(&app2) {
                let _ = tray.set_menu(Some(menu));
            }
        }
    });
    if let Err(e) = res {
        eprintln!("rebuild_tray: {e}");
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn run_tray_action(app: AppHandle, action: String) {
    std::thread::spawn(move || {
        let paths = Paths::detect();
        let mut emit_payload = json!({ "action": action });
        let _guard = store_guard();
        let result: Result<serde_json::Value, String> = match action.as_str() {
            "capture" => store::capture_current(&paths, None).map(|a| json!({ "name": a.name })),
            "launch" => {
                let (p, ok) = effective_zcode_path(&paths);
                if ok { store::launch_zcode(&p).map(|_| json!({})) } else { Err(i18n::trf("err.zcode.path_invalid", &[("p", &p)])) }
            }
            "kill" => match store::kill_zcode() {
                Ok(true) => Ok(json!({})),
                Ok(false) => Err(i18n::tr("err.zcode.kill_timeout")),
                Err(e) => Err(e),
            },
            _ => Ok(json!({})),
        };
        match result {
            Ok(v) => {
                emit_payload["ok"] = json!(true);
                emit_payload["result"] = v;
            }
            Err(e) => {
                emit_payload["ok"] = json!(false);
                emit_payload["error"] = json!(e);
            }
        }
        let _ = app.emit("tray-action", &emit_payload);
        rebuild_tray(&app);
    });
}

#[tauri::command]
async fn get_state() -> Result<AppState, String> {
    store::get_state(&Paths::detect())
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
async fn capture_current(app: AppHandle, name: Option<String>) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::capture_current(&Paths::detect(), name);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn rename_account(app: AppHandle, id: String, name: String) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::rename_account(&Paths::detect(), &id, &name);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn delete_account(app: AppHandle, id: String) -> Result<(), String> {
    let _guard = store_guard();
    let r = store::delete_account(&Paths::detect(), &id);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn update_account_from_live(app: AppHandle, id: String) -> Result<Account, String> {
    let _guard = store_guard();
    let r = store::update_account_from_live(&Paths::detect(), &id);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn switch_to(app: AppHandle, id: String, force: bool, restart: bool) -> Result<SwitchResult, String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let hot = load_settings(&paths).hot_switch();
    let r = store::switch_to(&paths, &id, force, restart, hot);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn get_live_quota() -> Result<quota::QuotaOverview, String> {
    run_blocking(|| store::live_quota(&Paths::detect())).await
}

#[tauri::command]
async fn get_account_quota(id: String, force: Option<bool>) -> Result<quota::QuotaOverview, String> {
    run_blocking(move || {
        let paths = Paths::detect();
        if risk::is_cooling(&paths, &id) && !force.unwrap_or(false) {
            let mins = (risk::cooling_remaining_secs(&paths, &id) + 59) / 60;
            return Err(i18n::trf("err.risk.cooling", &[("mins", &mins.to_string())]));
        }
        let r = store::account_quota(&paths, &id);
        if let Err(e) = &r {
            risk::observe_upstream_failure(&paths, &id, e);
        }
        r
    })
    .await
}

#[tauri::command]
async fn claim_preview(id: String) -> Result<Vec<claim::ClaimPlan>, String> {
    run_blocking(move || {
        let paths = Paths::detect();
        // 冷却期零上游流量：preview 也是 billing 请求，必须一起挡住
        if risk::is_cooling(&paths, &id) {
            let mins = (risk::cooling_remaining_secs(&paths, &id) + 59) / 60;
            return Err(i18n::trf("err.risk.cooling", &[("mins", &mins.to_string())]));
        }
        let mid = store::ensure_virtual_device_mid(&paths, &id)?;
        let acc = load_account(&paths, &id)?;
        claim::collect_claimable_plans(&paths.home, &acc.credentials, acc.config.as_ref(), mid)
            .map_err(|e| {
                risk::observe_upstream_failure(&paths, &id, &e);
                e
            })
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimCurrentResult {
    plans: Vec<claim::ActivePlan>,
    history: Vec<Value>,
}

#[tauri::command]
async fn claim_current(id: String) -> Result<ClaimCurrentResult, String> {
    run_blocking(move || {
        let paths = Paths::detect();
        // 冷却期零上游流量：current 只回本地历史，不发上游请求（避免冷却被反复刷新击穿）
        if risk::is_cooling(&paths, &id) {
            return Ok(ClaimCurrentResult {
                plans: Vec::new(),
                history: store::claim_history_for(&paths, &id),
            });
        }
        let mid = store::ensure_virtual_device_mid(&paths, &id)?;
        let acc = load_account(&paths, &id)?;
        let plans =
            claim::fetch_active_plans(&paths.home, &acc.credentials, acc.config.as_ref(), Some(mid))
                .map_err(|e| {
                    risk::observe_upstream_failure(&paths, &id, &e);
                    e
                })?;
        let history = store::claim_history_for(&paths, &id);
        Ok(ClaimCurrentResult { plans, history })
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimRefreshResult {
    plans: Vec<claim::ClaimPlan>,
    activated: bool,
    activation_error: Option<String>,
}

#[tauri::command]
async fn claim_refresh(id: String, force: Option<bool>) -> Result<ClaimRefreshResult, String> {
    run_blocking(move || {
        let paths = Paths::detect();
        let mid = store::ensure_virtual_device_mid(&paths, &id)?;
        let acc = load_account(&paths, &id)?;
        if risk::is_cooling(&paths, &id) && !force.unwrap_or(false) {
            let mins = (risk::cooling_remaining_secs(&paths, &id) + 59) / 60;
            return Err(i18n::trf("err.risk.cooling", &[("mins", &mins.to_string())]));
        }
        let (activated, activation_error) =
            match claim::telemetry_user_id(&paths.home, &acc.credentials) {
                Some(uid) => match claim::report_activation_events(&uid, &mid) {
                    Ok(()) => (true, None),
                    Err(e) => (false, Some(e)),
                },
                None => (false, None),
            };
        let plans =
            claim::collect_claimable_plans(&paths.home, &acc.credentials, acc.config.as_ref(), mid)
                .map_err(|e| {
                    risk::observe_upstream_failure(&paths, &id, &e);
                    e
                })?;
        Ok(ClaimRefreshResult { plans, activated, activation_error })
    })
    .await
}

#[tauri::command]
async fn claim_start(
    app: AppHandle,
    id: String,
    plan_id: String,
    auto: Option<bool>,
) -> Result<serde_json::Value, String> {
    let app2 = app.clone();
    run_blocking(move || {
        let paths = Paths::detect();
        let mid = store::ensure_virtual_device_mid(&paths, &id)?;
        let acc = load_account(&paths, &id)?;
        // 自动轮次受风控冷却约束；手动领取是用户显式意图，放行
        if auto.unwrap_or(false) && risk::is_cooling(&paths, &id) {
            return Err(i18n::tr("err.risk.cooling_auto"));
        }
        let plans = claim::collect_claimable_plans(
            &paths.home,
            &acc.credentials,
            acc.config.as_ref(),
            mid.clone(),
        )
        .map_err(|e| {
            risk::observe_upstream_failure(&paths, &id, &e);
            e
        })?;
        let plan = plans
            .iter()
            .find(|p| p.plan_id == plan_id)
            .ok_or_else(|| i18n::tr("err.claim.gone"))?;
        let display = if plan.name.is_empty() { plan.plan_id.clone() } else { plan.name.clone() };

        *pending_guard() = Some(PendingClaim {
            account_id: acc.id.clone(),
            account_name: acc.name.clone(),
            plan_id: plan.plan_id.clone(),
            plan_name: display.clone(),
            credentials: acc.credentials,
            config: acc.config,
            device_mid: mid,
        });
        open_captcha_window(&app2, auto.unwrap_or(false))?;
        Ok(json!({ "account": acc.name, "plan": display }))
    })
    .await
}

#[tauri::command]
async fn claim_captcha_config() -> Result<claim::CaptchaConfig, String> {
    run_blocking(claim::fetch_captcha_config).await
}

#[tauri::command]
async fn claim_captcha_submit(
    app: AppHandle,
    param: String,
    region: Option<String>,
) -> Result<serde_json::Value, String> {
    let app2 = app.clone();
    run_blocking(move || {
        let pending = pending_guard().take().ok_or_else(|| i18n::tr("err.claim.none_pending"))?;
        let paths = Paths::detect();
        let res = claim::submit_claim(
            &paths.home,
            &pending.credentials,
            pending.config.as_ref(),
            &pending.plan_id,
            &param,
            region.as_deref(),
            Some(pending.device_mid.clone()),
        );
        // 3007（验证码被拒/参数过期）：**不再换码自动重试**。
        // 参考实测结论（zcode2api 2.6.5 整改）：拒码本身就是上游风险信号，换码连打只会加剧，
        // 且会白耗一次唯一重试额度。这里直接走终态失败，让用户重新发起领取（重新取验证码）。
        if let Err(e) = &res {
            if e.is_captcha_rejected() {
                flowlog::log(
                    "claim",
                    "captcha-rejected-3007",
                    &format!("code={} 不自动重试", e.code),
                );
            }
        }
        close_captcha_window(&app2);
        let payload = match res {
            Ok(v) => {
                let ms = |k: &str| -> Option<i64> {
                    v.pointer(&format!("/data/plan/{k}"))
                        .and_then(|x| x.as_i64())
                        .map(|s| s * 1000)
                };
                let server_time = v
                    .pointer("/data/server_time")
                    .and_then(|x| x.as_i64())
                    .map(|s| s * 1000);
                // 权益真正生效的时刻在 entitlement 上（0 = 立即生效），starts_at 只是活动开始
                let effective_at = v
                    .pointer("/data/plan/entitlements")
                    .and_then(|e| e.as_array())
                    .and_then(|arr| {
                        arr.iter()
                            .filter_map(|it| it.get("effective_at").and_then(|x| x.as_i64()))
                            .min()
                    })
                    .map(|s| s * 1000);
                let outcome = claim::ClaimOutcome {
                    account_id: pending.account_id.clone(),
                    account_name: pending.account_name.clone(),
                    plan_name: pending.plan_name.clone(),
                    starts_at: ms("starts_at"),
                    ends_at: ms("ends_at"),
                    effective_at,
                    server_time,
                };
                let p = serde_json::to_value(&outcome).unwrap_or(Value::Null);
                // 领取成功落历史（best-effort），供「已领取生效中」展示
                store::record_claim_history(
                    &paths,
                    &pending.account_id,
                    &pending.plan_id,
                    &pending.plan_name,
                );
                let _ = app2.emit("claim://result", &p);
                p
            }
            Err(e) => {
                let p = claim::failure_payload(
                    &pending.account_id,
                    &pending.account_name,
                    &pending.plan_name,
                    &e,
                );
                let _ = app2.emit("claim://result", &p);
                return Ok(p);
            }
        };
        Ok(payload)
    })
    .await
}

#[tauri::command]
async fn claim_cancel(app: AppHandle) -> Result<(), String> {
    *pending_guard() = None;
    close_captcha_window(&app);
    Ok(())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityProbeItem {
    account_id: String,
    name: String,
    plans: Vec<claim::ClaimPlan>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityProbeResult {
    items: Vec<ActivityProbeItem>,
    /// 因风控冷却 / 账号读取失败被跳过的账号（前端用来显示「已跳过」而不是「无活动」）
    skipped: Vec<String>,
}

/// 只读活动探测：只请求 marketing/touch，不领取、不写 claim-history、不发 activation 事件。
/// 冷却中的账号直接跳过（零上游流量）；账号选择与节奏由前端调度器负责。
#[tauri::command]
async fn activity_probe(ids: Vec<String>) -> Result<ActivityProbeResult, String> {
    run_blocking(move || {
        let paths = Paths::detect();
        let mut items = Vec::new();
        let mut skipped = Vec::new();
        for id in ids {
            if risk::is_cooling(&paths, &id) {
                skipped.push(id);
                continue;
            }
            let Ok(mid) = store::ensure_virtual_device_mid(&paths, &id) else {
                skipped.push(id);
                continue;
            };
            let Ok(acc) = load_account(&paths, &id) else {
                skipped.push(id);
                continue;
            };
            let plans = claim::probe_marketing_only(
                &paths.home,
                &acc.credentials,
                acc.config.as_ref(),
                Some(mid),
            );
            items.push(ActivityProbeItem { account_id: id, name: acc.name, plans });
        }
        Ok(ActivityProbeResult { items, skipped })
    })
    .await
}

fn append_rolling_log(name: &str, cap: usize, message: &str) -> Result<(), String> {
    let dir = Paths::detect().store_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(name);
    let mut lines: Vec<String> = std::fs::read_to_string(&file)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    lines.push(format!("[{ts}] {}", message.replace('\n', " ⏎ ")));
    if lines.len() > cap {
        let excess = lines.len() - cap;
        lines.drain(0..excess);
    }
    std::fs::write(&file, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// 当前待领取的那一笔（用于「只取消自己发起的领取」）。
#[tauri::command]
fn claim_pending() -> Option<serde_json::Value> {
    pending_guard()
        .as_ref()
        .map(|p| json!({ "accountId": p.account_id, "planId": p.plan_id }))
}

/// 设置「新活动」提示：托盘 tooltip 附加文案（None 清除）。
/// 便携版拿不到 Windows Toast 时，悬停托盘图标是可见性兜底。
#[tauri::command]
async fn set_activity_tip(app: AppHandle, tip: Option<String>) -> Result<(), String> {
    let cleaned = tip.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    {
        let mut g = activity_tip_guard();
        *g = cleaned.clone();
    }
    rebuild_tray(&app);
    let _ = append_rolling_log(
        "probe.log",
        500,
        &format!("tray tip -> {:?}", cleaned),
    );
    if app.tray_by_id(TRAY_ID).is_none() {
        return Err("tray not found".into());
    }
    Ok(())
}

/// 原图标 + 右下角红点徽标（钉钉式）：红点出现/消失交替，比整图标染色自然。
fn badged_icon(base: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let (w, h) = (base.width(), base.height());
    let mut rgba = base.rgba().to_vec();
    let cx = w as f32 * 0.74;
    let cy = h as f32 * 0.74;
    let r = (h as f32 * 0.22).max(3.0);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if d <= r {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = 235;
                rgba[i + 1] = 64;
                rgba[i + 2] = 56;
                rgba[i + 3] = 255;
            } else if d <= r + 1.5 {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = 255;
                rgba[i + 1] = 255;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            }
        }
    }
    tauri::image::Image::new_owned(rgba, w, h)
}

/// 托盘图标交替闪烁（Windows 托盘没有闪烁 API，用图标交替模拟）。
/// 提醒态 = 原图标 + 右下角红点徽标，与常态来回切换 `times` 轮后恢复（0 = 直到点开活动）。
#[tauri::command]
async fn tray_flash(app: AppHandle, times: Option<u32>) -> Result<(), String> {
    use tauri::image::Image;
    let base = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "no window icon".to_string())?;
    let (w, h) = (base.width(), base.height());
    let base: Image<'static> = Image::new_owned(base.rgba().to_vec(), w, h);
    let attention: Image<'static> = badged_icon(&base);
    // times: None/0 = 持续闪烁直到用户点开活动（tray_flash_stop），
    // Some(n) = 闪 n 轮后自动停止（仅测试用）。
    let rounds = times.unwrap_or(0);
    TRAY_FLASH_ACTIVE.store(true, Ordering::SeqCst);
    std::thread::spawn(move || {
        let mut count = 0u32;
        while TRAY_FLASH_ACTIVE.load(Ordering::SeqCst) {
            count += 1;
            if rounds > 0 && count > rounds * 2 {
                TRAY_FLASH_ACTIVE.store(false, Ordering::SeqCst);
                break;
            }
            let holder = app.clone();
            let inner = app.clone();
            let ic = attention.clone();
            let _ = holder.run_on_main_thread(move || {
                if let Some(t) = inner.tray_by_id(TRAY_ID) {
                    let _ = t.set_icon(Some(ic));
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(420));
            let holder = app.clone();
            let inner = app.clone();
            let ic = base.clone();
            let _ = holder.run_on_main_thread(move || {
                if let Some(t) = inner.tray_by_id(TRAY_ID) {
                    let _ = t.set_icon(Some(ic));
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(420));
        }
        let _ = append_rolling_log("probe.log", 500, "tray flash done");
    });
    Ok(())
}

/// 停止托盘闪烁（用户已点开活动详情），并恢复常态图标。
#[tauri::command]
async fn tray_flash_stop(app: AppHandle) -> Result<(), String> {
    TRAY_FLASH_ACTIVE.store(false, Ordering::SeqCst);
    let owned = app.default_window_icon().map(|ic| {
        let (w, h) = (ic.width(), ic.height());
        tauri::image::Image::new_owned(ic.rgba().to_vec(), w, h)
    });
    if let Some(ic) = owned {
        let app2 = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(t) = app2.tray_by_id(TRAY_ID) {
                let _ = t.set_icon(Some(ic));
            }
        });
    }
    Ok(())
}

/// 探测留痕：`{home}/.zcode-switch/probe.log`（500 行滚动）。
/// 用于回答「开关开了但没探到」这类问题：能查到每轮选了谁、返回几条、是否被跳过。
#[tauri::command]
async fn log_probe(message: String) -> Result<(), String> {
    run_blocking(move || append_rolling_log("probe.log", 500, &message)).await
}

/// 前端异常落盘：`{home}/.zcode-switch/client-error.log`，最多保留 200 行。
/// 目的是让「界面冻结/报错」这类只能靠 DevTools 看到的问题可回传、可定位。
#[tauri::command]
async fn log_client_error(message: String) -> Result<(), String> {
    run_blocking(move || {
        let dir = Paths::detect().store_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join("client-error.log");
        let mut lines: Vec<String> = std::fs::read_to_string(&file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        lines.push(format!("[{ts}] {}", message.replace('\n', " ⏎ ")));
        if lines.len() > 200 {
            let excess = lines.len() - 200;
            lines.drain(0..excess);
        }
        std::fs::write(&file, lines.join("\n") + "\n").map_err(|e| e.to_string())
    })
    .await
}

fn close_captcha_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("captcha") {
        let _ = w.close();
    }
}

struct PendingOAuth {
    provider: String,
    state: String,
    flow: String,
}

#[derive(Clone)]
struct PollCfg {
    url: String,
    token: String,
    expires_at_ms: u128,
    interval_ms: u64,
}

static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

fn pending_oauth_guard() -> std::sync::MutexGuard<'static, Option<PendingOAuth>> {
    match PENDING_OAUTH.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[tauri::command]
async fn oauth_providers() -> Result<Vec<oauth::OAuthProvider>, String> {
    Ok(oauth::OAUTH_PROVIDERS.to_vec())
}

#[tauri::command]
async fn oauth_begin(app: AppHandle, provider: String) -> Result<serde_json::Value, String> {
    if !oauth::OAUTH_PROVIDERS.iter().any(|p| p.id == provider) {
        return Err(i18n::trf("err.oauth.unknown_provider", &[("provider", &provider)]));
    }
    let proxy_url: Option<tauri::Url> = match load_settings(&Paths::detect()).auth_proxy() {
        Some(p) => {
            let norm = oauth::parse_proxy_url(p)?;
            Some(norm.parse::<tauri::Url>().map_err(|e| i18n::trf("err.proxy.invalid", &[("e", &e.to_string())]))?)
        }
        None => None,
    };
    *pending_oauth_guard() = None;
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    let flow = uuid::Uuid::new_v4().to_string();
    flowlog::log(&flow, "begin", &format!("provider={provider} proxy={}", if proxy_url.is_some() { "on" } else { "off" }));
    let mid = uuid::Uuid::new_v4().to_string();
    let (p_init, m_init) = (provider.clone(), mid.clone());
    let init = match tauri::async_runtime::spawn_blocking(move || oauth::init_flow(&p_init, &m_init))
        .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            flowlog::log(&flow, "init-fail", &e);
            return Err(e);
        }
        Err(e) => {
            let m = i18n::trf("err.oauth.flow", &[("e", &e.to_string())]);
            flowlog::log(&flow, "init-fail", &m);
            return Err(m);
        }
    };
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let srv_flow = init.poll_url.rsplit('/').next().unwrap_or("");
        let expires_in = init.expires_at_ms.saturating_sub(now) / 1000;
        let authorize = init.authorize_url.split('?').next().unwrap_or("");
        let mode = if provider == "zai" { "official-login+relay" } else { "direct" };
        flowlog::log(
            &flow,
            "init-ok",
            &format!("mode={mode} authorize={authorize} server_flow={srv_flow} expires_in={expires_in}s interval={}ms", init.poll_interval_ms),
        );
    }
    // zai 第二段接力用服务端原始授权端点（已登录时直接发码；未登录它自己会 307 到登录页）
    let url = init.raw_authorize_url.clone();
    // zai：API 授权 client 没有邮箱入口，先用官网登录页建立 chat.z.ai 会话，
    // 登录完成后再接力到这个授权端点取码（已登录 → 不再要求手机号）
    let entry_url = if provider == "zai" {
        oauth::ZAI_LOGIN_ENTRY_URL.to_string()
    } else {
        url.clone()
    };
    let poll_cfg = PollCfg {
        url: init.poll_url.clone(),
        token: init.poll_token.clone(),
        expires_at_ms: init.expires_at_ms,
        interval_ms: init.poll_interval_ms,
    };
    *pending_oauth_guard() = Some(PendingOAuth {
        provider: provider.clone(),
        state: init.state.clone(),
        flow: flow.clone(),
    });

    let login_root = app
        .path()
        .app_local_data_dir()
        .map_err(|e| i18n::trf("err.oauth.appdata", &[("e", &e.to_string())]))?
        .join("login-webview");
    sweep_login_profiles(&login_root);
    let profile_dir = login_root.join(&flow);

    let app2 = app.clone();
    let (provider2, state2, flow2, mid2) = (provider.clone(), init.state.clone(), flow.clone(), mid.clone());
    let flow_close = flow.clone();
    let relay_to = url.clone();
    let relay_to2 = url.clone();
    let relay_armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(provider == "zai"));
    let relay_armed2 = relay_armed.clone();
    let relay_armed3 = relay_armed.clone();
    let flow_relay = flow.clone();
    let flow_relay2 = flow.clone();
    let flow_nav = flow.clone();
    let log_nav = provider == "zai";
    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        "login",
        tauri::WebviewUrl::External(entry_url.parse::<tauri::Url>().map_err(|e| i18n::trf("err.oauth.bad_authorize_url", &[("e", &e.to_string())]))?),
    )
    .title(i18n::tr("title.login"))
    .theme(Some(tauri::Theme::Dark))
    .inner_size(480.0, 680.0)
    .min_inner_size(420.0, 560.0)
    .resizable(true)
    .user_agent(oauth::LOGIN_WINDOW_UA)
    .data_directory(profile_dir);
    if let Some(u) = proxy_url {
        builder = builder.proxy_url(u);
    }
    // z.ai：官网登录页有邮箱入口，但 API 授权 client 的兜底页只放手机号；
    // 页面每次加载后注入按钮条，用户随时可切回邮箱登录 / 邮箱注册
    if provider == "zai" {
        let assist = oauth::zai_email_assist_script();
        let assist_load = assist.clone();
        let flow_load = flow.clone();
        builder = builder.on_page_load(move |wv, payload| {
            if let tauri::webview::PageLoadEvent::Finished = payload.event() {
                flowlog::log(&flow_load, "assist", "inject on-page-load");
                let _ = wv.eval(&assist_load);
            }
        });
        // 首次导航可能早于 on_page_load 注册生效，窗口起来后再补几次注入（脚本幂等，重复执行无副作用）
        let app_assist = app.clone();
        let flow_assist = flow.clone();
        std::thread::spawn(move || {
            let mut logged = false;
            for _ in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let ours = pending_oauth_guard().as_ref().map(|p| p.flow == flow_assist).unwrap_or(false);
                if !ours {
                    return;
                }
                let Some(w) = app_assist.get_webview_window("login") else { return };
                if w.eval(&assist).is_ok() && !logged {
                    flowlog::log(&flow_assist, "assist", "inject boost");
                    logged = true;
                }
            }
        });
    }
    builder
    .on_navigation(move |url| {
        if url.scheme() == "zcode" {
            let full = url.to_string();
            let (app3, p3, s3, f3, m3) = (app2.clone(), provider2.clone(), state2.clone(), flow2.clone(), mid2.clone());
            tauri::async_runtime::spawn(async move {
                finish_oauth(&app3, p3, s3, f3, m3, &full).await;
            });
            return false;
        }
        if log_nav && url.host_str().map_or(false, |h| h.ends_with("z.ai")) {
            flowlog::log(&flow_nav, "nav", &format!("{}{}", url.host_str().unwrap_or(""), url.path()));
        }
        // 窗口内「邮箱登录 / 邮箱注册」按钮会把用户带回官网入口页，此时重新武装接力：
        // 接力默认只有一次机会，手动绕回登录页后若不再武装，登录成功也不会取码
        if url.host_str() == Some("chat.z.ai")
            && url.path() == "/auth"
            && url.query().map_or(false, |q| q.contains("client_id="))
        {
            relay_armed2.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        // zai 第一段（官网登录页）完成 → 接力到本次 flow 的授权页取码（此时已登录，不再要求手机号）
        if url.host_str().map_or(false, |h| h == "z.ai" || h.ends_with(".z.ai"))
            && !is_login_page(url.path())
            && relay_armed2.swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            flowlog::log(
                &flow_relay,
                "zai-relay",
                &format!("nav {}{} -> authorize", url.host_str().unwrap_or(""), url.path()),
            );
            let target = relay_to.clone();
            let app4 = app2.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                if let Some(w) = app4.get_webview_window("login") {
                    if let Ok(t) = target.parse::<tauri::Url>() {
                        let _ = w.navigate(t);
                    }
                }
            });
            log_after_relay(app2.clone(), flow_relay.clone(), 5000);
        }
        true
    })
    .build()
    .map_err(|e| {
        let mut pending = pending_oauth_guard();
        if pending.as_ref().map(|p| p.flow == flow).unwrap_or(false) {
            *pending = None;
        }
        let m = i18n::trf("err.oauth.window", &[("e", &e.to_string())]);
        flowlog::log(&flow, "window-fail", &m);
        m
    })?;
    if let Some(w) = app.get_webview_window("login") {
        w.on_window_event(move |e| {
            if let tauri::WindowEvent::CloseRequested { .. } = e {
                let mut pending = pending_oauth_guard();
                if pending.as_ref().map(|p| p.flow == flow_close).unwrap_or(false) {
                    flowlog::log(&flow_close, "cancelled", "");
                    *pending = None;
                }
            }
        });
    }
    // 官网登录成功后走的是 SPA 内部路由（history.replaceState），on_navigation 收不到事件，
    // 所以再加一路轮询窗口 URL 兜底：一旦离开登录页、落到 z.ai 站点即接力取码
    if provider == "zai" {
        let app_relay = app.clone();
        let armed = relay_armed3;
        let target = relay_to2;
        let fl = flow_relay2;
        let guard_flow = flow.clone();
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let ours = pending_oauth_guard().as_ref().map(|p| p.flow == guard_flow).unwrap_or(false);
                if !ours || !armed.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let Some(w) = app_relay.get_webview_window("login") else { return };
                let Ok(u) = w.url() else { continue };
                let host = u.host_str().unwrap_or("");
                if !(host == "z.ai" || host.ends_with(".z.ai")) || is_login_page(u.path()) {
                    continue;
                }
                if !armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    return; // 已被 on_navigation 那路接力
                }
                std::thread::sleep(std::time::Duration::from_millis(1200)); // 留点时间让官网把回调收尾
                flowlog::log(&fl, "zai-relay", &format!("poll {host}{} -> authorize", u.path()));
                if let Ok(t) = target.parse::<tauri::Url>() {
                    let _ = w.navigate(t);
                }
                log_after_relay(app_relay.clone(), fl.clone(), 5000);
                return;
            }
        });
    }
    spawn_poll_loop(app.clone(), provider.clone(), flow.clone(), mid, poll_cfg);
    Ok(json!({ "opened": true, "provider": provider }))
}

/// 接力后留痕：几秒后把窗口实际落点写进日志，便于判断授权是否成功
fn log_after_relay(app: AppHandle, flow: String, delay_ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        if let Some(w) = app.get_webview_window("login") {
            if let Ok(u) = w.url() {
                flowlog::log(&flow, "zai-after", &format!("{}{}", u.host_str().unwrap_or(""), u.path()));
            }
        }
    });
}

/// 登录 / 注册相关路径：说明还没登录完成，zai 接力不能在这些页面触发
fn is_login_page(path: &str) -> bool {
    if path.starts_with("/login/callback") {
        return false; // 登录回跳页：登录已完成
    }
    ["/auth", "/login", "/signin", "/sign-in", "/signup", "/sign-up", "/register", "/reset", "/forgot"]
        .iter()
        .any(|p| path.starts_with(p))
}

fn sweep_login_profiles(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 24 * 3600);
    for e in entries.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() && meta.modified().map(|m| m < cutoff).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[tauri::command]
async fn set_auth_proxy(app: AppHandle, on: bool, url: Option<String>) -> Result<(), String> {
    {
        let _guard = store_guard();
        let paths = Paths::detect();
        let trimmed = url.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let normalized = match trimmed {
            Some(s) => Some(oauth::parse_proxy_url(s)?),
            None => None,
        };
        if on && normalized.is_none() {
            return Err(i18n::tr("err.proxy.need_url"));
        }
        let mut s = load_settings(&paths);
        s.auth_proxy_on = Some(on);
        s.auth_proxy_url = normalized;
        save_settings(&paths, &s)?;
    }
    let _ = app.emit("state-changed", ());
    Ok(())
}

async fn finish_oauth(app: &AppHandle, provider: String, state: String, flow: String, mid: String, callback_url: &str) {
    let result = {
        let provider = provider.clone();
        let state = state.clone();
        let flow = flow.clone();
        let mid = mid.clone();
        let callback_url = callback_url.to_string();
        tauri::async_runtime::spawn_blocking(move || -> Result<serde_json::Value, String> {
            {
                let pending = pending_oauth_guard();
                match pending.as_ref() {
                    Some(p) if p.provider == provider && p.state == state && p.flow == flow => {}
                    Some(_) | None => {
                        flowlog::log(&flow, "deeplink-superseded", "");
                        return Err("__superseded__".into());
                    }
                }
            }
            let (code, cb_state) = match oauth::parse_callback(&callback_url) {
                Ok(oauth::CallbackKind::Code { code, state }) => (code, state),
                Ok(oauth::CallbackKind::Attribution) => {
                    flowlog::log(&flow, "deeplink-attribution", "");
                    return Err("__attribution__".into());
                }
                Err(e) => {
                    flowlog::log(&flow, "deeplink-parse-fail", &e);
                    return Err(e);
                }
            };
            if cb_state != state {
                flowlog::log(&flow, "deeplink-state-mismatch", "");
                return Err(i18n::tr("err.oauth.state"));
            }
            let exchanged = match oauth::exchange_token(&provider, &code, &state, &mid) {
                Ok(v) => v,
                Err(e) => {
                    flowlog::log(&flow, "exchange-fail", &e);
                    return Err(e);
                }
            };
            match persist_oauth_account(&Paths::detect(), &provider, &exchanged["raw"], &flow, &mid, false) {
                Ok(v) => {
                    flowlog::log(
                        &flow,
                        "persist-ok",
                        &format!("channel=deeplink duplicate={}", v.get("duplicate").is_some()),
                    );
                    Ok(v)
                }
                Err(e) => {
                    if e != "__superseded__" {
                        flowlog::log(&flow, "persist-fail", &format!("channel=deeplink {e}"));
                    }
                    Err(e)
                }
            }
        })
        .await
        .unwrap_or_else(|e| Err(i18n::trf("err.oauth.flow", &[("e", &e.to_string())])))
    };
    if let Err(e) = &result {
        if deeplink_err_soft(e) {
            let ours = pending_oauth_guard().as_ref().map(|p| p.flow == flow).unwrap_or(false);
            if ours {
                flowlog::log(&flow, "soft-fail", e);
                let _ = app.emit("oauth://done", &json!({ "ok": false, "soft": true, "error": e }));
            }
            return;
        }
    }
    finalize_oauth_result(app, result);
}

fn deeplink_err_soft(e: &str) -> bool {
    e != "__superseded__" && e != "__attribution__"
}

fn persist_oauth_account(
    paths: &Paths,
    provider: &str,
    raw: &serde_json::Value,
    flow: &str,
    mid: &str,
    poll_ready: bool,
) -> Result<serde_json::Value, String> {
    let flow_still_ours = || {
        pending_oauth_guard()
            .as_ref()
            .map(|p| p.flow == flow)
            .unwrap_or(false)
    };
    if !flow_still_ours() {
        return Err("__superseded__".into());
    }
    let jwt = raw
        .pointer("/data/token")
        .and_then(|t| t.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| i18n::tr("err.oauth.no_token"))?
        .to_string();
    let raw_access = oauth::extract_access_token(provider, raw).unwrap_or_default();
    let access_token = if provider == "zai" && !raw_access.is_empty() {
        oauth::resolve_zai_business_token(&raw_access).ok_or_else(|| i18n::tr("err.oauth.zai_business"))?
    } else {
        raw_access
    };
    let userinfo = if poll_ready {
        oauth::extract_poll_user_profile(raw)
    } else {
        oauth::extract_user_profile(provider, raw)
    }
    .or_else(|| {
        (!access_token.is_empty())
            .then(|| oauth::fetch_userinfo(provider, &access_token))
            .flatten()
    });
    let refresh_token = oauth::extract_refresh_token(provider, raw);
    let credentials = oauth::assemble_credentials_with_token(
        provider,
        &jwt,
        userinfo.as_ref(),
        (!access_token.is_empty()).then_some(access_token.as_str()),
        refresh_token.as_deref(),
    );
    let config = oauth::assemble_config(provider, &jwt, &access_token);

    let _lock = store_guard();
    if !flow_still_ours() {
        return Err("__superseded__".into());
    }
    let accounts = list_accounts(paths)?;
    let hash = canonical_hash(&credentials);
    if let Some(i) = store::find_same_login(&credentials, &hash, &accounts, &paths.home) {
        let mut dup = accounts[i].clone();
        dup.hash = hash.clone();
        dup.credentials = credentials;
        dup.config = Some(config);
        dup.updated_at = now_ts();
        if dup.virtual_device_mid.as_deref().map_or(true, |m| m.trim().is_empty()) {
            dup.virtual_device_mid = Some(mid.to_string());
        }
        if !flow_still_ours() {
            return Err("__superseded__".into());
        }
        save_account(paths, &dup)?;
        *pending_oauth_guard() = None;
        return Ok(json!({ "id": dup.id, "name": dup.name, "provider": provider, "duplicate": true }));
    }
    let base = credentials
        .get(format!("oauth:{provider}:user_info"))
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|u| u.get("username").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| match provider {
            "zai" => "z.ai".to_string(),
            _ => "BigModel".to_string(),
        });
    let name = unique_name(&accounts, &base);
    let ts = now_ts();
    let acc = Account {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        created_at: ts.clone(),
        updated_at: ts,
        hash,
        credentials,
        config: Some(config),
        virtual_device_mid: Some(mid.to_string()),
        virtual_arms_uid: Some(store::new_arms_uid()),
    };
    if !flow_still_ours() {
        return Err("__superseded__".into());
    }
    save_account(paths, &acc)?;
    *pending_oauth_guard() = None;
    Ok(json!({ "id": acc.id, "name": acc.name, "provider": provider }))
}

fn finalize_oauth_result(app: &AppHandle, result: Result<serde_json::Value, String>) {
    if let Err(e) = &result {
        if e == "__superseded__" || e == "__attribution__" {
            return;
        }
    }
    if let Some(w) = app.get_webview_window("login") {
        let _ = w.close();
    }
    let payload = match result {
        Ok(v) => v,
        Err(e) => json!({ "ok": false, "error": e }),
    };
    let _ = app.emit("oauth://done", &payload);
}

fn spawn_poll_loop(app: AppHandle, provider: String, flow: String, mid: String, cfg: PollCfg) {
    std::thread::spawn(move || {
        let ours = || pending_oauth_guard().as_ref().map(|p| p.flow == flow).unwrap_or(false);
        let deadline = {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            std::cmp::min(cfg.expires_at_ms, now + u128::from(oauth::FLOW_TIMEOUT_MS))
        };
        loop {
            if !ours() {
                flowlog::log(&flow, "poll-exit", "flow-done-or-replaced");
                return;
            }
            match oauth::poll_flow_once(&cfg.url, &cfg.token, &mid) {
                Ok(oauth::PollOutcome::Pending) => {}
                Ok(oauth::PollOutcome::Ready(data)) => {
                    flowlog::log(&flow, "poll-ready", "");
                    let raw = json!({ "code": 0, "data": data });
                    let result = persist_oauth_account(&Paths::detect(), &provider, &raw, &flow, &mid, true);
                    match &result {
                        Ok(v) => flowlog::log(
                            &flow,
                            "persist-ok",
                            &format!("channel=poll duplicate={}", v.get("duplicate").is_some()),
                        ),
                        Err(e) if e != "__superseded__" => {
                            flowlog::log(&flow, "persist-fail", &format!("channel=poll {e}"));
                        }
                        Err(_) => {}
                    }
                    finalize_oauth_result(&app, result);
                    return;
                }
                Err(e) => {
                    if !ours() {
                        flowlog::log(&flow, "poll-exit", "superseded");
                        return;
                    }
                    flowlog::log(&flow, "poll-fail", &e);
                    *pending_oauth_guard() = None;
                    finalize_oauth_result(&app, Err(e));
                    return;
                }
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            if now >= deadline {
                if !ours() {
                    return;
                }
                flowlog::log(&flow, "poll-timeout", "");
                *pending_oauth_guard() = None;
                finalize_oauth_result(&app, Err(i18n::tr("err.oauth.expired")));
                return;
            }
            let sleep = (deadline - now).min(cfg.interval_ms as u128) as u64;
            std::thread::sleep(std::time::Duration::from_millis(sleep));
        }
    });
}

fn open_captcha_window(app: &AppHandle, auto: bool) -> Result<(), String> {
    let (w, h) = (380.0, 320.0);
    if let Some(win) = app.get_webview_window("captcha") {
        let _ = win.eval("location.reload()");
        center_over_main(app, &win, w, h);
        if auto {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "captcha",
        tauri::WebviewUrl::App("captcha.html".into()),
    )
    .title(i18n::tr("title.captcha"))
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(10, 10, 12, 255))
    .inner_size(w, h)
    .min_inner_size(340.0, 280.0)
    .maximizable(false)
    .resizable(false)
    .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --no-proxy-server")
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;
    center_over_main(app, &win, w, h);
    if !auto {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

#[tauri::command]
async fn kill_zcode(app: AppHandle) -> Result<(), String> {
    let _guard = store_guard();
    let r = if store::kill_zcode()? { Ok(()) } else { Err(i18n::tr("err.zcode.kill_timeout")) };
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn set_behavior(
    app: AppHandle,
    launch_after_switch: Option<bool>,
    close_to_tray: Option<bool>,
    hot_switch: Option<bool>,
    auto_claim: Option<bool>,
    activity_probe: Option<bool>,
    claim_at_effective: Option<bool>,
) -> Result<(), String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let mut s = load_settings(&paths);
    if let Some(v) = launch_after_switch {
        s.launch_after_switch = Some(v);
    }
    if let Some(v) = close_to_tray {
        s.close_to_tray = Some(v);
    }
    if let Some(v) = hot_switch {
        s.hot_switch = Some(v);
    }
    if let Some(v) = auto_claim {
        s.auto_claim = Some(v);
    }
    if let Some(v) = activity_probe {
        s.activity_probe = Some(v);
    }
    if let Some(v) = claim_at_effective {
        s.claim_at_effective = Some(v);
    }
    let r = save_settings(&paths, &s);
    rebuild_tray(&app);
    let _ = app.emit("state-changed", ());
    r
}

#[tauri::command]
async fn set_language(app: AppHandle, lang: String) -> Result<(), String> {
    let l = i18n::Lang::parse(&lang)
        .ok_or_else(|| i18n::trf("err.lang.unknown", &[("lang", &lang)]))?;
    {
        let _guard = store_guard();
        let paths = Paths::detect();
        let mut s = load_settings(&paths);
        s.language = Some(l.as_str().to_string());
        save_settings(&paths, &s)?;
    }
    i18n::set(l);
    rebuild_tray(&app);
    for (label, key) in [("settings", "title.settings"), ("captcha", "title.captcha"), ("login", "title.login")] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.set_title(&i18n::tr(key));
        }
    }
    let _ = app.emit("state-changed", ());
    Ok(())
}

#[tauri::command]
async fn reveal_main(app: AppHandle) -> Result<(), String> {
    let win = app.get_webview_window("main").ok_or_else(|| i18n::tr("err.main.missing"))?;
    win.show().map_err(|e| e.to_string())?;
    let _ = win.set_focus();
    Ok(())
}

#[tauri::command]
async fn open_settings(app: AppHandle) -> Result<(), String> {
    let (w, h) = (520.0, 700.0);
    if let Some(win) = app.get_webview_window("settings") {
        center_over_main(&app, &win, w, h);
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return Ok(());
    }
    let win = tauri::WebviewWindowBuilder::new(
        &app,
        "settings",
        tauri::WebviewUrl::App("settings.html".into()),
    )
    .title(i18n::tr("title.settings"))
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(10, 10, 12, 255))
    .inner_size(w, h)
    .min_inner_size(440.0, 540.0)
    .resizable(true)
    .additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --no-proxy-server")
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;
    center_over_main(&app, &win, w, h);
    let _ = win.show();
    let _ = win.set_focus();
    Ok(())
}

fn center_over_main(app: &AppHandle, win: &tauri::WebviewWindow, w: f64, h: f64) {
    let Some(m) = app.get_webview_window("main") else { return };
    let (Ok(p), Ok(s), Ok(scale)) = (m.outer_position(), m.outer_size(), m.scale_factor()) else { return };
    let (mx, my) = (p.x as f64 / scale, p.y as f64 / scale);
    let (mw, mh) = (s.width as f64 / scale, s.height as f64 / scale);
    let x = mx + (mw - w).max(0.0) / 2.0;
    let y = my + (mh - h).max(0.0) / 2.0;
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

#[tauri::command]
async fn autostart_status(app: AppHandle) -> Result<bool, String> {
    let al = app.state::<AutoLaunchManager>();
    al.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
async fn autostart_set(app: AppHandle, enable: bool) -> Result<bool, String> {
    let al = app.state::<AutoLaunchManager>();
    if enable {
        al.enable().map_err(|e| e.to_string())?;
    } else {
        al.disable().map_err(|e| e.to_string())?;
    }
    al.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
async fn export_pick_path(app: AppHandle, id: String) -> Result<serde_json::Value, String> {
    let acc = load_account(&Paths::detect(), &id)?;
    let default_name = format!("{}.zsb", sanitize_filename(&acc.name));
    let picked = app
        .dialog()
        .file()
        .add_filter(&i18n::tr("dialog.zsb"), &["zsb"])
        .set_file_name(&default_name)
        .blocking_save_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy(), "name": acc.name }))
}

#[tauri::command]
async fn export_finalize(path: String, id: String, password: String) -> Result<serde_json::Value, String> {
    let acc = load_account(&Paths::detect(), &id)?;
    let payload = store::export_bundle_value(std::slice::from_ref(&acc));
    let sealed = cipher::seal(&payload, &password, cipher::FORMAT_BUNDLE)?;
    let body = serde_json::to_string_pretty(&sealed).unwrap() + "\n";
    store::atomic_write(std::path::Path::new(&path), &body).map_err(|e| i18n::trf("err.write", &[("e", &e.to_string())]))?;
    Ok(json!({ "saved": true, "path": path }))
}

#[tauri::command]
async fn export_all_pick_path(app: AppHandle) -> Result<serde_json::Value, String> {
    let accounts = list_accounts(&Paths::detect())?;
    if accounts.is_empty() {
        return Err(i18n::tr("err.export.empty"));
    }
    let picked = app
        .dialog()
        .file()
        .add_filter(&i18n::tr("dialog.zsb"), &["zsb"])
        .set_file_name("zcode-accounts.zsb")
        .blocking_save_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy(), "count": accounts.len() }))
}

#[tauri::command]
async fn export_all_finalize(path: String, password: String) -> Result<serde_json::Value, String> {
    let accounts = list_accounts(&Paths::detect())?;
    if accounts.is_empty() {
        return Err(i18n::tr("err.export.empty_short"));
    }
    let payload = store::export_bundle_value(&accounts);
    let sealed = cipher::seal(&payload, &password, cipher::FORMAT_BUNDLE)?;
    let body = serde_json::to_string_pretty(&sealed).unwrap() + "\n";
    store::atomic_write(std::path::Path::new(&path), &body).map_err(|e| i18n::trf("err.write", &[("e", &e.to_string())]))?;
    Ok(json!({ "saved": true, "path": path, "count": accounts.len() }))
}

#[tauri::command]
async fn import_pick_files(app: AppHandle) -> Result<serde_json::Value, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter(&i18n::tr("dialog.zsb"), &["zsb"])
        .blocking_pick_files();
    let Some(files) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let mut sealed = vec![];
    let mut errors = vec![];
    for fp in files {
        let path = match fp.into_path() {
            Ok(p) => p,
            Err(e) => {
                errors.push(i18n::trf("err.path.conv", &[("e", &e.to_string())]));
                continue;
            }
        };
        let fname = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let raw = match std::fs::read_to_string(&path) {
            Ok(r) => r,
            Err(e) => {
                errors.push(i18n::trf("err.import.read", &[("fname", fname.as_str()), ("e", &e.to_string())]));
                continue;
            }
        };
        match serde_json::from_str::<Value>(&raw) {
            Ok(v) => {
                if !cipher::is_sealed(&v) {
                    errors.push(i18n::trf("err.import.not_sealed", &[("fname", fname.as_str())]));
                } else if v.get("format").and_then(|f| f.as_str()) != Some(cipher::FORMAT_BUNDLE) {
                    errors.push(i18n::trf("err.import.not_bundle", &[("fname", fname.as_str())]));
                } else {
                    sealed.push((fname, v));
                }
            }
            Err(e) => errors.push(i18n::trf("err.import.json", &[("fname", fname.as_str()), ("e", &e.to_string())])),
        }
    }
    Ok(json!({ "picked": true, "sealed": sealed, "errors": errors }))
}

#[tauri::command]
async fn import_sealed(app: AppHandle, files: Vec<(String, Value)>, password: String) -> Result<ImportReport, String> {
    let _guard = store_guard();
    let mut decrypted = vec![];
    let mut errors = vec![];
    for (fname, v) in files {
        match cipher::open(&v, &password) {
            Ok(payload) => decrypted.push((fname, payload)),
            Err(e) => errors.push(i18n::trf("err.import.wrap", &[("fname", fname.as_str()), ("e", &e)])),
        }
    }
    let mut report = if decrypted.is_empty() {
        ImportReport::default()
    } else {
        import_values(&Paths::detect(), &decrypted)?
    };
    report.picked = true;
    report.errors.extend(errors);
    rebuild_tray(&app);
    Ok(report)
}

#[tauri::command]
async fn pick_zcode_path(app: AppHandle) -> Result<serde_json::Value, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter(&i18n::tr("dialog.exe"), &["exe"])
        .blocking_pick_file();
    let Some(fp) = picked else {
        return Ok(json!({ "picked": false }));
    };
    let path = fp.into_path().map_err(|e| i18n::trf("err.path.invalid", &[("e", &e.to_string())]))?;
    Ok(json!({ "picked": true, "path": path.to_string_lossy() }))
}

#[tauri::command]
async fn set_zcode_path(app: AppHandle, path: String) -> Result<(), String> {
    let _guard = store_guard();
    let paths = Paths::detect();
    let mut s = load_settings(&paths);
    s.zcode_path = store::normalize_zcode_path(&path);
    let r = save_settings(&paths, &s);
    rebuild_tray(&app);
    let _ = app.emit("state-changed", ());
    r
}

#[tauri::command]
async fn launch_zcode(app: AppHandle) -> Result<(), String> {
    let paths = Paths::detect();
    let (p, ok) = effective_zcode_path(&paths);
    if !ok {
        return Err(i18n::trf("err.zcode.path_invalid_hint", &[("p", &p)]));
    }
    let r = store::launch_zcode(&p);
    rebuild_tray(&app);
    r
}

#[tauri::command]
async fn open_external(url: String) -> Result<(), String> {
    store::open_url(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_state,
            app_version,
            capture_current,
            rename_account,
            delete_account,
            update_account_from_live,
            switch_to,
            get_live_quota,
            get_account_quota,
            claim_preview,
            claim_refresh,
            claim_current,
            claim_start,
            claim_captcha_config,
            claim_captcha_submit,
            claim_cancel,
            claim_pending,
            activity_probe,
            set_activity_tip,
            tray_flash,
            tray_flash_stop,
            log_probe,
            log_client_error,
            oauth_providers,
            oauth_begin,
            set_auth_proxy,
            kill_zcode,
            set_behavior,
            set_language,
            autostart_status,
            autostart_set,
            export_pick_path,
            export_finalize,
            export_all_pick_path,
            export_all_finalize,
            import_pick_files,
            import_sealed,
            pick_zcode_path,
            set_zcode_path,
            launch_zcode,
            open_external,
            open_settings,
            reveal_main,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    let paths = Paths::detect();
                    if store::load_settings(&paths).close_to_tray() {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                if window.label() == "captcha" {
                    *pending_guard() = None;
                }
            }
        })
        .setup(|app| {
            i18n::init_from_settings(&store::load_settings(&Paths::detect()));
            if let Ok(data_dir) = app.path().app_local_data_dir() {
                flowlog::init(&data_dir);
            }
            {
                let home = Paths::detect().home;
                std::thread::spawn(move || changelog::refresh_cache(&home));
            }
            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().expect("no window icon").clone())
                .tooltip("Z·SWITCH")
                .menu(&tray_menu_inner(app.handle())?)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, event| {
                    let id = event.id().as_ref().to_string();
                    match id.as_str() {
                        "show" => show_main(app),
                        "activity" => {
                            TRAY_FLASH_ACTIVE.store(false, Ordering::SeqCst);
                            let _ = app.emit("tray-activity", ());
                        }
                        "quit" => app.exit(0),
                        other => run_tray_action(app.clone(), other.to_string()),
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { button: tauri::tray::MouseButton::Left, button_state: tauri::tray::MouseButtonState::Up, .. } = event {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
