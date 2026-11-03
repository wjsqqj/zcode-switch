
use crate::flowlog;
use crate::quota;
use crate::zcrypto;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

/// 上游基址：测试经 ZCODE_API_BASE 旁路指向本地 mock（默认官方）。
pub const API_BASE_DEFAULT: &str = "https://zcode.z.ai";

pub(crate) fn api_base() -> String {
    std::env::var("ZCODE_API_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| API_BASE_DEFAULT.to_string())
}

pub(crate) fn billing_preview_url() -> String {
    format!("{}/api/v1/zcode-plan/billing/preview", api_base())
}
pub(crate) fn billing_current_url() -> String {
    format!("{}/api/v1/zcode-plan/billing/current", api_base())
}
pub(crate) fn billing_claim_url() -> String {
    format!("{}/api/v1/zcode-plan/billing/claim", api_base())
}
pub(crate) fn client_configs_url() -> String {
    format!("{}/api/v1/client/configs", api_base())
}
pub(crate) fn marketing_touch_url() -> String {
    format!("{}/api/v1/marketing/touch?seq=0", api_base())
}

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
    /// 权益真实生效时间（ms）：`entitlements[].effective_at` 最小值；0 = 立即生效。
    /// `starts_at` 只是活动开始，**不能**当生效时间用；上游未下发时为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<i64>,
    /// 活动元数据（仅 marketing/touch 通道携带；preview 通道为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<ActivityCampaign>,
}

/// 活动按钮动作（如 `claim_zcode_plan` + plan_id）。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityAction {
    pub kind: String,
    pub plan_id: Option<String>,
}

/// marketing/touch delivery 上的活动元数据，供 UI 详情弹框使用。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCampaign {
    pub campaign_id: String,
    /// `banner` | `popup` | 空
    pub position: String,
    pub title: String,
    pub body: String,
    pub buttons: Vec<ActivityAction>,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
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
    /// 权益真实生效时间（ms）：`entitlements[].effective_at` 最小值；0 = 立即生效。
    pub effective_at: Option<i64>,
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

/// 诊断用：当前账号实际会用哪种 token 调上游（**只返回枚举，不泄露 token**）。
pub(crate) fn token_source(creds: &Value, config: Option<&Value>, secret: &str) -> &'static str {
    let jwt = creds
        .get("zcodejwttoken")
        .and_then(|v| v.as_str())
        .and_then(|v| decrypt_credential(v, secret));
    if jwt.filter(|t| t.trim().len() > 20).is_some() {
        return "zcodejwt";
    }
    if quota::zai_billing_token(creds, config, secret).is_some() {
        return "oauth";
    }
    "none"
}

fn decrypt_credential(v: &str, secret: &str) -> Option<String> {
    if zcrypto::is_encrypted(v) {
        zcrypto::decrypt_with_secret(v, secret).ok()
    } else {
        Some(v.to_string())
    }
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
    let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    for element in ACTIVATION_EVENTS {
        let body = activation_event_body(
            element,
            &uuid::Uuid::new_v4().to_string(),
            user_id,
            device_mid,
        );
        let payload = serde_json::to_string(&body).unwrap_or_default();
        let resp = quota::zcode_http(
            EVENT_REPORT_URL,
            "POST",
            &headers,
            Some(&payload),
            ACTIVATION_TIMEOUT_SECS,
        )
        .map_err(|e| crate::i18n::trf("err.claim.activate_req", &[("e", &e)]))?;
        if !resp.ok() {
            return Err(quota::http_status_error(
                &crate::i18n::tr("err.claim.activate_req"),
                &resp,
            ));
        }
        let v: Value = quota::zcode_http_json(&resp)?;
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
        "{}?app_version={}&platform={}",
        billing_preview_url(),
        quota::zcode_app_version(),
        quota::client_platform()
    );
    preview_once(&url, &token, device_mid)
}

