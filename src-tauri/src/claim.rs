
use crate::flowlog;
use crate::quota;
use crate::zcrypto;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

pub const BILLING_PREVIEW_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/preview";
pub const BILLING_CLAIM_URL: &str = "https://zcode.z.ai/api/v1/zcode-plan/billing/claim";
pub const CLIENT_CONFIGS_URL: &str = "https://zcode.z.ai/api/v1/client/configs";
pub const MARKETING_TOUCH_URL: &str = "https://zcode.z.ai/api/v1/marketing/touch?seq=0";

const CLAIM_TIMEOUT_SECS: u64 = 25;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaimGrant {
    pub name: String,
    pub units: f64,
    pub period: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ClaimPlan {
    pub plan_id: String,
    pub name: String,
    pub description: String,
    pub priority: i64,
    pub grants: Vec<String>,
    #[serde(default)]
    pub grant_items: Vec<ClaimGrant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptchaConfig {
    pub enabled: bool,
    pub region: String,
    pub prefix: String,
    pub scene_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimOutcome {
    pub account_id: String,
    pub account_name: String,
    pub plan_name: String,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    pub server_time: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ClaimError {
    pub code: i64,
    pub message: String,
    pub next_at: Option<i64>,
}

impl ClaimError {
    /// 3007：验证码校验失败（参数缺失/过期/已用），可换新验证码参数重试一次。
    pub fn is_captcha_rejected(&self) -> bool {
        self.code == 3007
    }
    /// 1001~1005：终态失败（套餐不存在/活动结束/已领取/不符条件/名额用完），不应重试。
    pub fn is_terminal(&self) -> bool {
        (1001..=1005).contains(&self.code)
    }
    /// HTTP 401/403 但携带验证码挑战特征：是风控人机校验而非凭证失效，不得按登录过期处理。
    pub fn is_challenge(&self) -> bool {
        self.code == CLAIM_ERR_CHALLENGE
    }
}

/// 非 claim 业务码的保留语义码：HTTP 401/403 携带验证码挑战特征。
pub const CLAIM_ERR_CHALLENGE: i64 = -2;

impl From<String> for ClaimError {
    fn from(message: String) -> Self {
        ClaimError {
            code: -1,
            message,
            next_at: None,
        }
    }
}

fn claim_error(code: i64, body: &Value) -> ClaimError {
    let next_at = if code == 1005 {
        body.pointer("/data/plan/ends_at")
            .and_then(|x| x.as_i64())
            .map(|s| s * 1000)
    } else {
        None
    };
    ClaimError {
        code,
        message: failure_message(code, body),
        next_at,
    }
}

pub fn failure_payload(
    account_id: &str,
    account_name: &str,
    plan_name: &str,
    err: &ClaimError,
) -> Value {
    serde_json::json!({
        "ok": false,
        "accountId": account_id,
        "accountName": account_name,
        "planName": plan_name,
        "code": err.code,
        "nextAt": err.next_at,
        "terminal": err.is_terminal(),
        "challenge": err.is_challenge(),
        "message": err.message,
    })
}

fn claim_token(creds: &Value, config: Option<&Value>, secret: &str) -> Result<String, String> {
    let jwt = creds
        .get("zcodejwttoken")
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_credential(v, secret));
    if let Some(t) = jwt.filter(|t| t.trim().len() > 20) {
        return Ok(t);
    }
    if let Some(t) = quota::zai_billing_token(creds, config, secret) {
        return Ok(t);
    }
    Err(crate::i18n::tr("err.claim.no_jwt"))
}

fn decrypt_credential(v: &str, secret: &str) -> Option<String> {
    if zcrypto::is_encrypted(v) {
        zcrypto::decrypt_with_secret(v, secret).ok()
    } else {
        Some(v.to_string())
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(CLAIM_TIMEOUT_SECS))
        .build()
}

pub const EVENT_REPORT_URL: &str = "https://zcode.z.ai/api/v1/event/report";
pub const ACTIVATION_EVENTS: [&str; 2] = ["app_launch", "app_daily_active"];
const SCREEN_RESOLUTION: &str = "2560x1440";
const ACTIVATION_TIMEOUT_SECS: u64 = 10;

pub(crate) fn telemetry_user_id(home: &Path, creds: &Value) -> Option<String> {
    let secret = zcrypto::default_secret(home);
    let provider = creds
        .get("oauth:active_provider")
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_credential(v, &secret))
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "zai".to_string());
    let raw = creds
        .get(format!("oauth:{provider}:user_info"))?
        .as_str()?;
    let plain = decrypt_credential(raw, &secret)?;
    let info: Value = serde_json::from_str(&plain).ok()?;
    let pick = |k: &str| {
        info.get(k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    pick("id").or_else(|| pick("user_id")).map(String::from)
}

fn activation_event_body(element: &str, event_id: &str, user_id: &str, mid: &str) -> Value {
    serde_json::json!({
        "event_id": event_id,
        "client_timezone": quota::client_timezone(),
        "client_language": quota::ZCODE_LANG,
        "element_name": element,
        "event_region": "app",
        "event_type": "view",
        "event_text": "",
        "event_extra_detail": {},
        "user_id": user_id,
        "screen_resolution": SCREEN_RESOLUTION,
        "app_version": quota::zcode_app_version(),
        "device_os_category": device_os_category(),
        "device_os_version": quota::os_version().unwrap_or_default(),
        "device_mid": mid,
        "mac_id": "",
        "marketing_params": "{}",
    })
}

fn device_os_category() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

pub fn report_activation_events(user_id: &str, device_mid: &str) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(ACTIVATION_TIMEOUT_SECS))
        .build();
    for element in ACTIVATION_EVENTS {
        let body = activation_event_body(
            element,
            &uuid::Uuid::new_v4().to_string(),
            user_id,
            device_mid,
        );
        let resp = agent
            .post(EVENT_REPORT_URL)
            .set("Content-Type", "application/json")
            .send_json(body)
            .map_err(|e| {
                crate::i18n::trf("err.claim.activate_req", &[("e", http_err("", e).trim())])
            })?
            .into_string()
            .map_err(|e| crate::i18n::trf("err.claim.activate_req", &[("e", &e.to_string())]))?;
        let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
        let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(failure_message(code, &v));
        }
    }
    Ok(())
}

