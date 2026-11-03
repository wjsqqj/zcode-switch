
use crate::flowlog;
use serde::Serialize;
use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 官方更新日志页：SSR 输出，初始 HTML 即含版本号（2026-10-24 实测，无需执行 JS）。
/// 客户端自带的 app-update.yml 指向 localhost 开发地址，不可用作版本源。
pub const CHANGELOG_URL: &str = "https://zcode.z.ai/cn/changelog";
/// 活动按 `X-ZCode-App-Version` 下发，版本信息越新越好：6h 过期即可重拉（原 24h 太长）。
const CACHE_TTL_SECS: u64 = 6 * 3600;
const FETCH_TIMEOUT_SECS: u64 = 10;
/// 首次（缓存缺失）同步拉取的硬超时：宁可本次用兜底版本，也不让活动刷新等太久。
const FIRST_FETCH_TIMEOUT_SECS: u64 = 2;
/// `ensure_fresh` 进程内节流：60s 内最多起一次后台刷新，避免刷新活动时雪崩式请求。
const ENSURE_THROTTLE_SECS: u64 = 60;

static LAST_ENSURE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();

fn changelog_url() -> String {
    std::env::var("ZCODE_CHANGELOG_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| CHANGELOG_URL.to_string())
}

fn cache_file(home: &Path) -> PathBuf {
    home.join(".zcode-switch").join("changelog-cache.json")
}

/// 后台保鲜：缓存未过期则跳过，否则拉取 changelog 并把最新版本写入缓存。
/// 任何失败静默（记日志、保留旧缓存），绝不影响调用方。
pub fn refresh_cache(home: &Path) {
    let file = cache_file(home);
    if read_cache(&file).is_some() && !cache_stale(&file) {
        return;
    }
    refresh_cache_inner(home, FETCH_TIMEOUT_SECS);
}

/// 带超时的同步保鲜：用于「缓存从未成功拉取」的第一次（启动 / 首次刷新活动）。
/// 返回是否成功写入缓存；调用方必须已经在后台/blocking 线程里。
pub fn refresh_cache_with_timeout(home: &Path, secs: u64) -> bool {
    refresh_cache_inner(home, secs)
}

fn refresh_cache_inner(home: &Path, secs: u64) -> bool {
    let file = cache_file(home);
    match fetch_latest_with_timeout(secs) {
        Some(v) => {
            if write_cache(&file, &v).is_err() {
                flowlog::log("version", "changelog-fail", "cache write failed");
                return false;
            }
            true
        }
        None => {
            flowlog::log("version", "changelog-fail", "fetch/parse failed");
            false
        }
    }
}

/// 只读缓存（不做网络请求）：返回已知的官方最新版本。
/// 过期缓存仍返回（旧值也比内置常量新鲜），TTL 只约束 refresh_cache 的重拉。
pub fn cached_latest(home: &Path) -> Option<String> {
    read_cache(&cache_file(home))
}

/// 每次刷新活动（refresh / preview / probe）前调用。
///
/// - 缓存新鲜：直接返回；
/// - **缓存缺失**（从未成功拉过）：同步拉一次（`FIRST_FETCH_TIMEOUT_SECS` 硬超时），
///   让本次请求就用最新版本——这是「旧构建 + 无缓存」唯一的补救窗口；
/// - 缓存过期（有旧值）：仍走后台节流重拉，绝不让每次刷新都等网络。
pub fn ensure_fresh(home: &Path) {
    let file = cache_file(home);
    if !cache_stale(&file) {
        return;
    }
    if read_cache(&file).is_none() && refresh_cache_with_timeout(home, FIRST_FETCH_TIMEOUT_SECS) {
        return;
    }
    let lock = LAST_ENSURE.get_or_init(|| Mutex::new(None));
    {
        let Ok(mut last) = lock.lock() else { return };
        if let Some(t) = *last {
            if t.elapsed().as_secs() < ENSURE_THROTTLE_SECS {
                return;
            }
        }
        *last = Some(Instant::now());
    }
    let home = home.to_path_buf();
    std::thread::spawn(move || refresh_cache(&home));
}

/// 版本诊断快照（给 UI / CLI 用）。
///
/// 注意 `reported` 已经是 `max(本机探测, 常量, 缓存)`，所以它**不可能**小于 `latest`；
/// 真正会导致被服务端过滤的只有「缓存缺失/过期」与「内置常量落后」。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionStatus {
    pub reported: String,
    pub latest: Option<String>,
    /// 安装包内置快照（打包时写入，换机/缓存被清时兜底）
    pub bundled: Option<String>,
    pub stale: bool,
    pub cache_age_secs: Option<u64>,
    pub client_constant: String,
    pub constant_behind: bool,
}

