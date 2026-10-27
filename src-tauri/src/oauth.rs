
use crate::quota;
use serde_json::{json, Value};
use std::time::Duration;

const TOKEN_URL: &str = "https://zcode.z.ai/api/v1/oauth/token";
const FLOW_INIT_URL: &str = "https://zcode.z.ai/api/v1/oauth/cli/init";
pub const FLOW_TIMEOUT_MS: u64 = 300_000;
pub const LOGIN_WINDOW_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct OAuthProvider {
    pub id: &'static str,
    pub display: &'static str,
}

pub const OAUTH_PROVIDERS: &[OAuthProvider] = &[
    OAuthProvider { id: "bigmodel", display: "BigModel（智谱开放平台）" },
    OAuthProvider { id: "zai", display: "z.ai（国际站）" },
];

const REDIRECT_ENC: &str = "zcode%3A%2F%2Foauth%2Fcallback";

/// z.ai 官网登录入口（官网 OAuth client：邮箱 / 第三方登录方式齐全）。
/// zcode 的 API 授权 client 只放手机号登录，所以 zai 入口先用这个页面建立 chat.z.ai 会话，
/// 登录成功后（回调 z.ai/login/callback）再接力到本次 flow 的 authorize_url 取码。
pub const ZAI_LOGIN_ENTRY_URL: &str = "https://chat.z.ai/auth?response_type=code&client_id=client_lS94_Ka2ycE9IwCNYisudg&redirect_uri=https%3A%2F%2Fz.ai%2Flogin%2Fcallback%3Fredirect%3D%2525252Fmodel-api&state=1790259680634";

