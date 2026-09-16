//! Explicit, non-streaming Responses compaction.

use std::{borrow::Cow, net::SocketAddr};

use axum::{
    body::Bytes,
    extract::{Extension, State, connect_info::ConnectInfo},
    http::HeaderMap,
    response::Response,
};
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    operation::{CompactRequest, RawJsonPayload},
    routing::PublicModelId,
};
use serde_json::{Map, Value};

use crate::{
    ApiState,
    openai::{
        auth::{authenticate_client, client_access_error_response},
        endpoint::collect_raw_json_response,
        error::gateway_error_response,
        responses::{OpenAiRequestHeaders, decompress_request_body, request_client_context},
    },
};

/// Register as `post(compact::compact_response)` at `/v1/responses/compact`.
pub(crate) async fn compact_response(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let service = state.openai();
    let client = match authenticate_client(service, &headers) {
        Ok(client) => client,
        Err(error) => return client_access_error_response(error),
    };
    let (model, request) = match decode_compact(body, &headers) {
        Ok(decoded) => decoded,
        Err(error) => return gateway_error_response(&error),
    };
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let started = match service
        .start_compact(client, model, request, client_ip, user_agent)
        .await
    {
        Ok(started) => started,
        Err(error) => return gateway_error_response(&error),
    };
    collect_raw_json_response(started).await
}

fn decode_compact(
    body: Bytes,
    headers: &HeaderMap,
) -> Result<(PublicModelId, CompactRequest), GatewayError> {
    let body = match decompress_request_body(&body, headers).map_err(|_| {
        invalid("compact request content encoding is invalid, unsupported or too large")
    })? {
        Cow::Borrowed(_) => body,
        Cow::Owned(decoded) => Bytes::from(decoded),
    };
    let object: Map<String, Value> =
        serde_json::from_slice(&body).map_err(|_| invalid("compact requires a JSON object"))?;
    // The raw JSON path cannot pin a connection-local response owner. Refuse all
    // non-null IDs rather than silently treating an unresolved ID as cross-account.
    if object
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(invalid(
            "previous_response_id is not supported for compact; send the complete input history",
        )
        .with_client_code("compact_previous_response_id_unsupported"));
    }
    for (field, message) in [
        (
            "stream",
            "stream is not supported for the non-streaming compact endpoint",
        ),
        (
            "use_websocket",
            "use_websocket is not supported for the compact HTTP endpoint",
        ),
        (
            "background",
            "background is not supported for the compact endpoint",
        ),
    ] {
        if object.contains_key(field) {
            return Err(invalid(message));
        }
    }
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .ok_or_else(|| invalid("compact requires a non-empty model"))?;
    let model = PublicModelId::from_client_wire(model.to_owned())
        .map_err(|_| invalid("compact model is invalid"))?;
    if !matches!(
        object.get("input"),
        Some(Value::String(_) | Value::Array(_))
    ) {
        return Err(invalid("compact requires input as a string or an array"));
    }
    if object
        .get("instructions")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(invalid("compact instructions must be a string or null"));
    }
    let context = OpenAiRequestHeaders::from_headers(headers).session_context();
    let payload = RawJsonPayload::new("openai", body)
        .map_err(|_| invalid("invalid compact protocol"))?
        .with_context(context);
    Ok((model, CompactRequest::from_raw_json(payload)))
}

fn invalid(message: &'static str) -> GatewayError {
    GatewayError::new(GatewayErrorKind::InvalidRequest, message)
}
