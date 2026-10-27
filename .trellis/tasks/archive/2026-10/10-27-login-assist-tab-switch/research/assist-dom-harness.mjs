// 注入脚本的行为验证：极简假 DOM + 虚拟时钟，覆盖 prd 的 AC1–AC6。
// 用法：node .trellis/tasks/10-27-login-assist-tab-switch/research/assist-dom-harness.mjs src-tauri/src/oauth.rs
import { readFileSync } from "node:fs";

/* ---------------- 假 DOM ---------------- */
const parseSel = (sel) =>
  sel.split(",").map((raw) => {
    const part = raw.trim();
    const tag = part.match(/^[a-z]+/)?.[0] || "";
    const attrs = [...part.matchAll(/\[([a-zA-Z-]+)(?:([~*^$]?=)"?([^\]"]*)"?)?\]/g)].map((m) => ({
      name: m[1],
      op: m[2] || "exists",
      val: m[3],
    }));
    return { tag, attrs };
  });

class El {
  constructor(tag, attrs = {}, text = "") {
    this.tagName = String(tag).toUpperCase();
    this.attrs = { ...attrs };
    this.ownText = text;
    this.children = [];
    this.parentElement = null;
    this.style = { display: "", visibility: "", opacity: "" };
    this.hiddenByCss = false; // 模拟隐藏面板：自身 display:none
    this.handlers = {};
    this.clicks = 0;
  }
  get id() { return this.attrs.id; }
  set id(v) { this.attrs.id = String(v); }
  get innerText() {
    return [this.ownText, ...this.descendants().map((d) => d.ownText)].filter(Boolean).join(" ");
  }
  get textContent() { return this.innerText; }
  set textContent(v) { this.ownText = String(v); }
  get styleDisplay() { return this.hiddenByCss ? "none" : this.style.display || "block"; }
  appendChild(el) { el.parentElement = this; this.children.push(el); return el; }
  descendants() {
    const out = [];
    const walk = (n) => n.children.forEach((c) => { out.push(c); walk(c); });
    walk(this);
    return out;
  }
  all() { return [this, ...this.descendants()]; }
  get previousElementSibling() {
    const sibs = this.parentElement ? this.parentElement.children : [];
    const i = sibs.indexOf(this);
    return i > 0 ? sibs[i - 1] : null;
  }
  getAttribute(n) { return this.attrs[n] ?? null; }
  setAttribute(n, v) {
    this.attrs[n] = String(v);
    if (n === "style") {
      String(v).split(";").forEach((d) => {
        const i = d.indexOf(":");
        if (i < 0) return;
        const key = d.slice(0, i).trim().replace(/-([a-z])/g, (_, c) => c.toUpperCase());
        if (key in this.style) this.style[key] = d.slice(i + 1).trim();
      });
    }
  }
  contains(el) { return this.all().includes(el); }
  matchesSpec(spec) {
    if (spec.tag && this.tagName !== spec.tag.toUpperCase()) return false;
    return spec.attrs.every((a) => {
      const v = this.getAttribute(a.name);
      if (a.op === "exists") return v !== null;
      if (v === null) return false;
      if (a.op === "=") return v === a.val;
      if (a.op === "*=") return v.includes(a.val);
      return false;
    });
  }
  querySelectorAll(sel) {
    const specs = parseSel(sel);
    return this.descendants().filter((el) => specs.some((s) => el.matchesSpec(s)));
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }
  click() {
    this.clicks++;
    (this.handlers.click || []).forEach((fn) => fn({ preventDefault() {}, stopPropagation() {} }));
  }
  getBoundingClientRect() {
    const shown = this.styleDisplay !== "none" && this.style.visibility !== "hidden";
    return { width: shown ? 80 : 0, height: shown ? 24 : 0 };
  }
}