/// z.ai 登录窗口内的辅助按钮条：手机号 / Google / GitHub / 邮箱登录 / 邮箱注册。
/// 登录页把「邮箱表单」和「手机号表单」放在同一个页面上（手机号表单默认隐藏），所以按钮一律
/// 先在页内找切换入口点一下；只有当前页确实没有可切入口、或人还停在 GitHub / Google 等第三方
/// 域名时，才整页跳回 z.ai 入口，并用 sessionStorage 记住意图，落地后自动补点直到停在目标表单。
const ZAI_EMAIL_ASSIST_JS: &str = r##"(function () {
  var ENTRY = __ZSW_ENTRY__;
  var PHONE_ENTRY = __ZSW_PHONE_ENTRY__;
  var GOOGLE_ENTRY = 'https://chat.z.ai/oauth/google/login';
  var GITHUB_ENTRY = 'https://chat.z.ai/oauth/github/login';
  // 第三方域名（GitHub / Google 授权页）上不做任何页内文案猜测，按钮只能整页回 z.ai
  var ZAI_HOST = /(^|\.)z\.ai$/i.test(location.hostname);
  var LBL = {
    login: __ZSW_LABEL_EMAIL_LOGIN__,
    signup: __ZSW_LABEL_EMAIL_SIGNUP__,
    phone: __ZSW_LABEL_PHONE_LOGIN__,
    google: __ZSW_LABEL_GOOGLE_LOGIN__,
    github: __ZSW_LABEL_GITHUB_LOGIN__,
    already: __ZSW_LABEL_ALREADY__,
    alreadySignup: __ZSW_LABEL_ALREADY_SIGNUP__,
    alreadyPhone: __ZSW_LABEL_ALREADY_PHONE__
  };
  // 登录 / 注册表单都有邮箱输入框，注册页只能靠表单特征文案区分（邮箱表单本身按输入框特征判）
  var PAGE = {
    email: ['邮箱登录', 'Email Login', 'Email login', 'Sign in with Email', 'Sign in with email'],
    signup: ['注册', 'Sign up', 'Sign Up', 'Register'],
    signupForm: ['创建账号', '已经拥有账号了？', 'Create account', 'Create Account', 'Already have an account'],
    toLogin: ['登录', 'Sign in', 'Sign In']
  };
  // 同页 tab / 链接的关键词（包含匹配，命中文案最短的元素，避免点到外层容器）
  var KW = {
    email: ['邮箱登录', '邮箱', '邮件', 'email', 'e-mail'],
    phone: ['手机号登录', '手机号', '手机', 'phone', 'mobile', 'sms'],
    signup: ['注册', '立即注册', 'sign up', 'signup', 'create account', 'register']
  };
  // 页内切换入口的候选元素：先只认交互元素（文案可长），再退到 tab 里的 li/span/div/label（文案必须很短）
  var TOGGLE_SEL = ['button,a,[role="button"],[tabindex]', 'li,span,div,label'];
  var TOGGLE_MAX = [24, 12];
  var INTENT_KEY = 'zsw_email_assist_intent';
  var BAR_ID = 'zsw-email-assist';
  var BTN_STYLE = 'cursor:pointer;border:1px solid rgba(255,255,255,.3);background:rgba(20,20,24,.88);color:#fff;border-radius:999px;padding:6px 12px;font:12px/1 system-ui,sans-serif;white-space:nowrap';
  var norm = function (s) { return (s || '').replace(/\s+/g, '').toLowerCase(); };
  var barEl = function () { return document.getElementById(BAR_ID); };
  var inBar = function (el) { var b = barEl(); return !!(b && b.contains(el)); };
  var clearFlag = function (key) { try { sessionStorage.removeItem(key); } catch (e) {} };
  // 真实可见性：隐藏面板（零尺寸 / display:none / visibility:hidden / opacity:0）一律算不可见。
  // 邮箱与手机号同页时，只有靠「当前可见的是哪个表单」才能区分用户此刻看到的是什么。
  var visible = function (el) {
    if (!el || !el.getBoundingClientRect) return false;
    var r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    var st = window.getComputedStyle(el);
    if (st.display === 'none' || st.visibility === 'hidden' || parseFloat(st.opacity || '1') === 0) return false;
    for (var p = el.parentElement; p; p = p.parentElement) {
      var ps = window.getComputedStyle(p);
      if (ps.display === 'none' || ps.visibility === 'hidden') return false;
    }
    return true;
  };
  var findText = function (txt) {
    var all = document.querySelectorAll('button,a');
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (inBar(el)) continue;
      if (norm(el.innerText) === norm(txt) && visible(el)) return el;
    }
    return null;
  };
  var findAny = function (list) {
    for (var i = 0; i < list.length; i++) {
      var el = findText(list[i]);
      if (el) return el;
    }
    return null;
  };
  // 第三方登录按钮文案不固定（Continue with Google 等），用包含匹配
  var findContains = function (list) {
    var all = document.querySelectorAll('button,a,[role="button"]');
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (inBar(el) || !visible(el)) continue;
      var txt = norm(el.innerText);
      if (!txt || txt.length > 40) continue;
      for (var j = 0; j < list.length; j++) {
        if (txt.indexOf(norm(list[j])) >= 0) return el;
      }
    }
    return null;
  };
  // 表单归属：按可见输入框的 type / placeholder / name / autocomplete 判断，比页面文案可靠
  var inputKind = function (el) {
    var hint = norm(
      (el.getAttribute('type') || '') + ' ' +
      (el.getAttribute('placeholder') || '') + ' ' +
      (el.getAttribute('name') || '') + ' ' +
      (el.getAttribute('autocomplete') || '') + ' ' +
      (el.getAttribute('aria-label') || '')
    );
    if (hint.indexOf('email') >= 0 || hint.indexOf('邮箱') >= 0 || hint.indexOf('邮件') >= 0) return 'email';
    if (hint.indexOf('tel') >= 0 || hint.indexOf('phone') >= 0 || hint.indexOf('mobile') >= 0 || hint.indexOf('手机') >= 0 || hint.indexOf('电话') >= 0) return 'phone';
    return '';
  };
  var formShown = function (kind) {
    var ins = document.querySelectorAll('input,textarea');
    for (var i = 0; i < ins.length; i++) {
      if (!visible(ins[i])) continue;
      if (inputKind(ins[i]) === kind) return ins[i];
    }
    return null;
  };
  // 切换入口的可读文案：按钮文字 + aria-label / title + 图标 alt（纯图标 tab 也能命中）
  var labelOf = function (el) {
    var t = (el.innerText || el.textContent || '') + ' ' + (el.getAttribute('aria-label') || '') + ' ' + (el.getAttribute('title') || '');
    var icons = el.querySelectorAll ? el.querySelectorAll('img,svg,i') : [];
    for (var i = 0; i < icons.length; i++) {
      t += ' ' + (icons[i].getAttribute('alt') || '') + ' ' + (icons[i].getAttribute('aria-label') || '') + ' ' + (icons[i].getAttribute('title') || '');
    }
    return norm(t);
  };
  var pickByKeyword = function (sel, kws, maxLen) {
    var all = document.querySelectorAll(sel);
    var hit = null;
    var best = 1e9;
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (inBar(el) || !visible(el)) continue;
      if (el.querySelector && el.querySelector('input')) continue;
      var txt = labelOf(el);
      if (!txt || txt.length > maxLen) continue;
      for (var j = 0; j < kws.length; j++) {
        if (txt.indexOf(norm(kws[j])) >= 0) {
          if (txt.length < best) { best = txt.length; hit = el; }
          break;
        }
      }
    }
    return hit;
  };
  var findToggle = function (kind, pass) {
    return pickByKeyword(TOGGLE_SEL[pass], KW[kind], TOGGLE_MAX[pass]);
  };
  var clickToggle = function (kind, pass) {
    var el = findToggle(kind, pass);
    if (!el) return false;
    el.click();
    return true;
  };
  // 手机号入口是快捷登录里第一个 Z.ai 图标按钮（无文字），用 Google 按钮的前一个兄弟定位
  var clickZaiIcon = function () {
    var google = document.querySelector('button[class*="ButtonContinueWithGoogle"]');
    if (!google) return false;
    var prev = google.previousElementSibling;
    var btn = prev && prev.tagName === 'BUTTON' ? prev : (prev && prev.querySelector ? prev.querySelector('button') : null);
    if (btn && btn !== google) { btn.click(); return true; }
    var parent = google.parentElement;
    if (parent) {
      var first = parent.querySelector('button');
      if (first && first !== google) { first.click(); return true; }
    }
    return false;
  };
  var PHONE_INTENT_KEY = 'zsw_phone_intent';
  // 统一轮询：按名字去重（同一文档被多次注入也不会叠加多个轮询），fn 返回 true 即收工并释放名额
  var poll = function (name, max, step, fn) {
    var flag = '__zswPoll' + name;
    try {
      if (window[flag]) return;
      window[flag] = true;
    } catch (e) { return; }
    var tries = 0;
    var timer = setInterval(function () {
      if (++tries > max) { clearInterval(timer); try { window[flag] = false; } catch (e) {} return; }
      var done = false;
      try { done = fn() === true; } catch (e) { done = true; }
      if (done) { clearInterval(timer); try { window[flag] = false; } catch (e) {} }
    }, step);
  };
  // 整页跳转是最后手段：落地后由 consume 自动补点到目标表单。
  // 意图不能只存 sessionStorage：它按 origin 隔离，在 github.com 写的值跳到 chat.z.ai 就读不到。
  // 所以同时用两个能跟着顶层跳转过境的载体：URL fragment（# 不发给服务端，不影响 OAuth 参数）
  // 和 window.name（同一标签页内跨文档保留），落地后任一读到了就补点，并把两个都清掉。
  var readName = function () {
    var m = /^zsw-(phone|login|signup)$/.exec((function () { try { return window.name || ''; } catch (e) { return ''; } })());
    return m ? m[1] : '';
  };
  var hashIntent = function () {
    var m = (location.hash || '').match(/zsw-(phone|login|signup)/);
    return m ? m[1] : '';
  };
  var goEntry = function (key, val, url, tag) {
    if (ZAI_HOST) { try { sessionStorage.setItem(key, val); } catch (e) {} }
    try { window.name = 'zsw-' + tag; } catch (e) {}
    location.href = url.split('#')[0] + '#zsw-' + tag;
  };
  var adoptHashIntent = function () {
    var k = hashIntent() || readName();
    if (!k) return;
    try { window.name = ''; } catch (e) {}
    try {
      sessionStorage.setItem(k === 'phone' ? PHONE_INTENT_KEY : INTENT_KEY, k === 'phone' ? '1' : k);
      // 标记只认一次，用完就从地址栏抹掉，免得下次回到这一页重复切换表单
      history.replaceState(null, '', location.pathname + location.search);
    } catch (e) {}
  };
  // 第三方登录：优先点页面自身入口（参数 / state 由页面生成），再按类名定位，最后才回退固定地址
  var clickProvider = function (words, sel, fallback) {
    return function (ev) {
      ev.preventDefault(); ev.stopPropagation();
      if (ZAI_HOST) {
        var el = findContains(words);
        if (!el) {
          var byClass = document.querySelector(sel);
          if (byClass && visible(byClass)) el = byClass;
        }
        if (el) { el.click(); return; }
      }
      location.href = fallback;
    };
  };
  // 页内切到手机号表单：先认文字 tab，再认旧版 Z.ai 图标按钮，最后放宽到 li / span / div / label
  var switchPhone = function () {
    return clickToggle('phone', 0) || clickZaiIcon() || clickToggle('phone', 1);
  };
  var switchEmail = function () {
    if (clickToggle('email', 0)) return true;
    var btn = findAny(PAGE.email);
    if (btn) { btn.click(); return true; }
    return clickToggle('email', 1);
  };
  // 注册入口：文案精确命中优先，否则按关键词找；排除「注册协议」这类条款链接
  var findSignup = function () {
    var el = findAny(PAGE.signup) || findToggle('signup', 0);
    if (el && /协议|条款|政策|term|privacy/.test(norm(el.innerText))) return null;
    return el;
  };
  var startSignupClick = function () {
    poll('signup', 20, 300, function () {
      if (findAny(PAGE.signupForm)) return true;
      var r = findSignup();
      if (!r) return false;
      r.click();
      return true;
    });
  };
  var openPhone = function (ev) {
    ev.preventDefault(); ev.stopPropagation();
    // 还停在 GitHub / Google 等第三方页：那里没有我们的表单，只能整页回 z.ai，意图靠 fragment 带过去
    if (!ZAI_HOST) { goEntry(PHONE_INTENT_KEY, '1', PHONE_ENTRY, 'phone'); return; }
    if (formShown('phone')) { hint(LBL.alreadyPhone); return; }
    if (switchPhone()) return;
    goEntry(PHONE_INTENT_KEY, '1', PHONE_ENTRY, 'phone');
  };
  var consumePhone = function () {
    var intent;
    try { intent = sessionStorage.getItem(PHONE_INTENT_KEY); } catch (e) { return; }
    if (!intent) return;
    poll('phone', 60, 300, function () {
      if (!ZAI_HOST) { clearFlag(PHONE_INTENT_KEY); return true; }
      if (formShown('phone')) { clearFlag(PHONE_INTENT_KEY); return true; }
      switchPhone();
      return false;
    });
  };
  var hint = function (msg) {
    var bar = barEl();
    var t = bar && bar.querySelector('[data-hint]');
    if (!t) return;
    t.textContent = msg;
    t.style.opacity = '1';
    setTimeout(function () { t.style.opacity = '0'; }, 1800);
  };
  var openEmail = function (signup) {
    var tag = signup ? 'signup' : 'login';
    if (!ZAI_HOST) { goEntry(INTENT_KEY, tag, ENTRY, tag); return; }
    var emailShown = !!formShown('email');
    var phoneShown = !!formShown('phone');
    var onSignupForm = !!findAny(PAGE.signupForm);
    if (signup) {
      if (onSignupForm && emailShown && !phoneShown) { hint(LBL.alreadySignup); return; }
      var toSignup = findSignup();
      if (toSignup) { toSignup.click(); return; }
    } else {
      // 只有「邮箱表单可见 + 当前显示的不是手机号表单 + 不在注册页」才算已在邮箱登录页，
      // 否则一律去点页内切换入口（旧版只靠文案特征，会在第三方页和同页手机号表单上误报）
      if (emailShown && !phoneShown && !onSignupForm) { hint(LBL.already); return; }
      if (onSignupForm) {
        var toLogin = findAny(PAGE.toLogin);
        if (toLogin) { toLogin.click(); return; }
      }
    }
    if (switchEmail()) {
      if (signup) startSignupClick();
      return;
    }
    goEntry(INTENT_KEY, tag, ENTRY, tag);
  };
  var consume = function () {
    var intent;
    try { intent = sessionStorage.getItem(INTENT_KEY); } catch (e) { return; }
    if (!intent) return;
    poll('email', 60, 300, function () {
      if (!ZAI_HOST) { clearFlag(INTENT_KEY); return true; }
      var done = false;
      if (intent === 'signup') {
        if (findAny(PAGE.signupForm)) done = true;
        else {
          var r = findSignup();
          if (r) { r.click(); done = true; } else switchEmail();
        }
      } else {
        if (formShown('email')) done = true;
        else done = switchEmail();
      }
      if (done) clearFlag(INTENT_KEY);
      return done;
    });
  };
  var render = function () {
    if (document.getElementById(BAR_ID) || !document.body) return;
    var bar = document.createElement('div');
    bar.id = BAR_ID;
    bar.setAttribute('style', 'position:fixed;top:10px;right:10px;z-index:2147483647;display:flex;flex-direction:column;align-items:flex-end;gap:6px');
    var mk = function (label, signup) {
      var b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      b.setAttribute('style', BTN_STYLE);
      b.addEventListener('click', function (ev) { ev.preventDefault(); ev.stopPropagation(); openEmail(signup); });
      return b;
    };
    var wrap = document.createElement('div');
    wrap.setAttribute('style', 'display:flex;gap:6px;flex-wrap:wrap;justify-content:flex-end');
    var mkBtn = function (label, handler) {
      var b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      b.setAttribute('style', BTN_STYLE);
      b.addEventListener('click', handler);
      return b;
    };
    wrap.appendChild(mkBtn(LBL.phone, openPhone));
    wrap.appendChild(mkBtn(LBL.google, clickProvider(['google'], 'button[class*="ButtonContinueWithGoogle"]', GOOGLE_ENTRY)));
    wrap.appendChild(mkBtn(LBL.github, clickProvider(['github'], 'button[class*="ButtonContinueWithGithub"]', GITHUB_ENTRY)));
    wrap.appendChild(mk(LBL.login, false));
    wrap.appendChild(mk(LBL.signup, true));
    var t = document.createElement('div');
    t.setAttribute('data-hint', '1');
    t.setAttribute('style', 'opacity:0;transition:opacity .2s;background:rgba(20,20,24,.9);color:#fff;border-radius:6px;padding:5px 9px;font:11px/1.2 system-ui,sans-serif');
    bar.appendChild(wrap);
    bar.appendChild(t);
    document.body.appendChild(bar);
  };
  // 授权确认页自动点「授权」：第三方登录（Google / GitHub）回调后常停在这一页，
  // 按钮文案不固定，先按关键词找；找不到时若页面上只有一个可点按钮也点它（排除取消 / 返回 / 登录类）。
  // 官网登录成功可能是 SPA 内部路由切到授权页（不再触发注入），所以这里常驻轮询而不是一次性判断。
  var autoAuthorize = function () {
    poll('authorize', 600, 500, function () {
      if (location.pathname.indexOf('/oauth/authorize') < 0) return false;
      var all = document.querySelectorAll('button,a,[role="button"]');
      var others = [];
      var hit = null;
      for (var i = 0; i < all.length; i++) {
        var el = all[i];
        if (inBar(el) || !visible(el)) continue;
        var raw = (el.innerText || '').trim();
        var txt = norm(raw);
        if (!raw || raw.length > 24) continue;
        if (/cancel|deny|拒绝|取消|返回|back|login|sign|登录/.test(txt)) continue;
        if (/授权|同意|允许|authorize|allow|agree|continue|确认|继续/.test(txt)) { hit = el; break; }
        others.push(el);
      }
      var btn = hit || (others.length === 1 ? others[0] : null);
      if (!btn) return false;
      btn.click();
      return true;
    });
  };
  var bootTries = 0;
  // 注入会被反复触发（on_page_load + 窗口起来后的补注入），render 靠 BAR_ID 幂等，
  // 各路轮询靠 poll 里的 window 标记去重，所以每次注入都可以直接再调一遍。
  var boot = function () {
    if (document.body) { adoptHashIntent(); render(); consume(); consumePhone(); autoAuthorize(); return; }
    if (++bootTries > 50) return;
    setTimeout(boot, 100);
  };
  boot();
})();"##;

