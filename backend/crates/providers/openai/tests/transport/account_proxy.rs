use super::*;
use gateway_core::{
    account::{CredentialRevision, OutboundProxy, ProviderAccount, ProviderAccountId},
    routing::ProviderKind,
};
use std::{
    io::{self, Cursor, Read},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, BufWriter, ReadBuf};

fn account(id: &str, proxy: Option<&str>) -> ProviderAccount {
    ProviderAccount::new(
        ProviderAccountId::new(format!("acct_{id}")).unwrap(),
        ProviderKind::new("openai").unwrap(),
        id.to_owned(),
        Some(id.to_owned()),
        "oauth".to_owned(),
        CredentialRevision::new(1).unwrap(),
        None,
    )
    .with_outbound_proxy(proxy.map(|value| OutboundProxy::parse(value).unwrap()))
}

async fn http_exit(label: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request_with_body(&mut stream).await;
        let body = format!(
            "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp_{label}\",\"status\":\"completed\",\"output\":[],\"usage\":{{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}}}}\n\n"
        );
        stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        let header_end = request
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        String::from_utf8(request[..header_end].to_vec()).unwrap()
    });
    (format!("http://{address}"), task)
}

async fn pooled_http_exit(
    expected_connections: usize,
    expected_requests: usize,
) -> (
    String,
    tokio::task::JoinHandle<Vec<(usize, hyper::HeaderMap)>>,
) {
    pooled_http_exit_with_body(
        expected_connections,
        expected_requests,
        concat!(
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{",
            "\"id\":\"resp_pool\",\"status\":\"completed\",\"output\":[],",
            "\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        ),
        "text/event-stream",
    )
    .await
}

async fn pooled_http_exit_with_body(
    expected_connections: usize,
    expected_requests: usize,
    body: &'static str,
    content_type: &'static str,
) -> (
    String,
    tokio::task::JoinHandle<Vec<(usize, hyper::HeaderMap)>>,
) {
    use std::convert::Infallible;

    use http_body_util::Full;
    use hyper::server::conn::http1;
    use hyper_util::rt::TokioIo;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::unbounded_channel();
        let accept = tokio::spawn(async move {
            let mut tasks = Vec::new();
            for connection_id in 0..expected_connections {
                let (stream, _) = listener.accept().await.unwrap();
                let seen_tx = seen_tx.clone();
                tasks.push(tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let seen_tx = seen_tx.clone();
                            async move {
                                seen_tx
                                    .send((connection_id, request.headers().clone()))
                                    .unwrap();
                                Ok::<_, Infallible>(
                                    hyper::Response::builder()
                                        .header("content-type", content_type)
                                        .body(Full::new(bytes::Bytes::from_static(body.as_bytes())))
                                        .unwrap(),
                                )
                            }
                        },
                    );
                    http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                        .unwrap();
                }));
            }
            tasks
        });
        let mut connection_ids = Vec::with_capacity(expected_requests);
        for _ in 0..expected_requests {
            connection_ids.push(seen_rx.recv().await.unwrap());
        }
        for task in accept.await.unwrap() {
            task.abort();
        }
        connection_ids
    });
    (format!("http://{address}"), task)
}

fn isolate_client_cache_test(name: &str) -> bool {
    const CASE_ENV: &str = "CPR_ACCOUNT_CLIENT_CACHE_TEST";
    if std::env::var(CASE_ENV).as_deref() == Ok(name) {
        println!("account-cache-case:{name}");
        return false;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("transport::account_proxy::{name}"),
            "--nocapture",
        ])
        .env(CASE_ENV, name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("account-cache-case:{name}"))
    );
    true
}

