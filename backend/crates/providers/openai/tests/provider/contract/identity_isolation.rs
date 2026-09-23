use super::*;

#[tokio::test]
async fn changed_turn_clears_opaque_header_and_all_known_body_mirrors() {
    let request = capture_turn_state_request_with_mirrors(
        "req_changed_turn_mirrors",
        Some("old"),
        Some("new"),
        Some("stale-state"),
        true,
    )
    .await;
    assert!(captured_header_values(&request, "x-codex-turn-state").is_empty());
    let body = captured_request_body(&request);
    assert!(body.get("turn_state").is_none());
    assert!(body["client_metadata"].get("x-codex-turn-state").is_none());
    assert!(body["client_metadata"].get("turnState").is_none());
    assert_eq!(body["client_metadata"]["keep"], "business");
    assert_eq!(body["input"][0]["content"][0]["text"], "current input");
    for raw in [
        body["turn_metadata"].as_str().unwrap(),
        body["client_metadata"]["turn_metadata"].as_str().unwrap(),
        request.headers["x-codex-turn-metadata"].to_str().unwrap(),
    ] {
        assert!(raw.is_ascii(), "new-turn metadata must remain ASCII JSON");
        let fields: Value = serde_json::from_str(raw).unwrap();
        assert!(fields.get("turn_state").is_none());
        assert_eq!(fields["keep"], "business");
        assert_eq!(fields["workspace"], "/tmp/\u{4e2d}\u{6587}/\u{1f680}");
    }
}

#[tokio::test]
async fn matching_turn_preserves_explicit_state_and_business_content() {
    let request = capture_turn_state_request_with_mirrors(
        "req_matching_turn_mirrors",
        Some("same"),
        Some("same"),
        Some("current-state"),
        true,
    )
    .await;
    assert_eq!(
        captured_header_values(&request, "x-codex-turn-state"),
        vec![b"current-state".to_vec()]
    );
    assert_eq!(
        captured_request_body(&request)["client_metadata"]["keep"],
        "business"
    );
}

#[test]
fn fallback_includes_string_input_and_normalizes_equivalent_text_messages() {
    let accounts = Arc::new(MemoryAccountStore::default());
    let provider = provider(&accounts);
    let affinity = |input| {
        let request = Operation::Generate(GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({"model":"gpt-5.4","instructions":"shared","input":input})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap(),
        ));
        provider
            .request_observation(&request, &ClientApiKeyId::new("test-key").unwrap())
            .continuation
            .affinity_hash
    };
    let string = affinity(json!("first"));
    assert!(string.is_some());
    assert_ne!(string, affinity(json!("different")));
    assert_eq!(string, affinity(json!([{"role":"user","content":"first"}])));
    assert_eq!(
        string,
        affinity(json!([{
            "role":"user","content":[{"type":"input_text","text":"first"}]
        }]))
    );
}

fn scoped_context(request_id: &str, client_key: &str, account: &str) -> AttemptContext {
    scoped_context_with_profile(request_id, client_key, account, None)
}

fn scoped_context_with_profile(
    request_id: &str,
    client_key: &str,
    account: &str,
    profile: Option<Arc<OpaqueProviderData>>,
) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new(format!("req_{request_id}")).unwrap(),
            ClientApiKeyId::new(client_key).unwrap(),
        )
        .with_request_profile(profile),
        NonZeroU32::new(1).unwrap(),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::diagnostic(
            BTreeSet::new(),
            ProviderAccountId::new(account).unwrap(),
            None,
        ),
        None,
        CancellationToken::new(),
    )
}

