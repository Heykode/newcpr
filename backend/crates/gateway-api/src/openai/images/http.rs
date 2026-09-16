//! Codex Images 非流式 HTTP adapter。

use std::net::SocketAddr;

use axum::{
    body::{Bytes, to_bytes},
    extract::{Extension, Request, State, connect_info::ConnectInfo},
    http::{
        HeaderMap, StatusCode,
        header::{CONTENT_ENCODING, CONTENT_TYPE},
    },
    response::Response,
};
use gateway_core::error::{GatewayError, GatewayErrorKind};
use gateway_core::operation::{ImageRequest, ImageRequestKind, Operation, RawJsonPayload};
use serde_json::Value;

use crate::ApiState;
use crate::openai::{
    auth::{authenticate_client, client_access_error_response},
    endpoint::collect_raw_json_response,
    error::{gateway_error_response, protocol_error_response},
    responses::{OpenAiRequestHeaders, RequestDecodeError, request_client_context},
};

use super::multipart::decode_edit_form;

const OPENAI_PROTOCOL: &str = "openai";
const IMAGE_TURN_ID_CONTEXT_KEY: &str = "image_turn_id";

/// `POST /v1/images/generations`。
pub(crate) async fn image_generations(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
) -> Response {
    handle_image_request(
        state,
        connect_info,
        request,
        ImageRequestKind::Generation,
        "/v1/images/generations",
    )
    .await
}

/// `POST /v1/images/edits`。
pub(crate) async fn image_edits(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
) -> Response {
    handle_image_request(
        state,
        connect_info,
        request,
        ImageRequestKind::Edit,
        "/v1/images/edits",
    )
    .await
}

async fn handle_image_request(
    state: ApiState,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    request: Request,
    kind: ImageRequestKind,
    endpoint: &'static str,
) -> Response {
    let service = state.openai();
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    let client = match authenticate_client(service, &headers) {
        Ok(client) => client,
        Err(error) => return client_access_error_response(error),
    };
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let multipart = content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("multipart/form-data"))
    });
    let body = if kind == ImageRequestKind::Edit && multipart {
        if headers.get(CONTENT_ENCODING).is_some_and(|value| {
            value
                .to_str()
                .map_or(true, |value| !value.trim().eq_ignore_ascii_case("identity"))
        }) {
            return gateway_error_response(&GatewayError::new(
                GatewayErrorKind::Unsupported,
                "compressed multipart image uploads are not supported",
            ));
        }
        match decode_edit_form(body, content_type.unwrap_or_default()).await {
            Ok(body) => body,
            Err(error) => return protocol_error_response(error.status, error.body),
        }
    } else {
        match to_bytes(body, usize::MAX).await {
            Ok(body) => body,
            Err(_) => {
                return protocol_error_response(
                    StatusCode::BAD_REQUEST,
                    RequestDecodeError::MalformedJson.protocol_body(),
                );
            }
        }
    };
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let operation = match image_operation(body, &headers, kind) {
        Ok(operation) => operation,
        Err(error) => return gateway_error_response(&error),
    };
    let started = match service
        .start_provider_endpoint(client, operation, client_ip, user_agent, endpoint)
        .await
    {
        Ok(started) => started,
        Err(error) => return gateway_error_response(&error),
    };
    collect_raw_json_response(started).await
}

fn image_operation(
    body: Bytes,
    headers: &HeaderMap,
    kind: ImageRequestKind,
) -> Result<Operation, GatewayError> {
    let mut context = OpenAiRequestHeaders::from_headers(headers).session_context();
    if let Some(turn_id) = headers
        .get("x-codex-image-turn-id")
        .and_then(|value| value.to_str().ok())
    {
        context.insert(
            IMAGE_TURN_ID_CONTEXT_KEY.to_owned(),
            Value::String(turn_id.to_owned()),
        );
    }
    let payload = RawJsonPayload::new(OPENAI_PROTOCOL, body)
        .map_err(|_| {
            GatewayError::new(
                GatewayErrorKind::Internal,
                "OpenAI protocol identifier is invalid",
            )
        })?
        .with_context(context);
    Ok(Operation::GenerateImage(ImageRequest::from_raw_json(
        kind, payload,
    )))
}