#[tokio::test]
async fn direct_http_accounts_share_the_pool_without_sharing_request_headers() {
    if isolate_client_cache_test(
        "direct_http_accounts_share_the_pool_without_sharing_request_headers",
    ) {
        return;
    }
    let (base_url, server) = pooled_http_exit(1, 4).await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for id in ["direct-a", "direct-b", "direct-a"] {
        let authorization = format!("Bearer synthetic-{id}");
        let cookie = format!("__cf_bm=synthetic-{id}");
        let turn_state = format!("synthetic-state-{id}");
        let mut context = request_context(id, Some(id));
        context.authorization = &authorization;
        context.cookie_header = Some(&cookie);
        context.turn_state = Some(&turn_state);
        context.installation_id = Some(id);
        base.for_account(&account(id, None))
            .unwrap()
            .create_response(&request, context)
            .await
            .unwrap();
    }
    base.for_account(&account("direct-b", None))
        .unwrap()
        .create_response(&request, request_context("no-state", Some("direct-b")))
        .await
        .unwrap();

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("shared direct account connection")
        .unwrap();
    assert!(
        connection_ids
            .iter()
            .all(|seen| seen.0 == connection_ids[0].0)
    );
    for ((_, headers), id) in connection_ids
        .iter()
        .zip(["direct-a", "direct-b", "direct-a"])
    {
        assert_eq!(headers["chatgpt-account-id"], id);
        assert_eq!(headers["authorization"], format!("Bearer synthetic-{id}"));
        assert_eq!(headers["cookie"], format!("__cf_bm=synthetic-{id}"));
        assert_eq!(
            headers["x-codex-turn-state"],
            format!("synthetic-state-{id}")
        );
        assert_eq!(headers["x-codex-installation-id"], id);
    }
    let last = &connection_ids[3].1;
    assert_eq!(last["chatgpt-account-id"], "direct-b");
    assert!(!last.contains_key("cookie"));
    assert!(!last.contains_key("x-codex-turn-state"));
    assert!(!last.contains_key("x-codex-installation-id"));
}

#[tokio::test]
async fn proxy_http_pool_reuses_one_profile_and_rotates_after_user_agent_change() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;

    if isolate_client_cache_test(
        "proxy_http_pool_reuses_one_profile_and_rotates_after_user_agent_change",
    ) {
        return;
    }
    let (proxy_url, server) = pooled_http_exit(2, 3).await;
    let account = account("profile-pool", Some(&proxy_url));
    let profile = test_wire_profile();
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://upstream.invalid",
        profile.clone(),
    );
    let default_client = base.for_account(&account).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for request_id in ["profile-default-a", "profile-default-b"] {
        default_client
            .create_response(&request, request_context(request_id, Some("profile-pool")))
            .await
            .unwrap();
    }

    profile
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
        })
        .unwrap();
    base.for_account(&account)
        .unwrap()
        .create_response(
            &request,
            request_context("profile-custom", Some("profile-pool")),
        )
        .await
        .unwrap();

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("default and custom profile connections")
        .unwrap();
    assert_eq!(connection_ids[0].0, connection_ids[1].0);
    assert_ne!(connection_ids[0].0, connection_ids[2].0);
    assert_ne!(
        connection_ids[0].1["user-agent"],
        connection_ids[2].1["user-agent"]
    );
}

#[tokio::test]
async fn direct_http_pool_preserves_frozen_profiles_across_user_agent_changes() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;

    let (base_url, server) = pooled_http_exit(1, 4).await;
    let profile = test_wire_profile();
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        profile.clone(),
    );
    let first = account("direct-profile-a", None);
    let second = account("direct-profile-b", None);
    let prepared = base.for_account(&first).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    prepared
        .create_response(
            &request,
            request_context("old-profile", Some("direct-profile-a")),
        )
        .await
        .unwrap();
    profile
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
        })
        .unwrap();
    for (client, id) in [
        (base.for_account(&second).unwrap(), "direct-profile-b"),
        (prepared, "direct-profile-a"),
        (base.for_account(&first).unwrap(), "direct-profile-a"),
    ] {
        client
            .create_response(&request, request_context(id, Some(id)))
            .await
            .unwrap();
    }
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(seen.iter().all(|entry| entry.0 == seen[0].0));
    assert_eq!(seen[0].1["user-agent"], seen[2].1["user-agent"]);
    assert_eq!(seen[1].1["user-agent"], seen[3].1["user-agent"]);
    assert_ne!(seen[0].1["user-agent"], seen[1].1["user-agent"]);
}