#[tokio::test]
async fn frozen_request_profile_survives_publication_without_reusing_account_identity() {
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        for account in ["acct_profile_first", "acct_profile_second"] {
            create_account(&store, account).await;
        }
        let http = MockServer::start().await;
        let (url, ws_server) = if websocket {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let mut captures = Vec::new();
                for _ in 0..3 {
                    let (socket, _) = listener.accept().await.unwrap();
                    let headers = Arc::new(Mutex::new(None));
                    let captured = headers.clone();
                    let mut socket = crate::transport::accept_codex_test_websocket_with(
                        socket,
                        move |request, response| {
                            *captured.lock().unwrap() = Some(request.headers().clone());
                            response.headers_mut().insert(
                                "sec-websocket-extensions",
                                "permessage-deflate".parse().unwrap(),
                            );
                        },
                    )
                    .await;
                    let frame = socket.next().await.unwrap().unwrap();
                    let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    super::generate_compat::send_completion(&mut socket, "resp_profile").await;
                    captures.push((headers.lock().unwrap().take().unwrap(), body));
                }
                captures
            });
            (url, Some(server))
        } else {
            Mock::given(method("POST"))
                .and(path("/codex/responses"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(CAPTURE_COMPLETED_SSE),
                )
                .expect(3)
                .mount(&http)
                .await;
            (http.uri(), None)
        };
        let state = wire_profile();
        let old_ua = state.snapshot().user_agent();
        let provider = provider_and_quota_with_profile(
            &store,
            Arc::new(MemorySessionAffinity::default()),
            url,
            Arc::new(TestLeaseCoordinator::default()),
            0,
            state.clone(),
        )
        .0;
        let frozen = Arc::new(provider.resolve_request_profile().unwrap().unwrap());
        for (index, account) in [
            "acct_profile_first",
            "acct_profile_second",
            "acct_profile_second",
        ]
        .into_iter()
        .enumerate()
        {
            if index == 1 {
                state.update_bundled_release(
                    &provider_openai::transport::profile::CodexBundledReleaseProfile {
                        codex_version: "0.155.0".to_owned(),
                        desktop_version: "26.999.12345".to_owned(),
                        desktop_build: "2".to_owned(),
                        verified_at: Utc::now(),
                    },
                );
            }
            let profile = if index < 2 {
                Arc::clone(&frozen)
            } else {
                Arc::new(provider.resolve_request_profile().unwrap().unwrap())
            };
            consume_identity_request(
                &provider,
                identity_request(json!("hello"), true, websocket),
                scoped_context_with_profile("profile-retry", "key-profile", account, Some(profile)),
            )
            .await;
        }
        let captures = if let Some(server) = ws_server {
            timeout(Duration::from_secs(5), server)
                .await
                .unwrap()
                .unwrap()
        } else {
            http.received_requests()
                .await
                .unwrap()
                .into_iter()
                .map(|request| (request.headers.clone(), captured_request_body(&request)))
                .collect()
        };
        assert_eq!(captures[0].0["user-agent"], old_ua);
        assert_eq!(captures[1].0["user-agent"], old_ua);
        assert_eq!(captures[2].0["user-agent"], state.snapshot().user_agent());
        assert_ne!(
            captures[0].0["authorization"],
            captures[1].0["authorization"]
        );
        assert_ne!(
            captures[0].1["client_metadata"]["x-codex-installation-id"],
            captures[1].1["client_metadata"]["x-codex-installation-id"]
        );
        assert_ne!(captures[0].0["thread-id"], captures[1].0["thread-id"]);
        assert_eq!(
            captures[1].1["client_metadata"]["x-codex-installation-id"],
            captures[2].1["client_metadata"]["x-codex-installation-id"]
        );
        for ((headers, body), account) in captures.iter().zip([
            "acct_profile_first",
            "acct_profile_second",
            "acct_profile_second",
        ]) {
            assert!(!headers.contains_key("x-codex-installation-id"));
            let selected = store
                .repository()
                .load_runtime_credential(&store.account(account).unwrap())
                .await
                .unwrap();
            assert_eq!(
                body["client_metadata"]["x-codex-installation-id"],
                selected.installation_id
            );
        }
    }
}

