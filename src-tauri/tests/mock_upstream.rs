
//! claim/changelog 上游 mock 集成测试：锁定协议语义（解析、错误矩阵、缓存 TTL、退避序列）。
//! 全部流量指向 127.0.0.1，断网可跑；env 相关用例经 ENV_LOCK 串行。

mod support;

use serde_json::json;
use support::{del_env, mock_creds, set_env, MockUpstream, Route, TempHome, ENV_LOCK};
use zcode_switch_lib::claim;

fn mock_account(mid: &str) -> (TempHome, serde_json::Value) {
    let home = TempHome::new("acct");
    let creds = mock_creds();
    let _ = mid;
    (home, creds)
}

#[test]
fn preview_parses_plans_and_entitlements() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json(
        "/api/v1/zcode-plan/billing/preview",
        vec![(200, r#"{"code":0,"data":{"server_time":100,"plans":[
            {"plan_id":"p1","name":"P One","description":"","priority":5,
             "entitlements":[
                {"entitlement_id":"e1","show_name":"GLM-5.3-Flash","meter":"model_usage","unit_type":"token","grant_units":300000000,"period":"one_time"},
                {"entitlement_id":"e2","show_name":"忽略项","meter":"other","unit_type":"token","grant_units":1,"period":"daily"}
             ]}]}}"#)],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m1");
    let plans = claim::preview_plans(home.path(), &creds, None, Some("11111111-2222-3333-4444-555555555555".into())).unwrap();
    del_env("ZCODE_API_BASE");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].plan_id, "p1");
    // 只保留 meter=model_usage + unit_type=token 的赠送项
    assert_eq!(plans[0].grant_items.len(), 1);
    assert_eq!(plans[0].grant_items[0].units, 300000000.0);
}

