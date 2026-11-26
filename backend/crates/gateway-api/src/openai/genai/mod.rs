//! GenAI 原生接口沿本地统一执行和交付链路处理。
mod error;
mod models;
use super::{
    auth::{authenticate_client, client_access_error_response},
    encoding::HttpResponseFormat,
    error::gateway_error_response,
    responses::{
        DecodedResponsesRequest, collect_execution_response_as, decompress_request_body_limit,
        request_client_context, stream_execution_response_as,
    },
};
use crate::ApiState;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, Extension, Path, RawQuery, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub(in crate::openai) use error::error_frame;
pub(crate) use error::project_error_response;
use gateway_core::{
    error::{GatewayError, GatewayErrorKind},
    operation::{GenerateRequest, Operation, ProtocolPayload, RawJsonPayload, TokenCountRequest},
};
use gateway_protocol::genai::GENAI_PROTOCOL;
use serde_json::{Map, Value, json};
use std::net::SocketAddr;
struct Route {
    endpoint: HttpResponseFormat,
    model: String,
    stream: bool,
}
pub(super) const MODEL_ACTION_PATH: &str = "/v1beta/models/{resource}";
pub(crate) fn router() -> Router<ApiState> {
    Router::new()
        .route(models::MODELS_PATH, get(models::list))
        .route(MODEL_ACTION_PATH, post(generate).get(models::detail))
        .route("/v1beta:{method}", post(metric))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(axum::middleware::map_response(project_error_response))
}
fn parse_route(resource: &str, query: Option<&str>) -> Result<Route, GatewayError> {
    let invalid = |message| GatewayError::new(GatewayErrorKind::InvalidRequest, message);
    let (model, method) = resource
        .split_once(':')
        .ok_or_else(|| invalid("A model action is required."))?;
    if !valid_segment(model) {
        return Err(invalid("Invalid model name."));
    }
    let (endpoint, stream) = match method {
        "generateContent" => (HttpResponseFormat::GenAi, false),
        "streamGenerateContent" => (HttpResponseFormat::GenAi, true),
        "countTokens" => (HttpResponseFormat::GenAiTokenCount, false),
        _ => {
            return Err(GatewayError::new(
                GatewayErrorKind::ModelNotFound,
                "Unknown model action.",
            ));
        }
    };
    let mut formats = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .filter_map(|(key, value)| (key == "alt").then_some(value));
    let format = formats.next();
    if formats.next().is_some()
        || (stream && format.as_deref() != Some("sse"))
        || (!stream && format.is_some_and(|value| value != "json"))
    {
        return Err(invalid(
            "Invalid response format for the requested model action.",
        ));
    }
    Ok(Route {
        endpoint,
        model: model.to_owned(),
        stream,
    })
}

pub(super) fn decode(
    model: &str,
    stream: bool,
    endpoint: HttpResponseFormat,
    body: Bytes,
    headers: &HeaderMap,
) -> Result<DecodedResponsesRequest, GatewayError> {
    let mut context = Map::new();
    context.insert("stream".into(), Value::Bool(stream));
    let mut attribution = Map::new();
    for key in [
        "user-agent",
        "x-goog-api-client",
        "x-http-session-id",
        "x-session-id",
    ] {
        let connection_option = headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|value| key.eq_ignore_ascii_case(value.trim()));
        if !connection_option && let Some(value) = headers.get(key).and_then(|v| v.to_str().ok()) {
            attribution.insert(key.into(), json!(value));
        }
    }
    context.insert("headers".into(), Value::Object(attribution));
    let passthrough: Vec<Value> =
        crate::openai::responses::passthrough_request_headers(headers, false)
            .map(|(name, value)| json!([name.as_str(), STANDARD.encode(value.as_bytes())]))
            .collect();
    if !passthrough.is_empty() {
        context.insert("opaque_request_headers".into(), Value::Array(passthrough));
    }
    let payload = ProtocolPayload::from_json_bytes(GENAI_PROTOCOL, body.clone())
        .map_err(|_| {
            GatewayError::new(
                GatewayErrorKind::InvalidRequest,
                "Request body must be a JSON object.",
            )
        })?
        .with_context(context);
    let operation = if matches!(endpoint, HttpResponseFormat::GenAiTokenCount) {
        Operation::CountTokens(
            TokenCountRequest::from_raw_json(
                RawJsonPayload::new(GENAI_PROTOCOL, body)
                    .map_err(|_| {
                        GatewayError::new(
                            GatewayErrorKind::InvalidRequest,
                            "Invalid request protocol.",
                        )
                    })?
                    .with_context(payload.context().clone()),
            )
            .requiring_native_protocol(),
        )
    } else {
        Operation::Generate(
            GenerateRequest::from_protocol_payload(payload).requiring_native_protocol(),
        )
    };
    Ok(DecodedResponsesRequest::from_protocol_operation(
        operation,
        model.to_owned(),
        stream,
    ))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-@".contains(&byte))
}

async fn generate(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    ingress_id: Option<Extension<tower_http::request_id::RequestId>>,
    Path(resource): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    use gateway_core::engine::execution::ClientTransport;
    let service = state.openai();
    let client = match authenticate_client(service, &headers) {
        Ok(client) => client,
        Err(error) => return client_access_error_response(error),
    };
    let route = match parse_route(&resource, query.as_deref()) {
        Ok(route) => route,
        Err(error) => return gateway_error_response(&error),
    };
    let decoded_body = match decompress_request_body_limit(
        &body,
        &headers,
        client.snapshot().responses_max_decompressed_body_bytes(),
    ) {
        Ok(body) => Bytes::copy_from_slice(&body),
        Err(error) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(error::payload(
                    axum::http::StatusCode::BAD_REQUEST,
                    &error.protocol_body().error.message,
                )),
            )
                .into_response();
        }
    };
    let decoded = match decode(
        &route.model,
        route.stream,
        route.endpoint.clone(),
        decoded_body,
        &headers,
    ) {
        Ok(decoded) => decoded,
        Err(error) => return gateway_error_response(&error),
    };
    let (client_ip, user_agent) = request_client_context(
        &headers,
        connect_info.map(|Extension(ConnectInfo(address))| address),
    );
    let guard = if route.stream {
        match service.try_register_connection() {
            Ok(guard) => Some(guard),
            Err(_) => return super::error::runtime_unavailable_response().into_response(),
        }
    } else {
        None
    };
    let started = match service
        .start_response(
            client,
            decoded.with_client_context(client_ip, user_agent),
            if route.stream {
                ClientTransport::HttpSse
            } else {
                ClientTransport::HttpJson
            },
            MODEL_ACTION_PATH,
        )
        .await
    {
        Ok(started) => started,
        Err(error) => return gateway_error_response(&error),
    };
    let trace = started.session.trace();
    trace.headers("client.request", json!({"ingressRequestId": ingress_id.as_ref().and_then(|Extension(id)| id.header_value().to_str().ok())}), headers.iter().map(|(name,value)| (name.as_str(),value.as_bytes())));
    trace.capture("client.request.body", &body);
    let response = if route.stream {
        stream_execution_response_as(started.session, guard, route.endpoint).await
    } else {
        collect_execution_response_as(started.session, route.endpoint).await
    };
    super::with_model_request_id(response, &started.request_id)
}
async fn metric(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    match authenticate_client(state.openai(), &headers) {
        Ok(_) => Json(json!({})).into_response(),
        Err(error) => client_access_error_response(error),
    }
}