#[tokio::test]
async fn proxy_location_precedence_preserves_fallback_identity_on_http_and_ws() {
    use gateway_core::account::RequestLocation;
    const ACCOUNT: &str = "acct_proxy_location";
    let global = RequestLocation {
        country: "US".into(),
        region: "California".into(),
        city: "Los Angeles".into(),
        timezone: chrono_tz::America::Los_Angeles,
    };
    let proxy = RequestLocation {
        country: "JP".into(),
        region: "Tokyo".into(),
        city: "Tokyo".into(),
        timezone: chrono_tz::Asia::Tokyo,
    };
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let mut identities = Vec::new();
        for (enabled, proxy_location, global_location, expected) in [
            (false, Some(proxy.clone()), Some(global.clone()), "London"),
            (true, None, None, "Auckland"),
            (true, None, Some(global.clone()), "Los Angeles"),
            (true, Some(proxy.clone()), Some(global.clone()), "Tokyo"),
        ] {
            store.set_request_location(ACCOUNT, proxy_location);
            let http = MockServer::start().await;
            let (url, ws_server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(socket).await;
                    let frame = socket.next().await.unwrap().unwrap();
                    let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    super::generate_compat::send_completion(&mut socket, "resp_location").await;
                    body
                });
                (url, Some(server))
            } else {
                Mock::given(method("POST"))
                    .and(path("/codex/responses"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "text/event-stream")
                            .set_body_string(CAPTURE_COMPLETED_SSE),
                    )
                    .expect(1)
                    .mount(&http)
                    .await;
                (http.uri(), None)
            };
            // No explicit thread/cache key: exercise CPR's content fallback.
            let body = json!({
                "model":"gpt-5.4", "input":"same unmarked question",
                "prompt_cache_retention":"24h",
                "tools":[{"type":"web_search","user_location":{"city":"London","timezone":"Europe/London"}}]
            });
            let request = GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                    .unwrap()
                    .with_context(Map::from_iter([("use_websocket".into(), json!(websocket))])),
            );
            let request_context = RequestAttemptContext::new(
                ModelRequestId::new("req_proxy_location").unwrap(),
                ClientApiKeyId::new("key-proxy-location").unwrap(),
            )
            .with_request_location(global_location);
            let context = AttemptContext::new(
                request_context,
                NonZeroU32::new(1).unwrap(),
                SystemTime::now() + Duration::from_secs(30),
                account_policy(),
                AccountAttemptContext::diagnostic(
                    BTreeSet::new(),
                    ProviderAccountId::new(ACCOUNT).unwrap(),
                    None,
                ),
                None,
                CancellationToken::new(),
            )
            .with_request_tuning(gateway_core::routing::RequestTuning {
                openai_location_override_enabled: enabled,
                ..Default::default()
            });
            consume_identity_request(&qx_provider(&store, url), request, context).await;
            let captured = if let Some(server) = ws_server {
                timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                let requests = http.received_requests().await.unwrap();
                let body = captured_request_body(&requests[0]);
                assert!(!requests[0].headers.contains_key("x-codex-installation-id"));
                body
            };
            let selected = store
                .repository()
                .load_runtime_credential(&store.account(ACCOUNT).unwrap())
                .await
                .unwrap();
            assert_eq!(
                captured["client_metadata"]["x-codex-installation-id"],
                selected.installation_id
            );
            assert_eq!(captured["tools"][0]["user_location"]["city"], expected);
            assert!(captured.get("prompt_cache_retention").is_none());
            assert!(captured["client_metadata"]["x-codex-installation-id"].is_string());
            assert!(captured["client_metadata"].get("installation_id").is_none());
            if enabled {
                identities.push(captured["prompt_cache_key"].clone());
            }
        }
        // Enabling the pre-existing overwrite policy may change an unmarked key;
        // new runtime and proxy overrides must not introduce further changes.
        assert!(identities.windows(2).all(|pair| pair[0] == pair[1]));
    }
}

