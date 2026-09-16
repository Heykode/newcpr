use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gateway_core::operation::{GenerateRequest, ProtocolPayload};
use gateway_core::provider_ports::ProviderUserAgentOverride;

// Plain HTTP/WS fixtures verify profile routing and ownership, not TLS handshakes.
const QX_USER_AGENT: &str = "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color";
const INSTALLATION_ID: &str = "85c64ae8-4b44-4f82-9696-909adef2144c";
const TURN_METADATA: &str =
    r#"{"installation_id":"85c64ae8-4b44-4f82-9696-909adef2144c","turn_id":"fixture-turn"}"#;

fn select_profile(profile: &CodexWireProfileState, qx: bool) {
    profile
        .apply_user_agent_override(&if qx {
            ProviderUserAgentOverride::Custom {
                user_agent: provider_openai::transport::profile::qx::DEFAULT_USER_AGENT.to_owned(),
            }
        } else {
            ProviderUserAgentOverride::Default
        })
        .expect("select global wire profile");
}

fn identity_context(request_id: &str) -> CodexRequestContext<'_> {
    CodexRequestContext {
        installation_id: Some(INSTALLATION_ID),
        session_id: Some("existing-session"),
        thread_id: Some("existing-thread"),
        client_request_id: Some("existing-client-request"),
        turn_metadata: Some(TURN_METADATA),
        ..request_context(request_id, Some("existing-chatgpt-account"))
    }
}

fn fixture_request() -> CodexResponsesRequest {
    let mut body = codex_request_body(
        "gpt-test",
        "be brief",
        vec![json!({"role": "user", "content": "synthetic loopback input"})],
    );
    body.insert("store".to_owned(), json!(false));
    body.insert("stream".to_owned(), json!(false));
    body.insert("service_tier".to_owned(), json!("priority"));
    body.insert("prompt_cache_key".to_owned(), json!("explicit-cache-key"));
    body.insert(
        "metadata".to_owned(),
        json!({"installation_id": INSTALLATION_ID, "fixture": "preserve-existing-device"}),
    );
    let mut request = CodexResponsesRequest::from_body(body);
    request.local_conversation_id = Some("profile-owner-conversation".to_owned());
    request
}

fn assert_identity_header(name: &str, value: Option<&str>, qx: bool) {
    let expected = match name {
        "authorization" => Some("Bearer access-token"),
        "chatgpt-account-id" => Some("existing-chatgpt-account"),
        "session-id" => Some("existing-session"),
        "thread-id" => Some("existing-thread"),
        "x-client-request-id" => Some("existing-thread"),
        "x-codex-turn-metadata" => Some(TURN_METADATA),
        "x-codex-installation-id" => Some(INSTALLATION_ID),
        "session_id" => Some("existing-session"),
        _ => panic!("unexpected fixture header"),
    };
    assert_eq!(value, expected, "{name} (QX={qx})");
}

const IDENTITY_HEADERS: &[&str] = &[
    "authorization",
    "chatgpt-account-id",
    "session-id",
    "thread-id",
    "x-client-request-id",
    "x-codex-turn-metadata",
    "x-codex-installation-id",
    "session_id",
];

fn assert_body(body: &Value, websocket: bool, previous: Option<&str>) {
    assert_eq!(body["model"], "gpt-test");
    assert_eq!(body["instructions"], "be brief");
    assert_eq!(
        body["input"],
        json!([{"role": "user", "content": "synthetic loopback input"}])
    );
    assert_eq!(body["store"], false);
    assert_eq!(body["service_tier"], "priority");
    assert_eq!(body["prompt_cache_key"], "explicit-cache-key");
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["metadata"],
        json!({"installation_id": INSTALLATION_ID, "fixture": "preserve-existing-device"})
    );
    assert_eq!(
        body.get("previous_response_id").and_then(Value::as_str),
        previous
    );
    if websocket {
        assert_eq!(body["type"], "response.create");
    } else {
        assert!(body.get("type").is_none());
    }
    for local_only in [
        "force_http_sse",
        "use_websocket",
        "local_conversation_id",
        "downstream_websocket_connection_id",
        "tls_profile",
        "raw_user_agent",
    ] {
        assert!(
            body.get(local_only).is_none(),
            "{local_only} leaked upstream"
        );
    }
}

