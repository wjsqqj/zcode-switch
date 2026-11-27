# z.ai 登录窗口注入脚本（assist bar）

Scope: `src-tauri/src/oauth.rs::ZAI_EMAIL_ASSIST_JS` + `zai_email_assist_script()`, injected by
`src-tauri/src/lib.rs::oauth_begin` via `wv.eval()` on every page load of the `login` webview.

The bar adds buttons (手机号登录 / Google / GitHub / 邮箱登录 / 邮箱注册) inside the external z.ai
login page, because the CLI authorize page does not always expose every sign-in form.

---

## Facts about the target page (verified by user testing, not automatable here)

- 邮箱登录 and 手机号登录 are **two forms on the same page**; the phone form is hidden by default
  (`display: none` panel), not a separate route. Do not model them as two pages.
- 登录 and 注册 share the same email input, so form identity must come from the *visible* inputs,
  while login-vs-signup still needs feature copy (`创建账号` / `已经拥有账号了？`).
- Third-party hops (GitHub / Google) leave the z.ai origin. `on_page_load` injects into those
  documents too, so the script runs on `github.com` etc.
- Jumping to the login entry lands on the **email** form by default, so "open the phone form"
  across a third-party hop needs an explicit switch after landing — a plain `location.href` can
  never show the phone form.
- 官网登录成功常走 SPA 内部路由（无新文档），一次性判断会错过后续状态；注入本身也会被重复触发
  （`on_page_load` + 起来后的补注入）。

## Rules

1. **Gate on origin before touching page copy.** `ZAI_HOST = /(^|\.)z\.ai$/i.test(location.hostname)`.
   On non-z.ai documents never read page text and never show "already on X page" hints — the only
   legal action is a full-page jump back to the z.ai entry. (Old bug: GitHub's own
   `Forgot password?` link matched the login-form heuristic and reported "已在邮箱登录页".)
2. **Decide "which form is showing" from visible inputs**, not from copy: `inputKind()` maps
   `type` / `placeholder` / `name` / `autocomplete` / `aria-label` to `email` | `phone`, and
   `formShown()` returns the first *visible* match. `visible()` must check rect size + computed
   `display` / `visibility` / `opacity` **and walk ancestors** — `offsetParent !== null` is not
   enough for hidden panels.
3. **Prefer in-page switching; navigation is the last resort.** Order for a switch:
   interactive elements with matching keywords (`button,a,[role=button],[tabindex]`) → legacy
   icon heuristics (`clickZaiIcon`) → short-text containers (`li,span,div,label`, ≤12 chars).
   Pick the **shortest** matching label so containers/terms links lose to the real tab.
4. **Remember intent across navigation and re-apply it.** `sessionStorage` keys
   `zsw_email_assist_intent` / `zsw_phone_intent`; after landing, `consume()` / `consumePhone()`
   poll until the *target form is visible*, then clear the flag — never assume one fixed DOM shape.
   ⚠ `sessionStorage` is **origin-scoped**: an intent written while the window sits on
   `github.com` is unreadable after the jump to `chat.z.ai`, so the user lands on the default
   (email) form. Any intent that must survive a third-party hop has to travel **in the URL or the
   tab**: `goEntry()` appends `#zsw-phone|login|signup` (fragments never reach the server and do not
   touch OAuth params) and also sets `window.name` (kept across top-level navigations in the same
   tab). `adoptHashIntent()` reads either, writes it back into same-origin storage, and strips the
   marker with `history.replaceState` so the switch happens once. Same-origin reloads may still use
   `sessionStorage` alone.
5. **All polling goes through `poll(name, max, step, fn)`**, which dedupes on a `window` flag and
   releases it on finish. Re-injection must not stack timers or click a button twice
   (`autoAuthorize` clicks once, `render()` is idempotent via `BAR_ID`).
6. **Injected script stays ES5, single IIFE, no `const`/`let`/arrow/template literals** — it runs in
   WebView2 against pages we do not control. Placeholders are `__ZSW_*__` and are filled from
   `crate::i18n::tr(...)`; adding one means adding the key to *both* ZH and EN tables.