pub(super) fn qx_provider(store: &Arc<MemoryAccountStore>, url: String) -> CodexProvider {
    let selected = wire_profile().snapshot();
    provider_and_quota_with_profile(
        store,
        Arc::new(MemorySessionAffinity::default()),
        url,
        Arc::new(TestLeaseCoordinator::default()),
        0,
        CodexWireProfileState::new(selected),
    )
    .0
}

#[tokio::test]
async fn location_switch_controls_actual_http_and_ws_payloads_without_changing_identity() {
    const ACCOUNT: &str = "acct_location_switch";
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let mut identities = Vec::new();
        for enabled in [false, true] {
            let http = MockServer::start().await;
            let (url, ws_server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(socket).await;
                    let frame = socket.next().await.unwrap().unwrap();
                    let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    super::generate_compat::send_completion(&mut socket, "resp_location").await;
                    body
                });
                (url, Some(server))
            } else {
                Mock::given(method("POST"))
                    .and(path("/codex/responses"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "text/event-stream")
                            .set_body_string(CAPTURE_COMPLETED_SSE),
                    )
                    .expect(1)
                    .mount(&http)
                    .await;
                (http.uri(), None)
            };
            let body = json!({
                "model":"gpt-5.4", "input":"same question",
                "thread_id":"location-thread", "session_id":"location-session",
                "tools":[{"type":"web_search","user_location":{"city":"London","timezone":"Europe/London"}}]
            });
            let request = GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                    .unwrap()
                    .with_context(Map::from_iter([(
                        "use_websocket".to_owned(),
                        json!(websocket),
                    )])),
            );
            consume_identity_request(
                &qx_provider(&store, url),
                request,
                scoped_context("location-switch", "key-location", ACCOUNT).with_request_tuning(
                    gateway_core::routing::RequestTuning {
                        openai_location_override_enabled: enabled,
                        ..Default::default()
                    },
                ),
            )
            .await;
            let captured = if let Some(server) = ws_server {
                timeout(Duration::from_secs(5), server)
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                captured_request_body(&http.received_requests().await.unwrap()[0])
            };
            assert_eq!(
                captured["tools"][0]["user_location"]["city"],
                if enabled { "Auckland" } else { "London" }
            );
            assert_eq!(body["tools"][0]["user_location"]["city"], "London");
            identities.push(captured["prompt_cache_key"].clone());
        }
        assert_eq!(identities[0], identities[1]);
    }
}

