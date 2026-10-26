import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { esc, toast, openPwModal, openConfirmModal, openProviderModal, openActivityModal, activityToast, installDelegation, dismissSplash } from "./ui.js";
import { ic } from "./icons.js";
import { init, t, has, lang, localeTag, stripErr } from "./i18n.js";

const $app = document.getElementById("app");
let state = null;
let renaming = null;
let busy = false;
let appVer = "";
let acctQuota = {};
let claimable = {};
let claimedState = {};
let claimAllRunning = false;
const REFRESH_CLAIM_COOLDOWN_MS = 60_000;
let refreshClaim = { running: false, done: 0, total: 0, cooldownUntil: 0 };
let acctRefreshBusy = {};
let acctRefreshCooldown = {};
let refreshTicker = null;

// 活动中心：已探测到的活动（accountId -> ClaimPlan[]，含 campaign 元数据）
let activities = {};
const KNOWN_KEY = "zsw-activity-known";
const SEEN_KEY = "zsw-activity-seen";
const PROBE = {
  baseMs: 20 * 60_000,        // 活跃窗口内轮次基准
  jitterMs: 5 * 60_000,       // + U(0,5min)
  acctDelayMs: 5 * 60_000,    // 每个账号独立再取 U(0,5min) 延迟
  acctGapMs: 60 * 60_000,     // 账号级最小间隔
  perAcctDaily: 20,           // 每账号每日配额
  lowGapMs: 3 * 3600_000,     // 12:00–16:00 与达配额后的间隔
  reqGapMs: [20_000, 90_000], // 串行请求间隔
};
let probe = { timer: null, nextAt: 0, running: false, roundRunning: false, lastAt: {}, done: {}, day: "", baselineDay: "", lastSummary: null };

// plan_id -> 活动元数据（campaign_id / 展示位 / 文案 / 时间窗）。
// 服务端只在 marketing delivery 上带 campaign_id，而「已领取」条目来自 billing/current（没有该字段）；
// 活动元数据本身是全局的，所以按 plan_id 缓存一次，所有账号都能复用（不是编造）。
const PLAN_META_KEY = "zsw-plan-meta";
let planMeta = {};
function loadPlanMeta() {
  try { planMeta = JSON.parse(localStorage.getItem(PLAN_META_KEY) || "{}"); } catch { planMeta = {}; }
}
function upsertPlanMeta(plans) {
  let changed = false;
  for (const p of plans || []) {
    const planId = p.plan_id || p.planId || "";
    const c = p.campaign;
    if (!planId || !c) continue;
    const campaignId = c.campaign_id || c.campaignId || "";
    if (!campaignId) continue;
    const prev = planMeta[planId] || {};
    const next = {
      campaignId,
      position: c.position || prev.position || "",
      title: c.title || prev.title || "",
      body: c.body || prev.body || "",
      buttons: (c.buttons || []).length ? c.buttons : (prev.buttons || []),
      startsAt: c.starts_at ?? c.startsAt ?? prev.startsAt ?? null,
      endsAt: c.ends_at ?? c.endsAt ?? prev.endsAt ?? null,
    };
    if (JSON.stringify(prev) !== JSON.stringify(next)) {
      planMeta[planId] = next;
      changed = true;
    }
  }
  if (changed) {
    const ids = Object.keys(planMeta);
    if (ids.length > 300) ids.slice(0, ids.length - 300).forEach((k) => delete planMeta[k]);
    try { localStorage.setItem(PLAN_META_KEY, JSON.stringify(planMeta)); } catch { /* ignore */ }
  }
}

/// 「新活动」提示：窗口标题（任务栏悬停可见）+ 托盘 tooltip（托盘悬停可见）。
/// Toast 在便携版上不可靠，这两处是系统级可见性兜底。
function setActivityTip(text) {
  const shown = text ? text.slice(0, 60) : null;
  try {
    getCurrentWindow().setTitle(shown ? `Z·SWITCH · ${shown}` : "Z·SWITCH");
  } catch { /* 忽略 */ }
  invoke("set_activity_tip", { tip: shown })
    .catch((e) => logProbe(`tray tip error: ${stripErr(e)}`));
}

/// 托盘图标交替闪烁（Windows 托盘无闪烁 API，由后端切图标模拟）。
/// times=0 → 持续闪烁，直到用户点开活动详情（openActivity / 托盘菜单）才停止。
function flashTray(times = 0) {
  invoke("tray_flash", { times }).catch((e) => logProbe(`tray flash error: ${stripErr(e)}`));
}
function stopTrayFlash() {
  invoke("tray_flash_stop").catch(() => {});
}

let notifyGranted = null;
/// 系统通知授权（缓存结果）：返回是否允许发送。
async function ensureNotifyPermission() {
  try {
    if (notifyGranted === true) return true;
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    notifyGranted = granted;
    return granted;
  } catch (e) {
    logClientError(`notify permission: ${stripErr(e)}`);
    return false;
  }
}

function fmtDateTime(ms) {
  return ms
    ? new Date(ms).toLocaleString(localeTag(), { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false })
    : "";
}

/// 「什么时候生效 → 什么时候结束」：生效优先 effective_at（权益生效），其次 starts_at
function claimTimeRange(p) {
  const startMs = p.effective_at ?? p.starts_at ?? null;
  const start = startMs ? fmtDateTime(startMs) : t("claim.notProvided");
  const end = p.ends_at ? fmtDateTime(p.ends_at) : t("claim.notProvided");
  const pending = !!startMs && startMs > Date.now();
  return { text: t(pending ? "claim.pendingRange" : "claim.timeRange", { start, end }), pending, startMs, end };
}

const AUTO_CLAIM_INTERVAL_MS = 10 * 60 * 1000;
const AUTO_CLAIM_FIRST_DELAY_MS = 2 * 60 * 1000;
const AUTO_CLAIM_WAIT_MS = 45_000;

const AUTO_ABORT_WAIT_MS = 90_000;
let autoClaimRunning = false;
let autoClaimCooldown = {};
let autoAbortRequested = false;
let claimActive = false;
let lastAutoRound = null;
let autoToggleBusy = false;

const REQ_TIMEOUT_MS = 60_000;

// 前端异常落盘：界面冻结时用户看不到 DevTools，问题必须可回传
function logClientError(msg) {
  try {
    invoke("log_client_error", { message: String(msg).slice(0, 2000) }).catch(() => {});
  } catch { /* 日志失败不影响界面 */ }
}
window.addEventListener("error", (ev) => logClientError(`error: ${ev.message} @ ${ev.filename}:${ev.lineno}`));
window.addEventListener("unhandledrejection", (ev) => logClientError(`rejection: ${stripErr(ev.reason) || ev.reason}`));

// 请求看门狗：上游可能因风控/网络长时间不返回，前端不允许无限 busy
async function invokeWithTimeout(cmd, args, ms = REQ_TIMEOUT_MS) {
  let timer = null;
  try {
    return await Promise.race([
      invoke(cmd, args),
      new Promise((_, rej) => { timer = setTimeout(() => rej(new Error(t("m.refreshTimeout"))), ms); }),
    ]);
  } finally {
    if (timer) clearTimeout(timer);
  }
}

const cooldownLeft = (until) => Math.max(0, Math.ceil(((until || 0) - Date.now()) / 1000));

// 唯一的刷新准入判据：所有入口共用，避免各处各写一遍「忙不忙」并静默 return
function refreshGate(scope) {
  if (claimAllRunning) return { ok: false, kind: "running" };
  if (scope === "global") {
    if (refreshClaim.running || claimActive) return { ok: false, kind: "running" };
    const wait = cooldownLeft(refreshClaim.cooldownUntil);
    return wait > 0 ? { ok: false, kind: "cooldown", waitSec: wait } : { ok: true };
  }
  if (refreshClaim.running || claimActive || autoClaimRunning || acctRefreshBusy[scope]) {
    return { ok: false, kind: "running" };
  }
  const wait = cooldownLeft(acctRefreshCooldown[scope]);
  return wait > 0 ? { ok: false, kind: "cooldown", waitSec: wait } : { ok: true };
}

function toastGate(g) {
  if (!g || g.ok) return false;
  if (g.kind === "cooldown") toast(t("m.refreshCooldownSec", { n: g.waitSec }), "warn");
  else toast(t("m.refreshBusyRunning"), "warn");
  return false;
}

// 独立于 $app 的错误条：render 全挂时仍可见、可关闭
function clearRenderError() {
  const el = document.getElementById("err-bar");
  if (el) { el.style.display = "none"; el.innerHTML = ""; }
}

function showRenderError(e) {
  const msg = stripErr(e) || String(e);
  let el = document.getElementById("err-bar");
  if (!el) {
    el = document.createElement("div");
    el.id = "err-bar";
    document.body.prepend(el);
  }
  el.style.display = "";
  el.innerHTML = `<span>${esc(t("m.renderError"))} · ${esc(msg)}</span>` +
    `<button class="eb-close" click="actions.dismissErrBar()" aria-label="close">✕</button>`;
  logClientError(`render: ${msg}`);
}

const NOTCH_COLORS = ["var(--notch-1)", "var(--notch-2)", "var(--notch-3)", "var(--notch-4)", "var(--notch-5)", "var(--notch-6)"];
function notchColor(id) {
  let h = 0;
  for (const c of id) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return NOTCH_COLORS[h % NOTCH_COLORS.length];
}

function fmtNum(v) {
  if (v == null) return t("q.unknown");
  const n = Number(v);
  if (!isFinite(n)) return t("q.unknown");
  if (lang() === "zh") {
    if (Math.abs(n) >= 1e8) return (n / 1e8).toFixed(2) + " 亿";
    if (Math.abs(n) >= 1e4) return (n / 1e4).toFixed(2) + " 万";
    return n.toLocaleString(localeTag(), { maximumFractionDigits: 2 });
  }
  if (Math.abs(n) >= 1e9) return (n / 1e9).toFixed(2) + "B";
  if (Math.abs(n) >= 1e6) return (n / 1e6).toFixed(2) + "M";
  return n.toLocaleString(localeTag(), { maximumFractionDigits: 2 });
}
function idLabel(id) {
  if (!id) return null;
  return id.display_name || id.username || id.email || null;
}

async function refresh() {
  state = await invoke("get_state");
  if (state?.language) init(state.language);
  maybeStartProbe();
  startReminderTicker();
}

function uiLocked() {
  return renaming !== null;
}

async function guard(fn) {
  if (busy) return;
  busy = true;
  try {
    await fn();
  } catch (e) {
    toast(stripErr(e), "err");
  } finally {
    busy = false;
  }
}

async function loadAcctQuota(id, force = false) {
  const cur = acctQuota[id] || {};
  if (cur.busy) return;
  acctQuota[id] = { busy: true };
  if (!uiLocked()) render();
  try {
    const data = await invokeWithTimeout("get_account_quota", { id, force });
    acctQuota[id] = { data, err: null, busy: false };
  } catch (e) {
    acctQuota[id] = { data: null, err: stripErr(e), busy: false };
  }
  if (!uiLocked()) render();
}