async fn receive_http(stream: &mut TcpStream, user_agent: &str, qx: bool) {
    let raw = read_http_request_with_body(stream).await;
    let head_end = raw.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&raw[..head_end]).unwrap();
    assert!(head.starts_with("POST /codex/responses HTTP/1.1\r\n"));
    assert_eq!(read_header_value(head, "user-agent"), Some(user_agent));
    assert_eq!(
        read_header_value(head, "originator"),
        Some(if qx { "codex-tui" } else { "codex_cli_rs" })
    );
    assert_eq!(
        read_header_value(head, "version"),
        Some(if qx { "0.146.0" } else { "1.2.3" })
    );
    assert_eq!(read_header_value(head, "accept"), Some("text/event-stream"));
    assert_eq!(
        read_header_value(head, "content-type"),
        Some("application/json")
    );
    assert_eq!(read_header_value(head, "content-encoding"), None);
    assert_eq!(
        read_header_value(head, "x-codex-routing-hint"),
        Some("model=gpt-test;tier=priority")
    );
    for name in IDENTITY_HEADERS {
        assert_identity_header(name, read_header_value(head, name), qx);
    }
    assert!(read_header_value(head, "openai-beta").is_none());
    assert_body(
        &serde_json::from_slice(&raw[head_end + 4..]).unwrap(),
        false,
        None,
    );
    let completed = completed_websocket_response("resp_http_fallback", 3, 1);
    let body = format!("event: response.completed\ndata: {completed}\n\n");
    assert_eq!(
        extract_sse_usage(&body)
            .unwrap()
            .expect("valid fixture usage")
            .total_tokens,
        4
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

async fn accept_profile_websocket(
    stream: TcpStream,
    user_agent: &str,
    qx: bool,
) -> tokio_tungstenite::WebSocketStream<TcpStream> {
    accept_codex_test_websocket_with(stream, |request, response| {
        assert_eq!(request.uri().path(), "/codex/responses");
        let headers = request.headers();
        assert_eq!(headers["user-agent"], user_agent);
        let default_qx_identity = user_agent == QX_USER_AGENT;
        assert_eq!(
            headers["originator"],
            if default_qx_identity {
                "codex-tui"
            } else {
                "codex_cli_rs"
            }
        );
        assert_eq!(
            headers["version"],
            if default_qx_identity {
                "0.146.0"
            } else {
                "1.2.3"
            }
        );
        assert_eq!(headers["openai-beta"], "responses_websockets=2026-02-06");
        for name in IDENTITY_HEADERS {
            assert_identity_header(
                name,
                headers.get(*name).map(|value| value.to_str().unwrap()),
                qx,
            );
        }
        response.headers_mut().insert(
            "sec-websocket-extensions",
            "permessage-deflate".parse().unwrap(),
        );
    })
    .await
}

async fn exchange(
    websocket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    response_id: &str,
    previous: Option<&str>,
) {
    let message = websocket.next().await.unwrap().unwrap();
    assert!(message.is_text(), "expected response.create");
    assert_body(
        &serde_json::from_str(message.to_text().unwrap()).unwrap(),
        true,
        previous,
    );
    websocket
        .send(Message::Text(
            completed_websocket_response(response_id, 2, 1).into(),
        ))
        .await
        .unwrap();
}

#[tokio::test]
async fn global_qx_http_uses_cli_headers_and_preserves_existing_device_metadata() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let profile = test_wire_profile();
    let cpr_user_agent = profile.snapshot().user_agent();
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        profile.clone(),
    );
    let server = async {
        for (user_agent, qx) in [(QX_USER_AGENT, true), (cpr_user_agent.as_str(), false)] {
            let (mut stream, _) = listener.accept().await.unwrap();
            receive_http(&mut stream, user_agent, qx).await;
        }
    };
    let client = async {
        for qx in [true, false] {
            select_profile(&profile, qx);
            let mut request = fixture_request();
            request.force_http_sse = true;
            let response = backend
                .create_response(&request, identity_context("http-profile"))
                .await
                .expect("selected profile HTTP request");
            assert_eq!(response.transport, CodexBackendTransport::HttpSse);
            assert!(response.body.contains("resp_http_fallback"));
            assert_eq!(response.usage.unwrap().total_tokens, 4);
        }
    };
    timeout(Duration::from_secs(6), async {
        tokio::join!(server, client)
    })
    .await
    .expect("bounded HTTP fixture");
}

