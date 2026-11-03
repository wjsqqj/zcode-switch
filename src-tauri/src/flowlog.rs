
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());

const MAX_BYTES: u64 = 256 * 1024;

pub fn init(base: &Path) {
    let dir = base.join("logs");
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = LOG_PATH.set(dir.join("oauth.log"));
    }
}

/// 按平台约定定位应用本地数据目录（与 Tauri `app_local_data_dir` 同值），
/// 供 CLI 初始化日志——否则 `--cli diag` 读不到 GUI 写的日志。
pub fn init_default() {
    const ID: &str = "com.zswitch.app";
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join(ID));
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support").join(ID));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|p| p.join(ID));
    if let Some(dir) = base {
        init(&dir);
    }
}

pub fn log(flow: &str, event: &str, detail: &str) {
    let Some(path) = LOG_PATH.get() else { return };
    append(path, &format_line(flow, event, detail));
}

/// 读取最近 `max_lines` 行（`.old` 轮转文件里的更早日志会补在前面）。
/// 文件缺失/不可读一律返回空，不影响导出。
pub fn tail(max_lines: usize) -> Vec<String> {
    let Some(path) = LOG_PATH.get() else {
        return Vec::new();
    };
    let mut lines: Vec<String> = Vec::new();
    if let Ok(raw) = std::fs::read_to_string(path) {
        lines.extend(raw.lines().map(str::to_string));
    }
    if lines.len() < max_lines {
        let old = path.with_extension("log.old");
        if let Ok(raw) = std::fs::read_to_string(&old) {
            let mut all: Vec<String> = raw.lines().map(str::to_string).collect();
            all.extend(lines);
            lines = all;
        }
    }
    if lines.len() > max_lines {
        lines.split_off(lines.len() - max_lines)
    } else {
        lines
    }
}

fn format_line(flow: &str, event: &str, detail: &str) -> String {
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let tag: String = flow.chars().take(8).collect();
    let clean: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        format!("{ts} [{tag}] {event}\n")
    } else {
        format!("{ts} [{tag}] {event} {clean}\n")
    }
}

fn append(path: &Path, line: &str) {
    let _guard = match WRITE_LOCK.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if std::fs::metadata(path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let old = path.with_extension("log.old");
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(path, &old);
    }
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = f.write_all(line.as_bytes());
}
