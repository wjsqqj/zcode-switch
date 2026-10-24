
use crate::flowlog;
use serde_json::Value;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// 官方更新日志页：SSR 输出，初始 HTML 即含版本号（2026-10-24 实测，无需执行 JS）。
/// 客户端自带的 app-update.yml 指向 localhost 开发地址，不可用作版本源。
pub const CHANGELOG_URL: &str = "https://zcode.z.ai/cn/changelog";
const CACHE_TTL_SECS: u64 = 24 * 3600;
const FETCH_TIMEOUT_SECS: u64 = 10;

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
    match fetch_latest() {
        Some(v) => {
            if write_cache(&file, &v).is_err() {
                flowlog::log("version", "changelog-fail", "cache write failed");
            }
        }
        None => flowlog::log("version", "changelog-fail", "fetch/parse failed"),
    }
}

/// 只读缓存（不做网络请求）：返回已知的官方最新版本。
/// 过期缓存仍返回（旧值也比内置常量新鲜），TTL 只约束 refresh_cache 的重拉。
pub fn cached_latest(home: &Path) -> Option<String> {
    read_cache(&cache_file(home))
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

fn fetch_latest() -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
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