#[tokio::test]
async fn direct_http_account_churn_and_cache_eviction_preserve_the_base_pool() {
    use provider_openai::transport::client::{
        build_reqwest_client, evict_account_http_clients, evict_all_account_http_clients,
    };

    if isolate_client_cache_test(
        "direct_http_account_churn_and_cache_eviction_preserve_the_base_pool",
    ) {
        return;
    }
    let (base_url, server) = pooled_http_exit(1, 301).await;
    let base = CodexBackendClient::new(
        build_reqwest_client().unwrap(),
        base_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for index in 0..300 {
        let id = format!("direct-churn-{index}");
        base.for_account(&account(&id, None))
            .unwrap()
            .create_response(&request, request_context(&id, Some(&id)))
            .await
            .unwrap();
    }
    let first = account("direct-churn-0", None);
    evict_account_http_clients(first.id().as_str());
    evict_all_account_http_clients();
    base.for_account(&first)
        .unwrap()
        .create_response(
            &request,
            request_context("after-eviction", Some("direct-churn-0")),
        )
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(seen.iter().all(|entry| entry.0 == seen[0].0));
}

#[tokio::test]
async fn proxy_http_accounts_keep_separate_pools_on_the_same_proxy() {
    if isolate_client_cache_test("proxy_http_accounts_keep_separate_pools_on_the_same_proxy") {
        return;
    }
    let (proxy_url, server) = pooled_http_exit(2, 3).await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://upstream.invalid",
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    for id in ["shared-proxy-a", "shared-proxy-b", "shared-proxy-a"] {
        base.for_account(&account(id, Some(&proxy_url)))
            .unwrap()
            .create_response(&request, request_context(id, Some(id)))
            .await
            .unwrap();
    }
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(seen[0].0, seen[1].0);
    assert_eq!(seen[0].0, seen[2].0);
}

#[tokio::test]
async fn recreating_account_client_preserves_the_direct_pool() {
    if isolate_client_cache_test("recreating_account_client_preserves_the_direct_pool") {
        return;
    }
    let (base_url, server) = pooled_http_exit(1, 2).await;
    let account = account("evicted", None);
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        base_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    base.for_account(&account)
        .unwrap()
        .create_response(&request, request_context("before-evict", Some("evicted")))
        .await
        .unwrap();
    base.for_account(&account)
        .unwrap()
        .create_response(&request, request_context("after-evict", Some("evicted")))
        .await
        .unwrap();

    let connection_ids = timeout(Duration::from_secs(5), server)
        .await
        .expect("connection after recreating account client")
        .unwrap();
    assert_eq!(connection_ids[0].0, connection_ids[1].0);
}

#[tokio::test]
async fn http_lru_preserves_hot_clients_and_retained_leases_at_256_entries() {
    use provider_openai::transport::client::build_account_http_client;

    if isolate_client_cache_test(
        "http_lru_preserves_hot_clients_and_retained_leases_at_256_entries",
    ) {
        return;
    }
    let (url, server) = pooled_http_exit(3, 5).await;
    let cold = build_account_http_client("lru-cold", None, "profile").unwrap();
    cold.get(&url).send().await.unwrap().bytes().await.unwrap();
    let hot = build_account_http_client("lru-hot", None, "profile").unwrap();
    hot.get(&url).send().await.unwrap().bytes().await.unwrap();
    for index in 2..256 {
        build_account_http_client(&format!("lru-{index}"), None, "profile").unwrap();
    }
    let hot = build_account_http_client("lru-hot", None, "profile").unwrap();
    build_account_http_client("lru-overflow", None, "profile").unwrap();
    build_account_http_client("lru-hot", None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    cold.get(&url).send().await.unwrap().bytes().await.unwrap();
    build_account_http_client("lru-cold", None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        seen[1].0, seen[2].0,
        "overflow must preserve the recently used pool"
    );
    assert_eq!(
        seen[0].0, seen[3].0,
        "eviction must not destroy outstanding client references"
    );
    assert_ne!(
        seen[0].0, seen[4].0,
        "the oldest cache entry must be evicted"
    );
    drop(hot);
}

#[tokio::test]
async fn account_http_eviction_keeps_other_accounts_and_live_clients() {
    use provider_openai::transport::client::{
        build_account_http_client, evict_account_http_clients,
    };

    if isolate_client_cache_test("account_http_eviction_keeps_other_accounts_and_live_clients") {
        return;
    }
    let (url, server) = pooled_http_exit(3, 5).await;
    let first = build_account_http_client("eviction-a", None, "profile").unwrap();
    let second = build_account_http_client("eviction-b", None, "profile").unwrap();
    first.get(&url).send().await.unwrap().bytes().await.unwrap();
    second
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    evict_account_http_clients("eviction-a");
    first.get(&url).send().await.unwrap().bytes().await.unwrap();
    build_account_http_client("eviction-b", None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    build_account_http_client("eviction-a", None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seen[0].0, seen[2].0);
    assert_eq!(seen[1].0, seen[3].0);
    assert_ne!(seen[0].0, seen[4].0);
}

#[tokio::test]
async fn token_refresh_pools_isolate_accounts_profiles_and_eviction() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;
    use provider_openai::credential::token_client::{
        OpenAiTokenClient, TokenClientConfig, TokenRefresher, evict_account_token_clients,
    };

    if isolate_client_cache_test("token_refresh_pools_isolate_accounts_profiles_and_eviction") {
        return;
    }
    let (url, server) = pooled_http_exit_with_body(
        4,
        6,
        r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh"}"#,
        "application/json",
    )
    .await;
    let profile = test_wire_profile();
    let client = OpenAiTokenClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        TokenClientConfig {
            client_id: "synthetic-client".to_owned(),
            token_endpoint: url,
        },
        profile.clone(),
    );
    let first = ProviderAccountId::new("acct_token-pool-a").unwrap();
    let second = ProviderAccountId::new("acct_token-pool-b").unwrap();
    for id in [&first, &second, &first] {
        client
            .refresh_for_account(id, "synthetic-refresh", None)
            .await
            .unwrap();
    }
    profile
        .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
            user_agent: "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
        })
        .unwrap();
    client
        .refresh_for_account(&first, "synthetic-refresh", None)
        .await
        .unwrap();
    client
        .refresh_for_account(&first, "synthetic-refresh", None)
        .await
        .unwrap();
    evict_account_token_clients(first.as_str());
    client
        .refresh_for_account(&first, "synthetic-refresh", None)
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(seen[0].0, seen[1].0);
    assert_eq!(seen[0].0, seen[2].0);
    assert_ne!(seen[0].0, seen[3].0);
    assert_ne!(seen[0].1["user-agent"], seen[3].1["user-agent"]);
    assert_eq!(seen[3].0, seen[4].0);
    assert_ne!(seen[4].0, seen[5].0);
}

