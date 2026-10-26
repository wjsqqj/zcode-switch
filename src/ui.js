import { invoke } from "@tauri-apps/api/core";
import { ic } from "./icons.js";
import { t, has, errCode, stripErr, localeTag } from "./i18n.js";

document.addEventListener("contextmenu", (e) => e.preventDefault());

document.addEventListener("keydown", (e) => {
  if (e.key === "F12") e.preventDefault();
  if (e.ctrlKey && e.shiftKey && ["I", "i", "J", "j", "C", "c"].includes(e.key)) e.preventDefault();
  if (e.ctrlKey && ["u", "s"].includes(e.key.toLowerCase())) e.preventDefault();
});

export function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

export function dismissSplash() {
  const el = document.getElementById("splash");
  if (!el) return;
  el.style.opacity = "0";
  setTimeout(() => el.remove(), 220);
}

export function toast(msg, kind = "ok", detail = "") {
  let zone = document.querySelector(".toast-zone");
  if (!zone) {
    zone = document.createElement("div");
    zone.className = "toast-zone";
    document.body.appendChild(zone);
  }
  const t = document.createElement("div");
  t.className = `toast ${kind}`;
  t.innerHTML = `<span class="t-ic">${ic(kind === "ok" ? "check" : "alert", 16)}</span>
    <span class="t-body">${esc(msg)}${detail ? `<span class="detail">${esc(detail)}</span>` : ""}</span>`;
  zone.appendChild(t);
  setTimeout(() => t.remove(), detail ? 5200 : 3200);
}

function runAttr(expr, event) {
  try {
    const open = expr.indexOf("(");
    if (open < 0 || !expr.trimEnd().endsWith(")")) return;
    let fn = window;
    for (const seg of expr.slice(0, open).trim().split(".")) fn = fn?.[seg];
    if (typeof fn !== "function") return;
    const src = expr.slice(open + 1, expr.trimEnd().length - 1).trim();
    const args = src
      ? src.split(/\s*,\s*/).map((a) => {
          if (a === "event") return event;
          const m = a.match(/^'([^']*)'$/);
          if (m) return m[1];
          return JSON.parse(a);
        })
      : [];
    fn(...args);
  } catch (e) {
    console.warn("attr handler error:", expr, e);
  }
}

export function installDelegation() {
  document.addEventListener("click", (e) => {
    const el = e.target.closest("[click]");
    if (!el) return;
    e.preventDefault();
    runAttr(el.getAttribute("click") || "", e);
  });
  document.addEventListener("keydown", (e) => {
    const el = e.target.closest("[keydown]");
    if (!el) return;
    runAttr(el.getAttribute("keydown") || "", e);
  });
  document.addEventListener("blur", (e) => {
    const el = e.target.closest("[blur]");
    if (!el) return;
    runAttr(el.getAttribute("blur") || "", e);
  }, true);
}

export function openConfirmModal(m) {
  document.querySelector(".cm-mask")?.remove();
  const kind = m.kind || "plain";
  const mask = document.createElement("div");
  mask.className = "cm-mask";
  mask.innerHTML = `
    <div class="cm-panel" role="alertdialog" aria-modal="true" aria-label="${esc(m.title)}">
      <div class="cm-strip ${kind}"></div>
      <div class="cm-head">
        <span class="cm-ic ${kind}">${ic(m.icon || "alert", 21)}</span>
        <div class="cm-title">${esc(m.title)}</div>
      </div>
      ${m.desc ? `<div class="cm-desc">${m.desc}</div>` : ""}
      <div class="cm-actions">
        <button class="btn-ghost cm-no">${esc(m.noLabel || t("common.cancel"))}</button>
        <button class="cm-yes ${kind}">${esc(m.yesLabel || t("common.confirm"))}</button>
      </div>
    </div>`;
  document.body.appendChild(mask);

  const panel = mask.querySelector(".cm-panel");
  const yes = mask.querySelector(".cm-yes");
  const no = mask.querySelector(".cm-no");
  const close = () => {
    document.removeEventListener("keydown", mask._key);
    mask.remove();
  };
  panel.addEventListener("click", (e) => e.stopPropagation());
  mask.addEventListener("click", close);
  no.addEventListener("click", close);
  yes.addEventListener("click", async () => {
    yes.disabled = true; no.disabled = true;
    try { await m.onYes?.(); } finally { close(); }
  });
  mask._key = (e) => {
    if (e.key === "Escape") close();
    if (e.key === "Enter" && e.target === document.body && !yes.disabled) yes.click();
  };
  document.addEventListener("keydown", mask._key);
  (m.focusNo || kind === "danger" ? no : yes).focus();
}