#[tokio::test]
async fn generate_compatibility_matches_wire_and_session_facts_in_both_profiles_and_transports() {
    const ACCOUNT: &str = "acct_generate_compat";
    for qx in [false, true] {
        for websocket in [false, true] {
            for store_value in [
                None,
                Some(json!(true)),
                Some(json!(false)),
                Some(Value::Null),
            ] {
                let store = Arc::new(MemoryAccountStore::default());
                create_account(&store, ACCOUNT).await;
                let http = MockServer::start().await;
                let (url, ws_server) = if websocket {
                    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let url = format!("http://{}", listener.local_addr().unwrap());
                    let server = tokio::spawn(async move {
                        let (socket, _) = listener.accept().await.unwrap();
                        let mut socket = accept_codex_test_websocket(socket).await;
                        let frame = socket.next().await.unwrap().unwrap();
                        let body: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                        super::generate_compat::send_completion(&mut socket, "resp_compat").await;
                        body
                    });
                    (url, Some(server))
                } else {
                    Mock::given(method("POST"))
                        .and(path("/codex/responses"))
                        .respond_with(
                            ResponseTemplate::new(200)
                                .insert_header("content-type", "text/event-stream")
                                .set_body_string(CAPTURE_COMPLETED_SSE),
                        )
                        .expect(1)
                        .mount(&http)
                        .await;
                    (http.uri(), None)
                };
                let provider = if qx {
                    qx_provider(&store, url)
                } else {
                    provider_with_base_url(&store, url)
                };
                let (input, expected_input) = if store_value.as_ref() == Some(&json!(true)) {
                    super::generate_compat::item_id_fixture()
                } else {
                    (
                        json!("  hello\n"),
                        json!([{
                            "type":"message", "role":"user",
                            "content":[{"type":"input_text","text":"  hello\n"}]
                        }]),
                    )
                };
                let mut body = json!({
                    "model":"gpt-5.4", "input":input, "stream":false,
                    "session_id":"compat-session", "thread_id":"compat-thread",
                    "top_p":0.8, "frequency_penalty":0.2, "presence_penalty":0.3,
                    "max_completion_tokens":100, "max_output_tokens":100, "temperature":0.7,
                    "future_field":{"keep":true}, "service_tier":"fast"
                });
                if let Some(value) = store_value {
                    body["store"] = value;
                }
                let request = GenerateRequest::from_protocol_payload(
                    ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                        .unwrap()
                        .with_context(Map::from_iter([(
                            "use_websocket".to_owned(),
                            json!(websocket),
                        )])),
                );
                let state = consume_identity_request(
                    &provider,
                    request,
                    scoped_context("compat", "key-compat", ACCOUNT),
                )
                .await
                .unwrap();
                let captured = if let Some(server) = ws_server {
                    timeout(Duration::from_secs(5), server)
                        .await
                        .unwrap()
                        .unwrap()
                } else {
                    captured_request_body(&http.received_requests().await.unwrap()[0])
                };
                assert_eq!(
                    captured.get("store"),
                    Some(body.get("store").unwrap_or(&Value::Bool(false)))
                );
                assert_eq!(captured["stream"], true);
                assert_eq!(captured["input"], expected_input);
                assert_eq!(captured["service_tier"], "priority");
                assert_eq!(captured["future_field"], body["future_field"]);
                for field in [
                    "top_p",
                    "frequency_penalty",
                    "presence_penalty",
                    "max_completion_tokens",
                ] {
                    assert_eq!(captured.get(field), body.get(field), "{field}");
                }
                for field in ["max_output_tokens", "temperature"] {
                    assert!(captured.get(field).is_none(), "{field}");
                }
                assert_eq!(
                    state.payload()["continuation_scope"],
                    if captured["store"] == true {
                        "persisted"
                    } else {
                        "replay_required"
                    }
                );
            }
        }
    }
}

fn identity_request(input: Value, explicit: bool, websocket: bool) -> GenerateRequest {
    let mut body = json!({"model":"gpt-5.4","input":input,"instructions":"stable"});
    if explicit {
        body["session_id"] = json!("raw-session");
        body["thread_id"] = json!("raw-thread");
        body["prompt_cache_key"] = json!("raw-client-cache");
    }
    GenerateRequest::from_protocol_payload(
        ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
            .unwrap()
            .with_context(Map::from_iter([
                ("use_websocket".to_owned(), json!(websocket)),
                ("client_api_key_id".to_owned(), json!("untrusted-key")),
            ])),
    )
}

async fn consume_identity_request(
    provider: &CodexProvider,
    request: GenerateRequest,
    context: AttemptContext,
) -> Option<ProviderSessionState> {
    let mut stream = provider
        .execute(
            planned_request("openai", Operation::Generate(request)),
            context,
        )
        .await
        .unwrap();
    let mut state = None;
    while let Some(event) = timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
    {
        if let Some(update) = event.unwrap().session_update() {
            state = Some(update.clone());
        }
    }
    state
}

