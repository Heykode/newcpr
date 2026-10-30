//! Responses WebSocket pre-send/post-send 与 pool/breaker 编排。

#[cfg(test)]
mod connection_budget_tests {
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
            Some((&pool, key.clone(), None)),
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
            Some((&pool, key, None)),
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
            Some((&pool, key.clone(), None)),
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
        let WebSocketPoolAcquire::Reused { connection, lease } = pool.acquire(&key, None).await
        else {
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
}

use std::{
    future::Future,
    time::{Duration, Instant},
};

use tokio::{
    sync::oneshot,
    time::{Instant as TokioInstant, timeout_at},
};

use super::{
    breaker::{
        WebSocketOriginBreaker, WebSocketOriginBreakerDecision, WebSocketOriginBreakerPermit,
    },
    error::CodexWebSocketExchangeError,
    exchange::{
        CodexWebSocketStreamingExchange, WebSocketStreamPoolReturn, stream_websocket_response,
    },
    handshake::{connect_pumped_websocket, send_websocket_request, websocket_connection_metadata},
    model::{
        CodexWebSocketConnection, CodexWebSocketRequest, PreviousResponseUnavailableReason,
        WebSocketContinuationRequirement,
    },
    pool::{
        self, CodexWebSocketPool, CodexWebSocketPoolKey, PooledWebSocketConnection,
        WebSocketContinuationState, WebSocketPoolAcquire, WebSocketPoolBypassReason,
        WebSocketPoolConnectLease, WebSocketPoolConnectOutcome, WebSocketPoolDecision,
        WebSocketPoolLease,
    },
    pump::{PumpLogContext, PumpedWebSocket, transport_metric_reason},
};

pub(crate) const WEBSOCKET_FAST_PATH_BUDGET: Duration = Duration::from_millis(800);

/// WebSocket 快路径的控制流结果；预算未命中不代表连接失败。
pub(crate) enum WebSocketFastPath<T> {
    /// 在前台预算内获得结果。
    Ready(T),
    /// 前台停止等待；池化 opening 可以继续在后台完成。
    Missed,
}

/// 尚未发送 `response.create` 的 WebSocket。只有该类型可以安全切换到 HTTP。
pub(crate) struct PreparedWebSocket {
    connection: PooledWebSocketConnection,
    binding: PoolBinding,
    connect_elapsed: Option<Duration>,
    decision_wait_elapsed: Duration,
    stream_idle_timeout: Option<Duration>,
}

enum PoolBinding {
    Unpooled,
    Pooled {
        lease: Box<WebSocketPoolLease>,
        reused: bool,
        decision: WebSocketPoolDecision,
    },
}

impl PoolBinding {
    fn decision(&self) -> Option<WebSocketPoolDecision> {
        match self {
            Self::Unpooled => None,
            Self::Pooled { decision, .. } => Some(*decision),
        }
    }

    fn reused(&self) -> bool {
        matches!(self, Self::Pooled { reused: true, .. })
    }

    fn into_parts(
        self,
    ) -> (
        Option<WebSocketPoolLease>,
        bool,
        Option<WebSocketPoolDecision>,
    ) {
        match self {
            Self::Unpooled => (None, false, None),
            Self::Pooled {
                lease,
                reused,
                decision,
            } => (Some(*lease), reused, Some(decision)),
        }
    }
}

impl PreparedWebSocket {
    pub(crate) fn pool_decision(&self) -> Option<WebSocketPoolDecision> {
        self.binding.decision()
    }

    pub(crate) fn reused(&self) -> bool {
        self.binding.reused()
    }

    pub(crate) fn connect_elapsed(&self) -> Option<Duration> {
        self.connect_elapsed
    }

