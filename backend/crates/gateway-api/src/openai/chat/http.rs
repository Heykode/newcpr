use std::net::SocketAddr;

use axum::{
    body::Bytes,
    extract::{Extension, State, connect_info::ConnectInfo},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use gateway_core::engine::execution::ClientTransport;
use gateway_protocol::openai::chat::decode_chat_request;
use serde_json::Value;

use crate::ApiState;
use crate::openai::{
    auth::{authenticate_client, client_access_error_response},
    encoding::HttpResponseFormat,
    error::{gateway_error_response, protocol_error_response, runtime_unavailable_response},
    responses::{
        ProtocolError, ProtocolErrorBody, RequestDecodeError, collect_execution_response_as,
        decode_object_with_headers, decompress_request_body_limit, request_client_context,
        stream_execution_response_as,
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
    let object = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            return protocol_error_response(
                StatusCode::BAD_REQUEST,
                RequestDecodeError::MalformedJson.protocol_body(),
            );
        }
    };
    let chat = match decode_chat_request(object) {
        Ok(chat) => chat,
        Err(error) => {
            return protocol_error_response(
                StatusCode::BAD_REQUEST,
                ProtocolErrorBody {
                    error: ProtocolError {
                        kind: "invalid_request_error",
                        code: "invalid_chat_request",
                        message: error.to_string(),
                        param: error.param().map(str::to_owned),
                    },
                },
            );
        }
    };
    let format = HttpResponseFormat::Chat {
        include_usage: chat.include_usage,
    };
    let streaming = chat.stream;
    let decoded = match decode_object_with_headers(chat.responses, &headers) {
        Ok(decoded) => decoded,
        Err(error) => {
            return protocol_error_response(StatusCode::BAD_REQUEST, error.protocol_body());
        }
    };
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
    let response = if streaming {
        stream_execution_response_as(started.session, guard, format).await
    } else {
        collect_execution_response_as(started.session, format).await
    };
    crate::openai::with_model_request_id(response, &request_id)
}