#[tokio::test]
async fn both_profiles_send_only_the_selected_account_installation_over_http_and_ws() {
    for qx in [false, true] {
        for websocket in [false, true] {
            for installation in [
                Some(INSTALLATION_ID),
                Some("d47ee351-2066-43ee-841e-651b21983808"),
                None,
                Some(""),
                Some("   "),
            ] {
                let expected = installation.filter(|value| !value.trim().is_empty());
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let profile = test_wire_profile();
                select_profile(&profile, qx);
                let backend = CodexBackendClient::new(
                    reqwest::Client::builder().no_proxy().build().unwrap(),
                    format!("http://{}", listener.local_addr().unwrap()),
                    profile,
                )
                .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_mins(1))));
                let payload =
                    ProtocolPayload::json_object("openai", fixture_request().body().clone())
                        .unwrap()
                        .with_context(Map::from_iter([(
                            "opaque_request_headers".to_owned(),
                            json!([
                                [
                                    "x-codex-installation-id",
                                    STANDARD.encode("downstream-device-a")
                                ],
                                [
                                    "X-Codex-Installation-Id",
                                    STANDARD.encode("downstream-device-b")
                                ]
                            ]),
                        )]));
                let mut request = provider_openai::encode_generate_request(
                    &GenerateRequest::from_protocol_payload(payload),
                    "gpt-test",
                    &Default::default(),
                )
                .unwrap();
                request.force_http_sse = !websocket;
                if websocket {
                    request = websocket_only_request(request);
                }
                let body_before = request.body().clone();
                let mut context = identity_context("selected-device");
                context.installation_id = installation;
                let server = async {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    if websocket {
                        let mut socket =
                            accept_codex_test_websocket_with(stream, |request, response| {
                                let values = request
                                    .headers()
                                    .get_all("x-codex-installation-id")
                                    .iter()
                                    .map(|value| value.to_str().unwrap())
                                    .collect::<Vec<_>>();
                                assert_eq!(values, expected.into_iter().collect::<Vec<_>>());
                                response.headers_mut().insert(
                                    "sec-websocket-extensions",
                                    "permessage-deflate".parse().unwrap(),
                                );
                            })
                            .await;
                        exchange(&mut socket, "resp_device", None).await;
                    } else {
                        let raw = read_http_request_with_body(&mut stream).await;
                        let end = raw.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
                        let head = std::str::from_utf8(&raw[..end]).unwrap();
                        assert_eq!(read_header_value(head, "x-codex-installation-id"), expected);
                        assert_eq!(
                            head.lines()
                                .filter(|line| line
                                    .to_ascii_lowercase()
                                    .starts_with("x-codex-installation-id:"))
                                .count(),
                            usize::from(expected.is_some())
                        );
                        assert_eq!(read_header_value(head, "content-encoding"), None);
                        assert_body(
                            &serde_json::from_slice(&raw[end + 4..]).unwrap(),
                            false,
                            None,
                        );
                        let completed = completed_websocket_response("resp_device", 2, 1);
                        let body = format!("event: response.completed\ndata: {completed}\n\n");
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                };
                let client = async {
                    let response = backend.create_response(&request, context).await.unwrap();
                    assert!(response.body.contains("resp_device"));
                    assert_eq!(response.usage.unwrap().total_tokens, 3);
                    assert_eq!(
                        response.transport,
                        if websocket {
                            CodexBackendTransport::WebSocket
                        } else {
                            CodexBackendTransport::HttpSse
                        }
                    );
                    assert_eq!(request.body(), &body_before);
                };
                timeout(Duration::from_secs(5), async {
                    tokio::join!(server, client)
                })
                .await
                .expect("bounded installation header fixture");
            }
        }
    }
}

