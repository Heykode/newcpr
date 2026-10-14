use gateway_core::diagnostics::TraceContext;
use gateway_core::engine::provider::ProviderStream;
use gateway_core::error::ProviderError;
use gateway_core::event::ProviderEvent;
use gateway_core::routing::RequestTuning;
use gateway_protocol::openai::sse::encode_sse_event;

use super::*;

#[tokio::test]
async fn sse_comments_are_forwarded_without_closing_the_retry_window() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let heartbeat = ": keep-alive\r\n\r\n";
    let (base_url, release, _, server) =
        paused_chunked_sse_server(heartbeat.to_owned(), sse(&overload())).await;
    let mut stream = provider_with_base_url(&store, base_url)
        .execute(
            planned_request("openai", http_generate_operation()),
            traced_context(&TraceContext::new("req_heartbeat_retry"), 0),
        )
        .await
        .unwrap();
    let event = timeout(Duration::from_secs(1), next_client_event(&mut stream))
        .await
        .expect("heartbeat cannot wait for a later event");
    assert_eq!(
        event.wire_event().unwrap().raw_sse_frame().unwrap(),
        heartbeat.as_bytes()
    );
    assert!(event.is_replayable_prelude());
    release.send(()).unwrap();
    let (_, error) = stream_failure(&mut stream).await;
    assert!(error.replay_is_safe());
    assert!(matches!(
        error.pre_delivery_retry(),
        Some(PreDeliveryRetry::SameAccountTransientRetry { .. })
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn ordinary_streams_forward_compaction_output_and_unknown_events_without_reopening_retry() {
    for websocket in [false, true] {
        for first in [
            json!({"type":"response.compaction.compacting","item_id":"compact_test","output_index":0}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"partial"}),
            json!({"type":"response.future","value":"opaque"}),
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let prelude = structural("response.created", 1024);
            let (base_url, release, server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base_url = format!("http://{}", listener.local_addr().unwrap());
                let (release, released) = oneshot::channel();
                let first = first.clone();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(socket).await;
                    socket.next().await.unwrap().unwrap();
                    for event in [first, prelude] {
                        socket
                            .send(Message::Text(event.to_string().into()))
                            .await
                            .unwrap();
                    }
                    released.await.unwrap();
                    socket
                        .send(Message::Text(overload().to_string().into()))
                        .await
                        .unwrap();
                });
                (base_url, release, server)
            } else {
                let (base_url, release, _, server) =
                    paused_chunked_sse_server(sse(&first) + &sse(&prelude), sse(&overload())).await;
                (base_url, release, server)
            };
            let operation = if websocket {
                generate_operation()
            } else {
                http_generate_operation()
            };
            let mut stream = provider_with_base_url(&store, base_url)
                .execute(
                    planned_request("openai", operation),
                    traced_context(&TraceContext::new("req_closed_retry"), 0),
                )
                .await
                .unwrap();
            let delivered = timeout(Duration::from_secs(1), next_client_event(&mut stream))
                .await
                .unwrap();
            assert_eq!(delivered.wire_event().unwrap().data(), &first);
            assert!(!delivered.is_replayable_prelude());
            let created = timeout(Duration::from_secs(1), next_client_event(&mut stream))
                .await
                .unwrap();
            assert!(!created.is_replayable_prelude());
            release.send(()).unwrap();
            let (_, error) = stream_failure(&mut stream).await;
            assert!(!error.replay_is_safe());
            assert!(error.pre_delivery_retry().is_none());
            server.await.unwrap();
        }
    }
}

fn traced_context(trace: &TraceContext, bytes: u64) -> AttemptContext {
    AttemptContext::new(
        RequestAttemptContext::new(
            ModelRequestId::new("req_precommit").unwrap(),
            ClientApiKeyId::new("key_openai_contract").unwrap(),
        )
        .with_trace(trace.clone()),
        NonZeroU32::new(1).unwrap(),
        SystemTime::now() + Duration::from_secs(30),
        account_policy(),
        AccountAttemptContext::new(BTreeSet::new(), None, None)
            .with_account_scope(contract_account_scope()),
        None,
        CancellationToken::new(),
    )
    .with_request_tuning(RequestTuning {
        stream_prefetch_bytes: bytes,
        ..RequestTuning::default()
    })
}

fn structural(kind: &str, padding: usize) -> Value {
    json!({"type":kind,"response":{"id":"resp_precommit","model":"gpt-5.4","status":"in_progress","instructions":"x".repeat(padding),"tools":[],"output":[]}})
}

fn overload() -> Value {
    json!({"type":"error","error":{"type":"service_unavailable_error","code":"server_is_overloaded","message":"busy"}})
}

fn connection_limit() -> Value {
    json!({"type":"error","status":400,"error":{"type":"invalid_request_error","code":"websocket_connection_limit_reached","message":"synthetic expired connection"}})
}