export function openPwModal(m) {
  document.querySelector(".pw-mask")?.remove();
  const isExport = m.mode === "export" || m.mode === "exportAll";
  const title = m.mode === "export" ? t("pw.exportTitle", { name: esc(m.name) })
    : m.mode === "exportAll" ? t("pw.exportAllTitle", { count: m.count })
    : t("pw.importTitle", { count: m.files.length });
  const sub = isExport
    ? t("pw.exportSub", { path: esc(m.path) })
    : t("pw.importSub", { files: m.files.map(([n]) => esc(n)).join(t("common.listSep")) });

  const mask = document.createElement("div");
  mask.className = "pw-mask";
  mask.innerHTML = `
    <div class="pw-panel">
      <div class="pw-title">${title}</div>
      <div class="pw-sub">${sub}</div>
      <input class="pw-input" type="password" id="pw1" placeholder="${isExport ? t("pw.setPw") : t("pw.pw")}" autocomplete="off">
      ${isExport ? `<input class="pw-input" type="password" id="pw2" placeholder="${t("pw.pwAgain")}" autocomplete="off">` : ""}
      <div class="pw-err"></div>
      <div class="pw-actions">
        <button class="btn-ghost pw-cancel">${t("common.cancel")}</button>
        <button class="btn-primary pw-go has-ic">${isExport ? ic("lock", 14) + " " + t("pw.encryptExport") : ic("lockOpen", 14) + " " + t("pw.decryptImport")}</button>
      </div>
    </div>`;
  document.body.appendChild(mask);

  const panel = mask.querySelector(".pw-panel");
  const errEl = mask.querySelector(".pw-err");
  const go = mask.querySelector(".pw-go");
  const input1 = mask.querySelector("#pw1");
  const input2 = mask.querySelector("#pw2");
  panel.addEventListener("click", (e) => e.stopPropagation());
  mask.addEventListener("click", () => mask.remove());
  mask.querySelectorAll(".pw-cancel").forEach((b) => b.addEventListener("click", () => mask.remove()));
  const key = (e) => {
    if (e.key === "Escape") mask.remove();
    if (e.key === "Enter") confirm();
  };
  [input1, input2].forEach((i) => i?.addEventListener("keydown", key));
  input1.focus();

  async function confirm() {
    const pw1 = input1?.value || "";
    const pw2 = input2?.value;
    if (pw1.length < 6) return (errEl.textContent = t("pw.errMinLen"));
    if (pw2 !== undefined && pw2 !== null && pw1 !== pw2) return (errEl.textContent = t("pw.errMismatch"));
    go.disabled = true;
    try {
      if (m.mode === "export") {
        const r = await invoke("export_finalize", { path: m.path, id: m.id, password: pw1 });
        toast(t("pw.exportedToast", { path: r.path }), "ok", t("pw.exportedToastDetail"));
        m.onDone?.(r);
      } else if (m.mode === "exportAll") {
        const r = await invoke("export_all_finalize", { path: m.path, password: pw1 });
        toast(t("pw.exportedAllToast", { count: r.count }), "ok", r.path);
        m.onDone?.(r);
      } else {
        let report = await invoke("import_sealed", { files: m.files, password: pw1 });
        if (m.preErrors?.length) {
          report.errors = [...m.preErrors, ...(report.errors || [])];
        }
        const wrongPw = (report.errors || []).some((e) => errCode(e) === "wrong_password" || String(e).includes("密码错误"));
        if (wrongPw) { errEl.textContent = t("pw.errWrong"); go.disabled = false; return; }
        m.onDone?.(report);
      }
      mask.remove();
    } catch (e) {
      errEl.textContent = stripErr(e);
      go.disabled = false;
    }
  }
  go.addEventListener("click", confirm);
}

/// 右下角活动弹窗（与全局居中 toast 区分）：可点击直达详情，8s 自动消失。
export function activityToast(text, onClick) {
  let zone = document.querySelector(".act-toast-zone");
  if (!zone) {
    zone = document.createElement("div");
    zone.className = "act-toast-zone";
    document.body.appendChild(zone);
  }
  const el = document.createElement("button");
  el.className = "act-toast";
  el.type = "button";
  el.textContent = text;
  const close = () => el.remove();
  el.addEventListener("click", () => { close(); onClick?.(); });
  zone.appendChild(el);
  setTimeout(close, 8000);
}