#[tokio::test]
async fn both_profiles_reject_unsafe_installation_before_connecting() {
    for qx in [false, true] {
        for websocket in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let profile = test_wire_profile();
            select_profile(&profile, qx);
            let backend = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                format!("http://{}", listener.local_addr().unwrap()),
                profile,
            )
            .with_websocket_pool(Arc::new(CodexWebSocketPool::new(Duration::from_mins(1))));
            let mut request = fixture_request();
            request.force_http_sse = !websocket;
            if websocket {
                request = websocket_only_request(request);
            }
            let mut context = identity_context("unsafe-device");
            context.installation_id = Some("device\r\nx-injected: invalid");
            let result = timeout(
                Duration::from_secs(2),
                backend.create_response(&request, context),
            )
            .await
            .expect("invalid installation must fail promptly");
            assert!(matches!(
                result,
                Err(CodexClientError::InvalidHeaderValue(_))
            ));
            assert!(
                timeout(Duration::from_millis(20), listener.accept())
                    .await
                    .is_err(),
                "unsafe installation must be rejected before network activity"
            );
        }
    }
}

async fn assert_profile_switch_keeps_exact_owner(initial_qx: bool, identical_identity: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut initial = test_wire_profile().snapshot();
    // Changing only default/custom selection with identical wire UA must not rotate sockets.
    initial.os_type = "Linux".to_owned();
    let profile = CodexWireProfileState::new(initial);
    let cpr_user_agent = profile.snapshot().user_agent();
    let select = |qx| {
        if qx && identical_identity {
            profile
                .apply_user_agent_override(&ProviderUserAgentOverride::Custom {
                    user_agent: cpr_user_agent.clone(),
                })
                .expect("custom selection with identical application identity");
            assert_eq!(profile.snapshot().user_agent(), cpr_user_agent);
        } else {
            select_profile(&profile, qx);
        }
    };
    select(initial_qx);
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        profile.clone(),
    )
    .with_websocket_pool(pool.clone());
    let server = async {
        let ua = if initial_qx && !identical_identity {
            QX_USER_AGENT
        } else {
            &cpr_user_agent
        };
        let (stream, _) = listener.accept().await.unwrap();
        let mut owner = accept_profile_websocket(stream, ua, initial_qx).await;
        exchange(&mut owner, "resp_old_owner", None).await;
        exchange(
            &mut owner,
            "resp_old_owner_continued",
            Some("resp_old_owner"),
        )
        .await;
        if identical_identity {
            exchange(&mut owner, "resp_new_profile", None).await;
            return;
        }
        // Keep the old socket alive. An independent chain on the same lane must
        // use a second connection because its global profile has changed.
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => accepted.unwrap(),
            message = owner.next() => panic!("independent chain reused old profile socket: {message:?}"),
        };
        let ua = if initial_qx || identical_identity {
            &cpr_user_agent
        } else {
            QX_USER_AGENT
        };
        let mut fresh = accept_profile_websocket(stream, ua, !initial_qx).await;
        exchange(&mut fresh, "resp_new_profile", None).await;
    };
    let client = async {
        let first_request = websocket_only_request(fixture_request());
        let first = backend
            .create_response_stream(&first_request, identity_context("owner-first"))
            .await
            .expect("initial owner");
        let owner_id = first.websocket_connection_id.expect("initial socket UUID");
        let first = collect_backend_response(first, Instant::now())
            .await
            .unwrap();
        assert_eq!(first.transport, CodexBackendTransport::WebSocket);
        assert_eq!(first.websocket_pool_decision.unwrap().kind(), "new");
        assert!(first.connection_local_continuation);

        select(!initial_qx);
        let mut continuation = first_request.clone();
        continuation.set_previous_response_id(Some("resp_old_owner".to_owned()));
        continuation.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
        let second = backend
            .create_response_stream(&continuation, identity_context("owner-continuation"))
            .await
            .expect("exact continuation after profile switch");
        assert_eq!(second.websocket_connection_id, Some(owner_id));
        let second = collect_backend_response(second, Instant::now())
            .await
            .unwrap();
        assert_eq!(second.transport, CodexBackendTransport::WebSocket);
        assert_eq!(second.websocket_pool_decision.unwrap().kind(), "reuse");
        assert!(second.body.contains("resp_old_owner_continued"));

        let fresh = backend
            .create_response_stream(&first_request, identity_context("independent-chain"))
            .await
            .expect("independent chain uses new global profile");
        assert_eq!(
            fresh.websocket_connection_id.expect("socket UUID") == owner_id,
            identical_identity
        );
        let fresh = collect_backend_response(fresh, Instant::now())
            .await
            .unwrap();
        assert_eq!(fresh.transport, CodexBackendTransport::WebSocket);
        assert_eq!(
            fresh.websocket_pool_decision.unwrap().kind(),
            if identical_identity { "reuse" } else { "new" }
        );
        assert!(fresh.body.contains("resp_new_profile"));
    };
    let outcome = timeout(Duration::from_secs(6), async {
        tokio::join!(server, client)
    })
    .await;
    pool.shutdown().await;
    outcome.expect("bounded profile switch fixture");
}