pub fn preview_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Vec<ClaimPlan>, String> {
    let secret = zcrypto::default_secret(home);
    let token = claim_token(creds, config, &secret)?;
    let url = format!(
        "{BILLING_PREVIEW_URL}?app_version={}&platform={}",
        quota::zcode_app_version(),
        quota::client_platform()
    );
    preview_once(&url, &token, device_mid)
}

fn http_err(prefix: &str, e: ureq::Error) -> String {
    if let ureq::Error::Status(code, resp) = e {
        let body = resp.into_string().unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| ["msg", "message", "error"]
                .iter()
                .find_map(|k| v.get(k).and_then(|x| x.as_str()).map(String::from)))
            .unwrap_or_default();
        return format!("{prefix} HTTP {code}: {msg}");
    }
    format!("{prefix} {e}")
}

/// claim 写请求的 HTTP 错误分类：401/403 若携带验证码挑战特征（响应头
/// x-aliyun-captcha-verify-param 或 body 含 captcha 字样），归类为「需过人机校验」
/// 而非凭证失效（zcode2api 实测教训：一次人机校验就把健康账号当登录过期会错杀）。
fn classify_claim_http_err(prefix: &str, e: ureq::Error) -> ClaimError {
    let status = match &e {
        ureq::Error::Status(s, _) => *s,
        _ => 0,
    };
    if status == 401 || status == 403 {
        if let ureq::Error::Status(_, resp) = e {
            let challenged = resp.header("x-aliyun-captcha-verify-param").is_some();
            let body = resp.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if challenged || body.to_lowercase().contains("captcha") {
                return ClaimError {
                    code: CLAIM_ERR_CHALLENGE,
                    message: crate::i18n::tr("claim.fail.challenge"),
                    next_at: None,
                };
            }
            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            if code > 0 {
                return claim_error(code, &v);
            }
            let msg = ["msg", "message", "error"]
                .iter()
                .find_map(|k| v.get(k).and_then(|x| x.as_str()).map(String::from))
                .unwrap_or_default();
            return ClaimError {
                code: -1,
                message: format!("{prefix} HTTP {status}: {msg}"),
                next_at: None,
            };
        }
    }
    ClaimError {
        code: -1,
        message: http_err(prefix, e),
        next_at: None,
    }
}

