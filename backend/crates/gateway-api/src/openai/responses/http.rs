//! OpenAI Responses HTTP 与 SSE adapter。

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::{
    body::{Body, Bytes},
    extract::{Extension, State, connect_info::ConnectInfo},
    http::{
        HeaderMap, HeaderName, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONTENT_TYPE, USER_AGENT},
    },
    response::{IntoResponse, Response},
};
use futures::{StreamExt, stream};
use gateway_core::diagnostics::TraceContext;
use gateway_core::engine::execution::{ClientTransport, ExecutionSession, StartedExecution};
use gateway_core::engine::{CommitRequirement, EngineError};
use gateway_core::error::{GatewayError, GatewayErrorKind};
use gateway_core::event::{ProviderEvent, ProviderResponseHeader};
use gateway_core::lifecycle::ConnectionGuard;
use gateway_protocol::openai::sse::DONE_SSE_FRAME;

use crate::ApiState;
use crate::openai::{
    auth::{authenticate_client, client_access_error_response},
    encoding::{HttpEncodeError, HttpResponseEncoder, HttpResponseFormat},
    error::{
        engine_error_response, gateway_error_from_engine, gateway_error_response,
        protocol_error_response, runtime_unavailable_response,
    },
};

use super::{ResponseEncodeError, request::decode_request_with_headers};

const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

/// `POST /v1/responses`。
pub(crate) async fn responses(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let service = state.openai();
    let client = match authenticate_client(service, &headers) {
        Ok(client) => client,
        Err(error) => return client_access_error_response(error),
    };
    let decoded = match decode_request_with_headers(&body, &headers) {
        Ok(decoded) => decoded,
        Err(error) => {
            return protocol_error_response(StatusCode::BAD_REQUEST, error.protocol_body());
        }
    };
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let decoded = decoded.with_client_context(client_ip, user_agent);
    let streaming = decoded.metadata().stream();
    let connection_guard = if streaming {
        match service.try_register_connection() {
            Ok(guard) => Some(guard),
            Err(_) => return runtime_unavailable_response().into_response(),
        }
    } else {
        None
    };
    let started = match service
        .start_response(
            client,
            decoded,
            if streaming {
                ClientTransport::HttpSse
            } else {
                ClientTransport::HttpJson
            },
            "/v1/responses",
        )
        .await
    {
        Ok(started) => started,
        Err(error) => return gateway_error_response(&error),
    };
    let trace = started.session.trace();
    trace.headers("client.request", serde_json::json!({
        "ingressRequestId": ingress_id.as_ref().and_then(|Extension(id)| id.header_value().to_str().ok()),
    }), headers.iter().map(|(name, value)| (name.as_str(), value.as_bytes())));
    trace.capture("client.request.body", &body);
    let request_id = started.request_id;
    let StartedExecution {
        stream, session, ..
    } = started;
    let response = if stream {
        stream_execution_response(session, connection_guard).await
    } else {
        drop(connection_guard);
        collect_execution_response(session).await
    };
    crate::openai::with_model_request_id(response, &request_id)
}

/// 从 socket 与标准转发头提取旧 Usage 页面使用的诊断事实。
pub(in crate::openai) fn request_client_context(
    headers: &HeaderMap,
    peer_address: Option<SocketAddr>,
) -> (Option<IpAddr>, Option<String>) {
    let client_ip = ["cf-connecting-ip", "x-real-ip"]
        .into_iter()
        .find_map(|name| header_ip(headers, name))
        .or_else(|| forwarded_client_ip(headers))
        .or_else(|| peer_address.map(|address| address.ip()));
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    (client_ip, user_agent)
}

fn header_ip(headers: &HeaderMap, name: &str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .and_then(|value| value.parse().ok())
}

fn forwarded_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let addresses = headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .split(',')
        .filter_map(|value| value.trim().parse::<IpAddr>().ok())
        .collect::<Vec<_>>();
    addresses
        .iter()
        .copied()
        .find(|address| !is_private_or_loopback(*address))
        .or_else(|| addresses.first().copied())
}

const fn is_private_or_loopback(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_private() || address.is_loopback(),
        IpAddr::V6(address) => address.is_unique_local() || address.is_loopback(),
    }
}

/// 编码完整 canonical event 集合，并在完整 JSON 成功后提交下游。
pub async fn collect_execution_response(session: Box<dyn ExecutionSession>) -> Response {
    collect_execution_response_as(session, HttpResponseFormat::Responses).await
}

