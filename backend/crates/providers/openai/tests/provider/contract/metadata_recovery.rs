//! 验证网关特化恢复与普通上游错误透传的边界

use super::precommit::{next_client_event, stream_failure};
use super::*;
use gateway_core::error::TransientRetryPolicy;

#[tokio::test]
async fn server_error_503_stream_recovery_requires_replayable_events() {
    for websocket in [false, true] {
        for blocking_event in [
            None,
            Some(
                json!({"type":"response.output_text.delta","response_id":"resp_503","item_id":"msg_503","output_index":0,"content_index":0,"delta":"hello"}),
            ),
            Some(json!({"type":"response.future","response_id":"resp_503"})),
        ] {
            let replayable = blocking_event.is_none();
            let store = Arc::new(MemoryAccountStore::default());
            create_account(&store, "acct_provider_contract").await;
            let failure = json!({"type":"error","status":503,"error":{"code":"server_error","type":"server_error","message":"You can retry your request."}});
            let mut upstream_events = vec![
                json!({"type":"codex.response.metadata","metadata":{}}),
                json!({"type":"response.created","response":{"id":"resp_503","status":"in_progress","output":[]}}),
                json!({"type":"response.in_progress","response":{"id":"resp_503","status":"in_progress","output":[]}}),
            ];
            if let Some(event) = blocking_event {
                upstream_events.push(event);
            }
            // 元数据仍完整交付，但不能重新开启已被输出或未知事件关闭的重放窗口
            upstream_events.push(json!({
                "type": "response.metadata",
                "response_id": "resp_503",
                "headers": {"x-codex-turn-state": "turn-503"},
                "metadata": {
                    "openai_chatgpt_moderation_metadata": {},
                    "type": "safety_buffering",
                    "use_cases": ["cyber"],
                    "reasons": ["user_risk"],
                    "future": true
                }
            }));
            let mut expected_events = upstream_events.clone();
            expected_events.push(failure.clone());
            let (base, release, server) = if websocket {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                let (release, released) = oneshot::channel();
                let failure = failure.clone();
                let server = tokio::spawn(async move {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut socket = accept_codex_test_websocket(socket).await;
                    socket.next().await.unwrap().unwrap();
                    for event in upstream_events {
                        socket
                            .send(Message::Text(event.to_string().into()))
                            .await
                            .unwrap();
                    }
                    released.await.unwrap();
                    socket
                        .send(Message::Text(failure.to_string().into()))
                        .await
                        .unwrap();
                });
                (base, release, server)
            } else {
                let body = upstream_events
                    .iter()
                    .map(|event| {
                        gateway_protocol::openai::sse::encode_sse_event(
                            event["type"].as_str().unwrap(),
                            &event.to_string(),
                        )
                    })
                    .collect::<String>();
                let (base, release, _, server) = paused_chunked_sse_server(
                    body,
                    gateway_protocol::openai::sse::encode_sse_event("error", &failure.to_string()),
                )
                .await;
                (base, release, server)
            };
            let operation = if websocket {
                generate_operation()
            } else {
                http_generate_operation()
            };
            let mut stream = provider_with_base_url(&store, base)
                .execute(
                    planned_request("openai", operation),
                    context("req_server_error_503", CancellationToken::new()).with_request_tuning(
                        gateway_core::routing::RequestTuning {
                            stream_prefetch_bytes: 128 * 1024,
                            ..Default::default()
                        },
                    ),
                )
                .await
                .unwrap();
            // 先确认前导已交付，再发送错误，避免同一 HTTP 块的原子失败绕过交付边界
            let mut events = Vec::new();
            for expected in &expected_events[..expected_events.len() - 1] {
                let event = timeout(Duration::from_secs(1), next_client_event(&mut stream))
                    .await
                    .unwrap();
                assert_eq!(event.wire_event().unwrap().data(), expected);
                events.push(event);
            }
            release.send(()).unwrap();
            let (remaining, mut error) =
                timeout(Duration::from_secs(1), stream_failure(&mut stream))
                    .await
                    .unwrap();
            events.extend(remaining);
            assert_eq!(error.kind(), ProviderErrorKind::Unavailable);
            assert_eq!(error.upstream_status(), Some(503));
            assert_eq!(error.send_state(), UpstreamSendState::Sent);
            assert_eq!(error.replay_is_safe(), replayable);
            if !replayable {
                assert!(error.pre_delivery_retry().is_none());
                assert!(
                    events
                        .iter()
                        .any(|event| event.has_client_event() && !event.is_replayable_prelude())
                );
            } else {
                assert!(!error.retry_is_prohibited());
                assert!(
                    matches!(error.pre_delivery_retry(), Some(PreDeliveryRetry::SameAccountTransientRetry {
                        max_retries,
                        initial_delay,
                        max_delay,
                        jitter,
                        policy: TransientRetryPolicy::RetryThenReturnError,
                    }) if max_retries.get() == 5
                        && initial_delay == Duration::from_millis(500)
                        && max_delay == Duration::from_secs(2)
                        && jitter >= Duration::from_millis(50)
                        && jitter < Duration::from_millis(250))
                );
                assert!(
                    events
                        .iter()
                        .filter(|event| event.has_client_event())
                        .all(|event| event.is_replayable_prelude())
                );
            }
            assert_eq!(
                serde_json::from_str::<Value>(error.raw_upstream_error().unwrap().as_str())
                    .unwrap(),
                failure
            );
            events.extend(error.take_atomic_client_events());
            assert_eq!(
                events
                    .iter()
                    .filter_map(|event| event.wire_event().map(|wire| wire.data().clone()))
                    .collect::<Vec<_>>(),
                expected_events
            );
            let account = store.account("acct_provider_contract").unwrap();
            assert_eq!(account.credential_state(), CredentialState::Ready);
            assert_eq!(account.quota().access(), QuotaAccessState::Unknown);
            server.await.unwrap();
        }
    }
}