#[tokio::test]
async fn http_lru_eviction_does_not_interrupt_an_inflight_response() {
    use provider_openai::transport::client::build_account_http_client;

    if isolate_client_cache_test("http_lru_eviction_does_not_interrupt_an_inflight_response") {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        read_http_request_with_body(&mut first).await;
        first
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello ")
            .await
            .unwrap();
        let (mut second, _) = listener.accept().await.unwrap();
        read_http_request_with_body(&mut second).await;
        second
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew")
            .await
            .unwrap();
        first.write_all(b"world").await.unwrap();
    });
    let client = build_account_http_client("inflight", None, "profile").unwrap();
    let mut response = client.get(&url).send().await.unwrap();
    assert_eq!(response.chunk().await.unwrap().unwrap(), "hello ");
    for index in 0..256 {
        build_account_http_client(&format!("inflight-fill-{index}"), None, "profile").unwrap();
    }
    let replacement = build_account_http_client("inflight", None, "profile").unwrap();
    let body = timeout(Duration::from_secs(5), replacement.get(url).send())
        .await
        .unwrap()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body, "new");
    assert_eq!(response.bytes().await.unwrap(), "world");
    server.await.unwrap();
}

#[tokio::test]
async fn token_client_lru_evicts_only_the_oldest_at_128_entries() {
    use provider_openai::credential::token_client::{
        OpenAiTokenClient, TokenClientConfig, TokenRefresher,
    };

    if isolate_client_cache_test("token_client_lru_evicts_only_the_oldest_at_128_entries") {
        return;
    }
    let (url, server) = pooled_http_exit_with_body(
        130,
        132,
        r#"{"access_token":"synthetic-access"}"#,
        "application/json",
    )
    .await;
    let client = OpenAiTokenClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        TokenClientConfig {
            client_id: "synthetic-client".to_owned(),
            token_endpoint: url,
        },
        test_wire_profile(),
    );
    let ids = (0..129)
        .map(|index| ProviderAccountId::new(format!("acct_token-lru-{index}")).unwrap())
        .collect::<Vec<_>>();
    for id in &ids[..128] {
        client
            .refresh_for_account(id, "synthetic-refresh", None)
            .await
            .unwrap();
    }
    for id in [&ids[1], &ids[128], &ids[1], &ids[0]] {
        client
            .refresh_for_account(id, "synthetic-refresh", None)
            .await
            .unwrap();
    }
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seen[1].0, seen[128].0);
    assert_eq!(seen[1].0, seen[130].0, "hot token client survives overflow");
    assert_ne!(seen[0].0, seen[131].0, "oldest token client is rebuilt");
}