pub(in crate::openai) async fn collect_execution_response_as(
    session: Box<dyn ExecutionSession>,
    format: HttpResponseFormat,
) -> Response {
    let mut execution = PendingExecution::new(session);
    let Some(session) = execution.session_mut() else {
        return internal_gateway_response("gateway response session is unavailable");
    };
    let events = match session.collect_uncommitted().await {
        Ok(events) => events,
        Err(error) => {
            let response_headers = session.response_headers().to_vec();
            let response = engine_error_response_with_headers_as(&error, &response_headers, format);
            return execution.record_response_status(response).await;
        }
    };

    let encoded = match encode_collected_events(&events, format) {
        Ok(encoded) => encoded,
        Err(error) => {
            let response = encoding_error_response(&error, format);
            let response = execution.record_response_status(response).await;
            execution.fail_delivery(error.gateway_error()).await;
            return response;
        }
    };
    let response_headers = session.response_headers().to_vec();
    session.trace().record(
        "downstream.encoded",
        serde_json::json!({"transport": "http_json", "bytes": encoded.len()}),
    );
    session.trace().dump("downstream.body", &encoded);
    let response = json_body_response(encoded, &response_headers);

    let Some(session) = execution.session_mut() else {
        return internal_gateway_response("gateway response session is unavailable");
    };
    if let Err(error) = session
        .commit_downstream(Some(StatusCode::OK.as_u16()))
        .await
    {
        let response = engine_error_response_with_headers_as(&error, &response_headers, format);
        let response = execution.record_response_status(response).await;
        execution
            .fail_delivery(gateway_error_from_engine(&error))
            .await;
        return response;
    }
    if !session.is_finalized() {
        let error = GatewayError::new(
            GatewayErrorKind::Internal,
            "gateway response was not finalized after commit",
        );
        let response = execution
            .record_response_status(gateway_error_response(&error))
            .await;
        execution.fail_delivery(error).await;
        return response;
    }
    execution.disarm();
    response
}

fn encode_collected_events(
    events: &[ProviderEvent],
    format: HttpResponseFormat,
) -> Result<Vec<u8>, HttpEncodeError> {
    let response = HttpResponseEncoder::new(format).finish_collected(events)?;
    serde_json::to_vec(&response)
        .map_err(|_| HttpEncodeError::Responses(ResponseEncodeError::Serialization))
}

fn json_body_response(encoded: Vec<u8>, response_headers: &[ProviderResponseHeader]) -> Response {
    let mut response = Response::new(Body::from(encoded));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    apply_response_headers(response, response_headers)
}