fn preview_once(url: &str, token: &str, mid: Option<String>) -> Result<Vec<ClaimPlan>, String> {
    let headers = quota::zai_billing_headers_with_mid(token, mid);
    let resp = quota::zcode_http(url, "GET", &headers, None, CLAIM_TIMEOUT_SECS)
        .map_err(|e| crate::i18n::trf("err.claim.preview_req", &[("e", &e)]))?;
    // 活动未上线时上游返回 404（zcode2api 2026-10-29 官方客户端 mitm 实测）：
    // 属正常态，静默为空——否则「今天还没投放」会被渲染成「刷新失败」。
    if resp.status == 404 {
        return Ok(Vec::new());
    }
    if !resp.ok() {
        return Err(quota::http_status_error(&crate::i18n::tr("err.claim.preview_req"), &resp));
    }
    let v: Value = quota::zcode_http_json(&resp)?;
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

/// 诊断用：原样返回上游 JSON（不解析、不降级）。用于排查「某账号为什么看不到活动 /
/// 生效时间到底取哪个字段」。任何非 2xx 都以错误返回，便于看清 HTTP 码。
fn fetch_raw_json(url: &str, token: &str, mid: Option<String>, prefix: &str) -> Result<Value, String> {
    let headers = quota::zai_billing_headers_with_mid(token, mid);
    let resp = quota::zcode_http(url, "GET", &headers, None, CLAIM_TIMEOUT_SECS)?;
    if !resp.ok() {
        return Err(quota::http_status_error(prefix, &resp));
    }
    Ok(quota::zcode_http_json(&resp)
        .unwrap_or_else(|_| Value::String(resp.body.clone())))
}

fn raw_token(home: &Path, creds: &Value, config: Option<&Value>) -> Result<String, String> {
    claim_token(creds, config, &zcrypto::default_secret(home))
}

/// 原始 `billing/preview` 响应。
pub fn raw_preview(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Value, String> {
    let token = raw_token(home, creds, config)?;
    let url = format!(
        "{}?app_version={}&platform={}",
        billing_preview_url(),
        quota::zcode_app_version(),
        quota::client_platform()
    );
    fetch_raw_json(&url, &token, device_mid, "preview")
}

/// 原始 `marketing/touch` 响应（活动下发通道）。
pub fn raw_marketing(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Value, String> {
    let token = raw_token(home, creds, config)?;
    fetch_raw_json(&marketing_touch_url(), &token, device_mid, "marketing")
}

/// 原始 `billing/current` 响应（已生效/待生效套餐，含 starts_at / status）。
pub fn raw_current(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Value, String> {
    let token = raw_token(home, creds, config)?;
    let url = format!(
        "{}?app_version={}&platform={}",
        billing_current_url(),
        quota::zcode_app_version(),
        quota::client_platform()
    );
    fetch_raw_json(&url, &token, device_mid, "current")
}

/// 活动探测专用入口：**只**走 `marketing/touch`。
/// 不领取、不写领取历史、不发 activation 事件——探测是只读行为，必须与领取链路口径隔离。
pub fn probe_marketing_only(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Vec<ClaimPlan> {
    marketing_touch_plans(home, creds, config, device_mid)
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
    let headers = quota::zai_billing_headers_with_mid(&token, device_mid);
    let resp = match quota::zcode_http(
        &marketing_touch_url(),
        "GET",
        &headers,
        None,
        CLAIM_TIMEOUT_SECS,
    ) {
        Ok(r) => r,
        Err(e) => return fail(&format!("marketing: {e}")),
    };
    if !resp.ok() {
        return fail(&quota::http_status_error("marketing", &resp));
    }
    let v: Value = match quota::zcode_http_json(&resp) {
        Ok(v) => v,
        Err(e) => return fail(&e),
    };
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
        let before = payloads.len();
        walk_marketing_delivery(d, &mut payloads, &mut button_ids);
        // 同一个 delivery 内扫出的 plan 共享该 delivery 的活动元数据（来源一致才可靠）
        let meta = campaign_meta_from_delivery(d);
        for p in payloads.iter_mut().skip(before) {
            if p.campaign.is_none() {
                p.campaign = Some(meta.clone());
            }
        }
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
            effective_at: None,
            campaign: None,
        });
    }
    let mut out: Vec<ClaimPlan> = payloads;
    out.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    out.dedup_by(|a, b| a.plan_id == b.plan_id);
    out
}

fn collect_strings(v: &Value, keys: &[&str], out: &mut Vec<String>) {
    match v {
        Value::Array(items) => items.iter().for_each(|it| collect_strings(it, keys, out)),
        Value::Object(map) => {
            for (k, val) in map {
                if keys.contains(&k.as_str()) {
                    if let Some(s) = val.as_str() {
                        let s = s.trim();
                        if !s.is_empty() && s.len() < 400 && !out.iter().any(|x| x == s) {
                            out.push(s.to_string());
                        }
                    }
                }
                collect_strings(val, keys, out);
            }
        }
        _ => {}
    }
}

fn collect_buttons(v: &Value, out: &mut Vec<ActivityAction>) {
    match v {
        Value::Array(items) => items.iter().for_each(|it| collect_buttons(it, out)),
        Value::Object(map) => {
            if let Some(action) = map.get("action").and_then(|a| a.as_object()) {
                let kind = action.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
                if !kind.is_empty() {
                    let plan_id = action
                        .get("args")
                        .and_then(|a| a.get("plan_id"))
                        .and_then(|p| p.as_str())
                        .or_else(|| action.get("planId").and_then(|p| p.as_str()))
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    out.push(ActivityAction { kind, plan_id });
                }
            }
            for val in map.values() {
                collect_buttons(val, out);
            }
        }
        _ => {}
    }
}

/// 服务端文案既可能是纯字符串，也可能是 `{content, format}` 结构（format=plaintext|html）。
fn text_of(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        let t = s.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
        return None;
    }
    v.get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 文案里夹带 HTML（如 `<p class=...>`）时，UI 只展示纯文本。
fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn first_text(v: &Value, paths: &[&str]) -> String {
    for p in paths {
        if let Some(t) = v.pointer(p).and_then(text_of) {
            return t;
        }
    }
    String::new()
}

/// 从一条 delivery 里宽松抽取活动元数据（字段位置随服务端版本变化，不做严格 schema）。
/// 文案优先取 `success_popup.{title,description}`（实测有效），时间窗兜底取内嵌 `zcode_plan`。
fn campaign_meta_from_delivery(d: &Value) -> ActivityCampaign {
    let mut titles = Vec::new();
    collect_strings(d, &["title", "heading", "headline"], &mut titles);
    let mut bodies = Vec::new();
    collect_strings(d, &["text", "subtitle", "description", "content", "body"], &mut bodies);
    let mut buttons = Vec::new();
    collect_buttons(d, &mut buttons);

    let title = {
        let t = first_text(
            d,
            &["/banner/success_popup/title", "/popup/title", "/banner/title", "/title"],
        );
        if t.is_empty() { titles.into_iter().next().unwrap_or_default() } else { t }
    };
    let body = {
        let b = strip_html(&first_text(
            d,
            &[
                "/banner/success_popup/description",
                "/popup/description",
                "/banner/description",
                "/description",
                "/banner/text",
                "/text",
            ],
        ));
        if b.is_empty() {
            strip_html(&bodies.into_iter().next().unwrap_or_default())
        } else {
            b
        }
    };
    let sec = |k: &str| {
        let paths = [
            format!("/{k}"),
            format!("/banner/background/args/zcode_plan/{k}"),
            format!("/popup/hero/args/zcode_plan/{k}"),
            format!("/background/args/zcode_plan/{k}"),
        ];
        paths
            .iter()
            .find_map(|p| d.pointer(p).and_then(|x| x.as_i64()))
            .map(|s| s * 1000)
    };

    ActivityCampaign {
        campaign_id: str_field(d, "campaign_id", "campaignId").unwrap_or("").trim().to_string(),
        position: str_field(d, "resource_position", "resourcePosition")
            .unwrap_or("")
            .trim()
            .to_string(),
        title,
        body,
        buttons,
        starts_at: sec("starts_at"),
        ends_at: sec("ends_at"),
    }
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
        if let Some(b) = base.iter_mut().find(|b| b.plan_id == p.plan_id) {
            // preview 优先，但 preview 的 plan 没有活动元数据（campaign_id/展示位/文案/时间窗）。
            // 同一 plan_id 在 marketing 通道里带着元数据时，必须把它补到 preview 那份上，
            // 否则前端永远显示「活动 ID 未提供」。
            if b.campaign.is_none() {
                b.campaign = p.campaign;
            }
            if b.description.is_empty() {
                b.description = p.description;
            }
        } else {
            base.push(p);
        }
    }
    base.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.plan_id.cmp(&b.plan_id)));
    base
}