/// 活动汇总弹框：分组（可领 / 已领取 / 已过期）+ 可展开详情。
/// m: { rows, focusAccount }
export function openActivityModal(m) {
  document.querySelector(".act-mask")?.remove();
  const rows = m?.rows || [];
  const groups = [
    { k: "claimable", label: t("act.groupClaimable") },
    { k: "claimed", label: t("act.groupClaimed") },
    { k: "expired", label: t("act.groupExpired") },
  ];
  const fmt = (ms) => (ms ? new Date(ms).toLocaleString(localeTag(), { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }) : t("act.notProvided"));
  const timeText = (r) => {
    const startMs = r.effectiveAt ?? r.startsAt ?? null;
    const pending = !!startMs && startMs > Date.now();
    return t(pending ? "act.pendingRange" : "act.timeRange", { start: fmt(startMs), end: fmt(r.endsAt) });
  };
  const itemHtml = (r) => `
    <details class="act-item ${r.status}"${m?.focusAccount && r.accountId === m.focusAccount ? " open" : ""}>
      <summary>
        <span class="act-name">${esc(r.title || r.planId)}</span>
        <span class="act-acct">${esc(r.accountName)}</span>
        <span class="act-time${r.status === "claimable" ? " warn-line" : ""}">${esc(timeText(r))}</span>
      </summary>
      <div class="act-body">
        <div class="act-kv"><b>${t("act.campaign")}</b><span>${esc(r.campaignId || t("act.notProvided"))}</span></div>
        <div class="act-kv"><b>${t("act.plan")}</b><span>${esc(r.planId)}</span></div>
        <div class="act-kv"><b>${t("act.position")}</b><span>${esc(r.position ? (has(`act.pos.${r.position}`) ? t(`act.pos.${r.position}`) : r.position) : t("act.notProvided"))}</span></div>
        <div class="act-kv"><b>${t("act.rewards")}</b><span>${esc(r.grants?.length ? r.grants.join(" · ") : t("act.notProvided"))}</span></div>
        ${r.body ? `<div class="act-copy">${esc(r.body)}</div>` : `<div class="act-copy dim">${t("act.noMeta")}</div>`}
        ${r.buttons?.length ? `<div class="act-kv"><b>${t("act.actions")}</b><span>${esc(r.buttons.map((b) => `${b.kind}${b.planId ? ":" + b.planId : ""}`).join(" · "))}</span></div>` : ""}
        ${r.status === "claimable" ? `<div class="act-claim-row"><button class="btn-claim has-ic" click="actions.claim('${r.accountId}')">${ic("gift", 13)} ${t("btn.claim")}</button></div>` : ""}
      </div>
    </details>`;

  const body = groups
    .map((g) => {
      const list = rows.filter((r) => r.status === g.k);
      if (!list.length) return "";
      return `<div class="act-group"><div class="act-group-head">${esc(g.label)} (${list.length})</div>${list.map(itemHtml).join("")}</div>`;
    })
    .join("") || `<div class="act-empty">${t("act.empty")}</div>`;

  const mask = document.createElement("div");
  mask.className = "act-mask";
  mask.innerHTML = `
    <div class="act-panel">
      <div class="act-title">${t("act.title")}</div>
      <div class="act-list">${body}</div>
      <div class="act-actions-row"><button class="btn-ghost act-close">${t("common.close")}</button></div>
    </div>`;
  document.body.appendChild(mask);
  const close = () => { mask.remove(); document.removeEventListener("keydown", onKey); };
  const onKey = (e) => { if (e.key === "Escape") close(); };
  // 只在点击遮罩本身时关闭；**不能**在面板上 stopPropagation，
  // 否则面板内 click="actions.xxx()" 的按钮无法冒泡到 document 级事件委托（会点了没反应）。
  mask.addEventListener("click", (e) => {
    if (e.target === mask) close();
  });
  mask.querySelector(".act-close").addEventListener("click", close);
  document.addEventListener("keydown", onKey);
}

export function openProviderModal(m) {
  document.querySelector(".pv-mask")?.remove();
  const mask = document.createElement("div");
  mask.className = "pv-mask";
  const displayOf = (p) => has(`prov.${p.id}`) ? t(`prov.${p.id}`) : p.display;
  mask.innerHTML = `
    <div class="pv-panel">
      <div class="pv-title">${t("prov.title")}</div>
      <div class="pv-sub">${t("prov.sub")}</div>
      <div class="pv-list">
        ${(m.providers || []).map((p) => `
          <button class="pv-item" data-id="${esc(p.id)}">
            <span class="pv-name">${esc(displayOf(p))}</span>
            <span class="pv-arrow">→</span>
          </button>`).join("")}
      </div>
      <div class="pv-actions"><button class="btn-ghost pv-cancel">${t("common.cancel")}</button></div>
    </div>`;
  document.body.appendChild(mask);
  const onKey = (e) => { if (e.key === "Escape") close(); };
  const close = () => { mask.remove(); document.removeEventListener("keydown", onKey); };
  mask.querySelectorAll(".pv-item").forEach((b) => {
    b.addEventListener("click", () => { close(); m.onPick?.(b.dataset.id); });
  });
  mask.querySelector(".pv-cancel").addEventListener("click", close);
  document.addEventListener("keydown", onKey);
  const first = mask.querySelector(".pv-item");
  if (first) first.focus();
}