fn apply_response_headers(
    mut response: Response,
    response_headers: &[ProviderResponseHeader],
) -> Response {
    let connection_options = super::response_connection_options(response_headers);
    for header in response_headers {
        if !super::response_header_is_forwardable(header.name(), &connection_options) {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(header.name().as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_bytes(header.value()) else {
            continue;
        };
        response.headers_mut().append(name, value);
    }
    response
}

fn engine_error_response_with_headers_as(
    error: &EngineError,
    response_headers: &[ProviderResponseHeader],
    format: HttpResponseFormat,
) -> Response {
    let response = match format {
        HttpResponseFormat::Responses => engine_error_response(error),
        HttpResponseFormat::Chat { .. } => {
            let mut response = gateway_error_response(&gateway_error_from_engine(error));
            if let EngineError::Provider(error) = error
                && let Some(upstream) = error.client_visible_upstream_response()
            {
                if let Ok(status) = StatusCode::from_u16(upstream.status())
                    && (status.is_client_error() || status.is_server_error())
                {
                    *response.status_mut() = status;
                }
                response = apply_response_headers(response, upstream.headers());
            } else if let EngineError::Provider(error) = error
                && let Some(request_id) = error.upstream_request_id()
                && let Ok(value) = HeaderValue::from_str(request_id.as_str())
            {
                response.headers_mut().insert("x-request-id", value);
            }
            response
        }
    };
    let failure_request_ids: Vec<_> = ["x-request-id", "x-oai-request-id"]
        .into_iter()
        .flat_map(|name| {
            response
                .headers()
                .get_all(name)
                .iter()
                .cloned()
                .map(move |value| (name, value))
        })
        .collect();
    // 失败仍保留 turn state，但 opening ID 不能代替当前失败的请求 ID。
    let mut response = apply_response_headers(response, response_headers);
    response.headers_mut().remove("x-request-id");
    response.headers_mut().remove("x-oai-request-id");
    for (name, value) in failure_request_ids {
        response.headers_mut().append(name, value);
    }
    response
}

fn encoding_error_response(error: &HttpEncodeError, format: HttpResponseFormat) -> Response {
    protocol_error_response(
        match format {
            HttpResponseFormat::Responses => StatusCode::INTERNAL_SERVER_ERROR,
            HttpResponseFormat::Chat { .. } => StatusCode::BAD_GATEWAY,
        },
        error.protocol_body(),
    )
}

/// 编码首个 SSE frame 后提交下游，再持续驱动同一执行会话。
pub async fn stream_execution_response(
    session: Box<dyn ExecutionSession>,
    connection_guard: Option<Box<dyn ConnectionGuard>>,
) -> Response {
    stream_execution_response_as(session, connection_guard, HttpResponseFormat::Responses).await
}

pub(in crate::openai) async fn stream_execution_response_as(
    session: Box<dyn ExecutionSession>,
    connection_guard: Option<Box<dyn ConnectionGuard>>,
    format: HttpResponseFormat,
) -> Response {
    let mut execution = PendingExecution::new(session);
    let mut encoder = HttpResponseEncoder::new(format);
    let (frames, terminal_frames) = loop {
        let Some(session) = execution.session_mut() else {
            return internal_gateway_response("gateway response session is unavailable");
        };
        let first = match session.next_event().await {
            Ok(Some(event)) => event,
            Ok(None) => {
                let error = GatewayError::new(
                    GatewayErrorKind::Internal,
                    "gateway response ended before its first event",
                );
                let response = gateway_error_response(&error);
                let response = execution.record_response_status(response).await;
                execution.fail_delivery(error).await;
                return response;
            }
            Err(error) => {
                let response_headers = session.response_headers().to_vec();
                let response =
                    engine_error_response_with_headers_as(&error, &response_headers, format);
                return execution.record_response_status(response).await;
            }
        };
        let first_requirement = first.commit_requirement();
        let first_events = first.into_provider_events();
        if first_requirement != CommitRequirement::CommitBeforeDelivery {
            let error = GatewayError::new(
                GatewayErrorKind::Internal,
                "gateway first event did not require commit",
            );
            let response = gateway_error_response(&error);
            let response = execution.record_response_status(response).await;
            execution.fail_delivery(error).await;
            return response;
        }
        let mut frames = Vec::new();
        let mut terminal_frames = Vec::new();
        for event in &first_events {
            match encoder.push_sse(event) {
                // A Chat finish/usage frame is deliverable only after the
                // execution confirms success, including in the first batch.
                Ok(encoded) if encoder.is_chat() && encoder.is_completed() => {
                    terminal_frames.extend(encoded);
                }
                Ok(encoded) => frames.extend(encoded),
                Err(error) => {
                    let response = encoding_error_response(&error, format);
                    let response = execution.record_response_status(response).await;
                    execution.fail_delivery(error.gateway_error()).await;
                    return response;
                }
            }
        }
        if !frames.is_empty() || !terminal_frames.is_empty() {
            break (frames, terminal_frames);
        }
        // Chat has no wire representation for transport comments/metadata.
        // Consume those without committing a successful downstream response.
        if matches!(format, HttpResponseFormat::Chat { .. }) && !encoder.is_completed() {
            let Some(session) = execution.session_mut() else {
                return internal_gateway_response("gateway response session is unavailable");
            };
            if let Err(error) = session.defer_downstream_commit() {
                let response_headers = session.response_headers().to_vec();
                let response =
                    engine_error_response_with_headers_as(&error, &response_headers, format);
                let response = execution.record_response_status(response).await;
                execution
                    .fail_delivery(gateway_error_from_engine(&error))
                    .await;
                return response;
            }
            continue;
        }
        let error = GatewayError::new(
            GatewayErrorKind::Internal,
            "gateway commit batch encoded no output",
        );
        let response = gateway_error_response(&error);
        let response = execution.record_response_status(response).await;
        execution.fail_delivery(error).await;
        return response;
    };
    let Some(session) = execution.session_mut() else {
        return internal_gateway_response("gateway response session is unavailable");
    };
    let response_headers = session.response_headers().to_vec();
    if let Err(error) = session
        .commit_downstream(Some(StatusCode::OK.as_u16()))
        .await
    {
        let response = engine_error_response_with_headers_as(&error, &response_headers, format);
        let response = execution.record_response_status(response).await;
        execution
            .fail_delivery(gateway_error_from_engine(&error))
            .await;
        return response;
    }
    let Some(session) = execution.into_session() else {
        return internal_gateway_response("gateway response session is unavailable");
    };
    let mut state = ResponsesStreamState::new(session, encoder, frames, connection_guard);
    if state.encoder.is_completed() {
        state.finish_completed(terminal_frames).await;
    }
    let output = Box::pin(stream::unfold(state, |mut state| async move {
        loop {
            if let Some(chunk) = state.pending.pop_front() {
                state.trace.dump("downstream.chunk", &chunk);
                state.handed_off_bytes += chunk.len() as u64;
                return Some((Ok::<Bytes, Infallible>(chunk), state));
            }
            if state.output_finished {
                return None;
            }
            state.advance().await;
        }
    }));
    // 保活只轮询已固定的输出 stream；不能取消并重建 advance/next_event future，
    // 否则一次心跳就可能丢失正在等待的上游事件或执行终态清理。
    let body = Body::from_stream(stream::unfold(output, |mut output| async move {
        let chunk = tokio::select! {
            biased;
            chunk = output.next() => chunk?,
            () = tokio::time::sleep(SSE_KEEPALIVE_INTERVAL) => {
                Ok(Bytes::from_static(b": keep-alive\n\n"))
            }
        };
        Some((chunk, output))
    }));
    event_stream_response(body, &response_headers)
}

fn event_stream_response(body: Body, response_headers: &[ProviderResponseHeader]) -> Response {
    let mut response = apply_response_headers(Response::new(body), response_headers);
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-transform"),
    );
    response
        .headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

fn internal_gateway_response(message: &'static str) -> Response {
    gateway_error_response(&GatewayError::new(GatewayErrorKind::Internal, message))
}

pub(in crate::openai) struct PendingExecution {
    session: Option<Box<dyn ExecutionSession>>,
}

impl PendingExecution {
    pub(in crate::openai) fn new(session: Box<dyn ExecutionSession>) -> Self {
        Self {
            session: Some(session),
        }
    }

    pub(in crate::openai) fn session_mut(
        &mut self,
    ) -> Option<&mut (dyn ExecutionSession + 'static)> {
        self.session.as_deref_mut()
    }

    pub(in crate::openai) async fn fail_delivery(&mut self, error: GatewayError) {
        if let Some(session) = self.session.as_mut() {
            let _ = session.fail_delivery(error).await;
        }
    }

    pub(in crate::openai) async fn record_response_status(
        &mut self,
        response: Response,
    ) -> Response {
        if let Some(session) = self.session.as_mut() {
            let _ = session
                .record_client_status(response.status().as_u16())
                .await;
        }
        response
    }

    pub(in crate::openai) fn disarm(&mut self) {
        self.session = None;
    }

    fn into_session(mut self) -> Option<Box<dyn ExecutionSession>> {
        self.session.take()
    }
}

impl Drop for PendingExecution {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        if session.is_finalized() {
            return;
        }
        session.trace().record(
            "downstream.cancelled",
            serde_json::json!({"reason": "response_guard_dropped"}),
        );
        session.cancel();
        detach_finalize(session);
    }
}

struct ResponsesStreamState {
    trace: TraceContext,
    handed_off_bytes: u64,
    session: Option<Box<dyn ExecutionSession>>,
    encoder: HttpResponseEncoder,
    pending: VecDeque<Bytes>,
    output_finished: bool,
    execution_terminal: bool,
    _connection_guard: Option<Box<dyn ConnectionGuard>>,
}

impl ResponsesStreamState {
    fn new(
        session: Box<dyn ExecutionSession>,
        encoder: HttpResponseEncoder,
        initial_frames: Vec<Bytes>,
        connection_guard: Option<Box<dyn ConnectionGuard>>,
    ) -> Self {
        Self {
            trace: session.trace(),
            handed_off_bytes: 0,
            session: Some(session),
            encoder,
            pending: initial_frames.into_iter().collect(),
            output_finished: false,
            execution_terminal: false,
            _connection_guard: connection_guard,
        }
    }

    async fn advance(&mut self) {
        let next = match self.session.as_mut() {
            Some(session) => session.next_event().await,
            None => {
                self.finish_with_gateway_error(GatewayError::new(
                    GatewayErrorKind::Internal,
                    "gateway response session is unavailable",
                ));
                return;
            }
        };
        match next {
            Ok(Some(event)) => {
                let requirement = event.commit_requirement();
                let events = event.into_provider_events();
                if requirement != CommitRequirement::AlreadyCommitted {
                    let error = GatewayError::new(
                        GatewayErrorKind::Internal,
                        "gateway requested another downstream commit",
                    );
                    self.fail_delivery(error.clone()).await;
                    self.finish_with_gateway_error(error);
                } else {
                    for event in events {
                        self.push_event(event).await;
                        if self.output_finished {
                            break;
                        }
                    }
                }
            }
            Ok(None) => {
                self.execution_terminal = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_finalized());
                if self.execution_terminal
                    && (!self.encoder.is_chat() || self.encoder.is_completed())
                {
                    self.pending
                        .push_back(Bytes::from_static(DONE_SSE_FRAME.as_bytes()));
                    self.output_finished = true;
                } else {
                    let error = GatewayError::new(
                        GatewayErrorKind::Internal,
                        "gateway response ended without a complete response and finalized execution",
                    );
                    self.fail_delivery(error.clone()).await;
                    self.finish_with_gateway_error(error);
                }
            }
            Err(error) => {
                self.execution_terminal = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_finalized());
                if self.encoder.has_wire_failure() {
                    self.pending
                        .push_back(Bytes::from_static(DONE_SSE_FRAME.as_bytes()));
                    self.output_finished = true;
                } else {
                    self.finish_with_gateway_error(gateway_error_from_engine(&error));
                }
            }
        }
    }

    async fn push_event(&mut self, event: ProviderEvent) {
        let frames = match self.encoder.push_sse(&event) {
            Ok(frames) => frames,
            Err(error) => {
                let error = error.gateway_error();
                self.fail_delivery(error.clone()).await;
                self.finish_with_gateway_error(error);
                return;
            }
        };
        if self.encoder.is_completed() {
            self.finish_completed(frames).await;
        } else {
            self.pending.extend(frames);
        }
    }

    async fn finish_completed(&mut self, frames: Vec<Bytes>) {
        let next = match self.session.as_mut() {
            Some(session) => session.next_event().await,
            None => {
                self.finish_with_gateway_error(GatewayError::new(
                    GatewayErrorKind::Internal,
                    "gateway response session is unavailable",
                ));
                return;
            }
        };
        match next {
            Ok(None)
                if self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_finalized()) =>
            {
                self.pending.extend(frames);
                self.execution_terminal = true;
                self.pending
                    .push_back(Bytes::from_static(DONE_SSE_FRAME.as_bytes()));
                self.output_finished = true;
            }
            Ok(None) => {
                let error = GatewayError::new(
                    GatewayErrorKind::Internal,
                    "gateway response was not finalized after its terminal event",
                );
                self.fail_delivery(error.clone()).await;
                self.finish_with_gateway_error(error);
            }
            Ok(Some(_)) => {
                let error = GatewayError::new(
                    GatewayErrorKind::Internal,
                    "gateway response continued after its terminal event",
                );
                self.fail_delivery(error.clone()).await;
                self.finish_with_gateway_error(error);
            }
            Err(error) => {
                self.execution_terminal = self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.is_finalized());
                self.finish_with_gateway_error(gateway_error_from_engine(&error));
            }
        }
    }

    async fn fail_delivery(&mut self, error: GatewayError) {
        if let Some(session) = self.session.as_mut() {
            let _ = session.fail_delivery(error).await;
            self.execution_terminal = session.is_finalized();
        }
    }

    fn finish_with_gateway_error(&mut self, error: GatewayError) {
        self.pending.push_back(self.encoder.error_frame(&error));
        self.pending
            .push_back(Bytes::from_static(DONE_SSE_FRAME.as_bytes()));
        self.output_finished = true;
    }
}

impl Drop for ResponsesStreamState {
    fn drop(&mut self) {
        self.trace.record("downstream.body.closed", serde_json::json!({
            "handedOffBytes": self.handed_off_bytes,
            "pendingChunks": self.pending.len(),
            "outputFinished": self.output_finished, "executionTerminal": self.execution_terminal,
        }));
        if self.execution_terminal {
            return;
        }
        let Some(session) = self.session.take() else {
            return;
        };
        session.cancel();
        // Body drop 后由 Core 继续状态机清理，HTTP body 生命周期不等待它。
        detach_finalize(session);
    }
}

fn detach_finalize(session: Box<dyn ExecutionSession>) {
    let finalize = session.detach_finalize();
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        drop(runtime.spawn(finalize));
    }
}
