use std::net::SocketAddr;

use axum::{
    body::Bytes,
    extract::{Extension, State, connect_info::ConnectInfo},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use gateway_core::engine::execution::ClientTransport;
use gateway_core::operation::{GenerateRequest, Operation, ProtocolPayload};
use gateway_protocol::chat_completions::{CHAT_PROTOCOL, parse_request};
use serde_json::{Map, Value, json};

use crate::ApiState;
use crate::openai::{
    auth::{authenticate_client, client_access_error_response},
    encoding::HttpResponseFormat,
    error::{gateway_error_response, protocol_error_response, runtime_unavailable_response},
    responses::{
        DecodedResponsesRequest, ProtocolError, ProtocolErrorBody, collect_execution_response_as,
        decompress_request_body_limit, request_client_context, stream_execution_response_as,
    },
};

pub(crate) async fn chat_completions(
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
    let body = match decompress_request_body_limit(
        &body,
        &headers,
        client.snapshot().responses_max_decompressed_body_bytes(),
    ) {
        Ok(body) => body,
        Err(error) => {
            return protocol_error_response(StatusCode::BAD_REQUEST, error.protocol_body());
        }
    };
    let chat = match parse_request(&body) {
        Ok(body) => body,
        Err(error) => {
            return protocol_error_response(
                StatusCode::BAD_REQUEST,
                ProtocolErrorBody {
                    error: ProtocolError {
                        kind: "invalid_request_error",
                        code: error.code(),
                        message: error.to_string(),
                        param: error.parameter().map(str::to_owned),
                    },
                },
            );
        }
    };
    let requested_model = chat["model"].as_str().unwrap_or_default().to_owned();
    let streaming = chat.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut context = Map::new();
    let passthrough: Vec<_> =
        crate::openai::responses::passthrough_request_headers(&headers, false)
            .map(|(name, value)| json!([name.as_str(), STANDARD.encode(value.as_bytes())]))
            .collect();
    for (key, aliases) in [
        ("session_id", &["session-id", "session_id"][..]),
        ("thread_id", &["thread-id"]),
        ("turn_metadata", &["x-codex-turn-metadata"]),
    ] {
        if let Some(value) = aliases.iter().find_map(|name| {
            passthrough
                .iter()
                .find(|pair| pair[0].as_str() == Some(*name))
                .and_then(|_| headers.get(*name)?.to_str().ok())
        }) {
            context.insert(key.into(), value.into());
        }
    }
    if !passthrough.is_empty() {
        context.insert("opaque_request_headers".into(), passthrough.into());
    }
    let payload =
        match ProtocolPayload::from_json_bytes(CHAT_PROTOCOL, Bytes::copy_from_slice(&body)) {
            Ok(payload) => payload.with_context(context),
            Err(_) => {
                return protocol_error_response(
                    StatusCode::BAD_REQUEST,
                    crate::openai::responses::RequestDecodeError::ExpectedObject.protocol_body(),
                );
            }
        };
    let decoded = DecodedResponsesRequest::from_protocol_operation(
        Operation::Generate(GenerateRequest::from_protocol_payload(payload)),
        requested_model,
        streaming,
    );
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let guard = if streaming {
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
            decoded.with_client_context(client_ip, user_agent),
            if streaming {
                ClientTransport::HttpSse
            } else {
                ClientTransport::HttpJson
            },
            "/v1/chat/completions",
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
    let format = HttpResponseFormat::Chat;
    let response = if streaming {
        stream_execution_response_as(started.session, guard, format).await
    } else {
        collect_execution_response_as(started.session, format).await
    };
    crate::openai::with_model_request_id(response, &request_id)
}