    pub(crate) fn decision_wait_elapsed(&self) -> Duration {
        self.decision_wait_elapsed
    }
}

/// 只建立或租用 WebSocket，不发送 payload。
#[expect(clippy::too_many_arguments)]
pub(crate) async fn prepare_response_create_request_with_pool(
    request: &CodexWebSocketRequest,
    pool: Option<(
        &CodexWebSocketPool,
        CodexWebSocketPoolKey,
        Option<gateway_core::lifecycle::CancellationToken>,
    )>,
    breaker: &WebSocketOriginBreaker,
    origin_key: &str,
    fast_path_budget: Option<Duration>,
    require_pool: bool,
    fallback_stream_idle_timeout: Option<Duration>,
    connection_budget: Option<gateway_core::engine::connection::ConnectionBudget>,
) -> Result<WebSocketFastPath<PreparedWebSocket>, CodexWebSocketExchangeError> {
    let decision_started_at = Instant::now();
    let Some((pool, key, downstream_cancellation)) = pool else {
        if require_pool || !request.continuation().permits_fresh_connection() {
            return Err(continuation_unavailable(
                PreviousResponseUnavailableReason::PoolUnavailable,
            ));
        }
        return prepare_unpooled_websocket(
            request,
            breaker,
            origin_key,
            fast_path_budget,
            fallback_stream_idle_timeout,
            decision_started_at,
            connection_budget,
        )
        .await;
    };

    let key = key.with_proxy_capacity(request.connection().outbound_proxy.as_ref());
    if let Some(cancellation) = &downstream_cancellation {
        pool.register_downstream(&key, cancellation);
    }
    let required_response_id = match request.continuation() {
        WebSocketContinuationRequirement::ConnectionLocal { response_id } => {
            Some(response_id.as_str())
        }
        WebSocketContinuationRequirement::NewChain
        | WebSocketContinuationRequirement::Persisted { .. }
        | WebSocketContinuationRequirement::ExternalUnknown { .. } => None,
    };
    let capacity_started_at = TokioInstant::now();
    let decision_deadline = fast_path_budget.map(|budget| capacity_started_at + budget);
    let acquired = match wait_for_fast_path(capacity_started_at, fast_path_budget, async {
        tokio::select! {
            biased;
            () = async { match &downstream_cancellation {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }} => WebSocketPoolAcquire::Bypass(WebSocketPoolBypassReason::Disabled),
            acquired = pool.acquire(&key, required_response_id) => acquired,
        }
    })
    .await
    {
        WebSocketFastPath::Ready(acquired) => acquired,
        WebSocketFastPath::Missed => return Ok(WebSocketFastPath::Missed),
    };
    match acquired {
        WebSocketPoolAcquire::Reused { connection, lease } => {
            if let WebSocketContinuationRequirement::ConnectionLocal { response_id } =
                request.continuation()
                && connection.continuation.latest_response_id() != Some(response_id.as_str())
            {
                lease.put(*connection).await;
                return Err(continuation_unavailable(
                    PreviousResponseUnavailableReason::LatestResponseMismatch,
                ));
            }
            Ok(WebSocketFastPath::Ready(PreparedWebSocket {
                connection: *connection,
                binding: PoolBinding::Pooled {
                    lease: Box::new(lease),
                    reused: true,
                    decision: WebSocketPoolDecision::reuse(),
                },
                connect_elapsed: None,
                decision_wait_elapsed: decision_started_at.elapsed(),
                stream_idle_timeout: pool.stream_idle_timeout(),
            }))
        }
        WebSocketPoolAcquire::Connect(connect_lease) => {
            if !request.continuation().permits_fresh_connection() {
                connect_lease.failed().await;
                return Err(continuation_unavailable(
                    PreviousResponseUnavailableReason::FreshConnectionRequired,
                ));
            }
            prepare_pooled_websocket(
                request,
                pool,
                connect_lease,
                breaker,
                origin_key,
                decision_deadline,
                decision_started_at,
                connection_budget,
            )
            .await
        }
        WebSocketPoolAcquire::Wait(waiter) => {
            if require_pool || !request.continuation().permits_fresh_connection() {
                return Err(continuation_unavailable(
                    PreviousResponseUnavailableReason::ConnectionBusy,
                ));
            }
            let shared_budget =
                shared_opening_budget(waiter.started_at(), fast_path_budget, decision_deadline);
            match wait_for_shared_connect(waiter, shared_budget).await {
                WebSocketFastPath::Missed => Ok(WebSocketFastPath::Missed),
                WebSocketFastPath::Ready(WebSocketPoolConnectOutcome::Ready) => Err(
                    continuation_unavailable(PreviousResponseUnavailableReason::ConnectionBusy),
                ),
                WebSocketFastPath::Ready(
                    WebSocketPoolConnectOutcome::Failed | WebSocketPoolConnectOutcome::Pending,
                ) => Err(CodexWebSocketExchangeError::SharedConnectFailed),
                WebSocketFastPath::Ready(WebSocketPoolConnectOutcome::LocalEgress(error)) => {
                    Err(CodexWebSocketExchangeError::Egress(error))
                }
            }
        }
        WebSocketPoolAcquire::ContinuationLost(observation) => Err(continuation_unavailable(
            PreviousResponseUnavailableReason::ReusedConnectionLost,
        )
        .with_connection_observation(observation)),
        WebSocketPoolAcquire::LocalEgress(error) => Err(CodexWebSocketExchangeError::Egress(error)),
        WebSocketPoolAcquire::Bypass(reason) => Err(bypass_unavailable_error(reason)),
    }
}

fn shared_opening_budget(
    opening_started_at: TokioInstant,
    budget: Option<Duration>,
    request_deadline: Option<TokioInstant>,
) -> Option<Duration> {
    budget.zip(request_deadline).map(|(budget, deadline)| {
        budget.min(deadline.saturating_duration_since(opening_started_at))
    })
}

#[cfg(test)]
#[test]
fn shared_opening_deadline_does_not_double_charge_account_capacity_wait() {
    let opening_started = TokioInstant::now();
    let request_started = opening_started + Duration::from_millis(100);
    let budget = Duration::from_millis(800);
    assert_eq!(
        shared_opening_budget(
            opening_started,
            Some(budget),
            Some(request_started + budget)
        ),
        Some(budget),
    );
    let later_opening = request_started + Duration::from_millis(300);
    assert_eq!(
        shared_opening_budget(later_opening, Some(budget), Some(request_started + budget)),
        Some(Duration::from_millis(500)),
    );
    assert_eq!(shared_opening_budget(opening_started, None, None), None);
}

async fn prepare_unpooled_websocket(
    request: &CodexWebSocketRequest,
    breaker: &WebSocketOriginBreaker,
    origin_key: &str,
    fast_path_budget: Option<Duration>,
    stream_idle_timeout: Option<Duration>,
    decision_started_at: Instant,
    connection_budget: Option<gateway_core::engine::connection::ConnectionBudget>,
) -> Result<WebSocketFastPath<PreparedWebSocket>, CodexWebSocketExchangeError> {
    let permit = acquire_breaker_permit(breaker, origin_key, fast_path_budget.is_some())?;
    let context = pump_log_context_from_connection(request.connection());
    let connected = connect_with_budget(
        request.connection(),
        fast_path_budget,
        context,
        connection_budget.as_ref(),
    )
    .await;
    let connected = match connected {
        Ok(WebSocketFastPath::Ready(connection)) => Ok(connection),
        Ok(WebSocketFastPath::Missed) => {
            if let Some(permit) = permit {
                permit.fast_timeout();
            }
            return Ok(WebSocketFastPath::Missed);
        }
        Err(error) => Err(error),
    };
    let (connection, connect_elapsed) = finish_breaker_attempt(permit, connected)?;
    Ok(WebSocketFastPath::Ready(PreparedWebSocket {
        connection,
        binding: PoolBinding::Unpooled,
        connect_elapsed: Some(connect_elapsed),
        decision_wait_elapsed: decision_started_at.elapsed(),
        stream_idle_timeout,
    }))
}

#[expect(clippy::too_many_arguments)]
async fn prepare_pooled_websocket(
    request: &CodexWebSocketRequest,
    pool: &CodexWebSocketPool,
    connect_lease: WebSocketPoolConnectLease,
    breaker: &WebSocketOriginBreaker,
    origin_key: &str,
    decision_deadline: Option<TokioInstant>,
    decision_started_at: Instant,
    connection_budget: Option<gateway_core::engine::connection::ConnectionBudget>,
) -> Result<WebSocketFastPath<PreparedWebSocket>, CodexWebSocketExchangeError> {
    let permit = match acquire_breaker_permit(breaker, origin_key, decision_deadline.is_some()) {
        Ok(permit) => permit,
        Err(error) => {
            connect_lease.failed().await;
            return Err(error);
        }
    };
    let fast_path_budget = decision_deadline
        .map(|deadline| deadline.saturating_duration_since(connect_lease.started_at()));
    let proxy_permit = match wait_for_fast_path(
        connect_lease.started_at(),
        fast_path_budget,
        Box::pin(super::pool::proxy_capacity::acquire(
            request.connection().outbound_proxy.as_ref(),
            Some(pool),
        )),
    )
    .await
    {
        WebSocketFastPath::Ready(Ok(permit)) => permit,
        WebSocketFastPath::Ready(Err(error)) => {
            if let Some(permit) = permit {
                permit.cancel();
            }
            connect_lease.failed().await;
            return Err(error);
        }
        WebSocketFastPath::Missed => {
            if let Some(permit) = permit {
                permit.cancel();
            }
            connect_lease.failed().await;
            return Ok(WebSocketFastPath::Missed);
        }
    };
    let mut waiter = start_pooled_websocket_connect(
        request.connection().clone(),
        pool.clone(),
        connect_lease,
        permit,
        connection_budget,
        proxy_permit,
    );
    match waiter.wait(fast_path_budget).await? {
        WebSocketFastPath::Ready(handoff) => Ok(WebSocketFastPath::Ready(PreparedWebSocket {
            connection: *handoff.connection,
            binding: PoolBinding::Pooled {
                lease: Box::new(handoff.lease),
                reused: false,
                decision: WebSocketPoolDecision::new(),
            },
            connect_elapsed: Some(handoff.connect_elapsed),
            decision_wait_elapsed: decision_started_at.elapsed(),
            stream_idle_timeout: pool.stream_idle_timeout(),
        })),
        WebSocketFastPath::Missed => Ok(WebSocketFastPath::Missed),
    }
}

struct PooledWebSocketConnectWaiter {
    started_at: tokio::time::Instant,
    receiver: oneshot::Receiver<Result<PooledWebSocketHandoff, CodexWebSocketExchangeError>>,
}

struct PooledWebSocketHandoff {
    connection: Box<PooledWebSocketConnection>,
    lease: WebSocketPoolLease,
    connect_elapsed: Duration,
}

impl PooledWebSocketConnectWaiter {
    async fn wait(
        &mut self,
        fast_path_budget: Option<Duration>,
    ) -> Result<WebSocketFastPath<PooledWebSocketHandoff>, CodexWebSocketExchangeError> {
        let received =
            match wait_for_fast_path(self.started_at, fast_path_budget, &mut self.receiver).await {
                WebSocketFastPath::Ready(received) => received,
                WebSocketFastPath::Missed => {
                    return Ok(WebSocketFastPath::Missed);
                }
            };
        received
            .map_err(|_| CodexWebSocketExchangeError::SharedConnectFailed)?
            .map(WebSocketFastPath::Ready)
    }
}

fn start_pooled_websocket_connect(
    connection: CodexWebSocketConnection,
    pool: CodexWebSocketPool,
    connect_lease: WebSocketPoolConnectLease,
    permit: Option<WebSocketOriginBreakerPermit>,
    connection_budget: Option<gateway_core::engine::connection::ConnectionBudget>,
    proxy_permit: Option<super::pool::proxy_capacity::ProxyConnectionPermit>,
) -> PooledWebSocketConnectWaiter {
    let task_key = connect_lease.key().clone();
    let started_at = connect_lease.started_at();
    let cancellation = connect_lease.cancellation_token();
    let context = PumpLogContext::new(
        Some(task_key.account_id().to_owned()),
        Some(task_key.conversation_id_hash()),
    );
    let (sender, receiver) = oneshot::channel();
    pool.spawn_connect_task(async move {
        let connected = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                if let Some(permit) = permit { permit.cancel(); }
                let error = connect_lease.local_egress_error().map_or(
                    CodexWebSocketExchangeError::SharedConnectFailed,
                    CodexWebSocketExchangeError::Egress,
                );
                let _ = sender.send(Err(error));
                connect_lease.failed().await;
                tracing::info!(
                    account_id = task_key.account_id(),
                    conversation_id_hash = task_key.conversation_id_hash(),
                    ws_preconnect_duration_ms = duration_millis_u64(started_at.elapsed()),
                    ws_preconnect_outcome = "cancelled",
                    "WebSocket pool connect finished"
                );
                return;
            }
            result = async {
                // Foreground fallback leaves this opening alive; only its final result
                // belongs to the origin breaker.
                connect_websocket_connection(&connection, context, connection_budget.as_ref(), proxy_permit).await
            } => result,
        };
        if let Err(error) = &connected
            && let Some(lease) = &connection.session_proxy
        {
            lease.report_websocket_error(error);
        }
        match finish_breaker_attempt(permit, connected) {
            Ok((connection, connect_elapsed)) => {
                match connect_lease.connected_reserved(connection).await {
                    Ok((connection, lease)) => {
                        let handoff = PooledWebSocketHandoff {
                            connection,
                            lease,
                            connect_elapsed,
                        };
                        let foreground_waiting = match sender.send(Ok(handoff)) {
                            Ok(()) => true,
                            Err(Ok(handoff)) => {
                                handoff.lease.put(*handoff.connection).await;
                                false
                            }
                            Err(Err(_)) => false,
                        };
                        tracing::info!(
                            account_id = task_key.account_id(),
                            conversation_id_hash = task_key.conversation_id_hash(),
                            ws_preconnect_duration_ms = duration_millis_u64(connect_elapsed),
                            foreground_waiting,
                            ws_preconnect_outcome = "ready",
                            "WebSocket pool connect finished"
                        );
                    }
                    Err((closing, local_egress)) => {
                        let error = local_egress.map_or_else(
                            || {
                                continuation_unavailable(
                                    PreviousResponseUnavailableReason::PoolUnavailable,
                                )
                            },
                            CodexWebSocketExchangeError::Egress,
                        );
                        let _ = sender.send(Err(error));
                        let _ = closing.await;
                        tracing::info!(
                            account_id = task_key.account_id(),
                            conversation_id_hash = task_key.conversation_id_hash(),
                            ws_preconnect_duration_ms = duration_millis_u64(connect_elapsed),
                            ws_preconnect_outcome = "rejected",
                            "WebSocket pool connect finished"
                        );
                    }
                }
            }
            Err(error) => {
                let connection_exit_reason = opening_failure_reason(&error);
                let error_message = error.to_string();
                let local_egress = match error.classified() {
                    CodexWebSocketExchangeError::Egress(error) => Some(*error),
                    _ => connect_lease.local_egress_error(),
                };
                // 先交付 opening 原始错误，避免连接池清理侵占前台 fast-path 预算。
                let error = local_egress.map_or(error, CodexWebSocketExchangeError::Egress);
                let foreground_waiting = sender.send(Err(error)).is_ok();
                connect_lease.failed_with_egress(local_egress).await;
                tracing::warn!(
                    account_id = task_key.account_id(),
                    conversation_id_hash = task_key.conversation_id_hash(),
                    ws_preconnect_duration_ms = duration_millis_u64(started_at.elapsed()),
                    foreground_waiting,
                    connection_exit_reason,
                    error = %error_message,
                    ws_preconnect_outcome = "failed",
                    "WebSocket pool connect finished"
                );
            }
        }
    });
    PooledWebSocketConnectWaiter {
        started_at,
        receiver,
    }
}