async function loadClaimPreview(id) {
  const cur = claimable[id] || {};
  if (cur.busy) return;
  claimable[id] = { plans: cur.plans || [], busy: true };
  try {
    const plans = await invokeWithTimeout("claim_preview", { id });
    claimable[id] = { plans: plans || [], err: null, busy: false };
  } catch (e) {
    claimable[id] = { plans: cur.plans || [], err: String(e), busy: false };
  }
}

let claimWaiter = null;
function waitForClaimResult(accountId, timeoutMs = 90000) {
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (!done) { done = true; claimWaiter = null; clearTimeout(t); resolve(v); } };
    const t = setTimeout(() => finish(null), timeoutMs);
    claimWaiter = { accountId, finish };
  });
}

async function awaitClaimPreviewFresh(id) {
  claimable[id] = { ...(claimable[id] || {}), busy: true };
  try {
    const plans = await invokeWithTimeout("claim_preview", { id });
    claimable[id] = { plans: plans || [], err: null, busy: false };
  } catch (e) {
    claimable[id] = { plans: claimable[id]?.plans || [], err: String(e), busy: false };
  }
}

// 已领取状态（billing/current + 本地历史）：区分「无活动」与「已领取生效中」。
// current 失败静默——可领列表优先展示，不受影响。
async function fetchClaimed(id) {
  try {
    const r = await invokeWithTimeout("claim_current", { id });
    claimedState[id] = { plans: r.plans || [], history: r.history || [], err: null };
    // 已领取/待生效也进目录：服务端之后不再下发，只能靠目录在重启后继续展示
    upsertCatalog(id, r.plans || [], true);
  } catch (e) {
    claimedState[id] = {
      plans: claimedState[id]?.plans || [],
      history: claimedState[id]?.history || [],
      err: String(e),
    };
  }
}

const accountName = (id) => (state?.accounts || []).find((a) => a.id === id)?.name || id;

function autoPillTitle(s) {
  const bits = [t("btn.autoClaimTitle")];
  if (autoClaimRunning) bits.push(t(s.auto_claim ? "m.autoClaimRound" : "m.autoClaimStopping"));
  if (lastAutoRound) {
    const d = new Date(lastAutoRound.at);
    const time = d.toDateString() === new Date().toDateString()
      ? d.toLocaleTimeString(localeTag(), { hour12: false })
      : `${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")} ${d.toLocaleTimeString(localeTag(), { hour12: false })}`;
    bits.push(t("m.autoClaimLast", { time, claimed: lastAutoRound.claimed, skipped: lastAutoRound.skipped }));
    if (lastAutoRound.noActivity) bits.push(t("m.autoClaimNoActivity"));
    if (lastAutoRound.cooldownAll) bits.push(t("m.autoClaimCooldownAll"));
  }
  return esc(bits.join(" · "));
}

const actions = {
  async refresh() { await refresh(); render(); },

  dismissErrBar() { clearRenderError(); },

  // 活动详情：accountId 传入时先展示该账号，未命中则回退到全量列表。
  // 手动打开视为显式意图：已开启探测时后台补探一次（30s 节流，绕过静默期）。
  openActivity(accountId) {
    stopTrayFlash(); // 用户已查看活动，停止持续闪烁
    setActivityTip(null); // 用户已查看活动，清掉标题/托盘提示
    if (state?.activity_probe && !probe.running && Date.now() - (probe.manualAt || 0) > 30_000) {
      probe.manualAt = Date.now();
      const ids = shuffle((state?.accounts || []).map((a) => a.id)).slice(0, 2);
      if (ids.length) probeOnce(ids, true).catch(() => {});
    }
    const rows = activityRows();
    if (!rows.length) { toast(t("m.activityNoneHint"), "warn"); return; }
    const mine = accountId ? rows.filter((r) => r.accountId === accountId) : [];
    openActivityModal({ rows: mine.length ? mine : rows, focusAccount: accountId });
  },

  // 手动刷新时强制探测一次（绕过静默期，用户显式意图）
  async probeActivityNow() {
    if (probe.running) { toast(t("m.refreshBusyRunning"), "warn"); return; }
    const ids = (state?.accounts || []).map((a) => a.id).slice(0, 2);
    if (!ids.length) return;
    await probeOnce(ids, true);
    actions.openActivity();
  },

  async capture() {
    await guard(async () => {
      const r = await invoke("capture_current", { name: null });
      toast(t("m.toastSaved", { name: r.name }), "ok", t("m.toastSavedDetail"));
      await refresh(); render();
      enrollAccounts();
    });
  },

  async rename(id) {
    renaming = id; render();
    const input = document.querySelector(`.row[data-id="${id}"] .rename-input`);
    if (input) { input.focus(); input.select(); }
  },

  async doRename(id) {
    const input = document.querySelector(`.row[data-id="${id}"] .rename-input`);
    const name = (input?.value || "").trim();
    if (!name) return;
    window.__renameSaving = true;
    clearTimeout(window.__renameBlurTimer);
    await guard(async () => {
      const r = await invoke("rename_account", { id, name });
      toast(t("m.toastRenamed", { name: r.name }));
      renaming = null;
      await refresh(); render();
    }).finally(() => { window.__renameSaving = false; });
  },

  cancelRename() { renaming = null; render(); },

  deferCancelRename(id) {
    clearTimeout(window.__renameBlurTimer);
    window.__renameBlurTimer = setTimeout(() => {
      if (renaming === id && !window.__renameSaving) actions.cancelRename();
    }, 180);
  },

  async delete(id) {
    const a = state?.accounts.find((x) => x.id === id);
    if (!a) return;
    openConfirmModal({
      kind: "danger",
      icon: "x",
      title: t("m.deleteTitle", { name: a.name }),
      desc: t("m.deleteDesc"),
      yesLabel: t("common.delete"),
      onYes: () => actions.doDelete(id),
    });
  },

  async doDelete(id) {
    await guard(async () => {
      await invoke("delete_account", { id });
      toast(t("m.toastDeleted"));
      await refresh(); render();
    });
  },

  askSwitch(id) {
    if (state.zcode_running && !state.hot_switch) {
      const a = state.accounts.find((x) => x.id === id);
      if (!a) return;
      openConfirmModal({
        kind: "warn",
        icon: "swap",
        title: t("m.switchTitle", { name: a.name }),
        desc: `<span class="warn-line">${t("m.switchDesc", { restart: t(state.launch_after_switch ? "m.switchRestartYes" : "m.switchRestartNo") })}</span>`,
        yesLabel: t("m.switchYes"),
        onYes: () => actions.doSwitch(id, true),
      });
    } else {
      actions.doSwitch(id, false);
    }
  },

  async doSwitch(id, force) {
    await guard(async () => {
      const restart = state.launch_after_switch;
      const r = await invoke("switch_to", { id, force, restart });
      if (r.already_active) {
        toast(t("m.toastAlready", { name: r.name }), "ok");
      } else {
        const bits = [];
        if (r.hot) bits.push(t("m.bitHot"));
        if (r.killed) bits.push(t("m.bitKilled"));
        if (r.preserved_as) bits.push(t("m.bitPreserved", { name: r.preserved_as }));
        if (r.launched) bits.push(t("m.bitLaunched"));
        if (r.config_stale) bits.push(t("m.bitConfigStale"));
        const probeBad = r.probe && r.probe.kind !== "entitled";
        if (probeBad) {
          bits.push(t(r.probe.kind === "no_entitlement" ? "m.bitProbeNoPlan" : "m.bitProbeAuthFailed"));
        }
        toast(t("m.toastSwitched", { name: r.name }), (r.config_stale || probeBad) ? "warn" : "ok", bits.join(t("common.listSep")));
      }
      await refresh(); render();
      pokeAccount(id);
    });
  },

  async updateFromLive(id) {
    await guard(async () => {
      const r = await invoke("update_account_from_live", { id });
      toast(t("m.toastSynced", { name: r.name }), "ok", t("m.toastSyncedDetail"));
      await refresh(); render();
    });
  },

  async exportOne(id) {
    await guard(async () => {
      const p = await invoke("export_pick_path", { id });
      if (!p.picked) { toast(t("m.exportCanceled")); return; }
      openPwModal({ mode: "export", id, path: p.path, name: p.name });
    });
  },

  async launch() {
    await guard(async () => {
      await invoke("launch_zcode");
      toast(t("m.launching"));
      setTimeout(() => actions.refresh(), 2500);
    });
  },

  askKill() {
    openConfirmModal({
      kind: "danger",
      icon: "power",
      title: t("m.killTitle"),
      yesLabel: t("m.killYes"),
      onYes: () => actions.doKill(),
    });
  },

  async doKill() {
    await guard(async () => {
      await invoke("kill_zcode");
      toast(t("m.toastKilled"));
      await refresh(); render();
    });
  },

  async openSettings() {
    try { await invoke("open_settings"); }
    catch (e) { toast(stripErr(e), "err"); }
  },
  acctQuota(id) {
    const dueAt = quotaDue[id];
    loadAcctQuota(id, true).then(() => {
      if (quotaDue[id] === dueAt) scheduleNext(id);
    });
  },

  async addAccount() {
    let providers;
    try { providers = await invoke("oauth_providers"); }
    catch (e) { toast(stripErr(e), "err"); return; }
    openProviderModal({
      providers,
      onPick: async (id) => {
        try {
          await invoke("oauth_begin", { provider: id });
          toast(t("m.loginWindowOpened"), "ok", t("m.loginWindowDetail"));
        } catch (e) {
          toast(stripErr(e), "err");
        }
      },
    });
  },

  async claim(id) {
    if (claimActive || claimAllRunning || refreshClaim.running) { toast(t("m.claimBusy"), "warn"); return; }
    // 数据源与行内条/弹框一致：探测发现的活动也要能直接领
    const plans = claimablePlansFor(id);
    const plan = plans[0];
    if (!plan) { toast(t("m.noClaimable"), "warn"); return; }
    claimActive = true;
    // 从活动弹框里点「领取」时先关掉弹框，避免挡住验证码窗口
    document.querySelector(".act-mask")?.remove();
    try {
      await invoke("claim_start", { id, planId: plan.plan_id });
      toast(t("m.claimVerify", { name: plan.name || plan.plan_id }), "ok", t("m.claimVerifyDetail"));
      const r = await waitForClaimResult(id);
      if (!r) toast(t("m.claimTimeout"), "warn");
      else if (r.ok !== false) {
        markClaimed(id, plan.plan_id);
        await fetchClaimed(id);
      }
    } catch (e) {
      toast(stripErr(e), "err");
    } finally {
      claimActive = false;
    }
  },

  async claimAll() {
    const ids = (state?.accounts || [])
      .map((a) => a.id)
      .filter((id) => (claimable[id]?.plans || []).length > 0);
    if (!ids.length) { toast(t("m.noClaimableAccounts"), "warn"); return; }
    const g = refreshGate("global");
    if (!g.ok) { toastGate(g); return; }
    claimAllRunning = true;
    claimActive = true;
    try {
      for (let i = 0; i < ids.length; i++) {
        const id = ids[i];
        const plan = claimable[id].plans[0];
        const name = state.accounts.find((a) => a.id === id)?.name || id;
        try {
          await invoke("claim_start", { id, planId: plan.plan_id });
        } catch (e) {
          toast(t("m.claimAccountErr", { name, err: stripErr(e) }), "err");
          continue;
        }
        const r = await waitForClaimResult(id, 120000);
        if (!r) {
          toast(t("m.claimAcctTimeout", { name }), "warn");
          await cancelClaimIfMine(id, plan.plan_id);
        } else if (r.ok !== false) {
          markClaimed(id, plan.plan_id);
          await fetchClaimed(id);
        }
        if (i < ids.length - 1) await new Promise((res) => setTimeout(res, 1200));
      }
    } finally {
      claimAllRunning = false;
      claimActive = false;
    }
  },

  async refreshClaim() {
    const ids = (state?.accounts || []).map((a) => a.id);
    if (!ids.length) return;
    // 自动轮次运行中允许抢跑（下面会先请求终止自动轮），因此这里不把 autoClaimRunning 判为忙
    const g = refreshGate("global");
    if (!g.ok) { toastGate(g); return; }
    refreshClaim = { running: true, done: 0, total: ids.length, cooldownUntil: 0, current: "" };
    startRefreshTicker();
    if (autoClaimRunning) {
      autoAbortRequested = true;
      const deadline = Date.now() + AUTO_ABORT_WAIT_MS;
      while (autoClaimRunning && Date.now() < deadline) {
        await new Promise((res) => setTimeout(res, 300));
      }
      if (autoClaimRunning) {
        refreshClaim.running = false;
        refreshClaim.cooldownUntil = Date.now() + REFRESH_CLAIM_COOLDOWN_MS;
        toast(t("m.claimBusy"), "warn");
        setTimeout(stopRefreshTickerIfIdle, 1100);
        return;
      }
    }
    let okCount = 0;
    try {
      for (let i = 0; i < ids.length; i++) {
        const id = ids[i];
        const name = state.accounts.find((a) => a.id === id)?.name || id;
        refreshClaim.done = i + 1;
        refreshClaim.current = name;
        claimable[id] = { plans: claimable[id]?.plans || [], busy: true };
        if (!uiLocked()) render();
        try {
          const r = await invokeWithTimeout("claim_refresh", { id, force: true });
          claimable[id] = { plans: r.plans || [], err: null, busy: false };
          await fetchClaimed(id);
          if ((r.plans || []).length) okCount++;
          if (r.activationError) toast(t("m.refreshClaimAcctErr", { name, err: stripErr(r.activationError) }), "warn");
        } catch (e) {
          claimable[id] = { plans: claimable[id]?.plans || [], err: String(e), busy: false };
          toast(t("m.refreshClaimAcctErr", { name, err: stripErr(e) }), "err");
        }
        scheduleNext(id);
        if (!uiLocked()) render();
        if (i < ids.length - 1) await new Promise((res) => setTimeout(res, 5000));
      }
    } finally {
      refreshClaim.running = false;
      refreshClaim.cooldownUntil = Date.now() + REFRESH_CLAIM_COOLDOWN_MS;
      if (!uiLocked()) render();
      setTimeout(stopRefreshTickerIfIdle, 1100);
    }
    toast(t("m.refreshClaimDone", { n: ids.length, k: okCount }), "ok");
  },

  async acctRefreshClaim(id) {
    const name = state?.accounts.find((a) => a.id === id)?.name || id;
    const g = refreshGate(id);
    if (!g.ok) { toastGate(g); return; }
    acctRefreshBusy[id] = true;
    acctRefreshCooldown[id] = 0;
    startRefreshTicker();
    claimable[id] = { plans: claimable[id]?.plans || [], busy: true };
    if (!uiLocked()) render();
    try {
      const r = await invokeWithTimeout("claim_refresh", { id, force: true });
      claimable[id] = { plans: r.plans || [], err: null, busy: false };
      await fetchClaimed(id);
      if (r.activationError) toast(t("m.refreshClaimAcctErr", { name, err: stripErr(r.activationError) }), "warn");
    } catch (e) {
      claimable[id] = { plans: claimable[id]?.plans || [], err: String(e), busy: false };
      toast(t("m.refreshClaimAcctErr", { name, err: stripErr(e) }), "err");
    } finally {
      acctRefreshBusy[id] = false;
      acctRefreshCooldown[id] = Date.now() + REFRESH_CLAIM_COOLDOWN_MS;
      scheduleNext(id);
      if (!uiLocked()) render();
      setTimeout(stopRefreshTickerIfIdle, 1100);
    }
  },

  async toggleAutoClaim() {
    if (autoToggleBusy) return;
    const next = !state.auto_claim;
    if (next && (autoClaimRunning || claimActive || claimAllRunning || refreshClaim.running)) {
      toast(t("m.claimBusy"), "warn");
      return;
    }
    autoToggleBusy = true;
    try {
      await invoke("set_behavior", { autoClaim: next });
      await refresh();
      render();
      if (state.auto_claim) {
        toast(t("m.autoClaimOn"), "ok", t("m.autoClaimOnDetail"));
        if (Date.now() - (lastAutoRound?.at ?? 0) > REFRESH_CLAIM_COOLDOWN_MS) autoClaimTick();
      } else {
        lastAutoRound = null;
      }
    } catch (e) { toast(stripErr(e), "err"); }
    finally { autoToggleBusy = false; }
  },
};

