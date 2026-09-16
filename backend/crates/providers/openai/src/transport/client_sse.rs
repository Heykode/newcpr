use gateway_core::diagnostics::{StreamCapture, StreamFormat, TraceContext, diagnostic_json};

use std::{sync::Arc, time::Instant};

use futures::{StreamExt, TryStreamExt};
use gateway_protocol::openai::{
    X_OPENAI_MEMGEN_REQUEST_HEADER,
    events::{self, retry_after_seconds_from_body},
    sse::{SseEventDecoder, SseFrame},
};
use reqwest::{
    Client, Response as ReqwestResponse,
    header::{CONTENT_ENCODING, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;

use crate::transport::{
    catalog::{
        CodexModelCatalogError, CodexModelCatalogSnapshot, MAX_CODEX_MODEL_CATALOG_BYTES,
        catalog_etag, parse_codex_model_catalog,
    },
    diagnostics::CodexUpstreamSendPhase,
    endpoints::{CODEX_RESPONSES_PATH, endpoint_url},
    headers::websocket_header_pairs,
    profile::CodexWireProfileState,
    protocol::{
        responses::{
            CodexResponsesRequest, ResponsesSseFailure, TransportRequirement, transport_requirement,
        },
        websocket::{
            websocket_audit_artifact_from_attempt, websocket_connection_limit_failure,
            websocket_payload_audit_snapshot,
        },
    },
    request::normalize_generate_upstream_body,
    response_meta,
    websocket::{
        CodexWebSocketConnection, CodexWebSocketExchangeError, CodexWebSocketPool,
        CodexWebSocketPoolKey, CodexWebSocketStreamingExchange, WEBSOCKET_FAST_PATH_BUDGET,
        WebSocketFastPath, WebSocketOriginBreaker, execute_prepared_response_create_request_stream,
        post_send_ambiguous, prepare_response_create_request_with_pool, websocket_audit_dir,
        write_websocket_audit_artifact_from_env,
    },
};

use super::client::*;

const HTTP_ZSTD_MIN_BYTES: usize = 1024;

impl CodexBackendClient {
    /// 构造客户端。
    pub fn new(
        client: Client,
        base_url: impl Into<String>,
        profile: CodexWireProfileState,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            direct_client: client.clone(),
            client,
            websocket_origin_key: websocket_origin_key(&base_url),
            outbound_proxy: None,
            egress_key: String::new(),
            egress_runtime: None,
            egress_route: None,
            egress_account: None,
            attempt_pinned: false,
            forced_pool_key: None,
            request_tuning: None,
            base_url,
            profile,
            websocket_pool: None,
            websocket_origin_breaker: WebSocketOriginBreaker::default(),
        }
    }

    /// 为 Responses WebSocket 请求启用连接池。
    pub fn with_websocket_pool(mut self, pool: Arc<CodexWebSocketPool>) -> Self {
        self.websocket_pool = Some(pool);
        self
    }

    /// 驱逐指定账号的 Responses WebSocket 池连接。
    pub async fn evict_websocket_account(&self, account_id: &str) {
        if let Some(pool) = &self.websocket_pool {
            pool.evict_account(account_id).await;
        }
    }

    /// 发送 Responses SSE 请求并返回 live SSE 流（HTTP SSE fallback）。
    pub(crate) async fn create_response_stream_http_sse(
        &self,
        upstream_request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
    ) -> CodexClientResult<CodexBackendStreamingResponse> {
        let profile = self.profile.snapshot();
        let headers = self.request_headers_for_http_response(upstream_request, context)?;
        let headers_started_at = Instant::now();
        // 身份投影先完成，再按最终 JSON 大小决定是否使用 zstd。
        // Codex 上游只交付 SSE；即使下游请求 `stream: false`，也要上游流式执行，
        // 再由 API 层收集 canonical events 并返回完整 JSON。不能把下游的传输偏好
        // 直接透传给 Codex，否则上游会以 400 拒绝非流式请求。
        let mut upstream_body = upstream_request.body().clone();
        if profile.application_profile == super::profile::CodexApplicationProfile::QxCompatible {
            super::qx_application::project_response_body(
                &mut upstream_body,
                upstream_request,
                context,
            );
        }
        normalize_generate_upstream_body(&mut upstream_body);
        upstream_body.insert("stream".to_owned(), serde_json::Value::Bool(true));
        upstream_body
            .entry("store")
            .or_insert(serde_json::Value::Bool(false));
        let body =
            serde_json::to_vec(&upstream_body).map_err(CodexClientError::RequestBodyEncode)?;
        let endpoint = endpoint_url(&self.base_url, CODEX_RESPONSES_PATH);
        let trace = context
            .trace
            .cloned()
            .unwrap_or_default()
            .exchange("http_sse");
        trace.headers(
            "upstream.request.headers",
            serde_json::json!({
                "method": "POST", "endpoint": CODEX_RESPONSES_PATH,
            }),
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        trace.capture("upstream.request.body", &body);
        let compressed = body.len() >= HTTP_ZSTD_MIN_BYTES;
        let body = if compressed {
            zstd::stream::encode_all(std::io::Cursor::new(body), 3)
                .map_err(CodexClientError::RequestCompression)?
        } else {
            body
        };
        let response = self
            .send_profiled(&profile, false, |client| {
                let builder = client.post(endpoint).headers(headers).body(body);
                if compressed {
                    builder.header(CONTENT_ENCODING, HeaderValue::from_static("zstd"))
                } else {
                    builder
                }
            })
            .await?;
        let upstream_headers_ms = elapsed_duration_millis(headers_started_at.elapsed());
        let http_version = http_version_name(response.version()).to_string();
        let status = response.status();
        trace.headers(
            "upstream.response.headers",
            serde_json::json!({
                "status": status.as_u16(), "httpVersion": http_version,
                "headersMs": upstream_headers_ms,
            }),
            response
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        let diagnostics = response_meta::diagnostics(Some(status.as_u16()), response.headers());
        let turn_state = response_meta::turn_state(response.headers());
        let set_cookie_headers = response_meta::set_cookie_headers(response.headers());
        let rate_limit_headers = response_meta::rate_limit_headers(response.headers());
        let response_metadata = response_meta::response_metadata(response.headers());
        let retry_after_seconds = retry_after_seconds(response.headers(), None);

        if !status.is_success() {
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .map(|value| value.as_bytes().to_vec());
            let client_headers = response_meta::client_headers(response.headers());
            let raw_body = read_error_response_body(response).await.map_err(|source| {
                CodexClientError::ErrorBodyRead {
                    source,
                    status,
                    diagnostics: Box::new(diagnostics.clone()),
                    transport: CodexBackendTransport::HttpSse,
                    transport_metrics: Box::new(CodexTransportMetrics {
                        upstream_headers_ms: Some(upstream_headers_ms),
                        http_version: Some(http_version.clone()),
                        ..CodexTransportMetrics::default()
                    }),
                }
            })?;
            trace.capture("upstream.error.body", &raw_body);
            let body = String::from_utf8_lossy(&raw_body).into_owned();
            let retry_after_seconds =
                retry_after_seconds.or_else(|| retry_after_seconds_from_body(&body));
            return Err(CodexClientError::Upstream {
                status,
                body,
                client_response: Some(Box::new(CodexClientVisibleUpstreamResponse::new(
                    status,
                    content_type,
                    client_headers,
                    raw_body,
                ))),
                retry_after_seconds,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers,
                rate_limit_headers,
                transport: CodexBackendTransport::HttpSse,
                transport_metrics: Box::new(CodexTransportMetrics {
                    upstream_headers_ms: Some(upstream_headers_ms),
                    http_version: Some(http_version),
                    ..CodexTransportMetrics::default()
                }),
                send_phase: CodexUpstreamSendPhase::AfterPayload,
            });
        }

        let rate_limit_updates = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        Ok(CodexBackendStreamingResponse {
            body: http_sse_stream(response, Arc::clone(&rate_limit_updates), trace),
            transport: CodexBackendTransport::HttpSse,
            websocket_connection_id: None,
            turn_state,
            set_cookie_headers,
            rate_limit_headers,
            rate_limit_updates: Some(rate_limit_updates),
            turn_state_update: None,
            websocket_pool_decision: None,
            diagnostics,
            response_metadata,
            transport_metrics: CodexTransportMetrics {
                upstream_headers_ms: Some(upstream_headers_ms),
                http_version: Some(http_version),
                ..CodexTransportMetrics::default()
            },
            connection_local_continuation: false,
        })
    }

    pub async fn create_response_stream_with_pool_account(
        &self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        pool_account_id: Option<&str>,
    ) -> CodexClientResult<CodexBackendStreamingResponse> {
        let prepared = self
            .prepare_response_transport_with_pool_account(request, context, pool_account_id)
            .await?;
        self.create_response_stream_with_prepared(request, context, prepared)
            .await
    }

    /// 在发送 payload 前完成 transport 选择和可取消的 WebSocket opening。
    #[doc(hidden)]
    pub(crate) async fn prepare_response_transport_with_pool_account(
        &self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        pool_account_id: Option<&str>,
    ) -> CodexClientResult<PreparedResponseTransport> {
        let mut client = self.clone();
        client.profile = self.profile.frozen();
        client.prepare_egress_attempt(request, context, pool_account_id)?;
        let mut prepared = client
            .prepare_response_transport_inner(request, context, pool_account_id)
            .await?;
        prepared.attempt_client = Some(Box::new(client));
        Ok(prepared)
    }

    async fn prepare_response_transport_inner(
        &self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        pool_account_id: Option<&str>,
    ) -> CodexClientResult<PreparedResponseTransport> {
        let requirement = transport_requirement(request);
        context.trace.cloned().unwrap_or_default().record(
            "transport.preparing",
            serde_json::json!({
                "requirement": requirement.as_str(),
            }),
        );
        if requirement == TransportRequirement::HttpRequired {
            return Ok(PreparedResponseTransport {
                attempt_client: None,
                requirement,
                route: PreparedResponseRoute::Http,
                metrics: CodexTransportMetrics {
                    decision: Some(CodexTransportDecision::HttpRequired),
                    ..CodexTransportMetrics::default()
                },
            });
        }

        let mut websocket_request = websocket_upstream_request(request);
        if self.profile.snapshot().application_profile
            == super::profile::CodexApplicationProfile::QxCompatible
        {
            // Preserve the original seed for the subsequent opening-header projection too.
            websocket_request =
                super::qx_application::project_response_request(&websocket_request, context);
        }
        let headers = self.request_headers_for_websocket_response(&websocket_request, context)?;
        let mut websocket_create = CodexWebSocketConnection::responses_create_request(
            &self.base_url,
            &generate_key(),
            websocket_header_pairs(&headers),
            &websocket_request,
        )
        .map_err(CodexClientError::WebSocketEncode)?;
        websocket_create.connection.outbound_proxy = self.outbound_proxy.clone();
        websocket_create.connection.tls_profile = self.profile.snapshot().tls_profile;
        websocket_create.connection.egress_source =
            self.egress_route.as_ref().map(|route| route.source);
        context.trace.cloned().unwrap_or_default().headers(
            "upstream.request.headers",
            serde_json::json!({"transport": "websocket", "phase": "prepared_opening"}),
            websocket_create
                .connection()
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        // 审计未启用时跳过 artifact 构造：payload 快照会深拷贝整个请求 body，
        // 且位于首字节前的关键路径上。
        if websocket_audit_dir().is_some() {
            let artifact = websocket_audit_artifact_from_attempt(
                &websocket_request,
                websocket_create.connection().opening_audit_snapshot(),
                websocket_payload_audit_snapshot(&websocket_request),
            );
            if let Err(error) = write_websocket_audit_artifact_from_env(&artifact).await {
                tracing::warn!(error = %error, "Failed to write Codex WebSocket audit artifact");
            }
        }
        let connection_profile = websocket_connection_profile(&headers, &self.profile.snapshot());
        let pool_key =
            self.websocket_pool_key(request, context, pool_account_id, &connection_profile);
        let pool_log_context = pool_key.as_ref().map(WebSocketPoolLogContext::from_key);
        let pool = self.websocket_pool.as_deref().zip(pool_key);
        let fast_path_budget = match requirement {
            TransportRequirement::PersistedContinuation | TransportRequirement::NewChain => {
                Some(WEBSOCKET_FAST_PATH_BUDGET)
            }
            TransportRequirement::ExplicitWebSocketWarmup
            | TransportRequirement::WebSocketNewChain
            | TransportRequirement::ExactWebSocketContinuation
            | TransportRequirement::ExternalUnknown => None,
            TransportRequirement::HttpRequired => None,
        };
        let fast_path_budget =
            fast_path_budget.filter(|_| self.request_tuning().websocket_http_fallback_enabled);
        let prepare_started_at = Instant::now();
        let prepared = prepare_response_create_request_with_pool(
            &websocket_create,
            pool,
            &self.websocket_origin_breaker,
            &self.websocket_origin_key,
            fast_path_budget,
            requirement.requires_websocket(),
            (self.request_tuning().websocket_stream_idle_timeout_ms != 0).then(|| {
                std::time::Duration::from_millis(
                    self.request_tuning().websocket_stream_idle_timeout_ms,
                )
            }),
        )
        .await;
        let prepared = match prepared {
            Ok(WebSocketFastPath::Ready(prepared)) => prepared,
            Ok(WebSocketFastPath::Missed) => {
                let decision = CodexTransportDecision::Http2WebSocketBudgetExhausted;
                let wait_ms = elapsed_duration_millis(prepare_started_at.elapsed());
                context.trace.cloned().unwrap_or_default().record(
                    "transport.fallback",
                    serde_json::json!({
                        "to": "http_sse", "reason": "websocket_fast_path_budget",
                        "requirement": requirement.as_str(), "decision": decision.as_str(),
                        "waitMs": wait_ms, "preconnectContinues": pool_log_context.is_some(),
                    }),
                );
                return Ok(PreparedResponseTransport {
                    attempt_client: None,
                    requirement,
                    route: PreparedResponseRoute::Http,
                    metrics: CodexTransportMetrics {
                        decision: Some(decision),
                        ws_connect_ms: None,
                        transport_decision_wait_ms: Some(wait_ms),
                        ..CodexTransportMetrics::default()
                    },
                });
            }
            Err(error)
                if self.request_tuning().websocket_http_fallback_enabled
                    && requirement.allows_pre_send_http_fallback()
                    && let Some(decision) = local_http_fallback_decision(&error) =>
            {
                let wait_ms = elapsed_duration_millis(prepare_started_at.elapsed());
                context.trace.cloned().unwrap_or_default().record(
                    "transport.fallback", serde_json::json!({
                        "to": "http_sse", "reason": "websocket_pre_send_failure",
                        "requirement": requirement.as_str(), "decision": decision.as_str(),
                        "waitMs": wait_ms,
                        "detail": diagnostic_json(&serde_json::json!({"message": error.to_string()})),
                    }),
                );
                return Ok(PreparedResponseTransport {
                    attempt_client: None,
                    requirement,
                    route: PreparedResponseRoute::Http,
                    metrics: CodexTransportMetrics {
                        decision: Some(decision),
                        ws_connect_ms: None,
                        transport_decision_wait_ms: Some(wait_ms),
                        ..CodexTransportMetrics::default()
                    },
                });
            }
            Err(error) => return Err(websocket_exchange_error_to_client_error(error)),
        };
        let decision = websocket_success_decision(requirement, &prepared);
        context.trace.cloned().unwrap_or_default().record(
            "transport.prepared",
            serde_json::json!({
                "decision": decision.as_str(),
                "requirement": requirement.as_str(),
                "connectMs": prepared.connect_elapsed().map(elapsed_duration_millis),
                "waitMs": elapsed_duration_millis(prepared.decision_wait_elapsed()),
            }),
        );
        let metrics = CodexTransportMetrics {
            decision: Some(decision),
            ws_connect_ms: prepared.connect_elapsed().map(elapsed_duration_millis),
            transport_decision_wait_ms: Some(elapsed_duration_millis(
                prepared.decision_wait_elapsed(),
            )),
            upstream_headers_ms: prepared.connect_elapsed().map(elapsed_duration_millis),
            first_event_ms: None,
            http_version: Some("HTTP/1.1".to_string()),
        };
        log_websocket_pool_decision(
            context,
            pool_account_id,
            pool_log_context.as_ref(),
            prepared.pool_decision(),
        );
        Ok(PreparedResponseTransport {
            attempt_client: None,
            requirement,
            route: PreparedResponseRoute::WebSocket(Box::new(PreparedWebSocketRoute {
                request: websocket_create,
                prepared,
            })),
            metrics,
        })
    }

    fn prepare_egress_attempt(
        &mut self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        pool_account_id: Option<&str>,
    ) -> CodexClientResult<()> {
        use super::websocket::CodexWebSocketRouting;
        self.attempt_pinned = true;
        self.forced_pool_key = None;
        if let Some((runtime, account)) = self
            .egress_runtime
            .as_ref()
            .zip(self.egress_account.as_ref())
        {
            runtime.check_account(account.id())?;
        }
        if transport_requirement(request) == TransportRequirement::HttpRequired {
            self.egress_route = self
                .egress_runtime
                .as_ref()
                .zip(self.egress_account.as_ref())
                .map(|(runtime, account)| runtime.select(account))
                .transpose()?
                .flatten();
            return Ok(());
        }
        let headers = self.request_headers_for_websocket_response(request, context)?;
        let profile_key = websocket_connection_profile(&headers, &self.profile.snapshot());
        let original_key = self.websocket_pool_key(request, context, pool_account_id, &profile_key);
        let exact =
            transport_requirement(request) == TransportRequirement::ExactWebSocketContinuation;
        let runtime = self.egress_runtime.as_ref();
        if exact
            && let Some((pool, key)) = self.websocket_pool.as_ref().zip(original_key.as_ref())
            && let Some(owner) = pool.routing_owner(key, request.previous_response_id())
            && let Some(routing) = &owner.routing
        {
            let mut selected = routing.egress.clone();
            if let Some(route) = &mut selected {
                route.continuation = true;
                runtime
                    .ok_or(super::egress::CodexEgressError::Unavailable)?
                    .check(route)?;
            }
            self.profile = CodexWireProfileState::new(routing.profile.clone());
            self.egress_route = selected;
            self.forced_pool_key = Some(owner);
            return Ok(());
        }
        self.egress_route = runtime
            .zip(self.egress_account.as_ref())
            .map(|(runtime, account)| runtime.select(account))
            .transpose()?
            .flatten();
        if let Some(route) = &self.egress_route {
            runtime
                .ok_or(super::egress::CodexEgressError::Unavailable)?
                .ensure_source_ready(route.source)?;
        }
        if let Some(mut key) = original_key {
            if let Some(route) = &self.egress_route {
                let suffix = if route.mode.is_fresh() {
                    format!(":fresh:{}", uuid::Uuid::new_v4())
                } else {
                    String::new()
                };
                key = key.with_egress_key(&format!(
                    "{}:ipv6:{}{suffix}",
                    self.egress_key,
                    route.key()
                ));
            }
            self.forced_pool_key = Some(
                key.with_routing(CodexWebSocketRouting {
                    account_id: self
                        .egress_account
                        .as_ref()
                        .map(|account| account.id().clone()),
                    profile: self.profile.snapshot(),
                    egress: self.egress_route.clone(),
                    original_egress_key: self.egress_key.clone(),
                }),
            );
        }
        Ok(())
    }

    #[doc(hidden)]
    pub(crate) async fn create_response_stream_with_prepared(
        &self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        prepared: PreparedResponseTransport,
    ) -> CodexClientResult<CodexBackendStreamingResponse> {
        let PreparedResponseTransport {
            requirement,
            route,
            metrics,
            attempt_client,
        } = prepared;
        let client = attempt_client.as_deref().unwrap_or(self);
        if let Some((runtime, account)) = client
            .egress_runtime
            .as_ref()
            .zip(client.egress_account.as_ref())
        {
            runtime.check_account(account.id())?;
        }
        if let Some((runtime, route)) = client
            .egress_runtime
            .as_ref()
            .zip(client.egress_route.as_ref())
        {
            runtime.check(route)?;
        }
        context.trace.cloned().unwrap_or_default().record(
            "transport.selected",
            serde_json::json!({
                "decision": metrics.decision.map(|decision| decision.as_str()),
                "requirement": requirement.as_str(), "waitMs": metrics.transport_decision_wait_ms,
            }),
        );
        match route {
            PreparedResponseRoute::Http => client
                .create_response_stream_http_sse(request, context)
                .await
                .map(|mut response| {
                    merge_preparation_metrics(&mut response.transport_metrics, metrics);
                    response
                }),
            PreparedResponseRoute::WebSocket(route) => {
                let PreparedWebSocketRoute {
                    request: websocket_request,
                    prepared,
                } = *route;
                let mut exchange = execute_prepared_response_create_request_stream(
                    &websocket_request,
                    prepared,
                    context
                        .trace
                        .cloned()
                        .unwrap_or_default()
                        .exchange("websocket"),
                )
                .await
                .map_err(websocket_exchange_error_to_client_error)?;
                if requirement.allows_connection_restart() {
                    match await_websocket_delivery_boundary(&mut exchange).await {
                        Ok(DeliveryBoundary::Ready) => {}
                        Ok(DeliveryBoundary::ConnectionLimitReached(failure)) => {
                            return Err(CodexClientError::WebSocket(
                                CodexWebSocketExchangeError::ConnectionLimitReached(failure),
                            ));
                        }
                        Err(error) => {
                            return Err(websocket_exchange_error_to_client_error(
                                post_send_ambiguous(error),
                            ));
                        }
                    }
                }
                Ok(CodexBackendStreamingResponse {
                    body: Box::pin(
                        exchange
                            .body
                            .map_err(post_send_ambiguous)
                            .map_err(websocket_exchange_error_to_client_error),
                    ),
                    transport: CodexBackendTransport::WebSocket,
                    websocket_connection_id: Some(exchange.websocket_connection_id),
                    turn_state: exchange.turn_state,
                    set_cookie_headers: exchange.set_cookie_headers,
                    rate_limit_headers: exchange.rate_limit_headers,
                    rate_limit_updates: Some(exchange.rate_limit_updates),
                    turn_state_update: Some(exchange.turn_state_update),
                    websocket_pool_decision: exchange.pool_decision,
                    diagnostics: exchange.diagnostics,
                    response_metadata: exchange.response_metadata,
                    transport_metrics: metrics,
                    connection_local_continuation: exchange.connection_local_continuation,
                })
            }
        }
    }

    fn websocket_pool_key(
        &self,
        request: &CodexResponsesRequest,
        context: CodexRequestContext<'_>,
        pool_account_id: Option<&str>,
        connection_profile: &str,
    ) -> Option<CodexWebSocketPoolKey> {
        if let Some(key) = &self.forced_pool_key {
            return Some(key.clone());
        }
        let account_id = pool_account_id.or(context.account_id)?;
        let conversation_id = request
            .local_conversation_id
            .as_deref()
            .or(request.previous_response_id())?;
        let mut key = CodexWebSocketPoolKey::new(&self.base_url, account_id, conversation_id)
            .with_client_scope(request.client_api_key_id.as_deref().unwrap_or_default())
            .with_egress_key(&self.egress_key)
            .with_connection_profile(connection_profile);
        if let Some(connection_id) = request.downstream_websocket_connection_id.as_deref() {
            key = key.with_downstream_connection_id(connection_id);
        }
        Some(key)
    }

    /// Client catalogs negotiate their version; background catalogs use the frozen profile.
    pub async fn fetch_models_with_context(
        &self,
        context: CodexRequestContext<'_>,
        client_version: Option<&str>,
    ) -> CodexClientResult<CodexModelCatalogSnapshot> {
        let endpoint = endpoint_url(&self.base_url, "codex/models");
        let profile = self.profile.snapshot();
        let headers = self.model_request_headers(&profile, context)?;
        let response = self
            .send_profiled(&profile, false, |client| {
                client
                    .get(endpoint)
                    .query(&[(
                        "client_version",
                        client_version.unwrap_or(profile.codex_version.as_str()),
                    )])
                    .headers(headers)
            })
            .await?;
        let status = response.status();
        let diagnostics = response_meta::diagnostics(Some(status.as_u16()), response.headers());
        let set_cookie_headers = response_meta::set_cookie_headers(response.headers());
        let retry_after_seconds = retry_after_seconds(response.headers(), None);
        let etag = status
            .is_success()
            .then(|| catalog_etag(response.headers()))
            .transpose()?
            .flatten();
        let body = read_model_catalog_body(response).await?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&body).into_owned();
            return Err(CodexClientError::Upstream {
                status,
                retry_after_seconds: retry_after_seconds
                    .or_else(|| retry_after_seconds_from_body(&body)),
                body,
                client_response: None,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers,
                rate_limit_headers: Vec::new(),
                transport: CodexBackendTransport::HttpSse,
                transport_metrics: Box::default(),
                send_phase: CodexUpstreamSendPhase::AfterPayload,
            });
        }
        Ok(parse_codex_model_catalog(&body, etag.as_deref())?)
    }
}

/// 首个可投递帧前的交付边界结果。
enum DeliveryBoundary {
    /// 已越过边界，可开始向下游投递。
    Ready,
    /// 首个可投递帧是上游连接寿命限制错误。
    ConnectionLimitReached(Box<ResponsesSseFailure>),
}

async fn await_websocket_delivery_boundary(
    exchange: &mut CodexWebSocketStreamingExchange,
) -> Result<DeliveryBoundary, CodexWebSocketExchangeError> {
    let mut prelude = Vec::new();
    loop {
        match exchange.body.next().await {
            Some(Ok(frame)) if is_websocket_lifecycle_prelude(&frame) => prelude.push(frame),
            Some(Ok(frame)) => {
                let connection_limit_failure = websocket_connection_limit_failure(&frame);
                prelude.push(frame);
                let remaining =
                    std::mem::replace(&mut exchange.body, Box::pin(futures::stream::empty()));
                exchange.body =
                    Box::pin(futures::stream::iter(prelude.into_iter().map(Ok)).chain(remaining));
                return Ok(if let Some(failure) = connection_limit_failure {
                    DeliveryBoundary::ConnectionLimitReached(Box::new(failure))
                } else {
                    DeliveryBoundary::Ready
                });
            }
            Some(Err(error)) => return Err(error),
            None => {
                return Err(CodexWebSocketExchangeError::closed_before_terminal_on(
                    exchange.websocket_connection_id,
                    None,
                    None,
                    None,
                ));
            }
        }
    }
}

fn is_websocket_lifecycle_prelude(frame: &[u8]) -> bool {
    frame.starts_with(b"event: response.created\n")
        || frame.starts_with(b"event: response.in_progress\n")
}

async fn read_model_catalog_body(response: ReqwestResponse) -> CodexClientResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CODEX_MODEL_CATALOG_BYTES as u64)
    {
        return Err(CodexModelCatalogError::ResponseTooLarge.into());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let Some(next_len) = body.len().checked_add(chunk.len()) else {
            return Err(CodexModelCatalogError::ResponseTooLarge.into());
        };
        if next_len > MAX_CODEX_MODEL_CATALOG_BYTES {
            return Err(CodexModelCatalogError::ResponseTooLarge.into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn websocket_connection_profile(
    headers: &HeaderMap,
    profile: &super::profile::CodexWireProfile,
) -> String {
    let identity = ["originator", "user-agent", X_OPENAI_MEMGEN_REQUEST_HEADER]
        .map(|name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
        })
        .join("\0");
    let identity = match profile.tls_profile {
        super::profile::CodexTlsProfile::Cpr => identity,
        super::profile::CodexTlsProfile::QxCompatible => format!("qx-compatible\0{identity}"),
    };
    match profile.application_profile {
        super::profile::CodexApplicationProfile::Native => identity,
        super::profile::CodexApplicationProfile::QxCompatible => {
            // Opening headers cannot change on a reused socket. Independent threads
            // need their own opening; exact continuations still resolve the old owner.
            let session = headers
                .get("session-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let thread = headers
                .get("thread-id")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            format!("qx-session-v1\0{identity}\0{session}\0{thread}")
        }
    }
}

fn http_sse_stream(
    response: ReqwestResponse,
    rate_limit_updates: CodexRateLimitUpdates,
    trace: TraceContext,
) -> CodexBackendSseStream {
    let stream: CodexBackendSseStream =
        Box::pin(response.bytes_stream().map_err(CodexClientError::Http));
    let stream: CodexBackendSseStream =
        Box::pin(futures::stream::unfold(Some(stream), |stream| async move {
            let mut stream = stream?;
            match tokio::time::timeout(UPSTREAM_STREAM_IDLE_TIMEOUT, stream.next()).await {
                Ok(Some(chunk)) => Some((chunk, Some(stream))),
                Ok(None) => None,
                Err(_) => Some((
                    Err(CodexClientError::StreamIdleTimeout {
                        timeout: UPSTREAM_STREAM_IDLE_TIMEOUT,
                    }),
                    None,
                )),
            }
        }));
    let stream = Box::pin(async_stream::stream! {
        let mut stream = stream;
        let mut capture = StreamCapture::new(trace.clone(), StreamFormat::Sse);
        while let Some(chunk) = stream.next().await {
            match &chunk {
                Ok(bytes) => capture.push(bytes),
                Err(error) => trace.record("upstream.read.failed", diagnostic_json(&serde_json::json!({"error": error.to_string()}))),
            }
            let failed = chunk.is_err();
            yield chunk;
            if failed { return; }
        }
        capture.finish();
    });
    observe_http_sse_rate_limits(stream, rate_limit_updates)
}

fn observe_http_sse_rate_limits(
    stream: CodexBackendSseStream,
    updates: CodexRateLimitUpdates,
) -> CodexBackendSseStream {
    Box::pin(futures::stream::unfold(
        (stream, SseEventDecoder::default(), updates),
        |(mut stream, mut decoder, updates)| async move {
            match stream.next().await {
                Some(chunk) => {
                    if let Ok(bytes) = &chunk {
                        append_http_sse_rate_limit_updates(decoder.push_frames(bytes), &updates)
                            .await;
                    }
                    Some((chunk, (stream, decoder, updates)))
                }
                None => {
                    append_http_sse_rate_limit_updates(decoder.finish_frames(), &updates).await;
                    None
                }
            }
        },
    ))
}

async fn append_http_sse_rate_limit_updates(
    frames: Vec<SseFrame>,
    updates: &CodexRateLimitUpdates,
) {
    let mut observations = Vec::new();
    for frame in frames {
        for event in frame.events() {
            if event
                .event
                .as_deref()
                .is_some_and(|event| event != "codex.rate_limits")
            {
                continue;
            }
            let Some(rate_limits) = events::parse_rate_limits_event_raw(&event.data) else {
                continue;
            };
            observations.push(rate_limits);
        }
    }
    if !observations.is_empty() {
        updates.lock().await.extend(observations);
    }
}