fn opening_failure_reason(error: &CodexWebSocketExchangeError) -> &'static str {
    match error.classified() {
        CodexWebSocketExchangeError::InvalidRequest(_) => "request_build_failure",
        CodexWebSocketExchangeError::Connect(error)
        | CodexWebSocketExchangeError::Transport(error) => transport_metric_reason(error),
        CodexWebSocketExchangeError::ConnectTimeout { .. } => "connect_timeout",
        CodexWebSocketExchangeError::Upstream(_) => "upgrade_rejected",
        CodexWebSocketExchangeError::OriginCircuitOpen
        | CodexWebSocketExchangeError::OriginHalfOpenBusy => "connect_suppressed",
        CodexWebSocketExchangeError::SharedConnectFailed => "shared_connect_failed",
        _ => "connect_failure",
    }
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis())
        .unwrap_or(u64::MAX)
        .max(1)
}

/// 非池化建连的 pump 日志上下文：从业务头提取账号归属（会话 hash 只有池 key 才有）。
fn pump_log_context_from_connection(connection: &CodexWebSocketConnection) -> PumpLogContext {
    let account_id = connection
        .headers()
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("chatgpt-account-id"))
        .map(|(_, value)| value.clone());
    PumpLogContext::new(account_id, None)
}

async fn connect_with_budget(
    connection: &CodexWebSocketConnection,
    fast_path_budget: Option<Duration>,
    context: PumpLogContext,
    connection_budget: Option<&gateway_core::engine::connection::ConnectionBudget>,
) -> Result<WebSocketFastPath<(PooledWebSocketConnection, Duration)>, CodexWebSocketExchangeError> {
    // Keep the nested handshake future off the caller's stack.
    match wait_for_fast_path(
        TokioInstant::now(),
        fast_path_budget,
        Box::pin(async {
            let permit = Box::pin(super::pool::proxy_capacity::acquire(
                connection.outbound_proxy.as_ref(),
                None,
            ))
            .await?;
            connect_websocket_connection(connection, context, connection_budget, permit).await
        }),
    )
    .await
    {
        WebSocketFastPath::Ready(connected) => connected.map(WebSocketFastPath::Ready),
        WebSocketFastPath::Missed => Ok(WebSocketFastPath::Missed),
    }
}