/// 冷却倒计时的「局部更新」：只改角标/顶部文案，避免每秒重建 innerHTML 导致行高抖动与滚动跳动。
function tickCooldowns() {
  let needRender = false;
  document.querySelectorAll("[data-cd-id]").forEach((el) => {
    const id = el.getAttribute("data-cd-id");
    const sec = cooldownLeft(acctRefreshCooldown[id]);
    if (sec <= 0) { needRender = true; return; }
    const txt = String(sec);
    if (el.textContent !== txt) el.textContent = txt;
  });
  const topLabel = document.getElementById("refresh-claim-label");
  if (topLabel) {
    const sec = cooldownLeft(refreshClaim.cooldownUntil);
    if (sec > 0 && !refreshClaim.running) {
      const txt = `${t("btn.refreshClaim")} (${sec}s)`;
      if (topLabel.textContent !== txt) topLabel.textContent = txt;
    } else if (sec <= 0) {
      needRender = true;
    }
  }
  if (needRender || refreshClaim.running) render();
}

function startRefreshTicker() {
  if (refreshTicker) return;
  refreshTicker = setInterval(() => {
    if (!uiLocked()) tickCooldowns();
    stopRefreshTickerIfIdle();
  }, 1000);
}
function stopRefreshTickerIfIdle() {
  const cooling = Date.now() < refreshClaim.cooldownUntil
    || Object.values(acctRefreshCooldown).some((until) => Date.now() < until);
  if (!refreshClaim.running && !cooling && refreshTicker) {
    clearInterval(refreshTicker); refreshTicker = null; if (!uiLocked()) render();
  }
}

function autoClaimCooldownFor(r) {
  const now = Date.now();
  // plan 级终态/1005 由 claimHold 记忆（见 holdForClaim），账号级只拦交互卡死与流程异常
  if (r.code === "interactive") return now + 60 * 60 * 1000;
  if (Number.isFinite(r.code) && r.code >= 1000) return 0;
  return now + AUTO_CLAIM_INTERVAL_MS;
}

// (账号|套餐) 级领取记忆：终态不重试、1005 等到 nextAt，避免浪费验证码求解与上游写请求
let claimHold = {};
const claimHoldKey = (id, planId) => `${id}|${planId}`;
function holdForClaim(r) {
  const now = Date.now();
  if (r.terminal && r.code === 1005) {
    return Number.isFinite(r.nextAt) && r.nextAt > now ? r.nextAt : now + 60 * 60 * 1000;
  }
  if (r.terminal) return Infinity;
  // 3007 / 挑战：拒码本身就是风控信号，3007 又不再自动重试，
  // 若按 10 分钟重试等于反复弹验证码窗口 → 拉长到 60 分钟。
  if (r.challenge || r.code === 3007) return now + 60 * 60 * 1000;
  return now + 60 * 60 * 1000;
}

/// 只在「待领取仍是自己发起的那一笔」时才取消：避免自动领取超时把用户手动的领取清掉。
async function cancelClaimIfMine(id, planId) {
  try {
    const p = await invoke("claim_pending");
    if (p && p.accountId === id && (!planId || p.planId === planId)) {
      await invoke("claim_cancel").catch(() => {});
      return true;
    }
  } catch { /* 查询失败则不动，不误伤 */ }
  return false;
}
// 已领取标记：同一账号对同一活动只领一次，且不必反复向上游查询「是否领过」
const MARKS_KEY = "zsw-claim-marks";
let claimMarks = {};
const markKey = (id, planId) => `${id}|${planId}`;
function loadMarks() {
  try { claimMarks = JSON.parse(localStorage.getItem(MARKS_KEY) || "{}"); } catch { claimMarks = {}; }
}
function saveMarks() {
  let keys = Object.keys(claimMarks);
  if (keys.length > 300) {
    keys.sort((a, b) => claimMarks[a] - claimMarks[b]).slice(0, keys.length - 300).forEach((k) => delete claimMarks[k]);
  }
  try { localStorage.setItem(MARKS_KEY, JSON.stringify(claimMarks)); } catch { /* ignore */ }
}
const isMarked = (id, planId) => !!claimMarks[markKey(id, planId)];
function markClaimed(id, planId) {
  if (!id || !planId) return;
  claimMarks[markKey(id, planId)] = Date.now();
  saveMarks();
  // 同步摘掉探测/刷新缓存里的这条，避免领完后可领条与「领取」按钮还在
  const keep = (p) => (p.plan_id || p.planId) !== planId;
  if (activities[id]) activities[id] = activities[id].filter(keep);
  if (claimable[id]?.plans) claimable[id].plans = claimable[id].plans.filter(keep);
}