#[tokio::test]
async fn qx_provider_projects_http_identity_after_account_selection_with_trusted_key() {
    let store = Arc::new(MemoryAccountStore::default());
    for account in ["acct_scope_same", "acct_scope_new"] {
        create_account(&store, account).await;
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(CAPTURE_COMPLETED_SSE),
        )
        .expect(4)
        .mount(&server)
        .await;
    let provider = qx_provider(&store, server.uri());
    for (index, key, account) in [
        (0, "key-a", "acct_scope_same"),
        (1, "key-a", "acct_scope_same"),
        (2, "key-b", "acct_scope_same"),
        (3, "key-a", "acct_scope_new"),
    ] {
        let state = consume_identity_request(
            &provider,
            identity_request(json!("hello"), true, false),
            scoped_context(&format!("req-{index}"), key, account),
        )
        .await
        .unwrap();
        assert_eq!(state.payload()["client_api_key_id"], key);
        assert_eq!(state.payload()["identity_seed"]["session"], "raw-session");
        assert_eq!(state.payload()["identity_seed"]["thread"], "raw-thread");
    }
    let requests = server.received_requests().await.unwrap();
    let mut threads = Vec::new();
    let mut devices = Vec::new();
    for (request, account) in requests.iter().zip([
        "acct_scope_same",
        "acct_scope_same",
        "acct_scope_same",
        "acct_scope_new",
    ]) {
        let body = captured_request_body(request);
        let thread = request.headers["thread-id"].to_str().unwrap();
        assert_ne!(thread, "raw-thread");
        assert_eq!(body["prompt_cache_key"], thread);
        assert_eq!(request.headers["x-client-request-id"], thread);
        assert_eq!(request.headers["x-codex-window-id"], format!("{thread}:0"));
        assert_eq!(body["input"][0]["content"][0]["text"], "hello");
        assert!(!request.headers.contains_key("x-codex-installation-id"));
        let selected = store
            .repository()
            .load_runtime_credential(&store.account(account).unwrap())
            .await
            .unwrap();
        assert_eq!(
            body["client_metadata"]["x-codex-installation-id"],
            selected.installation_id
        );
        threads.push(thread.to_owned());
        devices.push(body["client_metadata"]["x-codex-installation-id"].clone());
    }
    assert_eq!(threads[0], threads[1]);
    assert_ne!(threads[0], threads[2]);
    assert_ne!(threads[0], threads[3]);
    assert_eq!(devices[0], devices[2]);
    assert_ne!(devices[0], devices[3]);
}

#[tokio::test]
async fn qx_saved_seed_preserves_unmarked_continuation_and_rejects_other_key_before_send() {
    const ACCOUNT: &str = "acct_scope_same";
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, ACCOUNT).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(CAPTURE_COMPLETED_SSE),
        )
        .expect(2)
        .mount(&server)
        .await;
    let provider = qx_provider(&store, server.uri());
    let state = consume_identity_request(
        &provider,
        identity_request(json!("first"), false, false),
        scoped_context("req-first", "key-a", ACCOUNT),
    )
    .await
    .unwrap();
    let followup = identity_request(json!("incremental followup"), false, false)
        .with_provider_session_state(state);
    let error = match provider
        .execute(
            planned_request("openai", Operation::Generate(followup.clone())),
            scoped_context("req-wrong-owner", "key-b", ACCOUNT),
        )
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("foreign caller must be rejected before upstream send"),
    };
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    consume_identity_request(
        &provider,
        followup,
        scoped_context("req-followup", "key-a", ACCOUNT),
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].headers["thread-id"],
        requests[1].headers["thread-id"]
    );
    assert_eq!(
        captured_request_body(&requests[1])["input"][0]["content"][0]["text"],
        "incremental followup"
    );
}