/// 已生效套餐（billing/current）：区分「无活动」与「已领取生效中」。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActivePlan {
    pub user_plan_id: String,
    pub plan_id: String,
    pub name: String,
    pub status: String,
    pub starts_at: Option<i64>,
    pub ends_at: Option<i64>,
    /// 真实生效时间（ms）：取 `entitlements[].effective_at` 的最小值（秒×1000）。
    /// `plan.starts_at` 只是活动/计划开始，不等于权益生效；0 表示立即生效。
    pub effective_at: Option<i64>,
    pub grant_items: Vec<ClaimGrant>,
}

pub fn fetch_active_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: Option<String>,
) -> Result<Vec<ActivePlan>, String> {
    let secret = zcrypto::default_secret(home);
    let token = claim_token(creds, config, &secret)?;
    let url = format!(
        "{}?app_version={}&platform={}",
        billing_current_url(),
        quota::zcode_app_version(),
        quota::client_platform()
    );
    let headers = quota::zai_billing_headers_with_mid(&token, device_mid);
    let resp = quota::zcode_http(&url, "GET", &headers, None, CLAIM_TIMEOUT_SECS)
        .map_err(|e| crate::i18n::trf("err.claim.preview_req", &[("e", &e)]))?;
    if !resp.ok() {
        return Err(quota::http_status_error(&crate::i18n::tr("err.claim.preview_req"), &resp));
    }
    let v: Value = quota::zcode_http_json(&resp)?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(failure_message(code, &v));
    }
    let plans = v
        .pointer("/data/plans")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<ActivePlan> = plans
        .iter()
        .filter_map(|p| {
            let plan_id = str_field(p, "plan_id", "planId")?.trim().to_string();
            if plan_id.is_empty() {
                return None;
            }
            let grant_items = parse_entitlements(p);
            Some(ActivePlan {
                user_plan_id: str_field(p, "user_plan_id", "userPlanId").unwrap_or("").to_string(),
                plan_id,
                name: p.get("name").and_then(|s| s.as_str()).unwrap_or("").trim().to_string(),
                status: str_field(p, "status", "status").unwrap_or("").to_string(),
                starts_at: p.get("starts_at").and_then(|x| x.as_i64()).map(|s| s * 1000),
                ends_at: p.get("ends_at").and_then(|x| x.as_i64()).map(|s| s * 1000),
                // 权益真正生效的时刻在 entitlement 上（0 = 立即生效），plan.starts_at 不是它
                effective_at: entitlements_effective_at(p),
                grant_items,
            })
        })
        .collect();
    out.sort_by(|a, b| b.ends_at.cmp(&a.ends_at).then(a.plan_id.cmp(&b.plan_id)));
    Ok(out)
}