#[tokio::test]
async fn cpr_to_qx_keeps_exact_ws_owner_and_switches_independent_chain() {
    assert_profile_switch_keeps_exact_owner(false, false).await;
}

#[tokio::test]
async fn qx_to_cpr_keeps_exact_ws_owner_and_switches_independent_chain() {
    assert_profile_switch_keeps_exact_owner(true, false).await;
}

#[tokio::test]
async fn same_user_agent_keeps_unified_ws_owner_across_selection_change() {
    assert_profile_switch_keeps_exact_owner(false, true).await;
}

#[tokio::test]
async fn safe_ws_http_fallback_freezes_qx_profile_while_next_request_uses_cpr() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let profile = test_wire_profile();
    let cpr_user_agent = profile.snapshot().user_agent();
    select_profile(&profile, true);
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        profile.clone(),
    )
    .with_websocket_pool(pool.clone());
    let server = async {
        let (mut opening, _) = listener.accept().await.unwrap();
        let head = read_http_request(&mut opening).await;
        assert!(head.starts_with("GET /codex/responses HTTP/1.1\r\n"));
        assert_eq!(read_header_value(&head, "user-agent"), Some(QX_USER_AGENT));
        assert_eq!(read_header_value(&head, "originator"), Some("codex-tui"));
        assert_eq!(read_header_value(&head, "version"), Some("0.146.0"));
        for name in IDENTITY_HEADERS {
            assert_identity_header(name, read_header_value(&head, name), true);
        }
        select_profile(&profile, false);
        // The incomplete opening forces the existing bounded pre-payload
        // fallback; no response.create has been accepted by this upstream.
        let (mut fallback, _) = listener.accept().await.unwrap();
        receive_http(&mut fallback, QX_USER_AGENT, true).await;
        let (mut fresh, _) = listener.accept().await.unwrap();
        receive_http(&mut fresh, &cpr_user_agent, false).await;
        drop(opening);
    };
    let client = async {
        let mut request = fixture_request();
        let response = backend
            .create_response(&request, identity_context("safe-fallback"))
            .await
            .expect("safe HTTP fallback");
        assert_eq!(response.transport, CodexBackendTransport::HttpSse);
        assert!(response.body.contains("resp_http_fallback"));
        request.force_http_sse = true;
        let fresh = backend
            .create_response(&request, identity_context("after-fallback"))
            .await
            .expect("next request observes CPR");
        assert_eq!(fresh.transport, CodexBackendTransport::HttpSse);
        assert!(fresh.body.contains("resp_http_fallback"));
    };
    let outcome = timeout(Duration::from_secs(6), async {
        tokio::join!(server, client)
    })
    .await;
    pool.shutdown().await;
    outcome.expect("bounded fallback fixture");
}

