//! 活动诊断导出。
//!
//! 只包含版本、通道状态、条数与脱敏后的日志，**不含任何令牌/完整设备号/完整账号 id**，
//! 用户可直接把导出的 JSON 发给维护者排查「为什么这个账号刷不出活动」。

use serde_json::{json, Value};

const MAX_FLOWLOG_LINES: usize = 300;
const MAX_REFRESH_LINES: usize = 100;
const MAX_BYTES: usize = 512 * 1024;

/// 生成诊断 JSON 文本（pretty）。`account` 传 id 或名称时只导出该账号。
pub fn build_text(paths: &crate::store::Paths, account: Option<&str>) -> String {
    let version = crate::changelog::version_status(&paths.home);
    let accounts = crate::store::list_accounts(paths).unwrap_or_default();
    let items: Vec<Value> = accounts
        .iter()
        .filter(|a| account.map_or(true, |x| x == a.id || x == a.name))
        .map(|a| {
            let history = crate::store::claim_history_for(paths, &a.id, None).len();
            json!({
                "id": mask_id(&a.id),
                "name": a.name,
                "history": history,
            })
        })
        .collect();

    let log = crate::flowlog::tail(MAX_FLOWLOG_LINES);
    let mut refresh: Vec<String> = log
        .iter()
        .filter(|l| l.contains("[claim]") && (l.contains("] refresh ") || l.contains("] probe ")))
        .cloned()
        .collect();
    if refresh.len() > MAX_REFRESH_LINES {
        refresh = refresh.split_off(refresh.len() - MAX_REFRESH_LINES);
    }

    let mut doc = json!({
        "schema": 1,
        "notice": "仅包含版本、通道状态与计数，不含令牌；可公开分享。",
        "generatedAt": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "app": {
            "version": env!("CARGO_PKG_VERSION"),
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "language": crate::i18n::current().as_str(),
        },
        "version": version,
        "accounts": items,
        "recentRefresh": refresh,
        "flowlogTail": log,
        "truncated": false,
    });
    scrub_value(&mut doc);

    let mut out = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
    if out.len() > MAX_BYTES {
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("flowlogTail".into(), json!([]));
            obj.insert("truncated".into(), json!(true));
        }
        out = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
    }
    out
}

/// 一行刷新摘要（写入 flowlog，格式固定便于两端对比）。
pub fn refresh_line(
    account_id: &str,
    version: &str,
    token: &str,
    diag: &crate::claim::ClaimDiag,
    plans: usize,
) -> String {
    let fmt = |m: Option<&crate::claim::HttpMeta>| match m {
        Some(h) => format!("{}/{}/{}", h.status, h.code, h.count),
        None => "-".to_string(),
    };
    let transport = diag
        .preview
        .as_ref()
        .map(|h| h.transport.clone())
        .or_else(|| diag.marketing.as_ref().map(|h| h.transport.clone()))
        .unwrap_or_default();
    format!(
        "acct={} ver={} token={} preview={} marketing={} transport={} ms={} plans={}",
        mask_id(account_id),
        version,
        token,
        fmt(diag.preview.as_ref()),
        fmt(diag.marketing.as_ref()),
        transport,
        diag.elapsed_ms,
        plans
    )
}

/// 账号 id 只保留前 8 位。
pub fn mask_id(id: &str) -> String {
    let s = id.trim();
    if s.len() <= 8 {
        return format!("{s}…");
    }
    format!("{}…", &s[..8])
}

fn scrub_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = mask_secrets(s),
        Value::Array(a) => a.iter_mut().for_each(scrub_value),
        Value::Object(o) => o.values_mut().for_each(scrub_value),
        _ => {}
    }
}

/// 兜底脱敏：`Bearer <token>` 与 32 位以上的 hex/uuid 打码。
/// 采集阶段本就不写令牌，这里是防「别的模块误写」的最后一道。
pub fn mask_secrets(line: &str) -> String {
    let mut s = line.to_string();
    if let Some(i) = s.find("Bearer ") {
        let start = i + "Bearer ".len();
        let end = s[start..]
            .find(|c: char| c.is_whitespace() || c == '"')
            .map(|x| start + x)
            .unwrap_or(s.len());
        if end > start {
            s.replace_range(start..end, "***");
        }
    }

    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    for ch in s.chars() {
        if ch.is_whitespace() || ch == '"' || ch == ',' || ch == '{' || ch == '}' {
            if !word.is_empty() {
                out.push_str(&mask_word(&word));
                word.clear();
            }
            out.push(ch);
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        out.push_str(&mask_word(&word));
    }
    out
}

fn mask_word(w: &str) -> String {
    let looks_like_id = w.len() >= 32 && w.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if looks_like_id {
        format!("{}…", &w[..8])
    } else {
        w.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_hides_tokens_and_long_ids() {
        let raw = r#"{"auth":"Bearer eyJhbGciOiJIUzI1NiJ9.abc.def","mid":"8dba338d-9a3c-4be7-8c3e-4e6c4bfbf6e9"}"#;
        let s = mask_secrets(raw);
        assert!(!s.contains("eyJhbGciOiJIUzI1NiJ9"), "token 必须打码: {s}");
        assert!(!s.contains("8dba338d-9a3c-4be7-8c3e-4e6c4bfbf6e9"), "长 id 必须打码: {s}");
        assert!(s.contains("Bearer ***"));
        assert!(s.contains("8dba338d…"));
    }

    #[test]
    fn mask_keeps_normal_text() {
        assert_eq!(mask_id("4b4d0629-1b4a-424d-94ba-e9b522939db9"), "4b4d0629…");
        assert_eq!(mask_id("abc"), "abc…");
        assert_eq!(mask_secrets("preview=200/0/1"), "preview=200/0/1");
    }
}