/// 自动领取的准入集合：必须「已探测到活动 + 已开始 + 未过期 + 本账号未领过」。
/// 没有探测数据时返回空集 —— 自动领取不生效，且不发任何上游请求。
function autoTargets() {
  const now = Date.now();
  const out = [];
  for (const r of activityRows()) {
    if (r.status !== "claimable") continue;             // 已领取 / 已过期都不参与
    if (isMarked(r.accountId, r.planId)) continue;      // 本账号已领过该活动
    const startAt = r.startsAt ?? r.effectiveAt ?? null;
    if (startAt && startAt > now) continue;             // 活动还没开始
    if (r.endsAt && r.endsAt <= now) continue;          // 已过期
    // 必须用 claimablePlansFor（刷新 + 探测合并）——只读 claimable 会让「探测发现的活动」永远进不了自动领取
    const plan = claimablePlansFor(r.accountId).find((p) => (p.plan_id || p.planId) === r.planId);
    if (!plan) continue;                                // 无可用条目：不发请求，等下次探测
    if ((claimHold[claimHoldKey(r.accountId, r.planId)] ?? 0) > now) continue;
    if ((autoClaimCooldown[r.accountId] ?? 0) > now) continue;
    out.push({ accountId: r.accountId, plan });
  }
  return out;
}

/// 统一领取流程：`auto`（自动领取 10 分钟轮）与 `effective`（到点自动领取）共用。
async function runClaimTargets(targets, source) {
  autoClaimRunning = true; claimActive = true; autoAbortRequested = false;
  let roundClaimed = 0, roundSkipped = 0;
  if (!uiLocked()) render();
  try {
    for (const t of targets) {
      if (autoAbortRequested) break;
      if (source === "auto" && !state?.auto_claim) break;
      if (source === "effective" && !state?.claim_at_effective) break;
      const id = t.accountId;
      const plan = t.plan;
      try {
        await invoke("claim_start", { id, planId: plan.plan_id, auto: true });
      } catch (e) {
        autoClaimCooldown[id] = Date.now() + AUTO_CLAIM_INTERVAL_MS;
        roundSkipped++;
        continue;
      }
      const r = await waitForClaimResult(id, AUTO_CLAIM_WAIT_MS);
      if (!r) {
        await cancelClaimIfMine(id, plan.plan_id);
        autoClaimCooldown[id] = Date.now() + AUTO_CLAIM_INTERVAL_MS;
        roundSkipped++;
        continue;
      }
      if (r.ok === false) {
        claimHold[claimHoldKey(id, plan.plan_id)] = holdForClaim(r);
        autoClaimCooldown[id] = autoClaimCooldownFor(r);
        roundSkipped++;
        continue;
      }
      markClaimed(id, plan.plan_id);
      roundClaimed++;
      await awaitClaimPreviewFresh(id).catch(() => {});
      await fetchClaimed(id);
      if (!uiLocked()) render();
      await new Promise((res) => setTimeout(res, 1200));
    }
  } finally {
    autoClaimRunning = false; claimActive = false;
    if (source === "auto" && state?.auto_claim) {
      lastAutoRound = { at: Date.now(), claimed: roundClaimed, skipped: roundSkipped };
    }
    logProbe(`claim round(${source}) claimed=${roundClaimed} skipped=${roundSkipped}`);
    if (!uiLocked()) render();
  }
}

async function autoClaimTick() {
  if (!state?.auto_claim || autoClaimRunning) return;
  if (claimActive || claimAllRunning || refreshClaim.running) return;

  // 准入完全由「已探测到的活动」决定：没有可领且已开始、未过期的活动时，
  // 自动领取不生效，也不发任何上游请求（不再逐个账号 claim_refresh）。
  const targets = autoTargets();
  if (!targets.length) {
    lastAutoRound = { at: Date.now(), claimed: 0, skipped: 0, noActivity: true };
    if (!uiLocked()) render();
    return;
  }
  await runClaimTargets(targets, "auto");
}