async fn connect_websocket_connection(
    connection: &CodexWebSocketConnection,
    context: PumpLogContext,
    connection_budget: Option<&gateway_core::engine::connection::ConnectionBudget>,
    proxy_permit: Option<super::pool::proxy_capacity::ProxyConnectionPermit>,
) -> Result<(PooledWebSocketConnection, Duration), CodexWebSocketExchangeError> {
    let started_at = Instant::now();
    let (websocket, response) =
        connect_pumped_websocket(connection, context, connection_budget, proxy_permit).await?;
    Ok((
        PooledWebSocketConnection {
            websocket,
            metadata: websocket_connection_metadata(connection, &response),
            continuation: WebSocketContinuationState::default(),
            created_at: tokio::time::Instant::now(),
        },
        started_at.elapsed(),
    ))
}

fn acquire_breaker_permit(
    breaker: &WebSocketOriginBreaker,
    origin_key: &str,
    allow_http_fallback: bool,
) -> Result<Option<WebSocketOriginBreakerPermit>, CodexWebSocketExchangeError> {
    if !allow_http_fallback {
        return Ok(None);
    }
    match breaker.try_acquire(origin_key) {
        WebSocketOriginBreakerDecision::Allowed(permit) => Ok(Some(permit)),
        WebSocketOriginBreakerDecision::Open => Err(CodexWebSocketExchangeError::OriginCircuitOpen),
        WebSocketOriginBreakerDecision::HalfOpenBusy => {
            Err(CodexWebSocketExchangeError::OriginHalfOpenBusy)
        }
    }
}