#[test]
fn preview_nonzero_code_and_http401_error_out() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json(
        "/api/v1/zcode-plan/billing/preview",
        vec![
            (200, r#"{"code":1002,"msg":"event ended"}"#),
            (401, r#"{"code":401,"msg":"unauthorized"}"#),
        ],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m2");
    let e1 = claim::preview_plans(home.path(), &creds, None, None).unwrap_err();
    let e2 = claim::preview_plans(home.path(), &creds, None, None).unwrap_err();
    del_env("ZCODE_API_BASE");
    assert!(e1.contains("event ended"), "got {e1}");
    assert!(e2.contains("401"), "got {e2}");
}

#[test]
fn marketing_extracts_payloads_buttons_and_minimal_entries() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let body = r#"{"code":0,"data":{"server_time":1,"language":"zh-CN","deliveries":[{"campaign_id":"c1","resource_position":"banner","priority":50,"banner":{"buttons":[{"text":{"content":"关闭"},"action":{"type":"close","args":null}},{"text":{"content":"领"},"action":{"type":"claim_zcode_plan","args":{"plan_id":"btn-only-plan"}}}],"background":{"type":"bundle","args":{"zcode_plan":{"plan_id":"zcode-1024","name":"ZCode 1024 Dev Day","priority":101,"entitlements":[{"entitlement_id":"e1","show_name":"GLM-5.3-Flash","meter":"model_usage","unit_type":"token","grant_units":300000000,"period":"one_time"}]}}},"success_popup":{"hero":{"args":{"zcode_plan":{"plan_id":"zcode-1024","name":"ZCode 1024 Dev Day","priority":101,"entitlements":[{"entitlement_id":"e1","show_name":"GLM-5.3-Flash","meter":"model_usage","unit_type":"token","grant_units":300000000,"period":"one_time"}]}}}}}}]}}"#;
    let mock = MockUpstream::start(vec![Route::json("/api/v1/marketing/touch", vec![(200, body)])]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m3");
    let plans = claim::marketing_touch_plans(home.path(), &creds, None, None);
    del_env("ZCODE_API_BASE");
    // 内嵌 payload（zcode-1024，按钮同 id 去重）+ 无 payload 按钮合成最小条目
    assert_eq!(plans.len(), 2, "got {plans:?}");
    let p1024 = plans.iter().find(|p| p.plan_id == "zcode-1024").unwrap();
    assert_eq!(p1024.name, "ZCode 1024 Dev Day");
    assert_eq!(p1024.grant_items.len(), 1);
    let minimal = plans.iter().find(|p| p.plan_id == "btn-only-plan").unwrap();
    assert_eq!(minimal.name, "btn-only-plan");
    assert!(minimal.grant_items.is_empty());
}

#[test]
fn marketing_empty_deliveries_and_error_silent() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json(
        "/api/v1/marketing/touch",
        vec![
            (200, r#"{"code":0,"data":{"server_time":1,"language":"zh-CN","deliveries":[]}}"#),
            (200, r#"{"code":3012,"msg":"unusual activity"}"#),
        ],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m4");
    let empty = claim::marketing_touch_plans(home.path(), &creds, None, None);
    let errored = claim::marketing_touch_plans(home.path(), &creds, None, None);
    del_env("ZCODE_API_BASE");
    assert!(empty.is_empty());
    assert!(errored.is_empty(), "marketing 失败必须静默为空");
}

#[test]
fn merge_prefers_preview_and_sorts_by_priority() {
    let base = vec![claim::ClaimPlan {
        plan_id: "p1".into(),
        name: "P1".into(),
        description: String::new(),
        priority: 10,
        grants: vec![],
        grant_items: vec![],
    }];
    let extra = vec![
        claim::ClaimPlan { plan_id: "p1".into(), name: "dup".into(), description: String::new(), priority: 99, grants: vec![], grant_items: vec![] },
        claim::ClaimPlan { plan_id: "p2".into(), name: "P2".into(), description: String::new(), priority: 50, grants: vec![], grant_items: vec![] },
        claim::ClaimPlan { plan_id: "p3".into(), name: "P3".into(), description: String::new(), priority: 1, grants: vec![], grant_items: vec![] },
    ];
    let merged = claim::merge_marketing_plans(base, extra);
    let ids: Vec<&str> = merged.iter().map(|p| p.plan_id.as_str()).collect();
    assert_eq!(ids, vec!["p2", "p1", "p3"], "preview 优先去重 + priority 降序");
}

#[test]
fn current_parses_active_plans() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json(
        "/api/v1/zcode-plan/billing/current",
        vec![(200, r#"{"code":0,"data":{"plans":[
            {"user_plan_id":"u1","plan_id":"zcode-1024","name":"ZCode 1024 Dev Day","status":"active",
             "starts_at":100,"ends_at":200,
             "entitlements":[{"entitlement_id":"e1","show_name":"GLM-5.3","meter":"model_usage","unit_type":"token","grant_units":200000000,"period":"one_time"}]}]}}"#)],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m5");
    let plans = claim::fetch_active_plans(home.path(), &creds, None, None).unwrap();
    del_env("ZCODE_API_BASE");
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].status, "active");
    assert_eq!(plans[0].ends_at, Some(200000));
    assert_eq!(plans[0].grant_items[0].units, 200000000.0);
}