/// 到点自动领取（独立开关）：只要求「已到点且尚未领取」，
/// 不再只依赖「未来时间点的提醒」——否则开关打开时已经开始的活动永远不会被领。
async function effectiveClaimTick() {
  if (!state?.claim_at_effective) return;
  if (claimActive || claimAllRunning || refreshClaim.running || autoClaimRunning) {
    logProbe("effective tick skip(busy)");
    return;
  }
  const targets = autoTargets();
  logProbe(`effective tick targets=${targets.length}${targets.length ? " ids=" + targets.map((t) => t.accountId.slice(0, 8)).join(",") : ""}`);
  if (!targets.length) return;
  await runClaimTargets(targets, "effective");
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rand = (a, b) => a + Math.floor(Math.random() * (b - a));
function shuffle(list) {
  const a = [...list];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}

// 活动汇总：可领（claimable）/ 已领取（claimed，含待生效）/ 已过期（expired）
function activityRows() {
  const now = Date.now();
  const rows = [];
  const seen = new Set();
  const add = (acc, p, status) => {
    const c = p.campaign || null;
    const planId = p.plan_id || p.planId || "";
    const meta = planMeta[planId] || null;
    const campaignId = c?.campaign_id || c?.campaignId || meta?.campaignId || "";
    const key = `${acc.id}|${campaignId}|${planId}`;
    if (!planId || seen.has(key)) return;
    seen.add(key);
    rows.push({
      key,
      accountId: acc.id,
      accountName: acc.name,
      planId,
      campaignId,
      status,
      title: c?.title || meta?.title || p.name || planId,
      body: c?.body || meta?.body || p.description || "",
      position: c?.position || meta?.position || "",
      buttons: (c?.buttons || []).length ? c.buttons : (meta?.buttons || []),
      startsAt: p.starts_at ?? c?.starts_at ?? c?.startsAt ?? meta?.startsAt ?? null,
      effectiveAt: p.effective_at ?? null,
      endsAt: p.ends_at ?? c?.ends_at ?? c?.endsAt ?? meta?.endsAt ?? null,
      grants: (p.grant_items || []).map((g) => `${g.name} ${fmtTokens(g.units)}${g.period ? " · " + g.period : ""}`),
    });
  };
  for (const a of state?.accounts || []) {
    // 可领来源与行内条完全一致（claimablePlansFor），避免两处显示不一致
    upsertPlanMeta(activities[a.id] || []);
    upsertPlanMeta(claimable[a.id]?.plans || []);
    const live = claimablePlansFor(a.id);
    const liveIds = new Set(live.map((p) => p.plan_id || p.planId));
    for (const p of live) add(a, p, "claimable");

    const active = claimedState[a.id]?.plans || [];
    const claimedIds = new Set(active.map((p) => p.plan_id).filter(Boolean));
    for (const p of active) add(a, p, p.ends_at && p.ends_at < now ? "expired" : "claimed");

    // 持久化目录兜底：重启后 / 已静默不探测时，历史活动仍能展示（含已过期）
    for (const e of Object.values(catalog)) {
      if (e.accountId !== a.id) continue;
      if (liveIds.has(e.planId) || claimedIds.has(e.planId)) continue;
      const expired = !!e.endsAt && e.endsAt < now;
      const key = `${a.id}|${e.campaignId}|${e.planId}`;
      if (seen.has(key)) continue;
      seen.add(key);
      rows.push({
        key,
        accountId: a.id,
        accountName: a.name,
        planId: e.planId,
        campaignId: e.campaignId,
        status: expired ? "expired" : "claimed",
        title: e.title,
        body: e.body,
        position: e.position,
        buttons: e.buttons || [],
        startsAt: e.startsAt,
        effectiveAt: e.effectiveAt,
        endsAt: e.endsAt,
        grants: e.grants || [],
      });
    }

    if (!active.length && !live.length) {
      const hist = claimedState[a.id]?.history || [];
      const last = hist[hist.length - 1];
      if (last) add(a, { plan_id: last.planId, name: last.planName }, "expired");
    }
  }
  return rows;
}

function loadProbeState() {
  try {
    const v = JSON.parse(localStorage.getItem("zsw-probe-state") || "{}");
    probe.lastAt = v.lastAt || {};
    probe.done = v.done || {};
    probe.day = v.day || "";
    probe.baselineDay = v.baselineDay || "";
    probe.lastSummary = v.lastSummary || null;
  } catch { /* 坏数据忽略 */ }
}
function saveProbeState() {
  try {
    localStorage.setItem("zsw-probe-state", JSON.stringify({
      lastAt: probe.lastAt,
      done: probe.done,
      day: probe.day,
      baselineDay: probe.baselineDay,
      lastSummary: probe.lastSummary,
    }));
  } catch { /* 存储不可用忽略 */ }
}
function logProbe(msg) {
  invoke("log_probe", { message: String(msg).slice(0, 500) }).catch(() => {});
}
function knownQuiesce() {
  try {
    return JSON.parse(localStorage.getItem(KNOWN_KEY) || "{}");
  } catch {
    return {};
  }
}
function isQuiesced(id, now = Date.now()) {
  const k = knownQuiesce()[id];
  return !!k && (k.quiesceUntil || 0) > now;
}
function inLowWindow(d = new Date()) {
  const h = d.getHours();
  return h >= 12 && h < 16;
}

function applyProbeResult(item) {
  const id = item.accountId;
  const plans = item.plans || [];
  activities[id] = plans;
  upsertPlanMeta(plans); // 活动元数据全局缓存：供「已领取」条目复用 campaign_id / 展示位 / 时间窗
  upsertCatalog(id, plans); // 持久化目录：重启后仍能展示本账号的活动
  const claimed = claimedState[id]?.plans || [];
  if (claimed.length) upsertCatalog(id, claimed, true);
  const ends = plans
    .map((p) => p.endsAt ?? p.ends_at ?? p.campaign?.endsAt ?? p.campaign?.ends_at ?? 0)
    .filter(Boolean);
  if (ends.length) {
    const known = knownQuiesce();
    known[id] = { quiesceUntil: Math.max(...ends) };
    try { localStorage.setItem(KNOWN_KEY, JSON.stringify(known)); } catch { /* ignore */ }
  }
  notifyNewActivities(id, plans, true);
  if (!uiLocked()) render();
}

/// 发现去重与提醒：无论「可领」还是「已领取」，只要是首次见到的活动就弹一次 toast；
/// 只有可领的才自动打开详情弹框（已领的只提示，避免打断）。
// 可领活动的提醒去重是「会话级」：每次启动都会对未领取的活动再提醒一次（钉钉未读逻辑），
// 直到用户领取；已领取的活动则持久去重，只提醒一次。
let sessionSeen = new Set();

function notifyNewActivities(accountId, plans, claimable) {
  if (!accountId || !(plans || []).length) return;
  let persistent = [];
  try { persistent = JSON.parse(localStorage.getItem(SEEN_KEY) || "[]"); } catch { persistent = []; }
  const persistentSet = new Set(persistent);
  let fresh = 0;
  for (const p of plans) {
    const planId = p.plan_id || p.planId || "";
    if (!planId) continue;
    // 可领 → 会话级去重（每次启动都会再提醒，直到领取）；已领取 → 持久去重（只提醒一次）
    const campaignId = p.campaign?.campaign_id || p.campaign?.campaignId || "";
    const key = claimable ? `a:${accountId}|${campaignId || planId}` : `c:${campaignId || planId}`;
    const store = claimable ? sessionSeen : persistentSet;
    if (store.has(key)) continue;
    store.add(key);
    fresh++;
  }
  if (!claimable) {
    persistent = [...persistentSet];
    if (persistent.length > 300) persistent = persistent.slice(persistent.length - 300);
    try { localStorage.setItem(SEEN_KEY, JSON.stringify(persistent)); } catch { /* ignore */ }
  }
  if (!fresh) {
    logProbe(`notify skip(seen) ${accountId} plans=${plans.length} kind=${claimable ? "claimable" : "claimed"}`);
    return;
  }
  const open = () => {
    stopTrayFlash();
    setActivityTip(null);
    openActivityModal({
      rows: activityRows().filter((r) => r.accountId === accountId),
      focusAccount: accountId,
    });
  };
  const firstTitle = plans[0]?.campaign?.title || plans[0]?.name || plans[0]?.plan_id || "";
  setActivityTip(t("act.tipShort", { title: firstTitle }));
  flashTray(0); // 持续闪烁，直到用户点开活动详情
  // 前台 → 应用内右下角弹窗（可点击直达详情）；后台/最小化 → 系统通知（否则看不到）
  const msg = t("m.activityNew", { name: accountName(accountId) });
  // 系统通知始终发：窗口最小化、被其他窗口挡住、甚至用户没看屏幕时都能收到。
  ensureNotifyPermission().then((granted) => {
    logProbe(`notify sys ${accountId} granted=${granted}`);
    if (!granted) return;
    try {
      sendNotification({ title: t("act.title"), body: msg });
    } catch (e) {
      logClientError(`notify: ${stripErr(e)}`);
    }
  });
  activityToast(msg, open); // 应用内再留一条，回窗口时可直接点开详情
  // 便携版（未安装）在 Windows 上可能拿不到 Toast 权限：用任务栏闪烁兜底。
  // 注意：document.hasFocus() 在 Tauri 里不可靠，必须用窗口 API 判断；结果写入日志便于排查。
  (async () => {
    try {
      const focused = await getCurrentWindow().isFocused();
      if (focused) {
        logProbe(`attention skip(focused) ${accountId}`);
        return;
      }
      await getCurrentWindow().requestUserAttention(2);
      logProbe(`attention flash ${accountId}`);
    } catch (e) {
      logProbe(`attention error: ${stripErr(e)}`);
    }
  })();
  logProbe(`new ${accountId} +${fresh} (${claimable ? "claimable" : "claimed"})`);
}

async function probeOnce(ids, force = false, withClaimed = false) {
  if (probe.running) return;
  probe.running = true;
  try {
    for (let i = 0; i < ids.length; i++) {
      const id = ids[i];
      // 后台（最小化/关到托盘）也要探测：否则窗口不在前台时永远收不到活动通知。
      // 请求节奏仍由 20min+U(0,5min)、账号级 60min、日配额控制，与窗口可见性无关。
      if (!force && isQuiesced(id)) {
        logProbe(`skip ${id}（静默期：该账号已有有效期内活动）`);
        continue;
      }
      try {
        const r = await invokeWithTimeout("activity_probe", { ids: [id] }, 45_000);
        probe.lastAt[id] = Date.now();
        probe.done[id] = (probe.done[id] || 0) + 1;
        const item = (r.items || [])[0];
        if ((r.skipped || []).includes(id) || !item) {
          logProbe(`skip ${id}（风控冷却/账号读取失败/未返回）`);
        } else {
          const n = (item.plans || []).length;
          const title = item.plans?.[0]?.campaign?.title || item.plans?.[0]?.name || "";
          logProbe(`probe ${id} -> ${n} 条${n ? " · " + title : ""}`);
          probe.lastSummary = { at: Date.now(), hits: n };
          applyProbeResult(item);
        }
        saveProbeState();
        if (withClaimed) {
          // 已领账号服务端不再下发活动（marketing 返回 0 条），必须靠 billing/current 补出
          // 「已领取/待生效」那一条，否则活动会从列表里消失。基线每天只做一次，成本可控。
          await fetchClaimed(id);
          notifyNewActivities(id, claimedState[id]?.plans || [], false);
          if (!uiLocked()) render();
        }
      } catch (e) {
        logClientError(`probe ${id}: ${stripErr(e)}`);
        logProbe(`error ${id}: ${stripErr(e)}`);
      }
      if (i < ids.length - 1) await sleep(rand(PROBE.reqGapMs[0], PROBE.reqGapMs[1]));
    }
  } finally {
    probe.running = false;
  }
}

function scheduleProbeRound(delayMs) {
  clearTimeout(probe.timer);
  const d = Math.max(1000, delayMs);
  probe.nextAt = Date.now() + d;
  probe.timer = setTimeout(() => { probeRound().catch(() => {}); }, d);
}

// 后台不探测；回到前台补探一次（随机 30–120s，避免"一回到前台就立刻发包"的规律）
document.addEventListener("visibilitychange", () => {
  if (document.hidden || !state?.activity_probe || probe.running || probe.roundRunning) return;
  const remaining = (probe.nextAt || 0) - Date.now();
  if (remaining > 120_000 || remaining <= 0) {
    scheduleProbeRound(rand(30_000, 120_000));
  }
});

async function probeRound(force = false) {
  if (!state?.activity_probe || probe.roundRunning) return;
  probe.roundRunning = true;
  try {
  const now = Date.now();
  const day = new Date().toDateString();
  if (probe.day !== day) { probe.day = day; probe.done = {}; saveProbeState(); }
  const ids = (state?.accounts || [])
    .map((a) => a.id)
    .filter((id) => {
      const gap = (probe.done[id] || 0) >= PROBE.perAcctDaily ? PROBE.lowGapMs : PROBE.acctGapMs;
      return (probe.lastAt[id] || 0) + gap <= now && (force || !isQuiesced(id, now));
    });
  const picked = shuffle(ids).slice(0, 2);
  if (picked.length) {
    // 每个账号各自独立取 U(0,5min) 延迟（排序后串行），两次请求之间再插 20–90s 随机间隔，
    // 避免「同一轮两个请求几乎同时发出」这种脚本特征。
    const delays = picked.map(() => rand(0, PROBE.acctDelayMs)).sort((a, b) => a - b);
    let waited = 0;
    for (let i = 0; i < picked.length; i++) {
      const step = Math.max(0, delays[i] - waited);
      if (step > 0) await sleep(step);
      waited += step;
      await probeOnce([picked[i]], force);
      if (i < picked.length - 1) {
        const gap = rand(PROBE.reqGapMs[0], PROBE.reqGapMs[1]);
        await sleep(gap);
        waited += gap;
      }
    }
  }
  // 有候选：按活跃/低活跃窗口排下一轮；无候选（都在间隔/静默/配额内）：5 分钟后复查，
  // 不空转上游、也不至于把下一轮直接推到 3 小时后。
  const next = !picked.length
    ? 5 * 60_000
    : inLowWindow() ? PROBE.lowGapMs : PROBE.baseMs + Math.random() * PROBE.jitterMs;
  scheduleProbeRound(next);
  } finally {
    probe.roundRunning = false;
  }
}

function maybeStartProbe() {
  if (!state?.activity_probe) {
    if (probe.timer) { clearTimeout(probe.timer); probe.timer = null; }
    return;
  }
  if (probe.timer || probe.running || probe.roundRunning) return;
  // 开关打开 / 每次启动 → 立即跑基线；当天已做过完整探测时只补「已领/待生效」状态
  probeBaseline(new Date().toDateString()).catch(() => {});
}

/// 启动基线：串行遍历所有账号（1.5–3s 间隔），每账号「探测 marketing + 拉 billing/current」，
/// 之后才按 20min+抖动、每轮 2 个走。
/// 每次启动都做完整探测：这是「活动列表 + 已领展示 + 新活动弹窗」的数据基础，
/// 省掉它会直接表现为「活动 ID 缺失 / 没有弹窗」。
async function probeBaseline(today) {
  if (!state?.activity_probe || probe.running || probe.roundRunning) return;
  probe.roundRunning = true;
  const ids = (state?.accounts || []).map((a) => a.id);
  logProbe(`baseline start accounts=${ids.length}`);
  try {
    for (let i = 0; i < ids.length; i++) {
      // force=true：基线无视「静默到 ends_at」——每次启动只做一次（每账号 1 请求），
      // 否则静默账号在重启后既无展示数据、也不会补进活动目录。
      await probeOnce([ids[i]], true, true);
      if (i < ids.length - 1) await sleep(rand(1500, 3000));
    }
    probe.baselineDay = today;
    saveProbeState();
    if (!uiLocked()) render();
    logProbe("baseline done");
  } finally {
    probe.roundRunning = false;
    scheduleProbeRound(PROBE.baseMs + Math.random() * PROBE.jitterMs);
  }
}

// ---- 到点提醒 + 到点自动领取（独立于「自动领取」总开关）----
const FIRED_KEY = "zsw-reminder-fired";
let reminders = [];
let firedReminders = new Set();
let reminderTicker = null;

function loadReminders() {
  try {
    firedReminders = new Set(JSON.parse(localStorage.getItem(FIRED_KEY) || "[]"));
  } catch {
    firedReminders = new Set();
  }
}
function saveReminders() {
  let arr = [...firedReminders];
  if (arr.length > 500) arr = arr.slice(arr.length - 500);
  try { localStorage.setItem(FIRED_KEY, JSON.stringify(arr)); } catch { /* ignore */ }
}

function syncReminders() {
  const now = Date.now();
  for (const r of activityRows()) {
    for (const [kind, at] of [["start", r.startsAt], ["effective", r.effectiveAt]]) {
      if (!at || at <= now) continue;
      const key = `${r.accountId}|${r.planId}|${kind}`;
      if (firedReminders.has(key)) continue;
      if (!reminders.some((x) => x.key === key)) {
        reminders.push({ key, accountId: r.accountId, planId: r.planId, kind, at, name: r.accountName, title: r.title });
      }
    }
  }
}

// 60s ticker 比对时间戳（不用长 setTimeout）：系统休眠/时钟跳变后也能补触发
function checkReminders() {
  syncReminders();
  const now = Date.now();
  const due = reminders.filter((r) => r.at <= now);
  if (!due.length) return;
  reminders = reminders.filter((r) => r.at > now);
  for (const r of due) {
    if (firedReminders.has(r.key)) continue;
    firedReminders.add(r.key);
    saveReminders();
    toast(t(r.kind === "start" ? "act.startReminder" : "act.effectiveReminder", { name: r.name, title: r.title }), "ok");
    openActivityModal({ rows: activityRows().filter((x) => x.accountId === r.accountId), focusAccount: r.accountId });
    if (state?.claim_at_effective) scheduleClaimAtEffective();
  }
}

function startReminderTicker() {
  if (reminderTicker) return;
  reminderTicker = setInterval(checkReminders, 60_000);
  checkReminders();
}

function scheduleClaimAtEffective() {
  const delay = 5_000 + Math.random() * 25_000; // 5–30s 随机，避免整点齐射
  setTimeout(() => { effectiveClaimTick().catch(() => {}); }, delay);
}

/// 到点领取一次性尝试：只在该账号当前仍有可领套餐时发起；不启动任何循环。
async function claimForActivity(id) {
  if (claimActive || claimAllRunning || refreshClaim.running) {
    toast(t("m.refreshBusyRunning"), "warn");
    return;
  }
  const plan = (claimable[id]?.plans || [])[0];
  if (!plan) return; // 无可领（已领 / 无资格）：不打扰、不发请求
  if (isMarked(id, plan.plan_id)) return; // 本账号已领过该活动，不重复领取
  claimActive = true;
  try {
    await invoke("claim_start", { id, planId: plan.plan_id, auto: true });
    toast(t("m.claimVerify", { name: plan.name || plan.plan_id }), "ok", t("m.claimVerifyDetail"));
    const r = await waitForClaimResult(id, AUTO_CLAIM_WAIT_MS);
    if (!r) {
      await cancelClaimIfMine(id, plan.plan_id);
      toast(t("m.claimAcctTimeout", { name: accountName(id) }), "warn");
    } else     if (r.ok !== false) {
      markClaimed(id, plan.plan_id);
      await fetchClaimed(id);
    }
  } catch (e) {
    toast(stripErr(e), "err");
  } finally {
    claimActive = false;
    if (!uiLocked()) render();
  }
}

function quotaBarHtml(pct) {
  const used = pct == null ? null : Math.min(100, Math.max(0, pct));
  const remaining = used == null ? null : 100 - used;
  const danger = used != null && used >= 90 ? " danger" : used != null && used >= 70 ? " warn" : "";
  const txt = remaining == null ? "--" : remaining.toFixed(0) + "%";
  const txtCls = (remaining ?? 100) >= 58 ? " in-fill" : "";
  return `<div class="qbar${danger}"><div class="qbar-fill" style="width:${remaining ?? 100}%"></div><span class="qbar-pct${txtCls}">${txt}</span></div>`;
}

function itemKind(it) {
  if (it.kind) return it.kind;
  if (it.name.includes("提示次数")) return "prompt_count";
  if (it.name.includes("使用时长")) return "duration";
  return "raw";
}
function windowLabel(it) {
  if (it.window) {
    if (it.window.startsWith("hours:")) return t("q.win.hours", { n: it.window.slice(6) });
    return has(`q.win.${it.window}`) ? t(`q.win.${it.window}`) : it.window;
  }
  const m = it.name.match(/[（(]每\s*([^）)]+)[）)]/);
  if (m) return "每" + m[1].replace(/^每/, "");
  if (itemKind(it) === "duration") return t("q.monthlyShort");
  if (itemKind(it) === "prompt_count") return t("q.countShort");
  return it.name;
}
function resetLabel(it) {
  if (it.reset) return t("q.resets", { time: it.reset });
  return it.period_end || "";
}