async fn runtime_teardown_case(shared_account: bool) {
    use std::convert::Infallible;

    use bytes::Bytes;
    use futures::stream;
    use http_body_util::StreamBody;
    use hyper::body::Frame;
    use hyper_util::rt::TokioIo;
    use provider_openai::transport::client::build_account_http_client;
    use tokio::sync::{Notify, oneshot};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let tail = Arc::new(Notify::new());
    let server_tail = Arc::clone(&tail);
    let server = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        let mut connection = 0;
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            connection += 1;
            let release = Arc::clone(&server_tail);
            handlers.spawn(async move {
                let service = hyper::service::service_fn(
                    move |request: hyper::Request<hyper::body::Incoming>| {
                        let hold = request.uri().path() == "/stream";
                        let release = Arc::clone(&release);
                        async move {
                            let first = stream::iter([Ok::<_, Infallible>(Frame::data(
                                Bytes::from_static(b"a"),
                            ))]);
                            let last = stream::once(async move {
                                if hold {
                                    release.notified().await;
                                }
                                Ok::<_, Infallible>(Frame::data(Bytes::from_static(b"b")))
                            });
                            Ok::<_, Infallible>(
                                hyper::Response::builder()
                                    .header("x-test-connection", connection)
                                    .body(StreamBody::new(first.chain(last)))
                                    .unwrap(),
                            )
                        }
                    },
                );
                // Closing the other runtime deliberately disconnects the negative case.
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(socket), service)
                    .await;
            });
        }
    });
    let account_a = format!("runtime-owner-{}", uuid::Uuid::new_v4());
    let account_b = if shared_account {
        account_a.clone()
    } else {
        format!("runtime-owner-{}", uuid::Uuid::new_v4())
    };
    let (warmed_tx, warmed_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
    let warm_url = url.clone();
    let owner = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let connection = runtime.block_on(async {
            let client = build_account_http_client(&account_a, None, "runtime-fixture").unwrap();
            let response = client.get(format!("{warm_url}/warm")).send().await.unwrap();
            let connection = response.headers()["x-test-connection"].clone();
            assert_eq!(response.bytes().await.unwrap(), "ab");
            connection
        });
        warmed_tx.send(connection).unwrap();
        let _ = shutdown_rx.recv();
        drop(runtime);
    });
    let warm_connection = timeout(Duration::from_secs(5), warmed_rx)
        .await
        .unwrap()
        .unwrap();
    let client = build_account_http_client(&account_b, None, "runtime-fixture").unwrap();
    let mut response = timeout(
        Duration::from_secs(5),
        client.get(format!("{url}/stream")).send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        response.headers()["x-test-connection"] == warm_connection,
        shared_account
    );
    assert_eq!(response.chunk().await.unwrap().unwrap(), "a");
    shutdown_tx.send(()).unwrap();
    tokio::task::spawn_blocking(move || owner.join().unwrap())
        .await
        .unwrap();
    if shared_account {
        assert!(
            timeout(Duration::from_secs(5), response.chunk())
                .await
                .unwrap()
                .is_err(),
            "a shared test account reuses a connection owned by the stopped runtime"
        );
    } else {
        tail.notify_one();
        assert_eq!(
            timeout(Duration::from_secs(5), response.bytes())
                .await
                .unwrap()
                .unwrap(),
            "b"
        );
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn independent_test_runtimes_require_distinct_cached_account_ids() {
    if isolate_client_cache_test("independent_test_runtimes_require_distinct_cached_account_ids") {
        return;
    }
    runtime_teardown_case(true).await;
    runtime_teardown_case(false).await;
}

#[tokio::test]
async fn admin_client_cache_invalidation_preserves_unchanged_ua_and_other_accounts() {
    use gateway_core::provider_ports::ProviderUserAgentOverride;
    use provider_openai::transport::client::build_account_http_client;

    if isolate_client_cache_test(
        "admin_client_cache_invalidation_preserves_unchanged_ua_and_other_accounts",
    ) {
        return;
    }
    let config = crate::admin::valid_config();
    let bundle = provider_openai::initialize(config.config.clone(), crate::admin::provider_ports())
        .await
        .unwrap();
    let admin = bundle.admin_provider();
    let (url, server) = pooled_http_exit(6, 9).await;
    let first = ProviderAccountId::new("acct_lifecycle-a").unwrap();
    let second = ProviderAccountId::new("acct_lifecycle-b").unwrap();
    for id in [&first, &second] {
        build_account_http_client(id.as_str(), None, "profile")
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    admin
        .apply_outbound_user_agent(ProviderUserAgentOverride::Default)
        .unwrap();
    build_account_http_client(first.as_str(), None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let custom = ProviderUserAgentOverride::Custom {
        user_agent: "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color".to_owned(),
    };
    admin.apply_outbound_user_agent(custom.clone()).unwrap();
    for id in [&first, &second] {
        build_account_http_client(id.as_str(), None, "profile")
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    admin
        .account_facts_changed(std::slice::from_ref(&first))
        .await;
    for id in [&second, &first] {
        build_account_http_client(id.as_str(), None, "profile")
            .unwrap()
            .get(&url)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    admin.account_unavailable(&second).await;
    build_account_http_client(second.as_str(), None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    admin.apply_outbound_user_agent(custom).unwrap();
    build_account_http_client(first.as_str(), None, "profile")
        .unwrap()
        .get(&url)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        seen[0].0, seen[2].0,
        "unchanged default UA must preserve the pool"
    );
    assert_ne!(
        seen[0].0, seen[3].0,
        "actual UA update evicts old account clients"
    );
    assert_ne!(seen[1].0, seen[4].0);
    assert_eq!(
        seen[4].0, seen[5].0,
        "changing one account keeps other pools"
    );
    assert_ne!(seen[3].0, seen[6].0);
    assert_ne!(
        seen[5].0, seen[7].0,
        "unavailable account loses cached pool"
    );
    assert_eq!(
        seen[6].0, seen[8].0,
        "unchanged custom UA must preserve the pool"
    );
}

#[tokio::test]
async fn http_sse_accounts_use_distinct_proxies_and_clearing_restores_direct_client() {
    let (exit_a, a) = http_exit("exit_a").await;
    let (exit_b, b) = http_exit("exit_b").await;
    let (direct_url, direct) = http_exit("direct").await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        &direct_url,
        test_wire_profile(),
    );
    let client_a = base.for_account(&account("a", Some(&exit_a))).unwrap();
    let client_b = base.for_account(&account("b", Some(&exit_b))).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    let (response_a, response_b) = tokio::join!(
        client_a.create_response(&request, request_context("proxy-a", Some("a"))),
        client_b.create_response(&request, request_context("proxy-b", Some("b")))
    );
    assert!(response_a.unwrap().body.contains("resp_exit_a"));
    assert!(response_b.unwrap().body.contains("resp_exit_b"));
    let cleared = client_a.for_account(&account("a", None)).unwrap();
    assert!(
        cleared
            .create_response(&request, request_context("direct", Some("a")))
            .await
            .unwrap()
            .body
            .contains("resp_direct")
    );
    for head in [a.await.unwrap(), b.await.unwrap()] {
        assert!(head.starts_with(&format!("POST {direct_url}/codex/responses HTTP/1.1")));
    }
    assert!(
        direct
            .await
            .unwrap()
            .starts_with("POST /codex/responses HTTP/1.1")
    );
}

#[tokio::test]
async fn removing_account_proxy_reuses_the_original_direct_pool() {
    let (direct_url, direct) = pooled_http_exit(1, 2).await;
    let (proxy_url, proxy) = http_exit("temporary_proxy").await;
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        direct_url,
        test_wire_profile(),
    );
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    let initial = base.for_account(&account("proxy-roundtrip", None)).unwrap();
    initial
        .create_response(
            &request,
            request_context("before-proxy", Some("proxy-roundtrip")),
        )
        .await
        .unwrap();
    let proxied = initial
        .for_account(&account("proxy-roundtrip", Some(&proxy_url)))
        .unwrap();
    let response = proxied
        .create_response(
            &request,
            request_context("using-proxy", Some("proxy-roundtrip")),
        )
        .await
        .unwrap();
    assert!(response.body.contains("resp_temporary_proxy"));
    proxied
        .for_account(&account("proxy-roundtrip", None))
        .unwrap()
        .create_response(
            &request,
            request_context("after-proxy", Some("proxy-roundtrip")),
        )
        .await
        .unwrap();
    let seen = timeout(Duration::from_secs(5), direct)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(seen[0].0, seen[1].0);
    timeout(Duration::from_secs(5), proxy)
        .await
        .unwrap()
        .unwrap();
}

async fn websocket_exit(label: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let connect = read_http_request(&mut stream).await;
        assert!(connect.starts_with("CONNECT upstream.invalid:80 HTTP/1.1"));
        stream
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        let mut websocket = accept_codex_test_websocket(stream).await;
        assert!(matches!(websocket.next().await, Some(Ok(Message::Text(_)))));
        websocket
            .send(Message::Text(
                completed_websocket_response(label, 2, 1).into(),
            ))
            .await
            .unwrap();
        connect
    });
    (format!("http://user:$example@{address}"), task)
}

#[tokio::test]
async fn websocket_connect_and_pool_are_isolated_when_account_proxy_changes() {
    let (exit_a, a) = websocket_exit("resp_exit_a").await;
    let (exit_b, b) = websocket_exit("resp_exit_b").await;
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        "http://upstream.invalid",
        test_wire_profile(),
    )
    .with_websocket_pool(pool);
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.set_previous_response_id(Some("resp_previous".to_owned()));
    request.previous_response_scope = Some(PreviousResponseScope::Persisted);
    for (proxy, expected) in [(exit_a, "resp_exit_a"), (exit_b, "resp_exit_b")] {
        let client = base
            .for_account(&account("same-account", Some(&proxy)))
            .unwrap();
        let response = timeout(
            Duration::from_secs(5),
            client.create_response(&request, request_context("proxy-ws", Some("same-account"))),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.body.contains(expected));
    }
    for head in [a.await.unwrap(), b.await.unwrap()] {
        assert_eq!(
            read_header_value(&head, "Proxy-Authorization"),
            Some("Basic dXNlcjokZXhhbXBsZQ==")
        );
    }
}