7. **手机号入口固定为中国站 `zchat.z.ai/auth`.** The login window's **initial entry** for the
   `zai` provider is `oauth::ZCHAT_LOGIN_ENTRY_URL`, and `__ZSW_PHONE_ENTRY__` takes the same value
   — never the CLI `authorize_url`. The international CLI entry redirects a mainland phone number
   to a "正在跳转至中国站点" notice instead of a usable form; both the first page and the assist
   bar must land on the working phone-login page. The CLI authorize URL is used only as the relay
   target after login. `openPhone()` is an explicit exception to Rule 3: it **always** navigates to
   `PHONE_ENTRY`, never switches an in-page phone tab — otherwise clicking 邮箱登录 then 手机号登录
   on the international page leaves the user on the old CLI form. Only when already on
   `zchat.z.ai/auth` with the phone form visible does it show the "already" hint.
   **Email / signup / third-party buttons follow the auth-proxy switch**: `SHOW_EXTRA` is filled
   from `proxy_url.is_some()` in `oauth_begin`. With 「授权登录走代理」on, the login window exits via
   an overseas proxy, `init_flow` uses upstream `zai` (`chat.z.ai`) and the bar shows
   email/signup/Google/GitHub; with it off, upstream is `zai_cn` (`zchat.z.ai`) and only the phone
   button is shown. Handlers (`openEmail`, `clickProvider`) stay intact in both modes.
9. **Auto-check the phone-login agreement checkbox.** The zchat phone form cannot send an SMS code
   until "我已阅读并同意用户协议、隐私政策和Z.AI账号服务须知" is checked. `autoAgree()` uses the
   shared `poll()` dedupe, matches the checkbox / label by agreement copy (or `agree to` in EN),
   clicks only when unchecked, and stops once checked — never toggles it twice.
8. **Upstream provider follows the auth-proxy switch.** With 「授权登录走代理」off, `oauth::upstream_provider`
   maps `zai` → `zai_cn` (`zchat.z.ai`, phone only); with it on, `zai` stays `zai` (`chat.z.ai`,
   email/Google/GitHub via the overseas exit IP). The chosen upstream is stored in
   `FlowInit.upstream_provider`, carried through `PendingOAuth.upstream`, and passed to
   `persist_oauth_account`.
   **Never exchange a `zai_cn` access token at `/api/auth/z/login`**: ZCode 4.0.0's
   `zaiProviderAdapter` uses `tokenSet.accessToken` as-is for `zai_cn`, and the global business-login
   endpoint rejects it (`z.ai 用户信息异常` / `Z.ai user information is invalid`). `zai_cn` persists
   `data.zai.access_token` directly; only upstream `zai` calls `resolve_zai_business_token`.
   Paid-plan material for `zai_cn` comes from `data.bigmodel` (`customer_id` + `access_token`), and
   its Start Plan family is `bigmodel`. `is_authorize_page` treats `/auth/oauth/authorize` as a real
   authorize page (the zchat confirmation page); do not correct or re-relay it. The relay host
   allowlist is `{z.ai, chat.z.ai, zchat.z.ai}`.
   **Windows proxy caveat:** Tauri 2.11.5's `WebviewWindowBuilder::proxy_url` only stores the field
   and does not forward it to wry's `proxy_config`, so it has no effect on WebView2. The login window
   therefore also sets `--proxy-server=<url>` via `additional_browser_args`; keep both so platforms
   that honor `proxy_url` still work.

## Verification

```bash
# 行为验证（假 DOM + 虚拟时钟，覆盖 prd 的 AC1–AC6）
node .trellis/tasks/<task>/research/assist-dom-harness.mjs src-tauri/src/oauth.rs
cargo test --lib            # 含 oauth::assist_tests::assist_script_placeholders_and_labels
```

The harness loads the JS straight out of `oauth.rs`, fills the placeholders, and asserts clicks and
`location.href` writes — use it before touching the detection order again.

## Related

- Tasks: `.trellis/tasks/10-27-login-assist-tab-switch/` (original requirement + AC source),
  `.trellis/tasks/11-27-zai-intl-login-fix/` (phone entry = zchat.z.ai/auth + malformed-path correction)
- Relay logic that must not break: `oauth_begin`'s `on_navigation` + `is_login_page` /
  `is_authorize_page` in `src-tauri/src/lib.rs`.