/* ---------------- 假 window / 虚拟时钟 / 存储 ---------------- */
let clock = 0;
let timers = [];
let navs = [];
// 同一个标签页：window.name 跨文档保留，sessionStorage 按 origin 隔离（这正是本次 bug 的关键）
const makeTab = () => ({ name: "", stores: {} });
const storeFor = (tab, host) => (tab.stores[host] ||= new Map());
const makeWindow = (tab, hostname, pathname, hash = "", replaces = []) => {
  timers = [];
  navs = [];
  clock = 0;
  const store = storeFor(tab, hostname);
  const loc = {
    hostname,
    pathname,
    hash,
    _h: `https://${hostname}${pathname}${hash}`,
    get href() { return this._h; },
    set href(v) { navs.push(v); this._h = v; },
  };
  const win = {
    setInterval: (fn, ms) => { const t = { fn, ms, next: clock + ms, id: timers.length }; timers.push(t); return t.id; },
    setTimeout: (fn, ms) => { const t = { fn, ms, next: clock + ms, once: true, id: timers.length }; timers.push(t); return t.id; },
    clearInterval: (id) => { const t = timers.find((x) => x.id === id); if (t) t.dead = true; },
    clearTimeout: (id) => win.clearInterval(id),
    getComputedStyle: (el) => ({ display: el.styleDisplay, visibility: el.style.visibility || "visible", opacity: el.style.opacity || "1" }),
    location: loc,
    history: {
      replaceState(m, u, url) {
        replaces.push(url);
        loc.hash = (String(url).split("#")[1] || "") ? "#" + String(url).split("#")[1] : "";
        loc._h = url;
      },
    },
    sessionStorage: {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
      removeItem: (k) => store.delete(k),
      _store: store,
    },
  };
  Object.defineProperty(win, "name", { get: () => tab.name, set: (v) => { tab.name = String(v); } });
  win.window = win;
  return win;
};

// 推进虚拟时间：把所有到期回调按序跑掉
const advance = (msTotal) => {
  const end = clock + msTotal;
  while (clock < end) {
    const due = timers.filter((t) => !t.dead && t.next <= end).sort((a, b) => a.next - b.next)[0];
    if (!due) break;
    clock = due.next;
    due.next = clock + due.ms;
    if (due.once) due.dead = true;
    due.fn();
  }
  clock = end;
};

/* ---------------- 页面 fixture ---------------- */
const mkForm = (kind, shown) => {
  const wrap = new El("div");
  wrap.hiddenByCss = !shown;
  wrap.appendChild(new El("input", kind === "email"
    ? { type: "email", placeholder: "请输入邮箱" }
    : { type: "tel", placeholder: "请输入手机号" }));
  wrap.appendChild(new El("input", { type: "password", placeholder: "请输入密码" }));
  return wrap;
};

// 模拟真实登录页：邮箱 / 手机号表单同页，tab 点击互斥切换显隐
const buildLoginPage = ({ emailShown, phoneShown }) => {
  const body = new El("body");
  const quick = new El("div");
  quick.appendChild(new El("button", { class: "ButtonContinueWithZai" }, ""));
  quick.appendChild(new El("button", { class: "ButtonContinueWithGoogle" }, ""));
  quick.appendChild(new El("button", { class: "ButtonContinueWithGithub" }, ""));
  body.appendChild(quick);
  const emailForm = mkForm("email", emailShown);
  const phoneForm = mkForm("phone", phoneShown);
  body.appendChild(emailForm);
  body.appendChild(phoneForm);
  const toPhone = new El("button", { class: "tab" }, "手机号登录");
  const toEmail = new El("button", { class: "tab" }, "邮箱登录");
  toPhone.addEventListener("click", () => { phoneForm.hiddenByCss = false; emailForm.hiddenByCss = true; });
  toEmail.addEventListener("click", () => { emailForm.hiddenByCss = false; phoneForm.hiddenByCss = true; });
  body.appendChild(toPhone);
  body.appendChild(toEmail);
  body.appendChild(new El("a", { href: "#" }, "注册"));
  body.appendChild(new El("a", { href: "#" }, "登录"));
  body.appendChild(new El("a", { href: "#" }, "忘记密码？"));
  return body;
};