fn sse(event: &Value) -> String {
    encode_sse_event(event["type"].as_str().unwrap(), &event.to_string())
}

fn releases(trace: &TraceContext) -> Vec<Value> {
    trace.snapshot().unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["stage"] == "provider.precommit.released")
        .cloned()
        .collect()
}

async fn next_client_event(stream: &mut ProviderStream) -> ProviderEvent {
    loop {
        let event = stream.next().await.unwrap().unwrap();
        if event.has_client_event() {
            return event;
        }
    }
}

pub(super) async fn stream_failure(
    stream: &mut ProviderStream,
) -> (Vec<ProviderEvent>, ProviderError) {
    let mut events = Vec::new();
    loop {
        match stream.next().await.expect("upstream failure") {
            Ok(event) => events.push(event),
            Err(error) => return (events, error),
        }
    }
}

#[tokio::test]
async fn large_structural_events_keep_http_and_websocket_overload_replay_safe() {
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let created = structural("response.created", 38_000);
        let progress = structural("response.in_progress", 38_000);
        let failure = overload();
        let expected = vec![created.clone(), progress.clone(), failure.clone()];
        let (base_url, release, server) = if websocket {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let (release, released) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut ws = accept_codex_test_websocket(socket).await;
                ws.next().await.unwrap().unwrap();
                for event in [created, progress] {
                    ws.send(Message::Text(event.to_string().into()))
                        .await
                        .unwrap();
                }
                released.await.unwrap();
                ws.send(Message::Text(failure.to_string().into()))
                    .await
                    .unwrap();
            });
            (base_url, release, server)
        } else {
            let (base_url, release, _, server) =
                paused_chunked_sse_server(sse(&created) + &sse(&progress), sse(&failure)).await;
            (base_url, release, server)
        };
        let trace = TraceContext::new("req_precommit");
        let operation = if websocket {
            generate_operation()
        } else {
            http_generate_operation()
        };
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request("openai", operation),
                traced_context(&trace, 128 * 1024),
            )
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(1_500), next_client_event(&mut stream))
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let (events, mut error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
            .await
            .unwrap();
        assert!(events.iter().all(|event| !event.has_client_event()));
        assert!(error.replay_is_safe());
        assert!(matches!(
            error.pre_delivery_retry(),
            Some(PreDeliveryRetry::SameAccountTransientRetry { .. })
        ));
        let actual: Vec<_> = error
            .take_atomic_client_events()
            .iter()
            .filter_map(|event| event.wire_event().map(|wire| wire.data().clone()))
            .collect();
        assert_eq!(actual, expected);
        assert!(releases(&trace).is_empty());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn custom_thresholds_preserve_wire_and_only_zero_keeps_preludes_replayable() {
    for websocket in [false, true] {
        for (limit, padding, expected_reason) in [
            (0, 512, "disabled"),
            (1024, 2048, "byte_limit"),
            (30 * 1024, 31 * 1024, "byte_limit"),
            (60 * 1024, 61 * 1024, "byte_limit"),
            (120 * 1024, 121 * 1024, "byte_limit"),
            (256 * 1024, 257 * 1024, "byte_limit"),
        ] {
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let created = structural("response.created", padding);
            let body = sse(&created);
            let websocket_body = created.to_string();
            let (base_url, release, server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base_url = format!("http://{}", listener.local_addr().unwrap());
                let (release, released) = oneshot::channel();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut ws = accept_codex_test_websocket(socket).await;
                    ws.next().await.unwrap().unwrap();
                    ws.send(Message::Text(created.to_string().into()))
                        .await
                        .unwrap();
                    released.await.unwrap();
                    ws.send(Message::Text(overload().to_string().into()))
                        .await
                        .unwrap();
                });
                (base_url, release, server)
            } else {
                let (base_url, release, _, server) =
                    paused_chunked_sse_server(body.clone(), sse(&overload())).await;
                (base_url, release, server)
            };
            let trace = TraceContext::new("req_precommit");
            let mut stream = provider_with_base_url(&store, base_url)
                .execute(
                    planned_request(
                        "openai",
                        if websocket {
                            generate_operation()
                        } else {
                            http_generate_operation()
                        },
                    ),
                    traced_context(&trace, limit),
                )
                .await
                .unwrap();
            let event = timeout(Duration::from_secs(1), next_client_event(&mut stream))
                .await
                .unwrap();
            let wire = event.wire_event().unwrap();
            assert_eq!(event.is_replayable_prelude(), limit == 0);
            if websocket {
                assert!(wire.raw_sse_frame().is_none());
                assert_eq!(wire.raw_websocket_message().unwrap(), websocket_body);
            } else {
                assert_eq!(wire.raw_sse_frame().unwrap(), body.as_bytes());
                assert!(wire.raw_websocket_message().is_none());
            }
            release.send(()).unwrap();
            let (_, error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
                .await
                .unwrap();
            assert_eq!(error.replay_is_safe(), limit == 0);
            if limit == 0 {
                assert!(matches!(
                    error.pre_delivery_retry(),
                    Some(PreDeliveryRetry::SameAccountTransientRetry { .. })
                ));
                assert!(releases(&trace).is_empty());
                server.await.unwrap();
                continue;
            }
            assert!(error.pre_delivery_retry().is_none());
            let events = releases(&trace);
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["data"]["reason"], expected_reason);
            assert_eq!(events[0]["data"]["limitBytes"], limit);
            assert_eq!(events[0]["data"]["prefetchedBytes"], body.len());
            server.await.unwrap();
        }
    }
}