function winRowHtml(it, cls = "") {
  return `
  <div class="q-win${cls}">
    <span class="q-win-label">${esc(windowLabel(it))}</span>
    ${quotaBarHtml(it.percent_used)}
    <span class="q-win-reset" title="${esc(resetLabel(it))}">${it.reset || it.period_end ? esc(resetLabel(it)) : ""}</span>
  </div>`;
}

function fmtTokens(n) {
  if (n == null) return "";
  if (lang() === "zh") {
    if (n >= 1e8) return (n / 1e8).toFixed(n % 1e8 === 0 ? 0 : 1) + "亿";
    if (n >= 1e6) return (n / 1e6).toFixed(n % 1e6 === 0 ? 0 : 1) + "M";
    if (n >= 1e3) return Math.round(n / 1e3) + "K";
    return String(Math.round(n));
  }
  if (n >= 1e9) return (n / 1e9).toFixed(n % 1e9 === 0 ? 0 : 1) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(n % 1e6 === 0 ? 0 : 1) + "M";
  if (n >= 1e3) return Math.round(n / 1e3) + "K";
  return String(Math.round(n));
}
function balRowHtml(it) {
  const rem = it.total != null && it.remaining != null ? `${fmtTokens(it.remaining)}/${fmtTokens(it.total)}` : "";
  const label = it.name.replace(/^GLM-?/i, "");
  return `
  <div class="q-win mini">
    <span class="q-win-label" title="${esc(it.name)}">${esc(label)}</span>
    ${quotaBarHtml(it.percent_used)}
    <span class="q-win-reset">${esc(rem)}</span>
  </div>`;
}

function tierChipHtml(tier, code) {
  const c = String(code || "").toLowerCase();
  const s = String(tier || "").toLowerCase();
  let label, cls;
  if (c === "max" || (!c && s.includes("max"))) { label = "Max"; cls = "max"; }
  else if (c === "pro" || (!c && s.includes("pro"))) { label = "Pro"; cls = "pro"; }
  else if (c === "lite" || (!c && s.includes("lite"))) { label = "Lite"; cls = "lite"; }
  else if (c === "start") { label = "Start"; cls = "trial"; }
  else if (c === "trial" || (!c && (s.includes("trial") || String(tier || "").includes("体验")))) { label = t("q.trial"); cls = "trial"; }
  else { label = tier; cls = "other"; }
  return `<span class="tier-b ${cls}">${esc(label)}</span>`;
}
function tierBadgeFor(id) {
  const q = acctQuota[id];
  if (!q?.data) return "";
  const plans = q.data.plans || [];
  const pairs = plans.map((p) => [p.tier, p.tier_code]);
  const list = (pairs.length ? pairs : q.data.plan_tier ? [[q.data.plan_tier, null]] : []).slice(0, 2);
  if (!list.length) return `<span class="tier-b free">Free</span>`;
  return list.map(([tier, code]) => tierChipHtml(tier, code)).join("");
}

function grantLabel(plan) {
  const items = plan.grant_items || [];
  if (items.length) {
    const g = items[0];
    return t("q.grant", {
      name: g.name,
      amount: fmtTokens(g.units),
      period: t(`q.period.${g.period}`, {}) === `q.period.${g.period}` ? g.period : t(`q.period.${g.period}`, {}),
    });
  }
  return (plan.grants || [])[0] || "";
}
// 活动目录：把「探测/刷新/current」看到过的活动持久化。
// 否则重启后 activities（内存）为空，而「静默到 ends_at」又阻止重新探测 → 列表空白、无角标、无弹窗。
const CATALOG_KEY = "zsw-activity-catalog";
let catalog = {};
function loadCatalog() {
  try { catalog = JSON.parse(localStorage.getItem(CATALOG_KEY) || "{}"); } catch { catalog = {}; }
}
function saveCatalog() {
  let keys = Object.keys(catalog);
  if (keys.length > 300) {
    keys.sort((a, b) => (catalog[a].firstSeenAt || 0) - (catalog[b].firstSeenAt || 0));
    keys.slice(0, keys.length - 300).forEach((k) => delete catalog[k]);
  }
  try { localStorage.setItem(CATALOG_KEY, JSON.stringify(catalog)); } catch { /* ignore */ }
}
function upsertCatalog(accountId, plans, claimedHint = false) {
  let added = 0;
  for (const p of plans || []) {
    const planId = p.plan_id || p.planId || "";
    if (!planId) continue;
    const c = p.campaign || null;
    const meta = planMeta[planId] || {};
    const k = `${accountId}|${planId}`;
    const prev = catalog[k] || {};
    if (!prev.planId) added++;
    catalog[k] = {
      accountId,
      planId,
      campaignId: c?.campaign_id || c?.campaignId || prev.campaignId || meta.campaignId || "",
      title: c?.title || prev.title || meta.title || p.name || planId,
      body: c?.body || prev.body || meta.body || p.description || "",
      position: c?.position || prev.position || meta.position || "",
      buttons: (c?.buttons || []).length ? c.buttons : (prev.buttons || meta.buttons || []),
      startsAt: p.starts_at ?? c?.starts_at ?? c?.startsAt ?? prev.startsAt ?? meta.startsAt ?? null,
      effectiveAt: p.effective_at ?? prev.effectiveAt ?? null,
      endsAt: p.ends_at ?? c?.ends_at ?? c?.endsAt ?? prev.endsAt ?? meta.endsAt ?? null,
      grants: (p.grant_items || []).length
        ? p.grant_items.map((g) => `${g.name} ${fmtTokens(g.units)}${g.period ? " · " + g.period : ""}`)
        : (prev.grants || []),
      claimedHint: claimedHint || prev.claimedHint === true,
      firstSeenAt: prev.firstSeenAt || Date.now(),
    };
  }
  if (added) saveCatalog();
  return added;
}

/// 某账号当前可领套餐：刷新结果（preview+marketing）优先，探测结果兜底；按 plan_id 去重。
/// 行内活动条与「活动」弹框必须共用这一个来源，否则会出现「弹框说可领、行内空白」。
function claimablePlansFor(id) {
  const out = [];
  const seen = new Set();
  // 已领取（billing/current）或已标记领取的 plan 不再算可领：
  // 否则探测缓存里的旧条目会让「领取」按钮在领完后依旧显示。
  const claimed = new Set((claimedState[id]?.plans || []).map((p) => p.plan_id).filter(Boolean));
  for (const p of [...(claimable[id]?.plans || []), ...(activities[id] || [])]) {
    const pid = p.plan_id || p.planId;
    if (!pid || seen.has(pid)) continue;
    if (claimed.has(pid) || isMarked(id, pid)) continue;
    seen.add(pid);
    out.push(p);
  }
  return out;
}

function claimStripHtml(id) {
  const plan = claimablePlansFor(id)[0];
  if (!plan) return "";
  const grants = grantLabel(plan);
  const label = plan.name || plan.plan_id;
  const camp = plan.campaign;
  const tr = camp && (camp.endsAt || camp.ends_at)
    ? claimTimeRange({ starts_at: camp.startsAt ?? camp.starts_at, ends_at: camp.endsAt ?? camp.ends_at })
    : null;
  return `
  <div class="claim-strip" click="actions.openActivity('${id}')" title="${esc(t("act.openDetail"))}">
    ${ic("gift", 15)}
    <span class="claim-name">${esc(label)}</span>
    ${grants ? `<span class="claim-grants">${esc(grants)}</span>` : ""}
    ${tr ? `<span class="claim-grants${tr.pending ? " warn-line" : ""}">${esc(tr.text)}</span>` : ""}
    <button class="btn-claim has-ic" click="actions.claim('${id}')" ${claimAllRunning || refreshClaim.running || claimActive || autoClaimRunning ? "disabled" : ""}>${ic("gift", 13)} ${t("btn.claim")}</button>
  </div>`;
}