/* ---------------- 取出注入脚本 ---------------- */
const marker = 'const ZAI_EMAIL_ASSIST_JS: &str = r##"';
const src = readFileSync(process.argv[2] || "src-tauri/src/oauth.rs", "utf8");
let js = src.slice(src.indexOf(marker) + marker.length);
js = js.slice(0, js.indexOf('"##;'));
const fills = {
  __ZSW_PHONE_ENTRY__: "https://chat.z.ai/auth?phone-entry",
  __ZSW_ENTRY__: "https://chat.z.ai/auth?official-entry",
  __ZSW_LABEL_EMAIL_LOGIN__: "邮箱登录",
  __ZSW_LABEL_PHONE_LOGIN__: "手机号登录",
  __ZSW_LABEL_GOOGLE_LOGIN__: "Google 登录",
  __ZSW_LABEL_GITHUB_LOGIN__: "GitHub 登录",
  __ZSW_LABEL_EMAIL_SIGNUP__: "邮箱注册",
  __ZSW_LABEL_ALREADY__: "已在邮箱登录页",
  __ZSW_LABEL_ALREADY_SIGNUP__: "已在邮箱注册页",
  __ZSW_LABEL_ALREADY_PHONE__: "已在手机号登录页",
};
for (const [k, v] of Object.entries(fills)) js = js.split(k).join(JSON.stringify(v));

const inject = (env) => {
  const { doc, win } = env;
  const fn = new Function(
    "document", "window", "location", "sessionStorage", "setTimeout", "setInterval", "clearTimeout", "clearInterval", "history", "navigator",
    js,
  );
  fn(doc, win, win.location, win.sessionStorage, win.setTimeout, win.setInterval, win.clearTimeout, win.clearInterval, win.history, { userAgent: "" });
};

const setup = (hostname, pathname, body, opts = {}) => {
  const replaces = opts.replaces || [];
  const tab = opts.tab || makeTab();
  const win = makeWindow(tab, hostname, pathname, opts.hash || "", replaces);
  const doc = {
    body,
    createElement: (tag) => new El(tag),
    getElementById: (id) => body.all().find((el) => el.attrs.id === id) || null,
    querySelectorAll: (sel) => body.querySelectorAll(sel),
    querySelector: (sel) => body.querySelector(sel),
  };
  const env = { win, doc, tab, replaces };
  inject(env);
  advance(200);
  return env;
};

const barButton = (doc, label) =>
  doc.body.querySelectorAll("button").find((b) => b.attrs.style && b.ownText === label);
const hintText = (doc) => {
  const bar = doc.getElementById("zsw-email-assist");
  const t = bar && bar.querySelector("[data-hint]");
  return { text: t ? t.textContent : "", shown: !!t && t.style.opacity === "1" };
};

let failed = 0;
const check = (name, cond, extra = "") => {
  console.log(`${cond ? "  ✓" : "  ✗"} ${name}${cond ? "" : "   ← " + extra}`);
  if (!cond) failed++;
};
const findTab = (doc, label) => doc.body.querySelectorAll("button").find((b) => !b.attrs.style && b.ownText === label);

console.log("AC1 邮箱表单可见时点「手机号登录」→ 页内切换，不整页跳转、不误报");
{
  const { doc } = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  barButton(doc, "手机号登录").click();
  check("点了页内手机号入口", findTab(doc, "手机号登录").clicks === 1, `clicks=${findTab(doc, "手机号登录").clicks}`);
  check("手机号表单已可见", !!doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel" && !i.parentElement.hiddenByCss));
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
  check("没有误报提示", !hintText(doc).shown, JSON.stringify(hintText(doc)));
}

console.log("AC2 手机号表单可见时点「邮箱登录」→ 页内切回邮箱，不误报已在邮箱页");
{
  const { doc } = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: false, phoneShown: true }));
  barButton(doc, "邮箱登录").click();
  check("点了页内邮箱入口", findTab(doc, "邮箱登录").clicks === 1, `clicks=${findTab(doc, "邮箱登录").clicks}`);
  check("邮箱表单已可见", !!doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "email" && !i.parentElement.hiddenByCss));
  check("没有误报「已在邮箱登录页」", !hintText(doc).shown, JSON.stringify(hintText(doc)));
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
}

console.log("AC3 邮箱表单已可见时点「邮箱登录」→ 提示已在邮箱登录页");
{
  const { doc } = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  barButton(doc, "邮箱登录").click();
  const h = hintText(doc);
  check("提示已显示", h.shown && h.text === "已在邮箱登录页", JSON.stringify(h));
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
}