pub fn version_status(home: &Path) -> VersionStatus {
    let file = cache_file(home);
    let latest = read_cache(&file);
    let stale = latest.is_none() || cache_stale(&file);
    let reported = crate::quota::zcode_app_version_for(home);
    let client_constant = crate::quota::CLIENT_APP_VERSION.to_string();
    let constant_behind = latest
        .as_deref()
        .is_some_and(|l| !crate::quota::version_ge(&client_constant, l));
    VersionStatus {
        reported,
        latest,
        bundled: crate::quota::bundled_latest(),
        stale,
        cache_age_secs: cache_age_secs(&file),
        client_constant,
        constant_behind,
    }
}

fn cache_age_secs(file: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(file).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let fetched = v.get("fetched_at").and_then(|x| x.as_u64())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(now.saturating_sub(fetched))
}

fn cache_stale(file: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(file) else { return true };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return true };
    let fetched = v.get("fetched_at").and_then(|x| x.as_u64()).unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(fetched) > CACHE_TTL_SECS
}

fn fetch_latest_with_timeout(secs: u64) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10.min(secs)))
        .timeout(Duration::from_secs(secs))
        .build();
    let resp = agent.get(&changelog_url()).call().ok()?;
    let html = resp.into_string().ok()?;
    let best = extract_versions(&html)?.into_iter().max()?;
    Some(
        best.iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("."),
    )
}

/// 从 HTML 提取版本号（形如 3.16.3）。页面上裸数字噪声很多（SVG path 坐标、日期、
/// 千分位数字），因此只认两个可靠锚点：标题 `Release v3.16.3` 与下载链接
/// `releases/3.16.3/`。返回各版本的数字段列表（截断到 3 段）。
fn extract_versions(html: &str) -> Option<Vec<Vec<u64>>> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<Vec<u64>> = Vec::new();
    for anchor in ["release v", "releases/"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(anchor) {
            let at = from + pos + anchor.len();
            if let Some(v) = parse_version_at(html.as_bytes(), at) {
                out.push(v);
            }
            from = at;
        }
    }
    (!out.is_empty()).then_some(out)
}

fn parse_version_at(b: &[u8], start: usize) -> Option<Vec<u64>> {
    let n = b.len();
    let mut j = start;
    let mut segs: Vec<u64> = Vec::new();
    loop {
        let seg_start = j;
        while j < n && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == seg_start {
            return None;
        }
        let seg: u64 = std::str::from_utf8(&b[seg_start..j])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(u64::MAX);
        if seg >= 10000 {
            return None;
        }
        segs.push(seg);
        if j + 1 < n && b[j] == b'.' && b[j + 1].is_ascii_digit() {
            j += 1;
        } else {
            break;
        }
    }
    (segs.len() >= 3 && segs[0] < 100).then(|| segs.into_iter().take(3).collect())
}

fn read_cache(file: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(file).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let ver = v.get("version")?.as_str()?.trim().to_string();
    (!ver.is_empty()).then_some(ver)
}

