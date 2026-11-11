//! Messages 与输入计数的 HTTP 入口和原生错误表达

use std::net::SocketAddr;

use axum::{
    body::{Body, Bytes},
    extract::{Extension, State, connect_info::ConnectInfo},
    http::{HeaderMap, HeaderValue, StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response},
};
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    operation::{GenerateRequest, Operation, ProtocolPayload, RawJsonPayload, TokenCountRequest},
};
use gateway_protocol::{anthropic, openai::sse::encode_sse_event};
use serde_json::{Value, json};

use super::{
    auth::{authenticate_messages_client, client_access_error_response},
    encoding::HttpResponseFormat,
    error::gateway_error_response,
    responses::{
        DecodedResponsesRequest, OpenAiRequestHeaders, collect_execution_response_as,
        decompress_request_body_limit, request_client_context, stream_execution_response_as,
    },
};
use crate::ApiState;

pub(super) async fn messages(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    execute(
        state,
        connect_info,
        ingress_id,
        headers,
        body,
        HttpResponseFormat::Messages,
    )
    .await
}

pub(super) async fn count_tokens(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    execute(
        state,
        connect_info,
        ingress_id,
        headers,
        body,
        HttpResponseFormat::MessageTokenCount,
    )
    .await
}

async fn execute(
    state: ApiState,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    headers: HeaderMap,
    body: Bytes,
    protocol: HttpResponseFormat,
) -> Response {
    use gateway_core::engine::execution::ClientTransport;
    let service = state.openai();
    let client = match authenticate_messages_client(service, &headers) {
        Ok(client) => client,
        Err(error) => return project_error_response(client_access_error_response(error)).await,
    };
    let maximum = client.snapshot().responses_max_decompressed_body_bytes();
    let decoded = match decode_request(&body, &headers, maximum, &protocol) {
        Ok(decoded) => decoded,
        Err(error) => return project_error_response(gateway_error_response(&error)).await,
    };
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let streaming = decoded.metadata().stream();
    let guard = if streaming {
        match service.try_register_connection() {
            Ok(guard) => Some(guard),
            Err(_) => {
                return project_error_response(
                    super::error::runtime_unavailable_response().into_response(),
                )
                .await;
            }
        }
    } else {
        None
    };
    let endpoint = if matches!(protocol, HttpResponseFormat::MessageTokenCount) {
        "/v1/messages/count_tokens"
    } else {
        "/v1/messages"
    };
    let started = match service
        .start_response(
            client,
            decoded.with_client_context(client_ip, user_agent),
            if streaming {
                ClientTransport::HttpSse
            } else {
                ClientTransport::HttpJson
            },
            endpoint,
        )
        .await
    {
        Ok(started) => started,
        Err(error) => return project_error_response(gateway_error_response(&error)).await,
    };
    let trace = started.session.trace();
    trace.headers("client.request", serde_json::json!({
        "ingressRequestId": ingress_id.as_ref().and_then(|Extension(id)| id.header_value().to_str().ok()),
    }), headers.iter().map(|(name, value)| (name.as_str(), value.as_bytes())));
    trace.capture("client.request.body", &body);
    let request_id = started.request_id;
    let response = if streaming {
        stream_execution_response_as(started.session, guard, protocol).await
    } else {
        collect_execution_response_as(started.session, protocol).await
    };
    crate::openai::with_model_request_id(project_error_response(response).await, &request_id)
}

pub(super) fn decode_request(
    body: &[u8],
    headers: &HeaderMap,
    maximum: usize,
    protocol: &HttpResponseFormat,
) -> Result<DecodedResponsesRequest, GatewayError> {
    let invalid = |message| GatewayError::new(GatewayErrorKind::InvalidRequest, message);
    let body = decompress_request_body_limit(body, headers, maximum)
        .map_err(|_| invalid("Request body could not be decoded."))?;
    let object = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| invalid("Request body must be a JSON object."))?;
    let counted = matches!(protocol, HttpResponseFormat::MessageTokenCount);
    let validation = if counted {
        anthropic::validate_count_request(&object)
    } else {
        anthropic::validate_request(&object)
    };
    validation.map_err(|error| {
        invalid(match error {
            anthropic::TranslationError::InvalidRequest(message) => message,
            _ => "Messages request is invalid.",
        })
    })?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let stream = !counted
        && object
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let context = OpenAiRequestHeaders::from_headers(headers).session_context();
    let operation = if counted {
        let payload =
            RawJsonPayload::new(anthropic::ANTHROPIC_PROTOCOL, Bytes::copy_from_slice(&body))
                .map_err(|_| invalid("Request protocol is invalid."))?
                .with_context(context);
        Operation::CountTokens(TokenCountRequest::from_raw_json(payload))
    } else {
        let payload = ProtocolPayload::json_object(anthropic::ANTHROPIC_PROTOCOL, object)
            .map_err(|_| invalid("Request protocol is invalid."))?
            .with_context(context);
        Operation::Generate(GenerateRequest::from_protocol_payload(payload))
    };
    Ok(DecodedResponsesRequest::from_protocol_operation(
        operation, model, stream,
    ))
}

fn error_type(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 | 405 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        503 | 529 => "overloaded_error",
        _ => "api_error",
    }
}

pub(super) fn error_payload(value: &Value, status: StatusCode) -> Value {
    if value.get("type").and_then(Value::as_str) == Some("error") && value.get("error").is_some() {
        return value.clone();
    }
    let error = value
        .pointer("/response/error")
        .or_else(|| value.get("error"))
        .unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("The gateway could not complete the request.");
    json!({"type":"error", "error":{"type":error_type(status), "message":message}})
}

pub(super) async fn project_error_response(response: Response) -> Response {
    if response.status().is_success() {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .unwrap_or_default();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| json!({"error":{"message":String::from_utf8_lossy(&bytes)}}));
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let projected = error_payload(&value, parts.status);
    Response::from_parts(parts, Body::from(projected.to_string()))
}

pub(super) fn stream_error(value: &Value) -> Bytes {
    Bytes::from(encode_sse_event(
        "error",
        &error_payload(value, StatusCode::INTERNAL_SERVER_ERROR).to_string(),
    ))
}