fn finish_breaker_attempt(
    permit: Option<WebSocketOriginBreakerPermit>,
    connected: Result<(PooledWebSocketConnection, Duration), CodexWebSocketExchangeError>,
) -> Result<(PooledWebSocketConnection, Duration), CodexWebSocketExchangeError> {
    let Some(permit) = permit else {
        return connected;
    };
    match connected {
        Err(error) if error.connection_budget_exhausted() => {
            permit.cancel();
            Err(error)
        }
        Ok(connection) => {
            permit.succeed();
            Ok(connection)
        }
        Err(CodexWebSocketExchangeError::Upstream(upstream)) if upstream.status_code < 500 => {
            // 账号或请求级 opening 响应证明 origin 可达，不得污染 transport 熔断器。
            permit.succeed();
            Err(CodexWebSocketExchangeError::Upstream(upstream))
        }
        Err(error @ CodexWebSocketExchangeError::Egress(_)) => {
            // An armed permit's Drop records failure; local routing is not origin health.
            permit.cancel();
            Err(error)
        }
        Err(error) => {
            permit.fail();
            Err(error)
        }
    }
}

async fn wait_for_shared_connect(
    waiter: pool::WebSocketPoolConnectWaiter,
    fast_path_budget: Option<Duration>,
) -> WebSocketFastPath<WebSocketPoolConnectOutcome> {
    wait_for_fast_path(waiter.started_at(), fast_path_budget, waiter.wait()).await
}