pub fn zai_email_assist_script(phone_entry: &str) -> String {
    let lit = |s: String| serde_json::Value::String(s).to_string();
    ZAI_EMAIL_ASSIST_JS
        .replace("__ZSW_PHONE_ENTRY__", &lit(phone_entry.to_string()))
        .replace("__ZSW_ENTRY__", &lit(ZAI_LOGIN_ENTRY_URL.to_string()))
        .replace("__ZSW_LABEL_EMAIL_LOGIN__", &lit(crate::i18n::tr("login.assist.email")))
        .replace("__ZSW_LABEL_PHONE_LOGIN__", &lit(crate::i18n::tr("login.assist.phone")))
        .replace("__ZSW_LABEL_GOOGLE_LOGIN__", &lit(crate::i18n::tr("login.assist.google")))
        .replace("__ZSW_LABEL_GITHUB_LOGIN__", &lit(crate::i18n::tr("login.assist.github")))
        .replace("__ZSW_LABEL_EMAIL_SIGNUP__", &lit(crate::i18n::tr("login.assist.signup")))
        .replace("__ZSW_LABEL_ALREADY__", &lit(crate::i18n::tr("login.assist.hint")))
        .replace("__ZSW_LABEL_ALREADY_SIGNUP__", &lit(crate::i18n::tr("login.assist.hint_signup")))
        .replace("__ZSW_LABEL_ALREADY_PHONE__", &lit(crate::i18n::tr("login.assist.hint_phone")))
}