#[tokio::test]
async fn qx_saved_seed_survives_location_change_without_accepting_a_foreign_key() {
    const ACCOUNT: &str = "acct_location_continuation";
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, ACCOUNT).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/codex/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(CAPTURE_COMPLETED_SSE),
        )
        .expect(2)
        .mount(&server)
        .await;
    let first_provider = qx_provider(&store, server.uri());
    let mut profile = wire_profile().snapshot();
    profile.location = Default::default();
    let next_provider = provider_and_quota_with_profile(
        &store,
        Arc::new(MemorySessionAffinity::default()),
        server.uri(),
        Arc::new(TestLeaseCoordinator::default()),
        0,
        CodexWireProfileState::new(profile),
    )
    .0;
    let request = || {
        GenerateRequest::from_protocol_payload(
            ProtocolPayload::json_object(
                "openai",
                json!({
                    "model": "gpt-5.4", "input": "same unmarked question",
                    "tools": [{"type": "web_search"}]
                })
                .as_object()
                .unwrap()
                .clone(),
            )
            .unwrap()
            .with_context(Map::from_iter([("use_websocket".to_owned(), json!(false))])),
        )
    };
    let state = consume_identity_request(
        &first_provider,
        request(),
        scoped_context("location-first", "key-a", ACCOUNT).with_request_tuning(
            gateway_core::routing::RequestTuning {
                openai_location_override_enabled: true,
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap();
    let followup = request().with_provider_session_state(state);
    let error = match next_provider
        .execute(
            planned_request("openai", Operation::Generate(followup.clone())),
            scoped_context("location-wrong-key", "key-b", ACCOUNT),
        )
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("foreign owner must not reach the mock upstream"),
    };
    assert_eq!(error.kind(), ProviderErrorKind::InvalidRequest);
    assert_eq!(error.send_state(), UpstreamSendState::NotSent);
    consume_identity_request(
        &next_provider,
        followup,
        scoped_context("location-next", "key-a", ACCOUNT).with_request_tuning(
            gateway_core::routing::RequestTuning {
                openai_location_override_enabled: true,
                ..Default::default()
            },
        ),
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    let first = captured_request_body(&requests[0]);
    let next = captured_request_body(&requests[1]);
    assert_eq!(first["tools"][0]["user_location"]["city"], "Auckland");
    assert_eq!(next["tools"][0]["user_location"]["city"], "Piketon");
    assert_eq!(first["prompt_cache_key"], next["prompt_cache_key"]);
    assert_eq!(
        first["client_metadata"]["x-codex-installation-id"],
        next["client_metadata"]["x-codex-installation-id"]
    );
    for request in &requests {
        assert!(!request.headers.contains_key("x-codex-installation-id"));
    }
    for header in ["thread-id", "session-id"] {
        assert_eq!(
            requests[0].headers[header], requests[1].headers[header],
            "{header}"
        );
    }
}

#[tokio::test]
async fn provider_websocket_pool_isolates_keys_in_native_and_qx_policies() {
    const ACCOUNT: &str = "acct_scope_same";
    for qx in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, ACCOUNT).await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut captures = Vec::new();
            for count in [2, 1] {
                let (socket, _) = listener.accept().await.unwrap();
                let headers = Arc::new(Mutex::new(None));
                let captured = headers.clone();
                let mut socket = crate::transport::accept_codex_test_websocket_with(
                    socket,
                    move |request, response| {
                        *captured.lock().unwrap() = Some(request.headers().clone());
                        response.headers_mut().insert(
                            "sec-websocket-extensions",
                            "permessage-deflate".parse().unwrap(),
                        );
                    },
                )
                .await;
                let mut bodies = Vec::new();
                for _ in 0..count {
                    let frame = socket.next().await.unwrap().unwrap();
                    bodies.push(serde_json::from_str::<Value>(frame.to_text().unwrap()).unwrap());
                    super::generate_compat::send_completion(&mut socket, "resp_identity").await;
                }
                captures.push((headers.lock().unwrap().take().unwrap(), bodies));
            }
            captures
        });
        let provider = if qx {
            qx_provider(&store, url)
        } else {
            provider_with_base_url(&store, url)
        };
        for (index, key) in ["key-a", "key-a", "key-b"].into_iter().enumerate() {
            let body = json!({
                "model":"gpt-5.4", "input":"hello", "instructions":"stable",
                "session_id":"raw-session", "thread_id":"raw-thread",
                "prompt_cache_key":"raw-client-cache",
                "client_metadata": if index == 0 {
                    json!({"installation_id":"old-device"})
                } else {
                    json!({})
                }
            });
            let generate = GenerateRequest::from_protocol_payload(
                ProtocolPayload::json_object("openai", body.as_object().unwrap().clone())
                    .unwrap()
                    .with_context(Map::from_iter([("use_websocket".into(), json!(true))])),
            )
            .with_provider_session_state(
                ProviderSessionState::new(
                    "openai",
                    json!({
                        "account_id":ACCOUNT,
                        "conversation_id":"same-local-conversation",
                        "continuation_scope":"persisted"
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                )
                .unwrap(),
            );
            consume_identity_request(
                &provider,
                websocket_fixture_request(generate),
                scoped_context(&format!("ws-{qx}-{index}"), key, ACCOUNT),
            )
            .await;
        }
        let captures = timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(captures.len(), 2);
        let (opening, frames) = &captures[0];
        assert!(!opening.contains_key("x-codex-installation-id"));
        assert_eq!(
            frames.len(),
            2,
            "removing the legacy alias must reuse the owning socket"
        );
        assert_eq!(
            frames[0]["client_metadata"]["installation_id"],
            frames[0]["client_metadata"]["x-codex-installation-id"]
        );
        let selected = store
            .repository()
            .load_runtime_credential(&store.account(ACCOUNT).unwrap())
            .await
            .unwrap();
        for (headers, bodies) in &captures {
            assert!(!headers.contains_key("x-codex-installation-id"));
            for body in bodies {
                assert_eq!(
                    body["client_metadata"]["x-codex-installation-id"],
                    selected.installation_id
                );
            }
        }
        assert!(
            frames[1]["client_metadata"]
                .get("installation_id")
                .is_none()
        );
        assert_eq!(frames[0]["prompt_cache_key"], frames[1]["prompt_cache_key"]);
        if qx {
            let (first_headers, first_frames) = &captures[0];
            let (other_headers, other_frames) = &captures[1];
            let first_thread = first_headers["thread-id"].to_str().unwrap();
            assert_eq!(first_frames[0]["prompt_cache_key"], first_thread);
            assert_eq!(first_frames[1]["prompt_cache_key"], first_thread);
            assert_eq!(
                other_frames[0]["prompt_cache_key"],
                other_headers["thread-id"].to_str().unwrap()
            );
            assert_ne!(first_headers["thread-id"], other_headers["thread-id"]);
            assert_eq!(
                first_frames[0]["client_metadata"]["x-codex-installation-id"],
                other_frames[0]["client_metadata"]["x-codex-installation-id"]
            );
        }
    }
}