// 同一次 opening 的所有等待者共享截止时间；后来的请求不会重置 800ms 预算。
async fn wait_for_fast_path<F: Future>(
    started_at: TokioInstant,
    budget: Option<Duration>,
    future: F,
) -> WebSocketFastPath<F::Output> {
    let Some(budget) = budget else {
        return WebSocketFastPath::Ready(future.await);
    };
    match timeout_at(started_at + budget, future).await {
        Ok(value) => WebSocketFastPath::Ready(value),
        Err(_) => WebSocketFastPath::Missed,
    }
}

fn bypass_unavailable_error(reason: WebSocketPoolBypassReason) -> CodexWebSocketExchangeError {
    match reason {
        WebSocketPoolBypassReason::Busy => {
            continuation_unavailable(PreviousResponseUnavailableReason::ConnectionBusy)
        }
        WebSocketPoolBypassReason::Disabled | WebSocketPoolBypassReason::Cap => {
            continuation_unavailable(PreviousResponseUnavailableReason::PoolUnavailable)
        }
        WebSocketPoolBypassReason::ContinuationNotFound => {
            continuation_unavailable(PreviousResponseUnavailableReason::FreshConnectionRequired)
        }
    }
}

fn continuation_unavailable(
    reason: PreviousResponseUnavailableReason,
) -> CodexWebSocketExchangeError {
    CodexWebSocketExchangeError::ContinuationUnavailable { reason }
}