fn write_cache(file: &Path, version: &str) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::json!({
        "version": version,
        "fetched_at": std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    let tmp = file.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(body.to_string().as_bytes())?;
    }
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_versions_uses_anchors_and_max() {
        let html = r#"<h2>Release v3.16.3</h2><span>发布于 2026年10月23日</span>
            <h2>Release v3.16.2</h2><h2>Release v3.15.1</h2>
            <a href="https://cdn-zcode.z.ai/zcode/electron/releases/3.16.3/macos-arm64/ZCode-3.16.3-mac-arm64.dmg">dmg</a>"#;
        let vers = extract_versions(html).unwrap();
        assert!(vers.contains(&vec![3, 16, 3]));
        assert!(vers.contains(&vec![3, 16, 2]));
        assert!(vers.contains(&vec![3, 15, 1]));
        let best = vers.into_iter().max().unwrap();
        assert_eq!(best, vec![3, 16, 3]);
    }

    #[test]
    fn extract_versions_ignores_svg_and_noise() {
        // SVG path 坐标（57.787.384）、日期、千分位数字、无锚点裸版本都不应被识别
        let html = r#"<path d="M57.787.384q2.76 0 4.776 1.032"></path><path d="v3.99.9q1.1"></path>
            <span>1024 次下载</span><span>2026-10-23</span><span>3.16.3</span>"#;
        assert!(extract_versions(html).is_none());
    }

    #[test]
    fn fetch_latest_parses_mock_page() {
        // 用本地文件协议不可行（ureq http only），此处只测解析入口在坏输入下的行为
        assert!(extract_versions("").is_none());
    }

    fn temp_home(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let home = std::env::temp_dir().join(format!("zsw-version-{tag}-{n}"));
        std::fs::create_dir_all(home.join(".zcode-switch")).unwrap();
        home
    }

    fn write_cache_json(home: &Path, version: &str, age_secs: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let body = format!(r#"{{"fetched_at":{},"version":"{version}"}}"#, now.saturating_sub(age_secs));
        std::fs::write(cache_file(home), body).unwrap();
    }

    /// 无缓存：必须标记 stale（上报版本可能落后），但 reported 仍取常量。
    #[test]
    fn version_status_without_cache_is_stale() {
        let home = temp_home("none");
        let s = version_status(&home);
        assert!(s.stale, "缺缓存必须视为过期");
        assert_eq!(s.latest, None);
        assert_eq!(s.reported, crate::quota::CLIENT_APP_VERSION);
        assert!(!s.constant_behind);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn version_status_fresh_cache_reports_latest() {
        let home = temp_home("fresh");
        write_cache_json(&home, "3.17.2", 60);
        let s = version_status(&home);
        assert!(!s.stale);
        assert_eq!(s.latest.as_deref(), Some("3.17.2"));
        assert_eq!(s.reported, "3.17.2");
        assert!(!s.constant_behind, "常量已升到 3.17.2，不应落后");
        assert!(s.cache_age_secs.unwrap() >= 60);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 官方出新版本时，即使缓存值更高也要如实上报，并把「常量落后」暴露给 UI/CLI。
    #[test]
    fn version_status_flags_constant_behind() {
        let home = temp_home("behind");
        write_cache_json(&home, "99.0.0", 10);
        let s = version_status(&home);
        assert_eq!(s.reported, "99.0.0", "上报取 max(常量, 缓存)");
        assert!(s.constant_behind, "常量 3.17.2 < 99.0.0，必须标记落后");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 超过 6h TTL 的缓存必须判过期（触发 ensure_fresh 后台重拉）。
    #[test]
    fn version_status_expired_cache_is_stale() {
        let home = temp_home("old");
        write_cache_json(&home, "3.16.3", CACHE_TTL_SECS + 3600);
        let s = version_status(&home);
        assert!(s.stale, "超过 TTL 必须标记过期");
        // 过期缓存仍参与 max（旧值也可能比常量新），这里缓存更旧，所以上报仍取常量
        assert_eq!(s.reported, crate::quota::CLIENT_APP_VERSION);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    #[ignore = "需要外网，人工验证时运行：cargo test -- --ignored"]
    fn fetch_real_changelog_smoke() {
        let dir = std::env::temp_dir().join("zsw-changelog-smoke");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        refresh_cache(&dir);
        let v = cached_latest(&dir).expect("changelog fetch failed");
        assert!(v.starts_with("3."), "unexpected version: {v}");
        println!("latest changelog version: {v}");
    }
}