function slotRowsHtml(items) {
  const list = items || [];
  const isWin = (it) => itemKind(it) !== "raw";
  const wins = list.filter(isWin);
  const best = new Map();
  for (const it of list) {
    if (isWin(it)) continue;
    const cur = best.get(it.name);
    if (!cur || (it.total || 0) > (cur.total || 0)) best.set(it.name, it);
  }
  const pools = [...best.values()].sort((a, b) => (b.total || 0) - (a.total || 0));
  return [
    ...wins.map((it) => winRowHtml(it, " mini")),
    ...pools.map(balRowHtml),
  ].join("");
}

function expireInfo(s) {
  if (!s) return null;
  const hasTime = s.length >= 16;
  const ms = new Date(hasTime ? s.replace(" ", "T") : s + "T23:59:59") - Date.now();
  if (isNaN(ms)) return { text: s, soon: false, warn: false };
  const soon = ms <= 5 * 86400000;
  const warn = ms <= 7 * 86400000;
  return { text: soon && hasTime ? s : s.slice(0, 10), soon, warn };
}

function planGroupHtml(p) {
  const label = p.tier_code === "other" && !p.pid ? t("q.other") : (p.name || p.tier || "");
  const exp = expireInfo(p.expire);
  return `
  <div class="plan-grp">
    <div class="pg-head">
      ${p.tier ? tierChipHtml(p.tier, p.tier_code) : ""}
      <span class="pg-name" title="${esc(label)}">${esc(label)}</span>
      ${exp ? `<span class="pg-exp${exp.warn ? " warn-line" : ""}" title="${esc(t("q.validUntil", { date: exp.text }))}">${esc(t("q.validUntilShort", { date: exp.text }))}</span>` : ""}
    </div>
    ${slotRowsHtml(p.items)}
  </div>`;
}

function claimedStripHtml(id) {
  const cs = claimedState[id];
  if (!cs) return "";
  const grants = (p) => (p.grant_items || [])
    .map((g) => `${g.name} ${fmtTokens(g.units)}`)
    .join(" · ");
  const active = (cs.plans || []).filter((p) => !p.status || p.status === "active");
  let html = active.map((p) => {
    const tr = claimTimeRange(p);
    return `
    <div class="claim-strip claimed" click="actions.openActivity('${id}')" title="${esc(t("act.openDetail"))}">
      ${ic("gift", 15)}
      <span class="claim-name">${esc(p.name || p.plan_id)}</span>
      ${grants(p) ? `<span class="claim-grants">${esc(grants(p))}</span>` : ""}
      <span class="claim-grants${tr.pending ? " warn-line" : ""}">${esc(tr.text)}</span>
    </div>`;
  }).join("");
  // 有「当前可领」的活动时不再展示旧活动的历史条，避免同一行两三个条混淆
  if (!active.length && claimablePlansFor(id).length) return html;
  if (!active.length && (cs.history || []).length) {
    const last = cs.history[cs.history.length - 1];
    const name = last?.planName || last?.planId || "";
    if (name) {
      html += `
      <div class="claim-strip history">
        ${ic("gift", 15)}
        <span class="claim-name">${esc(t("claim.historyStrip", { name }))}</span>
      </div>`;
    }
  }
  return html;
}

function acctQuotaSlot(id) {
  const strip = claimStripHtml(id);
  const q = acctQuota[id];
  let inner = "";
  if (q?.busy) {
    inner = `<span class="aq-loading">${t("q.loading")}</span>`;
  } else if (q?.err) {
    const msg = q.err.length > 46 ? q.err.slice(0, 46) + "…" : q.err;
    inner = `<span class="aq-err">${esc(msg)}</span>`;
  } else if (q?.data) {
    const plans = q.data.plans || [];
    if (plans.length >= 2) {
      inner = plans.map(planGroupHtml).join("");
    } else {
      const items = q.data.items || [];
      const wins = items.filter((it) => itemKind(it) === "prompt_count");
      if (wins.length) {
        inner = wins.map((it) => winRowHtml(it, " mini")).join("");
      } else {
        inner = slotRowsHtml(items);
      }
    }
    // 额度防线：服务器侧余额不含一次性赠送池，全部耗尽但赠送池生效中时提示，防误判白领
    const exhaustedAll = plans.length > 0 && plans.every((p) => (p.percent_used ?? 100) >= 99.5);
    // 「赠送池生效」以权益生效时刻为准：plan.starts_at 只是活动开始，effective_at 才代表额度可用；
    // 0/缺失视为立即生效（服务端约定）。
    const hasActiveBonus = (claimedState[id]?.plans || []).some((p) =>
      (!p.status || p.status === "active") && (p.effective_at == null || p.effective_at <= Date.now()));
    if (exhaustedAll && hasActiveBonus) {
      inner += `<div class="q-bonus-hint">${esc(t("q.bonusActive"))}</div>`;
    }
  }
  const claimed = claimedStripHtml(id);
  if (!strip && !claimed && !inner) return `<div class="row-quota-slot"></div>`;
  return `<div class="row-quota-slot">${strip}${claimed}${inner}</div>`;
}

function captureScroll() {
  const list = $app.querySelector(".list");
  if (!list || list.scrollTop === 0) return null;
  const listTop = list.getBoundingClientRect().top;
  for (const row of list.querySelectorAll(".row[data-id]")) {
    if (row.getBoundingClientRect().bottom > listTop) {
      return { id: row.dataset.id, offset: row.getBoundingClientRect().top - listTop, scrollTop: list.scrollTop };
    }
  }
  return null;
}
function restoreScroll(cap) {
  if (!cap) return;
  const list = $app.querySelector(".list");
  if (!list) return;
  const row = list.querySelector(`.row[data-id="${CSS.escape(cap.id)}"]`);
  if (row) {
    const delta = row.getBoundingClientRect().top - list.getBoundingClientRect().top;
    list.scrollTop = delta - cap.offset;
  } else {
    list.scrollTop = cap.scrollTop;
  }
}

function render() {
  try {
    renderInner();
    clearRenderError();
  } catch (e) {
    // 渲染异常绝不允许冻结整页：保留错误条 + 落盘，后续 ticker 重试
    showRenderError(e);
  }
}