fn preview_once(url: &str, token: &str, mid: Option<String>) -> Result<Vec<ClaimPlan>, String> {
    let mut req = agent().get(url);
    for (k, v) in quota::zai_billing_headers_with_mid(token, mid) {
        req = req.set(&k, &v);
    }
    let resp = req
        .call()
        .map_err(|e| http_err(&crate::i18n::tr("err.claim.preview_req"), e))?
        .into_string()
        .map_err(|e| crate::i18n::trf("err.http.read", &[("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(failure_message(code, &v));
    }
    let plans = v
        .pointer("/data/plans")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<ClaimPlan> = plans.iter().filter_map(parse_plan).collect();
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    Ok(out)
}

/// 营销触达通道：新版活动（客户端弹窗/banner）只经 marketing/touch 下发，billing/preview 不返回。
/// deliveries 的按钮带可领 plan_id（action.type = claim_zcode_plan），payload 在 background/hero 的
/// args.zcode_plan 内嵌完整套餐结构（字段与 preview plans 同构）。任何失败静默降级为空列表并记本地
/// 日志，不影响 preview 结果。
pub fn marketing_touch_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Vec<ClaimPlan> {
    let fail = |detail: &str| -> Vec<ClaimPlan> {
        flowlog::log("claim", "marketing-fail", detail);
        Vec::new()
    };
    let secret = zcrypto::default_secret(home);
    let token = match claim_token(creds, config, &secret) {
        Ok(t) => t,
        Err(e) => return fail(&e),
    };
    let mut req = agent().get(MARKETING_TOUCH_URL);
    for (k, v) in quota::zai_billing_headers_with_mid(&token, device_mid) {
        req = req.set(&k, &v);
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(e) => return fail(&http_err("marketing", e)),
    };
    let resp = match resp.into_string() {
        Ok(s) => s,
        Err(e) => return fail(&crate::i18n::trf("err.http.read", &[("e", &e.to_string())])),
    };
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::Null);
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return fail(&format!("code {}", v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1)));
    }
    let mut payloads: Vec<ClaimPlan> = Vec::new();
    let mut button_ids: Vec<String> = Vec::new();
    for d in v
        .pointer("/data/deliveries")
        .and_then(|d| d.as_array())
        .map(|a| a.as_slice())
        .unwrap_or(&[])
    {
        walk_marketing_delivery(d, &mut payloads, &mut button_ids);
    }
    let missing: Vec<String> = button_ids
        .iter()
        .filter(|id| !payloads.iter().any(|p| &p.plan_id == *id))
        .cloned()
        .collect();
    for id in missing {
        payloads.push(ClaimPlan {
            plan_id: id.clone(),
            name: id,
            description: String::new(),
            priority: 0,
            grants: vec![],
            grant_items: vec![],
        });
    }
    let mut out: Vec<ClaimPlan> = payloads;
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    out.dedup_by(|a, b| a.plan_id == b.plan_id);
    out
}

fn walk_marketing_delivery(v: &Value, payloads: &mut Vec<ClaimPlan>, button_ids: &mut Vec<String>) {
    match v {
        Value::Array(items) => {
            for it in items {
                walk_marketing_delivery(it, payloads, button_ids);
            }
        }
        Value::Object(map) => {
            if let Some(action) = map.get("action").and_then(|a| a.as_object()) {
                let ty = action.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if ty == "claim_zcode_plan" || ty == "claim_plan" {
                    let id = action
                        .get("args")
                        .and_then(|a| a.get("plan_id"))
                        .and_then(|p| p.as_str())
                        .or_else(|| action.get("planId").and_then(|p| p.as_str()))
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    if let Some(id) = id {
                        button_ids.push(id.to_string());
                    }
                }
            }
            let is_plan_payload = map.get("plan_id").and_then(|p| p.as_str()).is_some_and(|s| !s.trim().is_empty())
                && (map.get("entitlements").and_then(|e| e.as_array()).is_some()
                    || map.get("name").and_then(|n| n.as_str()).is_some());
            if is_plan_payload {
                if let Some(plan) = parse_plan(v) {
                    payloads.push(plan);
                }
            }
            for val in map.values() {
                walk_marketing_delivery(val, payloads, button_ids);
            }
        }
        _ => {}
    }
}