console.log("AC5 手机号表单已可见时点「手机号登录」→ 提示已在手机号登录页");
{
  const { doc } = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: false, phoneShown: true }));
  barButton(doc, "手机号登录").click();
  const h = hintText(doc);
  check("提示已显示", h.shown && h.text === "已在手机号登录页", JSON.stringify(h));
}

console.log("AC4 停在 github.com 时点邮箱 / 手机号 → 不拿对方文案判断，跳转带 fragment 意图");
{
  const gh = new El("body");
  gh.appendChild(new El("a", { href: "#" }, "Forgot password?"));
  gh.appendChild(new El("input", { type: "text", autocomplete: "email", placeholder: "Enter your email or username" }));
  const { doc, win, tab } = setup("github.com", "/login", gh);
  barButton(doc, "邮箱登录").click();
  check("跳到官网邮箱入口并带标记", navs[0] === "https://chat.z.ai/auth?official-entry#zsw-login", JSON.stringify(navs));
  check("github 自域没写意图（写了也没用）", win.sessionStorage.getItem("zsw_email_assist_intent") === null);
  check("没误报「已在邮箱登录页」", !hintText(doc).shown, JSON.stringify(hintText(doc)));
  check("意图写进了 window.name", tab.name === "zsw-login", tab.name);
  navs.length = 0;
  barButton(doc, "手机号登录").click();
  check("跳到手机号入口并带标记", navs[0] === "https://chat.z.ai/auth?phone-entry#zsw-phone", JSON.stringify(navs));
  check("window.name 改成手机号意图", tab.name === "zsw-phone", tab.name);
}

console.log("AC4d 从 github.com 跳回登录页（跨域，新 origin 没意图）→ 靠 fragment 自动切到手机号表单");
{
  const tab = makeTab();
  const gh = new El("body");
  gh.appendChild(new El("a", { href: "#" }, "Forgot password?"));
  const first = setup("github.com", "/login", gh, { tab });
  barButton(first.doc, "手机号登录").click();
  const url = navs[navs.length - 1];
  check("跳转 URL 带 #zsw-phone", /#zsw-phone$/.test(url), url);
  // 落到 chat.z.ai：该 origin 的 sessionStorage 是空的（跳域前在 github.com 写的值读不到）
  check("跳域前 z.ai 侧 storage 确实为空", storeFor(tab, "chat.z.ai").size === 0, [...storeFor(tab, "chat.z.ai").keys()].join(","));
  const land = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }), {
    tab, hash: "#zsw-phone", replaces: [],
  });
  check("落地从 fragment 读回意图并写进同域 storage", land.win.sessionStorage.getItem("zsw_phone_intent") === "1", String(land.win.sessionStorage.getItem("zsw_phone_intent")));
  advance(18000);
  const tel = land.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel");
  check("自动切到手机号表单", !tel.parentElement.hiddenByCss);
  check("意图已写回同域并被消费", land.win.sessionStorage.getItem("zsw_phone_intent") === null);
  check("地址栏标记已抹掉", land.replaces.length === 1 && land.replaces[0].indexOf("#") < 0, JSON.stringify(land.replaces));
  check("window.name 已清空", tab.name === "", tab.name);
}

console.log("AC4e fragment 被 SPA 提前抹掉时，window.name 仍能带意图");
{
  const tab = makeTab();
  tab.name = "zsw-phone";
  const env = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }), { tab, hash: "" });
  advance(18000);
  const tel = env.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel");
  check("仍自动切到手机号表单", !tel.parentElement.hiddenByCss);
  check("window.name 已清空", tab.name === "", tab.name);
}

console.log("AC4f 没有意图标记时不乱点（默认留在邮箱表单）");
{
  const env = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  advance(18000);
  const tel = env.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel");
  const phoneTab = findTab(env.doc, "手机号登录");
  check("没碰手机号入口", phoneTab.clicks === 0, `clicks=${phoneTab.clicks}`);
  check("手机号表单仍隐藏", tel.parentElement.hiddenByCss === true);
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
}