#[tokio::test]
async fn qx_new_thread_does_not_reuse_opening_headers_of_another_thread() {
    const ACCOUNT: &str = "acct_scope_same";
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, ACCOUNT).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut workers = Vec::new();
        for count in [2, 1] {
            let (socket, _) = listener.accept().await.unwrap();
            workers.push(tokio::spawn(async move {
                let mut socket = accept_codex_test_websocket(socket).await;
                for _ in 0..count {
                    socket.next().await.unwrap().unwrap();
                    super::generate_compat::send_completion(&mut socket, "resp_thread").await;
                }
                // Keep the first socket healthy while the third request is routed.
                assert!(
                    timeout(Duration::from_millis(200), socket.next())
                        .await
                        .is_err()
                );
            }));
        }
        for worker in workers {
            worker.await.unwrap();
        }
    });
    let provider = qx_provider(&store, url);
    for (index, thread) in ["thread-a", "thread-a", "thread-b"].into_iter().enumerate() {
        let generate = generate_with_persisted_session_context(
            ACCOUNT,
            "same-local-conversation",
            "same-session",
            thread,
        );
        consume_identity_request(
            &provider,
            websocket_fixture_request(generate),
            scoped_context(&format!("thread-{index}"), "key-a", ACCOUNT),
        )
        .await;
    }
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