#[tokio::test]
async fn unavailable_proxy_fails_without_direct_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unreachable = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let direct = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", direct.local_addr().unwrap()),
        test_wire_profile(),
    );
    let client = base.for_account(&account("a", Some(&unreachable))).unwrap();
    let mut request = codex_request("gpt-5.5", "", Vec::new());
    request.force_http_sse = true;
    assert!(
        client
            .create_response(&request, request_context("blocked", Some("a")))
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(30), direct.accept())
            .await
            .is_err()
    );
}

async fn socks_exit(stream: &mut TcpStream) -> (u8, String, u16) {
    assert_eq!(stream.read_u8().await.unwrap(), 5);
    let count = stream.read_u8().await.unwrap();
    let mut methods = vec![0; usize::from(count)];
    stream.read_exact(&mut methods).await.unwrap();
    assert!(methods.contains(&2));
    stream.write_all(&[5, 2]).await.unwrap();
    assert_eq!(stream.read_u8().await.unwrap(), 1);
    let count = stream.read_u8().await.unwrap();
    let mut user = vec![0; usize::from(count)];
    stream.read_exact(&mut user).await.unwrap();
    let count = stream.read_u8().await.unwrap();
    let mut password = vec![0; usize::from(count)];
    stream.read_exact(&mut password).await.unwrap();
    assert_eq!(user, b"user@exit");
    assert_eq!(password, b"pass:word");
    stream.write_all(&[1, 0]).await.unwrap();
    let mut header = [0; 4];
    stream.read_exact(&mut header).await.unwrap();
    assert_eq!(&header[..3], &[5, 1, 0]);
    let host = match header[3] {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip).await.unwrap();
            std::net::Ipv4Addr::from(ip).to_string()
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip).await.unwrap();
            std::net::Ipv6Addr::from(ip).to_string()
        }
        3 => {
            let count = stream.read_u8().await.unwrap();
            let mut host = vec![0; usize::from(count)];
            stream.read_exact(&mut host).await.unwrap();
            String::from_utf8(host).unwrap()
        }
        _ => panic!("unexpected SOCKS address type"),
    };
    let port = stream.read_u16().await.unwrap();
    stream
        .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
        .await
        .unwrap();
    (header[3], host, port)
}

