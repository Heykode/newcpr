//! 验证上下游背压、取消与终态下的连接和任务回收

use super::*;
use gateway_core::{
    diagnostics::TraceContext, engine::response_control::ResponseControl,
    lifecycle::CancellationToken,
};

async fn wait_for_forwarded_frames(trace: &TraceContext, stage: &str, count: u64) {
    timeout(Duration::from_secs(2), async {
        while trace.snapshot().unwrap()["stageCounts"][stage]
            .as_u64()
            .unwrap_or_default()
            < count
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("forwarder should reach the backpressured frame");
}

fn response_tail(kind: &str) -> Message {
    match kind {
        "binary" => Message::Binary(vec![0].into()),
        "delta" => Message::Text(
            json!({"type":"response.output_text.delta", "delta":"x"})
                .to_string()
                .into(),
        ),
        "completed" => {
            Message::Text(completed_websocket_response("resp_backpressure", 2, 1).into())
        }
        kind => Message::Text(
            json!({"type":format!("response.{kind}"), "response":{
                "id":"resp_backpressure", "status":kind, "output":[]
            }})
            .to_string()
            .into(),
        ),
    }
}

async fn send_backpressured_response(
    websocket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    tail: &str,
) {
    // 先填满交付队列，后续帧由诊断计数确认已到达 forwarder，不靠 sleep 猜测调度
    for _ in 0..16 {
        websocket.send(response_tail("delta")).await.unwrap();
    }
    websocket.send(response_tail(tail)).await.unwrap();
}

async fn expect_upstream_socket_closed(
    websocket: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
) {
    let close = timeout(Duration::from_secs(2), websocket.next())
        .await
        .expect("cancellation must release the socket while the receiver remains alive");
    std::assert_matches!(
        close,
        None | Some(Ok(Message::Close(_)))
            | Some(Err(tungstenite::Error::Protocol(
                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
            )))
    );
}

#[tokio::test]
async fn backpressured_delivery_should_stop_on_downstream_cancel_or_pool_shutdown() {
    for cancel_downstream in [true, false] {
        for tail in ["delta", "binary", "completed", "failed", "incomplete"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let mut server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut websocket = accept_codex_test_websocket(socket).await;
                websocket.next().await.unwrap().unwrap();
                send_backpressured_response(&mut websocket, tail).await;
                expect_upstream_socket_closed(&mut websocket).await;
            });
            let pool = Arc::new(CodexWebSocketPool::default());
            let backend = CodexBackendClient::new(
                reqwest::Client::builder().no_proxy().build().unwrap(),
                format!("http://{addr}"),
                test_wire_profile(),
            )
            .with_websocket_pool(Arc::clone(&pool));
            let cancellation = CancellationToken::new();
            let control = ResponseControl::default();
            control.set_connection_cancellation(cancellation.clone());
            let trace = TraceContext::new("backpressured_delivery");
            let mut context = request_context("backpressured_delivery", Some("chatgpt-account"))
                .with_response_control(Some(&control));
            context.trace = Some(&trace);
            let mut request = pooled_websocket_request("backpressured_delivery");
            request.downstream_websocket_connection_id = Some("downstream_backpressure".into());
            let mut response = backend
                .create_response_stream(&request, context)
                .await
                .unwrap();
            let (stage, count) = if tail == "binary" {
                ("upstream.binary", 1)
            } else {
                ("upstream.event", 17)
            };
            wait_for_forwarded_frames(&trace, stage, count).await;

            if cancel_downstream {
                cancellation.cancel();
                // 必须在 shutdown 前看到物理连接关闭，避免停机掩盖下游取消失效
                (&mut server).await.unwrap();
            }
            timeout(Duration::from_secs(2), pool.shutdown())
                .await
                .expect("shutdown must join delivery even with a full receiver");
            if !cancel_downstream {
                (&mut server).await.unwrap();
            }
            // 收尾前不消费也不丢弃 body，已交付的事件仍可正常读完
            let mut buffered = 0;
            while response.body.next().await.transpose().unwrap().is_some() {
                buffered += 1;
            }
            assert_eq!(
                buffered, 16,
                "tail={tail}, cancel_downstream={cancel_downstream}"
            );
        }
    }
}

#[tokio::test]
async fn terminal_should_release_or_reuse_socket_before_delivery() {
    for tail in ["completed", "failed", "incomplete"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut websocket = accept_codex_test_websocket(socket).await;
            websocket.next().await.unwrap().unwrap();
            send_backpressured_response(&mut websocket, tail).await;
            if tail == "completed" {
                let next = timeout(Duration::from_secs(2), websocket.next())
                    .await
                    .expect("continuation should reuse the completed socket")
                    .unwrap()
                    .unwrap();
                let Message::Text(next) = next else {
                    panic!("expected continuation request");
                };
                assert_eq!(
                    serde_json::from_str::<Value>(&next).unwrap()["previous_response_id"],
                    "resp_backpressure"
                );
                websocket
                    .send(Message::Text(
                        completed_websocket_response("resp_next", 2, 1).into(),
                    ))
                    .await
                    .unwrap();
            }
            expect_upstream_socket_closed(&mut websocket).await;
        });
        let pool = Arc::new(CodexWebSocketPool::default());
        let backend = CodexBackendClient::new(
            reqwest::Client::builder().no_proxy().build().unwrap(),
            format!("http://{addr}"),
            test_wire_profile(),
        )
        .with_websocket_pool(Arc::clone(&pool));
        let trace = TraceContext::new("terminal_delivery");
        let mut context = request_context("terminal_delivery", Some("chatgpt-account"));
        context.trace = Some(&trace);
        let mut request = pooled_websocket_request("terminal_delivery");
        let mut response = backend
            .create_response_stream(&request, context)
            .await
            .unwrap();
        wait_for_forwarded_frames(&trace, "upstream.event", 17).await;
        if tail == "completed" {
            request.set_previous_response_id(Some("resp_backpressure".into()));
            request.previous_response_scope = Some(PreviousResponseScope::ConnectionLocal);
            let next = backend
                .create_response(
                    &request,
                    request_context("terminal_next", Some("chatgpt-account")),
                )
                .await
                .expect("completed continuation should be reusable before terminal consumption");
            assert!(next.websocket_pool_decision.unwrap().is_reuse());
        } else {
            // 失败终态必须释放 socket，不能依赖客户端读走终态
            (&mut server).await.unwrap();
        }
        let mut events = Vec::new();
        while let Some(chunk) = response.body.next().await {
            events.push(serde_json::from_slice::<Value>(&chunk.unwrap()).unwrap());
        }
        assert_eq!(events.len(), 17);
        assert_eq!(events.last().unwrap()["type"], format!("response.{tail}"));
        pool.shutdown().await;
        if tail == "completed" {
            (&mut server).await.unwrap();
        }
    }
}
