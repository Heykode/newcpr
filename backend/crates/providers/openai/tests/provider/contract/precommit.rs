use gateway_core::diagnostics::TraceContext;
use gateway_core::engine::provider::ProviderStream;
use gateway_core::error::ProviderError;
use gateway_core::event::ProviderEvent;
use gateway_core::routing::RequestTuning;
use gateway_protocol::openai::sse::encode_sse_event;

use super::*;

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

async fn stream_failure(stream: &mut ProviderStream) -> (Vec<ProviderEvent>, ProviderError) {
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
async fn custom_thresholds_release_wire_once_without_changing_body_or_retrying_after_commit() {
    for (limit, padding, expected_reason) in [
        (0, 512, "disabled"),
        (1024, 2048, "byte_limit"),
        (256 * 1024, 257 * 1024, "byte_limit"),
    ] {
        let store = Arc::new(MemoryAccountStore::default());
        create_account(&store, "acct_provider_contract").await;
        let body = sse(&structural("response.created", padding));
        let (base_url, release, _, server) =
            paused_chunked_sse_server(body.clone(), sse(&overload())).await;
        let trace = TraceContext::new("req_precommit");
        let mut stream = provider_with_base_url(&store, base_url)
            .execute(
                planned_request("openai", http_generate_operation()),
                traced_context(&trace, limit),
            )
            .await
            .unwrap();
        let event = timeout(Duration::from_secs(1), next_client_event(&mut stream))
            .await
            .unwrap();
        assert_eq!(
            event.wire_event().unwrap().raw_sse_frame().unwrap(),
            body.as_bytes()
        );
        release.send(()).unwrap();
        let (_, error) = timeout(Duration::from_secs(2), stream_failure(&mut stream))
            .await
            .unwrap();
        assert!(!error.replay_is_safe());
        assert!(error.pre_delivery_retry().is_none());
        let events = releases(&trace);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["data"]["reason"], expected_reason);
        assert_eq!(events[0]["data"]["limitBytes"], limit);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn nonzero_custom_limit_holds_past_old_threshold_and_uses_one_grace_deadline() {
    let store = Arc::new(MemoryAccountStore::default());
    create_account(&store, "acct_provider_contract").await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
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
            planned_request("openai", http_generate_operation()),
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