/// 合并 marketing/touch 活动套餐到 preview 结果：按 plan_id 去重（preview 优先），priority 降序。
pub fn merge_marketing_plans(mut base: Vec<ClaimPlan>, extra: Vec<ClaimPlan>) -> Vec<ClaimPlan> {
    for p in extra {
        if !base.iter().any(|b| b.plan_id == p.plan_id) {
            base.push(p);
        }
    }
    base.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    base
}

/// 可领套餐统一查询入口：billing/preview（错误上抛，保持既有报错体验）
/// + marketing/touch 活动套餐（失败静默为空）。
pub fn collect_claimable_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: String,
) -> Result<Vec<ClaimPlan>, String> {
    let plans = preview_plans(home, creds, config, Some(device_mid.clone()))?;
    let extra = marketing_touch_plans(home, creds, config, Some(device_mid));
    Ok(merge_marketing_plans(plans, extra))
}

pub fn submit_claim(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    plan_id: &str,
    captcha_param: &str,
    captcha_region: Option<&str>,
    device_mid: Option<String>,
) -> Result<Value, ClaimError> {
    if captcha_param.trim().is_empty() {
        return Err(crate::i18n::tr("err.claim.no_captcha").into());
    }
    let secret = zcrypto::default_secret(home);
    let token = claim_token(creds, config, &secret)?;
    let mut req = agent().post(BILLING_CLAIM_URL);
    for (k, v) in quota::zai_billing_headers_with_mid(&token, device_mid) {
        req = req.set(&k, &v);
    }
    req = req.set("X-Aliyun-Captcha-Verify-Param", captcha_param.trim());
    if let Some(r) = captcha_region.filter(|r| !r.trim().is_empty()) {
        req = req.set("X-Aliyun-Captcha-Verify-Region", r.trim());
    }
    let resp = req
        .send_json(serde_json::json!({ "plan_id": plan_id }))
        .map_err(|e| classify_claim_http_err(&crate::i18n::tr("err.claim.claim_req"), e))?
        .into_string()
        .map_err(|e| ClaimError {
            code: -1,
            message: crate::i18n::trf("err.http.read", &[("e", &e.to_string())]),
            next_at: None,
        })?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(claim_error(code, &v));
    }
    Ok(v)
}

pub fn fetch_captcha_config() -> Result<CaptchaConfig, String> {
    // 缓存 600s（上游配置低频变化）；失败回退上次成功值，再无则内置默认，
    // 保证领取流程不因配置接口抖动而中断。
    static CACHE: std::sync::OnceLock<std::sync::Mutex<(std::time::Instant, Option<CaptchaConfig>)>> =
        std::sync::OnceLock::new();
    const CONFIG_TTL: std::time::Duration = std::time::Duration::from_secs(600);
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new((std::time::Instant::now() - CONFIG_TTL, None)));
    let mut slot = cache.lock().unwrap_or_else(|p| p.into_inner());
    if slot.1.is_some() && slot.0.elapsed() < CONFIG_TTL {
        return Ok(slot.1.clone().unwrap());
    }
    match fetch_captcha_config_remote() {
        Ok(cfg) => {
            *slot = (std::time::Instant::now(), Some(cfg.clone()));
            Ok(cfg)
        }
        Err(e) => {
            flowlog::log("claim", "captcha-config-fail", &e);
            if let Some(cfg) = &slot.1 {
                return Ok(cfg.clone());
            }
            let def = default_captcha_config();
            *slot = (std::time::Instant::now(), Some(def.clone()));
            Ok(def)
        }
    }
}

fn default_captcha_config() -> CaptchaConfig {
    // 来源：client/configs 长期观测值，zcode2api constants.py 与 plan-claim CLAIM-LOGIC §3.4 一致
    CaptchaConfig {
        enabled: true,
        region: "cn".into(),
        prefix: "no8xfe".into(),
        scene_id: "11xygtvd".into(),
    }
}