function renderInner() {
  const scrollCap = captureScroll();
  if (!state) {
    $app.innerHTML = `<div class="loading">LOADING</div>`;
    return;
  }
  const s = state;
  const active = s.accounts.find((a) => a.is_active) || null;
  const unsaved = s.live_logged_in && !active;

  const cdTop = cooldownLeft(refreshClaim.cooldownUntil);
  const refreshTopOff = refreshClaim.running || claimAllRunning || claimActive || cdTop > 0;
  const refreshTopLabel = refreshClaim.running
    ? `${t("btn.refreshClaimRunning", { done: refreshClaim.done, total: refreshClaim.total })}${refreshClaim.current ? " · " + refreshClaim.current : ""}`
    : cdTop > 0 ? `${t("btn.refreshClaim")} (${cdTop}s)` : t("btn.refreshClaim");
  const refreshTopTitle = cdTop > 0 && !refreshClaim.running
    ? t("btn.refreshClaimCooldownTitle", { n: cdTop })
    : t("btn.refreshClaimTitle");
  const actRows = activityRows();
  const actCount = actRows.filter((r) => r.status === "claimable").length;
  const lastProbeText = probe.lastSummary?.at
    ? t("act.lastProbe", {
      time: new Date(probe.lastSummary.at).toLocaleTimeString(localeTag(), { hour12: false }),
      n: probe.lastSummary.hits,
    })
    : t("act.neverProbed");
  // 只要列表里有条目（可领 / 已领取 / 已过期），入口就不算「空」
  const actBtnTitle = `${actRows.length ? t("act.title") : t("m.activityNoneHint")} · ${lastProbeText}`;

  const dotCls = s.zcode_running ? "run" : s.live_logged_in ? "" : "off";
  const statusText = s.zcode_running
    ? t("m.status.running")
    : s.live_logged_in
      ? unsaved ? t("m.status.unsaved") : t("m.status.safe")
      : t("m.status.loggedOut");

  const rows = s.accounts.map((a) => {
    const isActive = a.is_active;
    if (renaming === a.id) {      return `
      <div class="row${isActive ? " active" : ""}" data-id="${a.id}">
        <span class="notch" style="background:${notchColor(a.id)}"></span>
        <div class="row-main">
          <input class="rename-input" value="${esc(a.name)}" maxlength="40"
            keydown="onRenameKey(event,'${a.id}')" blur="actions.deferCancelRename('${a.id}')">
          <div class="row-meta">${t("btn.renameMeta")}</div>
        </div>
        <div class="row-actions">
          <button class="btn-ghost" style="padding:4px 10px" click="actions.doRename('${a.id}')">${t("common.save")}</button>
          <button class="btn-ghost" style="padding:4px 10px" click="actions.cancelRename()">${t("common.cancel")}</button>
        </div>
      </div>`;
    }
    const ident = [a.identity?.username, a.identity?.email].filter(Boolean).join(" · ");
    const q = acctQuota[a.id];
    let meta = "";
    if (!a.has_config) meta += `<span class="no-cfg">${t("q.noCfg")}</span>`;
    const exp = expireInfo(q?.data?.plan_expire);
    if (exp) {
        meta += `<span class="${exp.warn ? "warn-line" : ""}">${esc(t("q.validUntil", { date: exp.text }))}</span>`;
    }
    if (ident) meta += `${meta ? " · " : ""}${esc(ident)}`;
    const rowCd = cooldownLeft(acctRefreshCooldown[a.id]);
    const rowOff = acctRefreshBusy[a.id] || claimAllRunning || refreshClaim.running || claimActive || autoClaimRunning || rowCd > 0;
    const rowTitle = rowCd > 0 && !acctRefreshBusy[a.id]
      ? t("m.refreshCooldownSec", { n: rowCd })
      : t("btn.acctRefreshTitle");
    return `
    <div class="row${isActive ? " active" : ""}" data-id="${a.id}">
      <div class="row-top">
        <span class="notch" style="background:${notchColor(a.id)}"></span>
        <div class="row-main">
          <div class="row-name">${esc(a.name)}${tierBadgeFor(a.id)}${isActive ? `<span class="tag-use">${t("btn.inUse")}</span>` : ""}${a.has_user_info === false ? `<span class="tag-relogin" title="${esc(t("btn.reloginTitle"))}">${t("btn.relogin")}</span>` : ""}${a.jwt_expired ? `<span class="tag-relogin" title="${esc(t("btn.jwtExpiredTitle"))}">${t("btn.jwtExpired")}</span>` : ""}</div>
          <div class="row-meta">${meta}</div>
        </div>
        <div class="row-actions">
          <button class="icon-btn${rowOff ? " is-off" : ""}" title="${esc(rowTitle)}" aria-label="${esc(rowTitle)}"
            click="actions.acctRefreshClaim('${a.id}')">${ic("refresh", 16)}${rowCd > 0 && !acctRefreshBusy[a.id] ? `<span class="ic-badge" data-cd-id="${a.id}">${rowCd}</span>` : ""}</button>
          <button class="icon-btn" title="${t("btn.quota")}" aria-label="${t("btn.quota")}" click="actions.acctQuota('${a.id}')">${ic("gauge", 16)}</button>
          <button class="icon-btn" title="${t("btn.rename")}" aria-label="${t("btn.rename")}" click="actions.rename('${a.id}')">${ic("pen", 16)}</button>
          <button class="icon-btn" title="${t("btn.export")}" aria-label="${t("btn.export")}" click="actions.exportOne('${a.id}')">${ic("export", 16)}</button>
          <button class="icon-btn danger" title="${t("btn.delete")}" aria-label="${t("btn.delete")}" click="actions.delete('${a.id}')">${ic("x", 16)}</button>
          <button class="btn-switch has-ic" click="actions.askSwitch('${a.id}')" ${isActive ? "disabled" : ""}>
            ${isActive ? t("btn.current") : ic("swap", 14) + " " + t("btn.switch")}
          </button>
        </div>
      </div>
      ${acctQuotaSlot(a.id)}
    </div>`;
  }).join("");

  const listHtml = s.accounts.length === 0
    ? `<div class="empty">
         <div class="glyph">${ic("empty", 34)}</div>
         ${t("m.emptyTitle")}<br>
         ${t("m.emptyBody")}
       </div>`
    : rows;

  const claimableCount = s.accounts.filter((a) => (claimable[a.id]?.plans || []).length > 0).length;

  $app.innerHTML = `
    <header class="topbar">
      <div class="wordmark">Z·SWITCH${appVer ? ` <span class="ver">v${esc(appVer)}</span>` : ""}</div>
      <div class="top-status${unsaved ? " unsaved" : ""}">
        <span class="status-dot ${dotCls}"></span>
        <span class="status-text">${esc(statusText)}</span>
      </div>
    </header>

    <section class="toolbar">
      <button class="btn-primary has-ic${unsaved ? " attention" : ""}" click="actions.capture()" ${!s.live_logged_in || active ? "disabled" : ""}
        title="${active ? esc(t("m.saveLoginDisabledTitle", { name: active.name })) : ""}">
        ${ic("capture", 16)} ${t("btn.saveLogin")}
      </button>
      ${claimableCount > 0
        ? `<button class="btn-ghost has-ic claim-all" click="actions.claimAll()" ${claimAllRunning || refreshClaim.running || autoClaimRunning ? "disabled" : ""}
            title="${t("btn.claimAllTitle")}">${ic("gift", 16)} ${t("btn.claimAll")}${claimableCount > 1 ? ` (${claimableCount})` : ""}</button>`
        : ""}
      ${(s.accounts.length > 0)
        ? `<button class="btn-ghost has-ic${refreshTopOff ? " is-off" : ""}" click="actions.refreshClaim()"
            title="${esc(refreshTopTitle)}">${ic("refresh", 16)} <span id="refresh-claim-label">${esc(refreshTopLabel)}</span></button>`
        : ""}
      <button class="btn-ghost has-ic${actCount ? "" : " is-off"}" click="actions.openActivity()"
        title="${esc(actBtnTitle)}">
        ${ic("gift", 16)} ${esc(t("btn.activity"))}${actCount ? ` (${actCount})` : ""}
      </button>
      <button class="tog-inline${s.auto_claim ? " on" : ""}${autoClaimRunning ? " running" : ""}"
        role="switch" aria-checked="${s.auto_claim}" aria-label="${t("btn.autoClaim")}"
        title="${autoPillTitle(s)}"
        click="actions.toggleAutoClaim()">
        <span class="toggle${s.auto_claim ? " on" : ""}" aria-hidden="true"><span class="knob"></span></span>
        ${t("btn.autoClaim")}
      </button>
      <button class="btn-ghost has-ic" click="actions.addAccount()" title="${t("btn.addAccountTitle")}">${ic("userPlus", 16)} ${t("btn.addAccount")}</button>
      ${s.zcode_running
        ? `<button class="btn-ghost has-ic" click="actions.askKill()" title="${t("btn.killZcode")}">${ic("power", 16)} ${t("btn.killZcode")}</button>`
        : `<button class="btn-ghost has-ic" click="actions.launch()" ${s.zcode_path_ok ? "" : "disabled"}>${ic("play", 14)} ${t("btn.launchZcode")}</button>`}
      <span class="tb-spacer"></span>
      <button class="btn-ghost tb-gear has-ic" click="actions.openSettings()" aria-label="${t("common.settings")}" title="${t("common.settings")}">${ic("sliders", 16)}</button>
    </section>

    <div class="section-head">
      <h2>${t("m.accounts")}</h2>
      <span class="count">${t("m.count", { count: s.accounts.length })}</span>
    </div>

    <main class="list">${listHtml}</main>
  `;
  restoreScroll(scrollCap);
}

window.actions = actions;
window.onRenameKey = (e, id) => {
  if (e.key === "Enter") actions.doRename(id);
  if (e.key === "Escape") actions.cancelRename();
};
installDelegation();
loadProbeState();
loadReminders();
loadMarks();
loadPlanMeta();
loadCatalog();

listen("tray-action", (ev) => {
  const p = ev.payload || {};
  if (p.action === "capture" && p.ok) toast(t("m.toastSaved", { name: p.result.name }));
  else if (!p.ok && p.error) toast(p.error, "err");
  refresh().then(() => { if (!uiLocked()) { render(); enrollAccounts(); } }).catch(() => {});
});

listen("claim://result", (ev) => {
  const p = ev.payload || {};
  if (claimWaiter && claimWaiter.accountId === p.accountId) claimWaiter.finish(p);
  if (p.ok === false) {
    let msg = p.message || t("m.unknownErr");
    if (p.code === 1005 && p.nextAt) {
      msg += t("m.claimNextAt", { time: new Date(p.nextAt).toLocaleString(localeTag(), { hour12: false }) });
    }
    toast(t("m.claimFailed", { name: p.accountName, msg }), "err");
  } else {
    const bits = [];
    const now = p.serverTime || Date.now();
    if (p.startsAt && p.startsAt > now) bits.push(t("m.claimStartsAt", { time: new Date(p.startsAt).toLocaleString(localeTag(), { hour12: false }) }));
    if (p.endsAt) bits.push(t("m.claimEndsAt", { time: new Date(p.endsAt).toLocaleString(localeTag(), { hour12: false }) }));
    toast(t("m.claimOk", { name: p.accountName, plan: p.planName }), "ok", bits.join(t("common.listSep")));
  }
  if (p.accountId) {
    loadAcctQuota(p.accountId, true);
    if (!autoClaimRunning) {
      loadClaimPreview(p.accountId).then(() => { if (!uiLocked()) render(); });
    }
    scheduleNext(p.accountId);
  }
});

listen("captcha://interactive", () => {
  if (!autoClaimRunning || !claimWaiter) return;
  const id = claimWaiter.accountId;
  cancelClaimIfMine(id).catch(() => {});
  autoClaimCooldown[id] = Date.now() + 60 * 60 * 1000;
  toast(t("m.autoClaimInteractive", { name: accountName(id) }), "warn", t("m.autoClaimInteractiveDetail"));
  claimWaiter.finish({ ok: false, code: "interactive" });
});

listen("oauth://done", (ev) => {
  const p = ev.payload || {};
  if (p.ok === false) {
    if (p.soft) {
      toast(t("m.oauthSoft", { err: p.error || t("m.unknownErr") }), "warn", t("m.oauthSoftDetail"));
      return;
    }
    toast(t("m.oauthFail", { err: p.error || t("m.unknownErr") }), "err");
    return;
  }
  if (p.duplicate) {
    toast(t("m.oauthDup", { name: p.name }), "warn", t("m.oauthDupDetail"));
    return;
  }
  toast(t("m.oauthOk", { name: p.name }), "ok", t("m.oauthOkDetail"));
  refresh().then(() => { if (!uiLocked()) { render(); enrollAccounts(); } }).catch(() => {});
});

listen("tray-activity", () => { actions.openActivity(); });

listen("state-changed", () => {
  refresh().then(() => { if (!uiLocked()) render(); }).catch(() => {});
});

const SWEEP_PERIOD = 5 * 60 * 1000;
const SWEEP_JITTER = 0.2;
const TICK_MS = 8000;
let quotaDue = {};
let ticking = false;

function scheduleNext(id, base = Date.now()) {
  const jitter = 1 + (Math.random() * 2 - 1) * SWEEP_JITTER;
  quotaDue[id] = base + Math.round(SWEEP_PERIOD * jitter);
}
function enrollAccounts() {
  const live = new Set((state?.accounts || []).map((a) => a.id));
  for (const id of live) if (!(id in quotaDue)) quotaDue[id] = Date.now();
  for (const id of Object.keys(quotaDue)) if (!live.has(id)) delete quotaDue[id];
}
function pokeAccount(id) { if (id) quotaDue[id] = Date.now(); }

async function sweepTick() {
  if (ticking) return;
  enrollAccounts();
  const now = Date.now();
  const due = (state?.accounts || []).find(
    (a) => (quotaDue[a.id] ?? Infinity) <= now && !acctQuota[a.id]?.busy && !claimable[a.id]?.busy,
  );
  if (!due) return;
  ticking = true;
  const dueAt = quotaDue[due.id];
  try {
    await loadAcctQuota(due.id);
    if (quotaDue[due.id] === dueAt) scheduleNext(due.id);
    if (!uiLocked()) render();
  } finally {
    ticking = false;
  }
}

(async () => {
  try {
    appVer = await invoke("app_version").catch(() => "");
    await refresh();
    render();
    await invoke("reveal_main");
    setTimeout(dismissSplash, 350);
    enrollAccounts();
    sweepTick();
    setInterval(() => {
      invoke("get_state").then((s) => { state = s; if (s?.language) init(s.language); enrollAccounts(); if (!uiLocked()) render(); }).catch(() => {});
    }, 5000);
    setInterval(sweepTick, TICK_MS);
    setTimeout(autoClaimTick, AUTO_CLAIM_FIRST_DELAY_MS);
    setInterval(autoClaimTick, AUTO_CLAIM_INTERVAL_MS);
    // 到点自动领取：每分钟检查「已到点且未领」的活动（无目标时不发任何请求）
    setTimeout(effectiveClaimTick, 30_000);
    setInterval(effectiveClaimTick, 60_000);
  } catch (e) {
    $app.innerHTML = `<div class="loading" style="color:var(--red)">${t("common.loadFail", { e: esc(stripErr(e)) })}</div>`;
    invoke("reveal_main").catch(() => {});
    dismissSplash();
  }
})();