#[tokio::test]
async fn every_ua_selection_retains_native_ws_custom_ca_validation() {
    use provider_openai::transport::tls::{CODEX_CA_CERT_ENV, SSL_CERT_FILE_ENV};

    const CASE_ENV: &str = "CODEX_PROXY_TEST_QX_PLAIN_WS_CA_CASE";
    const COMPLETED: &str = "qx-plain-ws-ca-case-completed:";
    const TEST_NAME: &str = concat!(
        "transport::qx_profile::",
        "every_ua_selection_retains_native_ws_custom_ca_validation"
    );

    if let Ok(case) = std::env::var(CASE_ENV) {
        let (qx, missing) = match case.as_str() {
            "qx-missing" => (true, true),
            "qx-empty" => (true, false),
            "cpr-missing" => (false, true),
            "cpr-empty" => (false, false),
            _ => panic!("unknown isolated CA case: {case}"),
        };
        let ca_path = std::path::PathBuf::from(std::env::var_os(CODEX_CA_CERT_ENV).unwrap());
        if missing {
            assert!(!ca_path.exists());
        } else {
            assert!(std::fs::read(&ca_path).unwrap().is_empty());
        }
        timeout(
            Duration::from_secs(6),
            assert_plain_ws_custom_ca_behavior(qx, missing, &ca_path),
        )
        .await
        .expect("bounded isolated CA regression");
        println!("\n{COMPLETED}{case}");
        return;
    }

    let fixtures = tempfile::tempdir().expect("isolated CA fixture directory");
    let missing = fixtures.path().join("missing-ca.pem");
    let empty = tempfile::NamedTempFile::new_in(fixtures.path()).expect("empty CA fixture");
    let current_exe = std::env::current_exe().expect("current test binary");
    for (case, path) in [
        ("qx-missing", missing.as_path()),
        ("qx-empty", empty.path()),
        ("cpr-missing", missing.as_path()),
        ("cpr-empty", empty.path()),
    ] {
        let mut command = Command::new(&current_exe);
        command
            .arg("--exact")
            .arg(TEST_NAME)
            .arg("--nocapture")
            .env(CASE_ENV, case)
            .env(CODEX_CA_CERT_ENV, path)
            .env_remove(SSL_CERT_FILE_ENV);
        // Keep the inherited developer proxy configuration out of this
        // synthetic loopback request, including the legacy CPR connector.
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env_remove(name);
        }
        let output = command.output().expect("run isolated plain WS CA case");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "isolated case {case} failed\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let marker = format!("{COMPLETED}{case}");
        assert_eq!(
            stdout.lines().filter(|line| *line == marker).count(),
            1,
            "isolated case {case} must execute exactly once\nstdout:\n{stdout}"
        );
    }
}

async fn assert_plain_ws_custom_ca_behavior(qx: bool, missing: bool, ca_path: &std::path::Path) {
    use provider_openai::transport::tls::{CODEX_CA_CERT_ENV, CustomCaError};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let profile = test_wire_profile();
    if qx {
        select_profile(&profile, true);
    }
    let pool = Arc::new(CodexWebSocketPool::new(Duration::from_mins(1)));
    // The supplied reqwest client deliberately does not invoke CPR's custom
    // CA builder: this regression exercises the WebSocket opening itself.
    let backend = CodexBackendClient::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        format!("http://{}", listener.local_addr().unwrap()),
        profile,
    )
    .with_websocket_pool(pool.clone());
    let request = websocket_only_request(fixture_request());
    let error = backend
        .create_response(&request, identity_context("cpr-invalid-ca"))
        .await
        .expect_err("default CPR must retain its original custom CA validation");
    let ca_error = std::iter::successors(
        Some(&error as &(dyn std::error::Error + 'static)),
        |error| error.source(),
    )
    .find_map(|error| {
        error.downcast_ref::<CustomCaError>().or_else(|| {
            // io::Error::source forwards to the wrapped error's source;
            // get_ref retains the actual CustomCaError payload.
            error
                .downcast_ref::<std::io::Error>()?
                .get_ref()?
                .downcast_ref::<CustomCaError>()
        })
    })
    .expect("CPR failure must originate from custom CA loading");
    match ca_error {
        CustomCaError::ReadCaFile {
            source_env,
            path,
            source,
        } if missing => {
            assert_eq!(*source_env, CODEX_CA_CERT_ENV);
            assert_eq!(path, ca_path);
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        CustomCaError::InvalidCaFile {
            source_env, path, ..
        } if !missing => {
            assert_eq!(*source_env, CODEX_CA_CERT_ENV);
            assert_eq!(path, ca_path);
        }
        other => panic!("unexpected CPR CA error: {other}"),
    }
    assert!(
        timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err(),
        "default CPR must fail CA validation before opening a socket"
    );
    pool.shutdown().await;
}