fn fetch_captcha_config_remote() -> Result<CaptchaConfig, String> {
    let mut req = agent().get(CLIENT_CONFIGS_URL);
    for (k, v) in quota::zai_billing_headers("") {
        req = req.set(&k, &v);
    }
    let resp = req
        .call()
        .map_err(|e| crate::i18n::trf("err.claim.config_req", &[("e", &e.to_string())]))?
        .into_string()
        .map_err(|e| crate::i18n::trf("err.http.read", &[("e", &e.to_string())]))?;
    let v: Value = serde_json::from_str(&resp).unwrap_or(Value::String(resp));
    if v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1) != 0 {
        return Err(crate::i18n::tr("err.claim.config_unavailable"));
    }
    let c = v.pointer("/data/configs/captcha").cloned().unwrap_or(Value::Null);
    let s = |k: &str| c.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    Ok(CaptchaConfig {
        enabled: c.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
        region: s("region"),
        prefix: s("prefix"),
        scene_id: s("sceneId"),
    })
}

fn str_field<'a>(e: &'a Value, snake: &str, camel: &str) -> Option<&'a str> {
    e.get(snake)
        .and_then(|v| v.as_str())
        .or_else(|| e.get(camel).and_then(|v| v.as_str()))
}

fn num_field(e: &Value, snake: &str, camel: &str) -> Option<f64> {
    e.get(snake)
        .and_then(|v| v.as_f64())
        .or_else(|| e.get(camel).and_then(|v| v.as_f64()))
}

fn parse_plan(p: &Value) -> Option<ClaimPlan> {
    let plan_id = str_field(p, "plan_id", "planId")?.trim().to_string();
    if plan_id.is_empty() {
        return None;
    }
    let grant_items: Vec<ClaimGrant> = p
        .get("entitlements")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|e| {
                    str_field(e, "meter", "meter") == Some("model_usage")
                        && str_field(e, "unit_type", "unitType") == Some("token")
                        && str_field(e, "show_name", "showName")
                            .map(|s| !s.trim().is_empty())
                            .unwrap_or(false)
                })
                .map(|e| ClaimGrant {
                    name: str_field(e, "show_name", "showName").unwrap_or("").to_string(),
                    units: num_field(e, "grant_units", "grantUnits").unwrap_or(0.0),
                    period: str_field(e, "period", "period").unwrap_or("one_time").to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let grants = grant_items
        .iter()
        .map(|g| {
            let period_cn = match g.period.as_str() {
                "daily" => "每日",
                "weekly" => "每周",
                "monthly" => "每月",
                _ => "一次性",
            };
            format!("{} · {} Token（{period_cn}）", g.name, fmt_units(g.units))
        })
        .collect();
    Some(ClaimPlan {
        name: p.get("name").and_then(|s| s.as_str()).unwrap_or("").trim().to_string(),
        description: p
            .get("description")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string(),
        priority: p.get("priority").and_then(|x| x.as_i64()).unwrap_or(0),
        plan_id,
        grants,
        grant_items,
    })
}

fn fmt_units(n: f64) -> String {
    let trim = |x: f64| {
        let r = (x * 10.0).round() / 10.0;
        if (r - r.trunc()).abs() < f64::EPSILON {
            format!("{}", r.trunc() as i64)
        } else {
            format!("{r:.1}")
        }
    };
    if n >= 1e8 {
        format!("{}亿", trim(n / 1e8))
    } else if n >= 1e4 {
        format!("{}万", trim(n / 1e4))
    } else {
        format!("{}", n.round() as i64)
    }
}

pub fn failure_message(code: i64, body: &Value) -> String {
    let server_msg = ["msg", "message"]
        .iter()
        .find_map(|k| body.get(k).and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_default();
    let base = crate::i18n::tr(match code {
        1001 => "claim.fail.1001",
        1002 => "claim.fail.1002",
        1003 => "claim.fail.1003",
        1004 => "claim.fail.1004",
        1005 => "claim.fail.1005",
        3001 => "claim.fail.3001",
        3007 => "claim.fail.3007",
        401 => "claim.fail.401",
        _ => "claim.fail.generic",
    });
    if server_msg.is_empty() {
        base
    } else {
        crate::i18n::trf("claim.fail.with_server", &[("base", &base), ("server_msg", &server_msg)])
    }
}