#[cfg(test)]
mod assist_tests {
    use super::zai_email_assist_script;
    use crate::i18n::{tr_lang, Lang};

    /// 注入脚本的占位符必须全部替换掉，且按钮 / 提示文案在中英文表里都齐备
    /// （tr_lang 未命中会原样返回 key，英文表缺 key 时会回退成中文，所以再按 ASCII 方向校一次）。
    #[test]
    fn assist_script_placeholders_and_labels() {
        const KEYS: [&str; 8] = [
            "login.assist.email",
            "login.assist.phone",
            "login.assist.google",
            "login.assist.github",
            "login.assist.signup",
            "login.assist.hint",
            "login.assist.hint_signup",
            "login.assist.hint_phone",
        ];
        for lang in [Lang::Zh, Lang::En] {
            for key in KEYS {
                let v = tr_lang(lang, key);
                assert_ne!(v, key, "{lang:?} 缺少文案 {key}");
                assert!(!v.is_empty(), "{lang:?} 文案 {key} 为空");
                assert_eq!(v.is_ascii(), lang == Lang::En, "{key} 的 {lang:?} 文案不对：{v}");
            }
            let js = zai_email_assist_script("https://chat.z.ai/auth?x=1");
            assert!(!js.contains("__ZSW_"), "{lang:?} 注入脚本有未替换的占位符");
            // 同页表单判定 / 第三方域名收敛 / 页内切换 / 去重轮询，缺一个就是本次改动回归
            for token in ["ZAI_HOST", "formShown", "switchPhone", "switchEmail", "poll("] {
                assert!(js.contains(token), "{lang:?} 注入脚本缺少 {token}");
            }
        }
    }
}