pub(crate) async fn execute_prepared_response_create_request_stream(
    request: &CodexWebSocketRequest,
    prepared: PreparedWebSocket,
    response_control: Option<gateway_core::engine::response_control::ResponseControl>,
    trace: gateway_core::diagnostics::TraceContext,
) -> Result<CodexWebSocketStreamingExchange, CodexWebSocketExchangeError> {
    let PreparedWebSocket {
        connection,
        binding,
        stream_idle_timeout,
        ..
    } = prepared;
    let (lease, reused, pool_decision) = binding.into_parts();
    let PooledWebSocketConnection {
        mut websocket,
        mut metadata,
        continuation,
        created_at,
    } = connection;
    // Only the current exchange owns an active exit reference. Idle sockets do not.
    metadata.session_proxy = request.connection.session_proxy.clone();
    trace.record("upstream.connection", serde_json::json!({
        "connectionId": websocket.connection_id().to_string(), "reused": reused,
        "pool": pool_decision.map_or("unpooled", WebSocketPoolDecision::kind),
        "status": metadata.diagnostics.status_code,
        "upstreamRequestId": metadata.diagnostics.request_id,
        "headers": gateway_core::diagnostics::diagnostic_headers(metadata.diagnostics.trace_headers.iter().map(|(name, value)| (name.as_str(), value.as_str()))),
    }));
    trace.capture("upstream.request.body", request.payload_text().as_bytes());
    if let Err(error) = send_websocket_request(&websocket, request.payload_text()).await {
        let not_started = matches!(error, CodexWebSocketExchangeError::SendNotStarted);
        if let Some(lease) = &metadata.session_proxy {
            lease.report_websocket_error(&error);
        }
        let observation = websocket.observation();
        let observation = if observation.exit_reason() == "running" {
            observation.with_exit_reason("outbound_transport_error")
        } else {
            observation
        };
        trace.record(
            "upstream.send.failed",
            serde_json::json!({
                "phase": "websocket_payload", "sendState": if not_started { "not_sent" } else { "ambiguous" },
                "failureReason": error.transport_failure_reason().unwrap_or(observation.exit_reason()),
                "connectionId": observation.connection_id().to_string(),
                "connectionAgeMs": observation.age_ms(),
                "connectionIdleMs": observation.idle_ms(),
                "reused": reused,
            }),
        );
        discard_after_send(websocket, lease, observation.clone()).await;
        let error = if not_started {
            if request.continuation().permits_fresh_connection() {
                error
            } else {
                continuation_unavailable(PreviousResponseUnavailableReason::ReusedConnectionLost)
            }
        } else {
            post_send_ambiguous(error)
        };
        return Err(error.with_connection_observation(observation));
    }
    trace.record("upstream.payload.sent", serde_json::json!({}));
    websocket.bind_control(response_control.as_ref());
    let connection_local_available = lease.is_some();
    let pool_return = lease.map(|lease| WebSocketStreamPoolReturn {
        lease,
        created_at,
        continuation,
    });
    let mut exchange = stream_websocket_response(
        websocket,
        metadata,
        pool_return,
        reused,
        stream_idle_timeout,
        trace,
        response_control,
    );
    exchange.pool_decision = pool_decision;
    exchange.connection_local_continuation = connection_local_available;
    Ok(exchange)
}

async fn discard_after_send(
    websocket: PumpedWebSocket,
    lease: Option<WebSocketPoolLease>,
    observation: super::pump::WebSocketConnectionObservation,
) {
    if let Some(lease) = lease {
        let _ = lease.discard_with_observation(websocket, observation).await;
    } else {
        websocket.abort_and_wait().await;
    }
}

pub(crate) fn post_send_ambiguous(
    error: CodexWebSocketExchangeError,
) -> CodexWebSocketExchangeError {
    match error {
        error @ CodexWebSocketExchangeError::Upstream(_)
        | error @ CodexWebSocketExchangeError::PostSendAmbiguous { .. } => error,
        error => CodexWebSocketExchangeError::PostSendAmbiguous {
            message: error.to_string(),
            source: Some(Box::new(error)),
        },
    }
}