#[test]
fn claim_error_matrix_3007_terminal_challenge_success() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::with_headers(
        "/api/v1/zcode-plan/billing/claim",
        vec![
            (200, Vec::new(), r#"{"code":3007,"msg":"captcha failed"}"#),
            (200, Vec::new(), r#"{"code":1003,"msg":"already claimed"}"#),
            (403, vec![("x-aliyun-captcha-verify-param", "challenge")], r#"forbidden"#),
            (200, Vec::new(), r#"{"code":0,"data":{"plan":{"user_plan_id":"u1","plan_id":"p1","status":"active","starts_at":1,"ends_at":2},"server_time":3}}"#),
        ],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let (home, creds) = mock_account("m6");
    let e3007 = claim::submit_claim(home.path(), &creds, None, "p1", "cap", None, None).unwrap_err();
    assert!(e3007.is_captcha_rejected(), "got {:?}", e3007.code);
    let e1003 = claim::submit_claim(home.path(), &creds, None, "p1", "cap", None, None).unwrap_err();
    assert!(e1003.is_terminal());
    let echal = claim::submit_claim(home.path(), &creds, None, "p1", "cap", None, None).unwrap_err();
    assert!(echal.is_challenge(), "403+挑战头必须归为 challenge，got {:?}", echal.code);
    let ok = claim::submit_claim(home.path(), &creds, None, "p1", "cap", None, None).unwrap();
    del_env("ZCODE_API_BASE");
    assert_eq!(ok.pointer("/data/plan/status"), Some(&json!("active")));
}

#[test]
fn captcha_config_uses_mock_then_falls_back_to_default() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    claim::reset_captcha_config_cache_for_tests();
    let mock = MockUpstream::start(vec![Route::json(
        "/api/v1/client/configs",
        vec![(200, r#"{"code":0,"data":{"configs":{"captcha":{"enabled":true,"region":"cn","prefix":"mockpx","sceneId":"mockscene"}}}}"#)],
    )]);
    set_env("ZCODE_API_BASE", &mock.base);
    let cfg = claim::fetch_captcha_config().unwrap();
    del_env("ZCODE_API_BASE");
    assert_eq!(cfg.prefix, "mockpx");
    // 缓存命中（不重发请求）→ 改 env 到不可达端口也应返回缓存值
    set_env("ZCODE_API_BASE", "http://127.0.0.1:1");
    let cached = claim::fetch_captcha_config().unwrap();
    assert_eq!(cached.prefix, "mockpx");
    // 清缓存后请求失败 → 内置默认兜底，流程不中断
    claim::reset_captcha_config_cache_for_tests();
    let def = claim::fetch_captcha_config().unwrap();
    del_env("ZCODE_API_BASE");
    assert_eq!(def.prefix, "no8xfe");
    assert_eq!(def.scene_id, "11xygtvd");
    claim::reset_captcha_config_cache_for_tests();
}

#[test]
fn changelog_refresh_ttl_and_stale_refetch() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json(
        "/changelog",
        vec![
            (200, r#"<h2>Release v3.16.3</h2><h2>Release v3.15.1</h2>"#),
            (200, r#"<h2>Release v3.17.0</h2>"#),
        ],
    )]);
    let home = TempHome::new("chlog");
    set_env("ZCODE_CHANGELOG_URL", &format!("{}/changelog", mock.base));
    crate_changelog_refresh(home.path());
    assert_eq!(crate_changelog_cached(home.path()).as_deref(), Some("3.16.3"));
    // TTL 内不重发请求：mock 第二条响应未被消费（hits==1）
    crate_changelog_refresh(home.path());
    assert_eq!(mock.hits(0), 1, "24h TTL 内不得重复请求");
    // 缓存过期 → 重新拉取到新版本
    make_cache_stale(home.path());
    crate_changelog_refresh(home.path());
    del_env("ZCODE_CHANGELOG_URL");
    assert_eq!(crate_changelog_cached(home.path()).as_deref(), Some("3.17.0"));
}

#[test]
fn changelog_bad_html_falls_back_silently() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mock = MockUpstream::start(vec![Route::json("/changelog", vec![(200, "<p>no versions here 1024 2026-10-23</p>")])]);
    let home = TempHome::new("chlog-bad");
    set_env("ZCODE_CHANGELOG_URL", &format!("{}/changelog", mock.base));
    crate_changelog_refresh(home.path());
    del_env("ZCODE_CHANGELOG_URL");
    assert_eq!(crate_changelog_cached(home.path()), None, "解析失败必须静默回退");
}

// —— changelog 模块经 pub API 暴露的薄包装（保持集成测试面最小）——
fn crate_changelog_refresh(home: &std::path::Path) {
    zcode_switch_lib::changelog::refresh_cache(home);
}
fn crate_changelog_cached(home: &std::path::Path) -> Option<String> {
    zcode_switch_lib::changelog::cached_latest(home)
}

fn make_cache_stale(home: &std::path::Path) {
    let file = home.join(".zcode-switch").join("changelog-cache.json");
    let raw = std::fs::read_to_string(&file).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let stale = json!({
        "version": v["version"],
        "fetched_at": 0,
    });
    std::fs::write(&file, stale.to_string()).unwrap();
}