console.log("AC4b 同域带手机号意图落到「邮箱表单默认」的登录页 → 自动补点切到手机号表单");
{
  const env = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  env.win.sessionStorage.setItem("zsw_phone_intent", "1");
  inject(env);
  advance(18000);
  const tel = env.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel");
  check("自动切到手机号表单", !tel.parentElement.hiddenByCss);
  check("手机号入口被点过", findTab(env.doc, "手机号登录").clicks >= 1);
  check("意图已清除", env.win.sessionStorage.getItem("zsw_phone_intent") === null);
}

console.log("AC4c 同域带邮箱登录意图落到「手机号表单默认」的旧页面 → 自动补点切到邮箱表单");
{
  const env = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: false, phoneShown: true }));
  env.win.sessionStorage.setItem("zsw_email_assist_intent", "login");
  inject(env);
  advance(18000);
  const mail = env.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "email");
  check("自动切到邮箱表单", !mail.parentElement.hiddenByCss);
  check("意图已清除", env.win.sessionStorage.getItem("zsw_email_assist_intent") === null);
}

console.log("AC6 Google / GitHub 点页面自身入口；重复注入不叠加轮询");
{
  const env = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  barButton(env.doc, "GitHub 登录").click();
  const ghBtn = env.doc.body.querySelectorAll("button").find((b) => (b.attrs.class || "").includes("Github"));
  check("点了页面 GitHub 按钮", ghBtn.clicks === 1, `clicks=${ghBtn.clicks}`);
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
  const before = timers.filter((t) => !t.dead).length;
  inject(env);
  inject(env);
  advance(1000);
  check("重复注入未新增活动定时器", timers.filter((t) => !t.dead).length === before, `before=${before} after=${timers.filter((t) => !t.dead).length}`);
  check("按钮条仍只有一个", env.doc.body.querySelectorAll("div").filter((d) => d.attrs.id === "zsw-email-assist").length === 1);
}

console.log("AC6b 授权确认页自动点「授权」，且只点一次");
{
  const body = new El("body");
  const ok = new El("button", {}, "授权");
  const cancel = new El("button", {}, "取消");
  body.appendChild(ok);
  body.appendChild(cancel);
  const env = setup("chat.z.ai", "/oauth/authorize", body);
  inject(env);
  inject(env);
  advance(6000);
  check("点了授权", ok.clicks === 1, `clicks=${ok.clicks}`);
  check("没点取消", cancel.clicks === 0);
}

console.log("AC1b 旧版页面（手机号只有 Z.ai 图标，无文字入口）→ 回退点图标，仍能切手机号");
{
  const body = buildLoginPage({ emailShown: true, phoneShown: false });
  // 去掉所有带文字的切换入口，只留快捷登录图标
  body.children = body.children.filter((el) => !(el.tagName === "BUTTON" && el.ownText));
  const forms = body.children.filter((c) => c.tagName === "DIV" && c.querySelectorAll("input").length);
  const emailForm = forms[0];
  const phoneForm = forms[1];
  const zai = body.children[0].children.find((b) => (b.attrs.class || "").includes("Zai"));
  zai.addEventListener("click", () => { phoneForm.hiddenByCss = false; emailForm.hiddenByCss = true; });
  const env = setup("chat.z.ai", "/auth", body);
  barButton(env.doc, "手机号登录").click();
  const tel = env.doc.body.querySelectorAll("input").find((i) => i.getAttribute("type") === "tel");
  check("点了 Z.ai 图标入口", zai.clicks === 1, `clicks=${zai.clicks}`);
  check("手机号表单已可见", !tel.parentElement.hiddenByCss);
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
}

console.log("AC6c 「邮箱注册」→ 先点登录页的注册入口");
{
  const { doc } = setup("chat.z.ai", "/auth", buildLoginPage({ emailShown: true, phoneShown: false }));
  barButton(doc, "邮箱注册").click();
  const link = doc.body.querySelectorAll("a").find((a) => a.ownText === "注册");
  check("点了注册入口", link.clicks === 1, `clicks=${link.clicks}`);
  check("没有整页跳转", navs.length === 0, JSON.stringify(navs));
}

console.log(failed ? `\n✗ ${failed} 项不通过` : "\n✓ 全部通过");
process.exit(failed ? 1 : 0);