/// 本次刷新的通道诊断：用于区分「服务端明确返回空」与「我们没拿到」。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimDiag {
    pub preview_empty: bool,
    pub marketing_empty: bool,
    /// preview 通道本身的错误（marketing 兜底成功时仍然记录，供 UI 解释降级）
    pub preview_error: Option<String>,
}

/// 可领套餐统一查询入口：billing/preview（错误上抛，保持既有报错体验）
/// + marketing/touch 活动套餐（失败静默为空）。
pub fn collect_claimable_plans(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: String,
) -> Result<Vec<ClaimPlan>, String> {
    collect_claimable_plans_diag(home, creds, config, device_mid).map(|(plans, _)| plans)
}

/// 同上，但额外返回通道诊断（`claim_refresh` 用它解释空态）。
pub fn collect_claimable_plans_diag(
    home: &Path,
    creds: &Value,
    config: Option<&Value>,
    device_mid: String,
) -> Result<(Vec<ClaimPlan>, ClaimDiag), String> {
    // 双通道互为兜底：preview 失败不立即整单失败，先看 marketing 有没有货
    let (plans, preview_error) = match preview_plans(home, creds, config, Some(device_mid.clone())) {
        Ok(p) => (p, None),
        Err(e) => {
            flowlog::log("claim", "preview-fail", &e);
            (Vec::new(), Some(e))
        }
    };
    let extra = marketing_touch_plans(home, creds, config, Some(device_mid));
    // 两通道都没货且 preview 有错 → 保留既有报错体验（不吞真实故障）
    if plans.is_empty() && extra.is_empty() {
        if let Some(e) = &preview_error {
            return Err(e.clone());
        }
    }
    let diag = ClaimDiag {
        preview_empty: plans.is_empty(),
        marketing_empty: extra.is_empty(),
        preview_error,
    };
    Ok((merge_marketing_plans(plans, extra), diag))
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
    let mut headers = quota::zai_billing_headers_with_mid(&token, device_mid);
    headers.push(("X-Aliyun-Captcha-Verify-Param".into(), captcha_param.trim().to_string()));
    if let Some(r) = captcha_region.filter(|r| !r.trim().is_empty()) {
        headers.push(("X-Aliyun-Captcha-Verify-Region".into(), r.trim().to_string()));
    }
    let body = serde_json::json!({ "plan_id": plan_id }).to_string();
    let resp = quota::zcode_http(
        &billing_claim_url(),
        "POST",
        &headers,
        Some(&body),
        CLAIM_TIMEOUT_SECS,
    )
    .map_err(|e| ClaimError {
        code: -1,
        message: e,
        next_at: None,
    })?;
    if !resp.ok() {
        return Err(claim_error_from_resp(&crate::i18n::tr("err.claim.claim_req"), &resp));
    }
    let v: Value = quota::zcode_http_json(&resp).map_err(|e| ClaimError {
        code: -1,
        message: e,
        next_at: None,
    })?;
    let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
    if code != 0 {
        return Err(claim_error(code, &v));
    }
    Ok(v)
}

