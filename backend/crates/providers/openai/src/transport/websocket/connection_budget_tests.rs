use super::*;
use futures::StreamExt;
use gateway_core::engine::connection::ConnectionBudget;
use tokio::{net::TcpListener, time::timeout};

thread_local! { static NOW: std::cell::Cell<Instant> = std::cell::Cell::new(Instant::now()); }
fn now() -> Instant {
    NOW.with(std::cell::Cell::get)
}

fn request(base: &str) -> CodexWebSocketRequest {
    CodexWebSocketRequest {
        connection: CodexWebSocketConnection::responses(
            base,
            &tungstenite::handshake::client::generate_key(),
            Vec::new(),
        ),
        payload_text: "{}".to_owned(),
        continuation: WebSocketContinuationRequirement::NewChain,
    }
}

async fn serve_connection(socket: tokio::net::TcpStream) {
    let mut extensions = tungstenite::extensions::ExtensionsConfig::default();
    extensions.permessage_deflate =
        Some(tungstenite::extensions::compression::deflate::DeflateConfig::default());
    let mut config = tungstenite::protocol::WebSocketConfig::default();
    config.extensions = extensions;
    let mut websocket = tokio_tungstenite::accept_async_with_config(socket, Some(config))
        .await
        .unwrap();
    while let Some(Ok(_)) = websocket.next().await {}
}

#[tokio::test]
async fn connection_budget_counts_handshake_but_not_pooled_reuse() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        serve_connection(socket).await;
    });
    let pool = CodexWebSocketPool::new(Duration::from_secs(60));
    let key = CodexWebSocketPoolKey::new(&base, "synthetic-account", "synthetic-session");
    let breaker = WebSocketOriginBreaker::default();
    let budget = ConnectionBudget::default();
    for _ in 0..3 {
        budget.begin().unwrap();
    }
    let request = request(&base);
    let first = prepare_response_create_request_with_pool(
        &request,
        Some((&pool, key.clone())),
        &breaker,
        &base,
        None,
        false,
        None,
        Some(budget.clone()),
    )
    .await
    .unwrap();
    let WebSocketFastPath::Ready(first) = first else {
        panic!("expected handshake");
    };
    assert!(!first.reused());
    assert!(
        budget.begin().is_err(),
        "WS must consume the fourth shared opening"
    );
    let id = first.connection.websocket.connection_id();
    let (lease, _, _) = first.binding.into_parts();
    lease.unwrap().put(first.connection).await;
    let reused = prepare_response_create_request_with_pool(
        &request,
        Some((&pool, key)),
        &breaker,
        &base,
        None,
        false,
        None,
        Some(budget),
    )
    .await
    .unwrap();
    let WebSocketFastPath::Ready(reused) = reused else {
        panic!("expected reuse");
    };
    assert!(reused.reused());
    assert_eq!(reused.connection.websocket.connection_id(), id);
    let (lease, _, _) = reused.binding.into_parts();
    lease.unwrap().put(reused.connection).await;
    pool.shutdown().await;
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn connection_budget_completion_keeps_background_preconnect_owned_by_pool() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (release, ready) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        ready.await.unwrap();
        serve_connection(socket).await;
    });
    let pool = CodexWebSocketPool::new(Duration::from_secs(60));
    let key = CodexWebSocketPoolKey::new(&base, "synthetic-account", "background-session");
    let budget = ConnectionBudget::default();
    let result = prepare_response_create_request_with_pool(
        &request(&base),
        Some((&pool, key.clone())),
        &WebSocketOriginBreaker::default(),
        &base,
        Some(Duration::from_millis(50)),
        false,
        None,
        Some(budget.clone()),
    )
    .await
    .unwrap();
    assert!(matches!(result, WebSocketFastPath::Missed));
    // The foreground HTTP path uses the same budget, then finishes sending.
    budget.begin().unwrap();
    budget.complete();
    let WebSocketPoolAcquire::Wait(waiter) = pool.acquire(&key, None).await else {
        panic!("pool must still own the background handshake");
    };
    release.send(()).unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(5), waiter.wait())
            .await
            .unwrap(),
        WebSocketPoolConnectOutcome::Ready
    ));
    let WebSocketPoolAcquire::Reused { connection, lease } = pool.acquire(&key, None).await else {
        panic!("background handshake should remain reusable");
    };
    lease.put(*connection).await;
    pool.shutdown().await;
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn exhausted_connection_budget_never_opens_websocket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let budget = ConnectionBudget::default();
    for _ in 0..4 {
        budget.begin().unwrap();
    }
    let breaker = WebSocketOriginBreaker::default();
    for _ in 0..4 {
        let result = prepare_response_create_request_with_pool(
            &request(&base),
            None,
            &breaker,
            &base,
            Some(Duration::from_millis(800)),
            false,
            None,
            Some(budget.clone()),
        )
        .await;
        assert!(matches!(result, Err(error) if error.connection_budget_exhausted()));
    }
    // Repeated local exhaustion must not trip the origin breaker.
    match breaker.try_acquire(&base) {
        WebSocketOriginBreakerDecision::Allowed(permit) => permit.cancel(),
        _ => panic!("local budget exhaustion must not mark the origin unhealthy"),
    }
    assert!(
        timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn connection_budget_deadline_does_not_count_as_origin_failure() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let budget = ConnectionBudget::with_clock(now);
    budget.begin().unwrap();
    NOW.with(|clock| clock.set(clock.get() + Duration::from_millis(29_950)));
    let result = prepare_response_create_request_with_pool(
        &request(&base),
        None,
        &WebSocketOriginBreaker::default(),
        &base,
        Some(Duration::from_millis(800)),
        false,
        None,
        Some(budget),
    )
    .await;
    assert!(matches!(result, Err(error) if error.connection_budget_exhausted()));
    // The TCP listener accepted the dial, but did not complete the WS handshake.
    assert!(
        timeout(Duration::from_secs(1), listener.accept())
            .await
            .is_ok()
    );
}