#[tokio::test]
async fn socks_account_egress_preserves_dns_mode_auth_and_ipv6_for_sse_and_websocket() {
    for websocket in [false, true] {
        let mut targets = vec![
            ("socks5", "localhost"),
            ("socks5h", "upstream.invalid"),
            ("socks5h", "[::1]"),
        ];
        // reqwest's local resolver does not accept IPv6 literals; use socks5h for those.
        if websocket {
            targets.push(("socks5", "[::1]"));
        }
        for (scheme, host) in targets {
            let proxy_address = if host == "upstream.invalid" {
                "[::1]:0"
            } else {
                "127.0.0.1:0"
            };
            let listener = TcpListener::bind(proxy_address).await.unwrap();
            let proxy = format!(
                "{scheme}://user%40exit:pass%3Aword@{}",
                listener.local_addr().unwrap()
            );
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let target = socks_exit(&mut stream).await;
                if websocket {
                    let mut stream = accept_codex_test_websocket(stream).await;
                    assert!(matches!(stream.next().await, Some(Ok(Message::Text(_)))));
                    stream
                        .send(Message::Text(
                            completed_websocket_response("resp_socks", 2, 1).into(),
                        ))
                        .await
                        .unwrap();
                } else {
                    read_http_request_with_body(&mut stream).await;
                    let body = format!(
                        "data: {}\n\n",
                        completed_websocket_response("resp_socks", 2, 1)
                    );
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                target
            });
            let client = CodexBackendClient::new(
                provider_openai::transport::build_reqwest_client().unwrap(),
                format!("http://{host}"),
                test_wire_profile(),
            )
            .for_account(&account("socks", Some(&proxy)))
            .unwrap();
            let mut request = codex_request("gpt-5.5", "", Vec::new());
            request.force_http_sse = !websocket;
            if websocket {
                request.set_previous_response_id(Some("resp_previous".to_owned()));
                request.previous_response_scope = Some(PreviousResponseScope::Persisted);
            }
            let response = timeout(
                Duration::from_secs(5),
                client.create_response(&request, request_context("socks", Some("socks"))),
            )
            .await
            .unwrap()
            .unwrap_or_else(|error| {
                panic!("websocket={websocket}, scheme={scheme}, host={host}: {error}")
            });
            assert!(response.body.contains("resp_socks"));
            let (address_type, target, port) = task.await.unwrap();
            assert_eq!(port, 80);
            match host {
                "localhost" => {
                    assert!(matches!(address_type, 1 | 4));
                    assert!(target.parse::<std::net::IpAddr>().unwrap().is_loopback());
                }
                "[::1]" => assert_eq!((address_type, target.as_str()), (4, "::1")),
                _ => assert_eq!((address_type, target.as_str()), (3, host)),
            }
        }
    }
}