/// HTTP 非 2xx 的领取错误分类（等价于原 `classify_claim_http_err`，改为吃 HttpResp，
/// 这样 curl/ureq 两条通道的响应头都能参与「人机校验」判定）。
fn claim_error_from_resp(prefix: &str, resp: &quota::HttpResp) -> ClaimError {
    if resp.status == 401 || resp.status == 403 {
        let challenged = resp.header("x-aliyun-captcha-verify-param").is_some();
        if challenged || resp.body.to_lowercase().contains("captcha") {
            return ClaimError {
                code: CLAIM_ERR_CHALLENGE,
                message: crate::i18n::tr("claim.fail.challenge"),
                next_at: None,
            };
        }
        if let Ok(v) = serde_json::from_str::<Value>(&resp.body) {
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
                message: format!("{prefix} HTTP {}: {msg}", resp.status),
                next_at: None,
            };
        }
    }
    ClaimError {
        code: -1,
        message: quota::http_status_error(prefix, resp),
        next_at: None,
    }
}

static CAPTCHA_CONFIG_CACHE: std::sync::OnceLock<std::sync::Mutex<(std::time::Instant, Option<CaptchaConfig>)>> =
    std::sync::OnceLock::new();
const CONFIG_TTL: std::time::Duration = std::time::Duration::from_secs(600);

pub fn fetch_captcha_config() -> Result<CaptchaConfig, String> {
    // 缓存 600s（上游配置低频变化）；失败回退上次成功值，再无则内置默认，
    // 保证领取流程不因配置接口抖动而中断。
    let cache = CAPTCHA_CONFIG_CACHE.get_or_init(|| std::sync::Mutex::new((std::time::Instant::now() - CONFIG_TTL, None)));
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

/// 仅供测试：清空验证码配置缓存（否则 600s TTL 会污染相邻用例）。
#[doc(hidden)]
pub fn reset_captcha_config_cache_for_tests() {
    if let Some(m) = CAPTCHA_CONFIG_CACHE.get() {
        let mut slot = m.lock().unwrap_or_else(|p| p.into_inner());
        *slot = (std::time::Instant::now() - CONFIG_TTL, None);
    }
}

fn fetch_captcha_config_remote() -> Result<CaptchaConfig, String> {
    let headers = quota::zai_billing_headers("");
    let resp = quota::zcode_http(&client_configs_url(), "GET", &headers, None, CLAIM_TIMEOUT_SECS)
        .map_err(|e| crate::i18n::trf("err.claim.config_req", &[("e", &e)]))?;
    if !resp.ok() {
        return Err(quota::http_status_error("captcha-config", &resp));
    }
    let v: Value = quota::zcode_http_json(&resp)?;
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

fn parse_entitlements(p: &Value) -> Vec<ClaimGrant> {
    p.get("entitlements")
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
        .unwrap_or_default()
}

/// 权益真实生效时间（ms）：`entitlements[].effective_at` 最小值，0 = 立即生效。
/// 必须独立于 `parse_entitlements`（后者按 meter/unit_type/show_name 过滤，只用于额度展示）。
/// `starts_at` 是活动开始时间，不参与这里的取值。
pub(crate) fn entitlements_effective_at(p: &Value) -> Option<i64> {
    p.get("entitlements")
        .and_then(|e| e.as_array())
        .and_then(|arr| {
            arr.iter()
                .filter_map(|it| {
                    it.get("effective_at")
                        .and_then(|x| x.as_i64())
                        .or_else(|| it.get("effectiveAt").and_then(|x| x.as_i64()))
                })
                .min()
        })
        .map(|s| s * 1000)
}

fn parse_plan(p: &Value) -> Option<ClaimPlan> {
    let plan_id = str_field(p, "plan_id", "planId")?.trim().to_string();
    if plan_id.is_empty() {
        return None;
    }
    let grant_items = parse_entitlements(p);
    let effective_at = entitlements_effective_at(p);
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
        effective_at,
        campaign: None,
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