#[tokio::test]
async fn zero_prefetch_ws_releases_preludes_and_preserves_only_safe_late_recovery() {
    for tail in [
        json!({"type":"response.completed","response":{"id":"resp_precommit","model":"gpt-5.4","status":"completed","output":[]}}),
        overload(),
        connection_limit(),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let created = structural("response.created", 512);
        let progress = structural("response.in_progress", 512);
        let expected = [created.to_string(), progress.to_string()];
        let expected_tail = tail.to_string();
        let failed = tail["type"] == "error";
        let overloaded = tail == overload();
        let (release, released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            ws.next().await.unwrap().unwrap();
            for event in [created, progress] {
                ws.send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
            }
            released.await.unwrap();
            ws.send(Message::Text(tail.to_string().into()))
                .await
                .unwrap();
            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err()
            );
        });
        let trace = TraceContext::new("req_precommit");
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request("openai", generate_operation()),
                traced_context(&trace, 0),
            )
            .await
            .unwrap();
        // The fixture withholds the terminal/error until both preludes reach the caller.
        for frame in expected {
            let event = timeout(Duration::from_secs(1), next_client_event(&mut stream))
                .await
                .unwrap();
            assert_eq!(
                event.wire_event().unwrap().raw_websocket_message().unwrap(),
                frame
            );
        }
        release.send(()).unwrap();
        let events = if failed {
            let (events, error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
                .await
                .unwrap();
            assert_eq!(error.replay_is_safe(), overloaded);
            if overloaded {
                assert!(matches!(
                    error.pre_delivery_retry(),
                    Some(PreDeliveryRetry::SameAccountTransientRetry { .. })
                ));
            }
            let mut error = error;
            let mut events = events;
            events.extend(error.take_atomic_client_events());
            events
        } else {
            timeout(Duration::from_secs(2), async {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    events.push(event.unwrap());
                }
                events
            })
            .await
            .unwrap()
        };
        let wire: Vec<_> = events
            .iter()
            .filter_map(|event| {
                event
                    .wire_event()
                    .and_then(|wire| wire.raw_websocket_message())
            })
            .collect();
        assert_eq!(wire, vec![expected_tail.as_str()]);
        let events = releases(&trace);
        if failed {
            assert!(events.is_empty());
        } else {
            assert_eq!(events.len(), 1);
            assert_eq!(events[0]["data"]["reason"], "terminal");
            assert_eq!(events[0]["data"]["limitBytes"], 0);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn zero_prefetch_immediate_ws_rejection_preserves_pre_delivery_retry() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut ws = accept_codex_test_websocket(socket).await;
        ws.next().await.unwrap().unwrap();
        ws.send(Message::Text(connection_limit().to_string().into()))
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err()
        );
    });
    let trace = TraceContext::new("req_precommit");
    let mut stream = provider_with_base_url_and_retry_budget(&store, base_url, 1)
        .execute(
            planned_request("openai", generate_operation()),
            traced_context(&trace, 0),
        )
        .await
        .unwrap();
    let (events, error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
        .await
        .unwrap();
    assert!(events.iter().all(|event| !event.has_client_event()));
    assert!(error.replay_is_safe());
    assert!(matches!(
        error.pre_delivery_retry(),
        Some(PreDeliveryRetry::SameAccountTransportRetry { .. })
    ));
    assert!(releases(&trace).is_empty());
    server.await.unwrap();
}

