# Design: 上游 mock 测试体系

## URL 旁路（前置，A/B/C/D 各任务已铺）

```rust
// 每个模块内：
fn api_base() -> String { std::env::var("ZCODE_API_BASE").unwrap_or_else(|_| "https://zcode.z.ai".into()) }
// URL 常量 → 函数：billing_preview_url() / marketing_touch_url() / changelog_url() 等
```
- 注意 env 并发：测试用 #[test] 默认并行，env 全局——mock 测试统一用 `serial` 串行（自写轻量 mutex guard）或每测试内 spawn mock 前设置 env 并在结束时清理；Rust std env 为进程级，采用"每测试文件单二进制目标 + 内部互斥"约定。

## Mock（tests/support/mock.rs，#[cfg(test)] 编译）

```rust
pub struct MockUpstream { pub addr: String, handle: JoinHandle<()> }
impl MockUpstream {
    pub fn start(routes: Vec<Route>) -> Self;   // Route{ path_prefix: &str, responses: Vec<Resp> }
    // Resp{ status, body } 按请求序号循环/消费；TcpListener + 手写最小 HTTP/1.1 解析（只读 path + 返回定长 body）
}
```
- 手写 HTTP：GET 请求读一行 request line 取 path；响应固定 Content-Type: application/json + Content-Length。~120 行。

## 用例组织

- src-tauri/tests/upstream_mock.rs（integration test，lib 需暴露 cfg(test) 外可用的入口：纯函数直接 pub；HTTP 路径走 env 旁路）。
- 纯逻辑（version_ge/merge/walk_marketing_delivery/strikes）直接单测不需 mock。

## 验证

- `cargo test --manifest-path src-tauri/Cargo.toml`；断网 CI 可跑。
