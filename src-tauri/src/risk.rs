
use crate::store::{atomic_write, Paths};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// 上游风控（3012 / unusual activity）退避状态机，参数取自 zcode2api 实测结论：
/// 3012 是频道级瞬时频控，数小时自愈——不能硬禁用，用指数退避 + 7 天窗口衰减。
const STRIKE_WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;
const COOLING_BASE_MS: i64 = 900 * 1000;
const COOLING_CAP_MS: i64 = 24 * 3600 * 1000;
const STRIKE_ESCALATE: usize = 4;

fn risk_file(paths: &Paths) -> PathBuf {
    paths.store_dir().join("risk-state.json")
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

fn read_state(paths: &Paths) -> BTreeMap<String, Value> {
    fs::read_to_string(risk_file(paths))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
        .unwrap_or_default()
}

fn write_state(paths: &Paths, state: &BTreeMap<String, Value>) {
    let mut ordered = Map::new();
    for (k, v) in state {
        ordered.insert(k.clone(), v.clone());
    }
    let _ = atomic_write(&risk_file(paths), &Value::Object(ordered).to_string());
}

fn entry_strikes(entry: &Value, now: i64) -> Vec<i64> {
    entry
        .get("strikes")
        .and_then(|s| s.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).filter(|t| now - *t <= STRIKE_WINDOW_MS).collect())
        .unwrap_or_default()
}

fn entry_cooling_until(entry: &Value) -> i64 {
    entry.get("coolingUntil").and_then(|x| x.as_i64()).unwrap_or(0)
}

/// 账号当前是否处于风控冷却中。
pub(crate) fn is_cooling(paths: &Paths, account_id: &str) -> bool {
    read_state(paths)
        .get(account_id)
        .map(|e| entry_cooling_until(e) > now_ms())
        .unwrap_or(false)
}

/// 冷却剩余秒数（未冷却为 0），供提示文案使用。
pub(crate) fn cooling_remaining_secs(paths: &Paths, account_id: &str) -> i64 {
    read_state(paths)
        .get(account_id)
        .map(|e| (entry_cooling_until(e) - now_ms()).max(0) / 1000)
        .unwrap_or(0)
}

/// 记录一次风控命中：7 天窗口内累计，指数退避（900s × 2ⁿ⁻¹ 封顶 24h），
/// 窗口内累计 4 次升级为 24h 长冷却。冷却到期自动恢复（无需人工介入）。
pub(crate) fn record_strike(paths: &Paths, account_id: &str) {
    let mut state = read_state(paths);
    let now = now_ms();
    let entry = state.get(account_id).cloned().unwrap_or(Value::Null);
    let mut strikes = entry_strikes(&entry, now);
    strikes.push(now);
    let count = strikes.len();
    let cooling = if count >= STRIKE_ESCALATE {
        COOLING_CAP_MS
    } else {
        (COOLING_BASE_MS << (count - 1).min(16)).min(COOLING_CAP_MS)
    };
    state.insert(
        account_id.to_string(),
        json!({ "strikes": strikes, "coolingUntil": now + cooling }),
    );
    write_state(paths, &state);
    crate::flowlog::log(
        "risk",
        "strike",
        &format!("account={account_id} count={count} cooling={}s", cooling / 1000),
    );
}

/// 上游错误消息观测：仅命中风控特征才记 strike（避免网络抖动误伤）。
pub(crate) fn observe_upstream_failure(paths: &Paths, account_id: &str, message: &str) {
    let l = message.to_lowercase();
    let hit = message.contains("3012")
        || l.contains("unusual activity")
        || message.contains("风控")
        || l.contains("risk control");
    if hit {
        record_strike(paths, account_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths(tag: &str) -> (tempdir::TempDir, Paths) {
        let dir = tempdir::TempDir::with_tag(tag);
        let home = dir.path().to_path_buf();
        std::env::set_var("ZCODE_SWITCH_HOME", &home);
        let paths = Paths::detect();
        let _ = fs::remove_file(risk_file(&paths));
        (dir, paths)
    }

    // tempdir helper：见 tests 通用说明；此处用系统临时目录
    pub(crate) mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        pub struct TempDir(pub PathBuf);
        impl TempDir {
            pub fn with_tag(tag: &str) -> Self {
                let n = SEQ.fetch_add(1, Ordering::SeqCst);
                let p = std::env::temp_dir().join(format!("zsw-risk-{tag}-{}-{n}", std::process::id()));
                let _ = std::fs::remove_dir_all(&p);
                std::fs::create_dir_all(&p).unwrap();
                TempDir(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn strike_backoff_escalates_and_decays() {
        let (_d, paths) = temp_paths("escalate");
        let acct = "a1";
        assert!(!is_cooling(&paths, acct));
        record_strike(&paths, acct);
        let r1 = cooling_remaining_secs(&paths, acct);
        assert!(r1 > 800 && r1 <= 900, "first strike ~900s, got {r1}");
        record_strike(&paths, acct);
        let r2 = cooling_remaining_secs(&paths, acct);
        assert!(r2 > 1600 && r2 <= 1800, "second strike ~1800s, got {r2}");
        // 第 4 次 → 升级 24h
        record_strike(&paths, acct);
        record_strike(&paths, acct);
        let r4 = cooling_remaining_secs(&paths, acct);
        assert!(r4 > 23 * 3600, "fourth strike escalates to 24h, got {r4}");
    }

    #[test]
    fn observe_filters_non_risk_messages() {
        let (_d, paths) = temp_paths("filter");
        observe_upstream_failure(&paths, "a2", "HTTP 500: internal error");
        assert!(!is_cooling(&paths, "a2"));
        observe_upstream_failure(&paths, "a2", "code 3012 unusual activity");
        assert!(is_cooling(&paths, "a2"));
    }

    #[test]
    fn cooling_expires_and_window_decays() {
        let (_d, paths) = temp_paths("decay");
        let acct = "a3";
        record_strike(&paths, acct);
        // 手工把状态改老：strike 与 cooling 都推到 8 天前
        let mut state = read_state(&paths);
        let old = now_ms() - 8 * 24 * 3600 * 1000;
        state.insert(acct.to_string(), json!({ "strikes": [old], "coolingUntil": old }));
        write_state(&paths, &state);
        assert!(!is_cooling(&paths, acct), "expired cooling must recover");
        // 窗口外的旧 strike 不再累计：再次命中应从 900s 重新起算
        record_strike(&paths, acct);
        let r = cooling_remaining_secs(&paths, acct);
        assert!(r > 800 && r <= 900, "old strikes decayed out of window, got {r}");
    }
}