pub fn bridge_redirect_uri() -> String {
    format!("https://zcode.z.ai/app/oauth/login?redirect={REDIRECT_ENC}&app_version={}", quota::CLIENT_APP_VERSION)
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub struct FlowInit {
    pub authorize_url: String,
    pub state: String,
    pub poll_url: String,
    pub poll_token: String,
    pub expires_at_ms: u128,
    pub poll_interval_ms: u64,
}

pub fn new_poll_token() -> String {
    let mut buf = [0u8; 32];
    getrandom_fallback(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn init_flow(provider: &str, mid: &str) -> Result<FlowInit, String> {
    init_flow_at(FLOW_INIT_URL, provider, mid)
}

fn init_flow_at(url: &str, provider: &str, mid: &str) -> Result<FlowInit, String> {
    let poll_token = new_poll_token();
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build();
    let mut req = agent.post(url);
    for (k, v) in quota::zai_oauth_headers_with_mid(&poll_token, Some(mid.to_string())).0 {
        req = req.set(&k, &v);
    }
    let resp = req
        .send_json(json!({ "provider": provider }))
        .map_err(|e| crate::i18n::trf("err.oauth.init", &[("e", &e.to_string())]))?
        .into_string()
        .map_err(|e| crate::i18n::trf("err.oauth.init", &[("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let invalid = || crate::i18n::tr("err.oauth.init_invalid");
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(crate::i18n::trf("err.oauth.init_invalid_msg", &[("msg", msg)]));
    }
    let data = v.get("data").ok_or_else(invalid)?;
    let flow_id = data.get("flow_id").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(invalid)?;
    let authorize_raw = data.get("authorize_url").and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty())
        .ok_or_else(invalid)?;
    let authorize = match normalize_zai_authorize_entry(provider, authorize_raw) {
        Some(u) => u,
        None => authorize_raw.to_string(),
    };
    let poll_token = data
        .get("poll_token")
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or(poll_token);
    let expires_at_ms = data.get("expires_at").and_then(|x| x.as_f64()).map(|s| (s * 1000.0) as u128)
        .ok_or_else(invalid)?;
    let poll_interval_ms = data.get("poll_interval_sec").and_then(|x| x.as_f64()).map(|s| (s * 1000.0) as u64)
        .ok_or_else(invalid)?;

    let auth_url: tauri::Url = authorize.parse().map_err(|_| invalid())?;
    let state = auth_url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .and_then(|(_, v)| {
            let s = v.trim().to_string();
            (!s.is_empty()).then_some(s)
        })
        .ok_or_else(invalid)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let remaining = expires_at_ms.saturating_sub(now);
    if auth_url.scheme() != "https" || remaining == 0 || poll_interval_ms < 1_000 || poll_interval_ms as u128 >= remaining {
        return Err(invalid());
    }
    let init_base: String = url
        .parse::<tauri::Url>()
        .map(|u| u.origin().ascii_serialization())
        .map_err(|_| invalid())?;
    Ok(FlowInit {
        authorize_url: auth_url.to_string(),
        state,
        poll_url: format!("{init_base}/api/v1/oauth/cli/poll/{}", urlencode(flow_id)),
        poll_token,
        expires_at_ms,
        poll_interval_ms,
    })
}

/// z.ai 登录窗口入口规范化：服务端下发的 `https://chat.z.ai/api/oauth/authorize?...` 本身
/// 会 307 跳到官网登录页 `/auth`，这里直接以 `/auth` 形态打开（参数按官网排布、redirect_uri 做百分号编码）。
/// client_id / redirect_uri / state 必须沿用本次 flow 的服务端下发值，否则登录无法回调 zcode、
/// 服务端 flow 不会 ready，账号也就无法入库。
fn normalize_zai_authorize_entry(provider: &str, raw: &str) -> Option<String> {
    if provider != "zai" {
        return None;
    }
    let u: tauri::Url = raw.parse().ok()?;
    if u.host_str() != Some("chat.z.ai") || u.path() != "/api/oauth/authorize" {
        return None;
    }
    let pick = |key: &str| {
        u.query_pairs()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v.into_owned())
    };
    let response_type = pick("response_type").unwrap_or_else(|| "code".to_string());
    let client_id = pick("client_id")?;
    let redirect_uri = pick("redirect_uri")?;
    let state = pick("state")?;
    let mut out = format!(
        "https://chat.z.ai/auth?response_type={}&client_id={}&redirect_uri={}&state={}",
        urlencode(&response_type),
        urlencode(&client_id),
        urlencode(&redirect_uri),
        urlencode(&state),
    );
    for (k, v) in u
        .query_pairs()
        .filter(|(k, _)| !matches!(k.as_ref(), "response_type" | "client_id" | "redirect_uri" | "state"))
    {
        out.push('&');
        out.push_str(&urlencode(&k));
        out.push('=');
        out.push_str(&urlencode(&v));
    }
    Some(out)
}

#[derive(Debug)]
pub enum PollOutcome {
    Pending,
    Ready(Value),
}

pub fn poll_flow_once(url: &str, poll_token: &str, mid: &str) -> Result<PollOutcome, String> {
    let agent = web_agent();
    let mut req = agent.get(url);
    for (k, v) in quota::zai_oauth_headers_with_mid(poll_token, Some(mid.to_string())).0 {
        req = req.set(&k, &v);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, resp)) if (400..500).contains(&code) && code != 408 && code != 429 => {
            let body = resp.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if v.get("code").and_then(|c| c.as_i64()) == Some(3004) {
                return Err(crate::i18n::tr("err.oauth.expired"));
            }
            return Err(crate::i18n::trf("err.oauth.poll_terminal", &[("code", &code.to_string())]));
        }
        Err(_) => return Ok(PollOutcome::Pending),
    };
    let body = match resp.into_string() {
        Ok(b) => b,
        Err(_) => return Ok(PollOutcome::Pending),
    };
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
    let invalid = || crate::i18n::tr("err.oauth.poll_invalid");
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(crate::i18n::trf("err.oauth.poll_invalid_msg", &[("msg", msg)]));
    }
    let data = v.get("data").cloned().ok_or_else(invalid)?;
    match data.get("status").and_then(|s| s.as_str()).unwrap_or("") {
        "pending" => Ok(PollOutcome::Pending),
        "failed" => Err(crate::i18n::tr("err.oauth.flow_failed")),
        "ready" => {
            let br_str = |ptr: &str| {
                data.pointer(ptr)
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            };
            let token = br_str("/token");
            let access = match data.get("zai").or_else(|| data.get("bigmodel")) {
                Some(p) => br_str2(p, &["access_token", "accessToken"]),
                None => None,
            };
            let user_id = br_str("/user/user_id");
            if token.is_none() || access.is_none() || user_id.is_none() {
                return Err(invalid());
            }
            Ok(PollOutcome::Ready(data))
        }
        _ => Err(invalid()),
    }
}

fn br_str2(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

#[derive(Debug, PartialEq)]
pub enum CallbackKind {
    Code { code: String, state: String },
    Attribution,
}

pub fn parse_callback(url: &str) -> Result<CallbackKind, String> {
    let rest = url
        .strip_prefix("zcode://oauth/callback")
        .ok_or_else(|| crate::i18n::tr("err.oauth.not_callback"))?;
    let qs = rest.trim_start_matches('?');
    let mut code = String::new();
    let mut state = String::new();
    let mut has_attribution = false;
    for kv in qs.split('&') {
        let (k, v) = kv.split_once('=').ok_or_else(|| crate::i18n::tr("err.oauth.bad_cb"))?;
        match urldecode(k).as_str() {
            "authCode" => code = urldecode(v),
            "code" if code.is_empty() => code = urldecode(v),
            "state" => state = urldecode(v),
            "channel_id" | "utm_source" | "utm_campaign" => has_attribution = true,
            _ => {}
        }
    }
    if state.is_empty() {
        return Err(crate::i18n::tr("err.oauth.state"));
    }
    if !code.is_empty() {
        return Ok(CallbackKind::Code { code, state });
    }
    if has_attribution {
        return Ok(CallbackKind::Attribution);
    }
    Err(crate::i18n::tr("err.oauth.no_code_state"))
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                    out.push(h << 4 | l);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn getrandom_fallback(buf: &mut [u8]) {
    for chunk in buf.chunks_mut(16) {
        let u = uuid::Uuid::new_v4();
        chunk.copy_from_slice(&u.as_bytes()[..chunk.len()]);
    }
}

pub fn parse_proxy_url(input: &str) -> Result<String, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err(crate::i18n::tr("err.proxy.empty"));
    }
    let lower = s.to_ascii_lowercase();
    let (scheme, rest) = if let Some(r) = lower.strip_prefix("http://") {
        ("http", r)
    } else if let Some(r) = lower.strip_prefix("socks5://") {
        ("socks5", r)
    } else {
        return Err(crate::i18n::tr("err.proxy.scheme"));
    };
    if rest.contains('@') {
        return Err(crate::i18n::tr("err.proxy.no_auth"));
    }
    if rest.contains('/') || rest.contains('\\') {
        return Err(crate::i18n::tr("err.proxy.no_path"));
    }
    let Some((host, port)) = rest.rsplit_once(':') else {
        return Err(crate::i18n::tr("err.proxy.need_port"));
    };
    if host.is_empty() {
        return Err(crate::i18n::tr("err.proxy.empty_host"));
    }
    if host.contains(' ') || host.contains(':') {
        return Err(crate::i18n::tr("err.proxy.bad_host"));
    }
    let port_num: u32 = port.parse().map_err(|_| crate::i18n::trf("err.proxy.port_nan", &[("port", port)]))?;
    if !(1..=65535).contains(&port_num) {
        return Err(crate::i18n::trf("err.proxy.port_range", &[("port", &port_num.to_string())]));
    }
    Ok(format!("{scheme}://{host}:{port_num}"))
}

pub fn exchange_token(provider: &str, code: &str, state: &str, mid: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build();
    let mut req = agent.post(TOKEN_URL);
    for (k, v) in quota::zai_oauth_headers_with_mid("", Some(mid.to_string())).0 {
        if k == "Authorization" {
            continue;
        }
        req = req.set(&k, &v);
    }
    let resp = req
        .send_json(json!({
            "provider": provider,
            "code": code,
            "redirect_uri": bridge_redirect_uri(),
            "state": state,
        }))
        .map_err(|e| crate::i18n::trf("err.oauth.exchange_req", &[("e", &e.to_string())]))?
        .into_string()
        .map_err(|e| crate::i18n::trf("err.http.read", &[("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code_n = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code_n != 0 {
        let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(crate::i18n::trf("err.oauth.exchange", &[("code", &code_n.to_string()), ("msg", msg)]));
    }
    let token = v
        .pointer("/data/token")
        .and_then(|t| t.as_str())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| crate::i18n::tr("err.oauth.no_token"))?;
    Ok(json!({ "jwt": token, "raw": v }))
}

pub fn extract_user_profile(provider: &str, raw: &Value) -> Option<Value> {
    if provider != "zai" {
        return None;
    }
    raw.pointer("/data/user").and_then(backend_user_profile)
}

pub fn extract_poll_user_profile(raw: &Value) -> Option<Value> {
    raw.pointer("/data/user").and_then(backend_user_profile)
}

fn backend_user_profile(u: &Value) -> Option<Value> {
    let nonempty = |k: &str| {
        u.get(k)
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    };
    if !["user_id", "email", "name", "avatar"].iter().any(|k| nonempty(k)) {
        return None;
    }
    let id = u
        .get("user_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let name = u
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| u.get("email").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .or(id);
    Some(json!({
        "id": id,
        "username": name.unwrap_or(""),
        "displayName": name.unwrap_or(""),
        "email": u.get("email").and_then(|v| v.as_str()).unwrap_or(""),
        "avatarUrl": u.get("avatar").and_then(|v| v.as_str()).unwrap_or(""),
    }))
}

pub fn fetch_userinfo(provider: &str, token: &str) -> Option<Value> {
    let (url, bearer) = match provider {
        "bigmodel" => ("https://bigmodel.cn/api/biz/customer/getCustomerInfo", false),
        "zai" => ("https://chat.z.ai/api/oauth/userinfo", true),
        _ => return None,
    };
    let auth = if bearer {
        format!("Bearer {token}")
    } else {
        token.to_string()
    };
    let resp = web_agent()
        .get(url)
        .set("Authorization", &auth)
        .set("Content-Type", "application/json")
        .set("User-Agent", LOGIN_WINDOW_UA)
        .call()
        .ok()?
        .into_string()
        .ok()?;
    let v: Value = serde_json::from_str(&resp).ok()?;
    if v.get("code").and_then(|c| c.as_i64()).map(|c| c != 0).unwrap_or(false) {
        return None;
    }
    let data = v.get("data").cloned().unwrap_or(v);
    let pick = |k: &str, alts: &[&str]| -> Option<String> {
        alts.iter()
            .find_map(|a| data.get(a).and_then(|x| x.as_str()).map(String::from))
            .or_else(|| data.get(k).and_then(|x| x.as_str()).map(String::from))
    };
    Some(json!({
        "id": pick("id", &["customerNumber", "sub", "id"]),
        "username": pick("username", &["username", "name", "preferred_username", "email"]),
        "displayName": pick("displayName", &["username", "name", "preferred_username"]),
        "avatarUrl": pick("avatarUrl", &["avatar", "picture"]),
        "email": pick("email", &["email"]),
    }))
}

pub fn extract_refresh_token(provider: &str, raw: &Value) -> Option<String> {
    if provider != "bigmodel" {
        return None;
    }
    raw.pointer("/data/bigmodel/refresh_token")
        .or_else(|| raw.pointer("/data/bigmodel/refreshToken"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

pub fn assemble_credentials_with_token(
    provider: &str,
    jwt: &str,
    userinfo: Option<&Value>,
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("zcodejwttoken".into(), json!(jwt));
    m.insert("oauth:active_provider".into(), json!(provider));
    if let Some(at) = access_token.filter(|s| !s.trim().is_empty()) {
        m.insert(format!("oauth:{provider}:access_token"), json!(at));
    }
    if let Some(rt) = refresh_token.filter(|s| !s.trim().is_empty()) {
        m.insert(format!("oauth:{provider}:refresh_token"), json!(rt));
    }
    if let Some(ui) = userinfo {
        m.insert(format!("oauth:{provider}:user_info"), json!(ui.to_string()));
    }
    Value::Object(m)
}

pub const BIGMODEL_BIZ_BASE: &str = "https://bigmodel.cn";
pub const ZAI_API_BASE: &str = "https://api.z.ai";
pub const ZAI_BUSINESS_LOGIN_URL: &str = "https://api.z.ai/api/auth/z/login";
pub const BIGMODEL_ANTHROPIC_BASE: &str = "https://open.bigmodel.cn/api/anthropic";
pub const ZAI_ANTHROPIC_BASE: &str = "https://api.z.ai/api/anthropic";
pub const START_PLAN_ANTHROPIC_BASE: &str = "https://zcode.z.ai/api/v1/zcode-plan/anthropic";
const API_KEY_NAME: &str = "zcode-api-key";

fn web_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .build()
}

pub fn extract_access_token(provider: &str, raw: &Value) -> Option<String> {
    let pick = |v: &Value| -> Option<String> {
        ["access_token", "accessToken"]
            .iter()
            .find_map(|k| v.get(k).and_then(|x| x.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let data = raw.get("data")?;
    match provider {
        "bigmodel" => data.get("bigmodel").and_then(&pick).or_else(|| pick(data)),
        "zai" => data.get("zai").and_then(&pick),
        _ => None,
    }
}

pub fn pick_org_project(customer: &Value) -> Option<(String, String)> {
    let root = customer.get("data").unwrap_or(customer);
    let orgs = root.get("organizations")?.as_array()?;
    let id_of = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => (!s.is_empty()).then(|| s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        }
    };
    let keep = |p: &Value| -> bool {
        match p.get("projectType") {
            Some(Value::String(s)) => s.trim() != "2",
            Some(Value::Number(n)) => n.to_string() != "2",
            _ => true,
        }
    };
    let mut cands: Vec<(&Value, String, Vec<&Value>)> = vec![];
    for o in orgs {
        let Some(org_id) = o.get("organizationId").and_then(id_of) else {
            continue;
        };
        let projects: Vec<&Value> = o
            .get("projects")
            .and_then(|p| p.as_array())
            .map(|a| a.iter().filter(|p| keep(p)).collect())
            .unwrap_or_default();
        if projects.is_empty() {
            continue;
        }
        cands.push((o, org_id, projects));
    }
    fn name_of<'a>(v: &'a Value, key: &str) -> &'a str {
        v.get(key).and_then(|x| x.as_str()).unwrap_or("")
    }
    let (_, org_id, projects) = cands
        .iter()
        .find(|(o, _, _)| name_of(o, "organizationName").contains("默认机构"))
        .or_else(|| cands.first())?;
    let proj = projects
        .iter()
        .find(|p| name_of(p, "projectName").contains("默认项目"))
        .or_else(|| projects.first())?;
    let pid = proj.get("projectId").and_then(id_of)?;
    Some((org_id.clone(), pid))
}

fn keys_array(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.get("data").and_then(|d| d.as_array()).map(|a| a.iter().collect()).unwrap_or_default(),
        _ => vec![],
    }
}

pub fn resolve_biz_api_key(base: &str, auth: &str, require_secret: bool) -> Option<String> {
    let agent = web_agent();
    let get_json = |url: &str| -> Option<Value> {
        agent
            .get(url)
            .set("Authorization", auth)
            .set("Content-Type", "application/json")
            .call()
            .ok()?
            .into_string()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    };
    let cust = get_json(&format!("{base}/api/biz/customer/getCustomerInfo"))?;
    let (org, proj) = pick_org_project(&cust)?;
    let keys_url = format!("{base}/api/biz/v1/organization/{org}/projects/{proj}/api_keys");
    let list = get_json(&keys_url)?;
    let mut found = keys_array(&list)
        .into_iter()
        .find(|k| k.get("name").and_then(|n| n.as_str()) == Some(API_KEY_NAME))
        .map(|k| k.clone());
    if found.is_none() {
        found = agent
            .post(&keys_url)
            .set("Authorization", auth)
            .set("Content-Type", "application/json")
            .send_json(json!({ "name": API_KEY_NAME }))
            .ok()?
            .into_string()
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
    }
    let key = found
        .as_ref()
        .and_then(|k| k.get("apiKey").and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())?
        .to_string();
    let secret = get_json(&format!("{keys_url}/copy/{}", urlencode(&key)))
        .and_then(|v| v.get("secretKey").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_default();
    if secret.trim().is_empty() {
        return if require_secret { None } else { Some(key) };
    }
    Some(format!("{key}.{}", secret.trim()))
}

pub fn resolve_zai_business_token(zai_access_token: &str) -> Option<String> {
    resolve_zai_business_token_at(ZAI_BUSINESS_LOGIN_URL, zai_access_token)
}

fn resolve_zai_business_token_at(url: &str, zai_access_token: &str) -> Option<String> {
    let resp = web_agent()
        .post(url)
        .set("Content-Type", "application/json")
        .send_json(json!({ "token": zai_access_token }))
        .ok()?
        .into_string()
        .ok()?;
    let v: Value = serde_json::from_str(&resp).ok()?;
    ["access_token", "accessToken"]
        .iter()
        .find_map(|k| {
            v.pointer(&format!("/data/{k}"))
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
}

pub fn assemble_config(provider: &str, jwt: &str, access_token: &str) -> Value {
    let entry = |name: &str, key: &str, base: &str| {
        let k = key.trim();
        json!({
            "name": name,
            "kind": "anthropic",
            "options": if k.is_empty() {
                json!({ "apiKey": "", "apiKeyRequired": true, "baseURL": base })
            } else {
                json!({ "apiKey": k, "baseURL": base })
            },
            "enabled": !k.is_empty(),
            "source": "custom",
        })
    };
    let mut providers = serde_json::Map::new();
    match provider {
        "bigmodel" => {
            let key = if access_token.trim().is_empty() {
                None
            } else {
                resolve_biz_api_key(BIGMODEL_BIZ_BASE, access_token.trim(), false)
            }
            .unwrap_or_default();
            providers.insert("builtin:bigmodel".into(), entry("Bigmodel - API Key", &key, BIGMODEL_ANTHROPIC_BASE));
            providers.insert(
                "builtin:bigmodel-coding-plan".into(),
                entry("BigModel - Coding Plan", &key, BIGMODEL_ANTHROPIC_BASE),
            );
            providers.insert(
                "builtin:bigmodel-start-plan".into(),
                entry("BigModel- Coding Plan", jwt, START_PLAN_ANTHROPIC_BASE),
            );
        }
        "zai" => {
            providers.insert(
                "builtin:zai".into(),
                entry("Z.ai - API Key", "", ZAI_ANTHROPIC_BASE),
            );
            providers.insert(
                "builtin:zai-start-plan".into(),
                entry("Z.ai - Coding Plan", jwt, START_PLAN_ANTHROPIC_BASE),
            );
            let key = if access_token.trim().is_empty() {
                None
            } else {
                resolve_biz_api_key(ZAI_API_BASE, &format!("Bearer {}", access_token.trim()), true)
            }
            .unwrap_or_default();
            providers.insert(
                "builtin:zai-coding-plan".into(),
                entry("Z.ai - Coding Plan", &key, ZAI_ANTHROPIC_BASE),
            );
        }
        _ => {}
    }
    json!({ "provider": providers })
}
