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

## Verification

```bash
# 行为验证（假 DOM + 虚拟时钟，覆盖 prd 的 AC1–AC6）
node .trellis/tasks/<task>/research/assist-dom-harness.mjs src-tauri/src/oauth.rs
cargo test --lib            # 含 oauth::assist_tests::assist_script_placeholders_and_labels
```

The harness loads the JS straight out of `oauth.rs`, fills the placeholders, and asserts clicks and
`location.href` writes — use it before touching the detection order again.

## Related

- Task: `.trellis/tasks/10-27-login-assist-tab-switch/` (PRD = requirement + AC source)
- Relay logic that must not break: `oauth_begin`'s `on_navigation` + `is_login_page` /
  `is_authorize_page` in `src-tauri/src/lib.rs`.
