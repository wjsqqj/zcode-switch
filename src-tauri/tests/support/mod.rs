
//! 测试专用轻量 mock 上游：std TcpListener 手写最小 HTTP/1.1，零外部依赖。
//! 按路径前缀路由，响应按请求序消费（最后一条无限重复），实现故障序列注入。

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

pub struct Route {
    pub path: &'static str,
    /// (status, extra headers, body)，按请求序消费，最后一条无限重复
    pub responses: Vec<(u16, Vec<(&'static str, &'static str)>, String)>,
}

impl Route {
    pub fn json(path: &'static str, responses: Vec<(u16, &str)>) -> Route {
        Route {
            path,
            responses: responses
                .into_iter()
                .map(|(s, b)| (s, Vec::new(), b.to_string()))
                .collect(),
        }
    }
    pub fn with_headers(
        path: &'static str,
        responses: Vec<(u16, Vec<(&'static str, &'static str)>, &str)>,
    ) -> Route {
        Route {
            path,
            responses: responses
                .into_iter()
                .map(|(s, h, b)| (s, h, b.to_string()))
                .collect(),
        }
    }
}

pub struct MockUpstream {
    pub base: String,
    counters: Arc<Vec<AtomicUsize>>,
    _shutdown: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockUpstream {
    pub fn start(routes: Vec<Route>) -> MockUpstream {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock upstream");
        let port = listener.local_addr().unwrap().port();
        // 非阻塞 accept + 轮询 shutdown：否则 Drop 的 join 会永远等不到阻塞中的 accept
        listener.set_nonblocking(true).ok();
        let counters: Arc<Vec<AtomicUsize>> = Arc::new(routes.iter().map(|_| AtomicUsize::new(0)).collect());
        let routes = Arc::new(routes);
        let serve_routes = routes.clone();
        let serve_counters = counters.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let sd = shutdown.clone();
        let handle = std::thread::spawn(move || {
            loop {
                if sd.load(Ordering::Relaxed) {
                    return;
                }
                let (mut stream, _) = match listener.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(15));
                        continue;
                    }
                    Err(_) => return,
                };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let _ = stream.set_nonblocking(false);
                let req = read_request(&mut stream);
                let Some(path) = req else { continue };
                // 找第一条前缀匹配路由
                let idx = serve_routes.iter().position(|r| path.starts_with(r.path));
                let resp = match idx {
                    Some(i) => {
                        let route = &serve_routes[i];
                        let n = route.responses.len();
                        let c = serve_counters[i].fetch_add(1, Ordering::SeqCst);
                        let k = c.min(n - 1);
                        let (status, headers, body) = &route.responses[k];
                        format_response(*status, headers, body)
                    }
                    None => format_response(404, &Vec::new(), r#"{"code":404,"msg":"mock no route"}"#),
                };
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        MockUpstream {
            base: format!("http://127.0.0.1:{port}"),
            counters,
            _shutdown: shutdown,
            handle: Some(handle),
        }
    }

    /// 某路由已被请求的次数（TTL 断言用）
    pub fn hits(&self, route_idx: usize) -> usize {
        self.counters[route_idx].load(Ordering::SeqCst)
    }
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        self._shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    // 读到 header 结束
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return None,
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let request_line = head.lines().next()?.to_string();
    let path = request_line.split_whitespace().nth(1)?.to_string();
    // POST：按 Content-Length 排干 body，避免残留影响下一连接
    let content_length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    if content_length > 0 {
        let header_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4).unwrap_or(buf.len());
        let already = buf.len().saturating_sub(header_end);
        let mut remaining = content_length.saturating_sub(already);
        let mut sink = vec![0u8; 4096];
        while remaining > 0 {
            match stream.read(&mut sink) {
                Ok(0) => break,
                Ok(n) => remaining = remaining.saturating_sub(n),
                Err(_) => break,
            }
        }
    }
    // 去掉 query
    Some(path.split('?').next().unwrap_or(&path).to_string())
}

fn format_response(status: u16, headers: &[(&'static str, &'static str)], body: &str) -> String {
    let mut out = format!("HTTP/1.1 {status} MOCK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    out.push_str(body);
    out
}

/// 环境变量测试互斥：env 是进程全局，env 相关用例统一串行。
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn set_env(k: &str, v: &str) {
    std::env::set_var(k, v);
}
pub fn del_env(k: &str) {
    std::env::remove_var(k);
}

/// 隔离的假 home（凭证/缓存/风控文件都写进去，测试结束清理）
pub struct TempHome(std::path::PathBuf);
impl TempHome {
    pub fn new(tag: &str) -> TempHome {
        use std::sync::atomic::AtomicU32;
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("zsw-mock-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempHome(p)
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn mock_creds() -> serde_json::Value {
    // 明文 JWT（>20 字符即通过 claim_token 的直通分支），不含真实凭证
    serde_json::json!({ "zcodejwttoken": "mock.jwt.token-for-tests-only-000000" })
}