#[tokio::test]
async fn nonzero_custom_limit_holds_past_old_threshold_and_uses_one_grace_deadline() {
    for websocket in [false, true] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (release, released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            if websocket {
                let mut ws = accept_codex_test_websocket(socket).await;
                ws.next().await.unwrap().unwrap();
                ws.send(Message::Text(
                    structural("response.created", 129 * 1024)
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
                tokio::time::sleep(Duration::from_millis(1_500)).await;
                ws.send(Message::Text(
                    structural("response.in_progress", 512).to_string().into(),
                ))
                .await
                .unwrap();
                released.await.unwrap();
                ws.send(Message::Text(json!({"type":"response.completed","response":{"id":"resp_precommit","model":"gpt-5.4","status":"completed","output":[]}}).to_string().into())).await.unwrap();
                return;
            }
            read_http_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n").await.unwrap();
            write_http_chunk(
                &mut socket,
                &sse(&structural("response.created", 129 * 1024)),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(1_500)).await;
            write_http_chunk(&mut socket, &sse(&structural("response.in_progress", 512))).await;
            released.await.unwrap();
            socket.write_all(b"0\r\n\r\n").await.unwrap();
        });
        let trace = TraceContext::new("req_precommit");
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request(
                    "openai",
                    if websocket {
                        generate_operation()
                    } else {
                        http_generate_operation()
                    },
                ),
                traced_context(&trace, 256 * 1024),
            )
            .await
            .unwrap();
        assert!(
            timeout(Duration::from_millis(1_500), next_client_event(&mut stream))
                .await
                .is_err()
        );
        let event = timeout(Duration::from_secs(2), next_client_event(&mut stream))
            .await
            .unwrap();
        assert_eq!(
            event.wire_event().unwrap().event_type(),
            Some("response.created")
        );
        release.send(()).unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let events = releases(&trace);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["reason"], "grace_timeout");
        assert!((2500..3500).contains(&events[0]["data"]["waitMs"].as_u64().unwrap()));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn buffered_ws_rejection_respects_retry_budget_without_adding_fallback() {
    for (budget, used, should_retry) in [(0, 0, false), (1, 0, true), (1, 1, false), (3, 3, false)]
    {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = accept_codex_test_websocket(socket).await;
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Text(
                structural("response.created", 512).to_string().into(),
            ))
            .await
            .unwrap();
            ws.send(Message::Text(connection_limit().to_string().into()))
                .await
                .unwrap();
            assert!(
                timeout(Duration::from_millis(250), listener.accept())
                    .await
                    .is_err()
            );
        });
        let trace = TraceContext::new("req_precommit");
        let mut attempt = traced_context(&trace, 60 * 1024).with_request_tuning(RequestTuning {
            stream_prefetch_bytes: 60 * 1024,
            websocket_max_retries: budget,
            websocket_http_fallback_enabled: false,
            ..RequestTuning::default()
        });
        if let Some(index) = NonZeroU32::new(used) {
            attempt = attempt.with_transport(AttemptTransport::Retry(index));
        }
        let mut stream = provider_with_base_url_and_retry_budget(&store, base_url, budget)
            .execute(planned_request("openai", generate_operation()), attempt)
            .await
            .unwrap();
        let (events, error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
            .await
            .unwrap();
        assert!(events.iter().all(|event| !event.has_client_event()));
        assert!(error.replay_is_safe());
        if should_retry {
            assert!(
                matches!(error.pre_delivery_retry(), Some(PreDeliveryRetry::SameAccountTransportRetry { retry_index, .. }) if retry_index.get() == 1)
            );
        } else {
            assert_eq!(error.pre_delivery_retry(), None);
        }
        assert!(releases(&trace).is_empty());
        server.await.unwrap();
    }
}

#[tokio::test]
async fn semantic_output_terminal_and_eof_do_not_wait_for_the_byte_threshold() {
    for (reason, event) in [
        (
            "semantic_output",
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"hello"}),
        ),
        (
            "semantic_output",
            json!({"type":"response.web_search_call.in_progress","item_id":"search_precommit","output_index":0}),
        ),
        (
            "terminal",
            json!({"type":"response.completed","response":{"id":"resp_precommit","model":"gpt-5.4","status":"completed","output":[]}}),
        ),
        ("eof", Value::Null),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let mut body = sse(&structural("response.created", 512));
        if !event.is_null() {
            body.push_str(&sse(&event));
        }
        let (base_url, release, _, server) =
            paused_chunked_sse_server(body.clone(), String::new()).await;
        let mut release = Some(release);
        if reason == "eof" {
            release.take().unwrap().send(()).unwrap();
        }
        let trace = TraceContext::new("req_precommit");
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request("openai", http_generate_operation()),
                traced_context(&trace, u64::MAX),
            )
            .await
            .unwrap();
        let first = timeout(Duration::from_secs(1), next_client_event(&mut stream))
            .await
            .unwrap();
        if let Some(release) = release {
            release.send(()).unwrap();
        }
        let mut wire = first
            .wire_event()
            .unwrap()
            .raw_sse_frame()
            .unwrap()
            .to_vec();
        while let Some(event) = stream.next().await {
            if let Some(frame) = event
                .unwrap()
                .wire_event()
                .and_then(|wire| wire.raw_sse_frame())
            {
                wire.extend_from_slice(frame);
            }
        }
        assert_eq!(wire, body.as_bytes());
        let events = releases(&trace);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["reason"], reason);
        server.await.unwrap();
    }
}