#[tokio::test]
async fn https_account_proxy_starts_tls_and_rejection_never_falls_back_to_direct() {
    for websocket in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("https://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = [0; 5];
            stream.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..2], &[22, 3]);
            let mut hello = vec![0; usize::from(u16::from_be_bytes([header[3], header[4]]))];
            stream.read_exact(&mut hello).await.unwrap();
            stream.write_all(&[21, 3, 3, 0, 2, 2, 40]).await.unwrap();
        });
        let direct = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{}", direct.local_addr().unwrap()),
            test_wire_profile(),
        )
        .for_account(&account("tls-proxy", Some(&proxy)))
        .unwrap();
        let mut request = codex_request("gpt-5.5", "", Vec::new());
        request.force_http_sse = !websocket;
        if websocket {
            request.set_previous_response_id(Some("resp_previous".to_owned()));
            request.previous_response_scope = Some(PreviousResponseScope::Persisted);
        }
        assert!(
            timeout(
                Duration::from_secs(5),
                client.create_response(&request, request_context("tls-proxy", Some("tls-proxy")))
            )
            .await
            .unwrap()
            .is_err()
        );
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(
            timeout(Duration::from_millis(30), direct.accept())
                .await
                .is_err()
        );
    }
}

// 以底层写入调用为边界模拟问题代理，避免将 TCP 单次读取误当成报文边界。
struct GreetingSensitiveProxy {
    greeting: Vec<u8>,
    replies: Cursor<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl GreetingSensitiveProxy {
    fn new(authenticated: bool) -> Self {
        let (greeting, mut replies) = if authenticated {
            (vec![5, 2, 0, 2], vec![5, 2, 1, 0])
        } else {
            (vec![5, 1, 0], vec![5, 0])
        };
        replies.extend_from_slice(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80]);
        Self {
            greeting,
            replies: Cursor::new(replies),
            writes: Vec::new(),
        }
    }
}

impl AsyncRead for GreetingSensitiveProxy {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.writes.first() != Some(&self.greeting) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy rejected partial method negotiation",
            )));
        }
        if buf.remaining() > 0 {
            let mut byte = [0];
            let count = Read::read(&mut self.replies, &mut byte)?;
            buf.put_slice(&byte[..count]);
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for GreetingSensitiveProxy {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writes.push(buf.to_vec());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn socks_proxy_flush_contract_coalesces_greeting_without_losing_auth_or_connect() {
    use tokio_tungstenite::proxy::connect_via_proxy;
    use tungstenite::proxy::ProxyConfig;

    for scheme in ["socks5", "socks5h"] {
        for authenticated in [false, true] {
            let authentication = if authenticated { "user:pass@" } else { "" };
            let config =
                ProxyConfig::parse(&format!("{scheme}://{authentication}127.0.0.1:1080")).unwrap();
            let raw = connect_via_proxy(
                GreetingSensitiveProxy::new(authenticated),
                &config,
                "upstream.invalid",
                443,
            )
            .await;
            assert!(matches!(raw, Err(tungstenite::Error::Io(ref error))
                if error.kind() == io::ErrorKind::UnexpectedEof));

            let buffered = connect_via_proxy(
                BufWriter::new(GreetingSensitiveProxy::new(authenticated)),
                &config,
                "upstream.invalid",
                443,
            )
            .await
            .unwrap();
            assert!(buffered.buffer().is_empty());
            let stream = buffered.into_inner();
            let mut expected = vec![stream.greeting];
            if authenticated {
                expected.push(b"\x01\x04user\x04pass".to_vec());
            }
            expected.push(b"\x05\x01\x00\x03\x10upstream.invalid\x01\xbb".to_vec());
            assert_eq!(stream.writes, expected);
            assert_eq!(
                stream.replies.position(),
                stream.replies.get_ref().len() as u64
            );
        }
    }
}
