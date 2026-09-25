//! Client image-tool requests use the selected account's Excel image backend.

use bytes::Bytes;
use futures::StreamExt;
use reqwest::{header, multipart};
use serde_json::{Map, Value, json};

use super::{ExcelRequestError, images};
use crate::transport::{
    CodexBackendClient, CodexBackendJsonResponse, CodexBackendTransport, CodexClientError,
    CodexRequestContext, CodexTransportMetrics,
    client::{CodexClientVisibleUpstreamResponse, read_error_response_body, retry_after_seconds},
    diagnostics::CodexUpstreamSendPhase,
    endpoints::{CODEX_IMAGE_EDITS_PATH, CODEX_IMAGE_GENERATIONS_PATH},
    response_meta,
};

const INVALID: ExcelRequestError = ExcelRequestError::Image;
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn is_image_path(path: &str) -> bool {
    path.ends_with(CODEX_IMAGE_GENERATIONS_PATH) || path.ends_with(CODEX_IMAGE_EDITS_PATH)
}

pub(crate) struct PreparedImage {
    pub(crate) endpoint: String,
    fields: Map<String, Value>,
    pictures: Vec<images::Picture>,
}

impl PreparedImage {
    pub(crate) fn parse(body: &[u8], path: &str) -> Result<Self, ExcelRequestError> {
        let source: Map<String, Value> = serde_json::from_slice(body).map_err(|_| INVALID)?;
        let prompt = source
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or(INVALID)?;
        if source
            .get("model")
            .is_some_and(|v| !v.is_null() && v != "gpt-image-2")
            || source
                .get("output_format")
                .is_some_and(|v| !v.is_null() && v != "png")
            || source
                .get("stream")
                .is_some_and(|v| !v.is_null() && v != false)
            || source.get("mask").is_some_and(|v| !v.is_null())
            || source
                .get("response_format")
                .is_some_and(|v| !v.is_null() && v != "b64_json")
            || ["input_fidelity", "output_compression"]
                .iter()
                .any(|name| source.get(*name).is_some_and(|v| !v.is_null()))
        {
            return Err(INVALID);
        }
        let mut fields = json!({"model":"gpt-image-2","prompt":prompt,"output_format":"png"})
            .as_object()
            .expect("object literal")
            .clone();
        for (name, choices) in [
            (
                "size",
                &["auto", "1024x1024", "1536x1024", "1024x1536", "1280x720"][..],
            ),
            ("quality", &["auto", "low", "medium", "high"][..]),
            ("background", &["auto", "opaque"][..]),
        ] {
            let value = match source.get(name) {
                None | Some(Value::Null) => "auto",
                Some(value) => value
                    .as_str()
                    .filter(|value| choices.contains(value))
                    .ok_or(INVALID)?,
            };
            fields.insert(name.into(), value.into());
        }
        if let Some(n) = source.get("n").filter(|v| !v.is_null()) {
            let count = n.as_u64().filter(|n| (1..=3).contains(n)).ok_or(INVALID)?;
            fields.insert("n".into(), count.into());
        }
        let edit = path.ends_with(CODEX_IMAGE_EDITS_PATH);
        let mut pictures = Vec::new();
        if edit {
            let values = source
                .get("images")
                .and_then(Value::as_array)
                .filter(|v| !v.is_empty() && v.len() <= 16)
                .ok_or(INVALID)?;
            let mut total = 0;
            for value in values {
                let url = value
                    .get("image_url")
                    .and_then(Value::as_str)
                    .filter(|v| v.starts_with("data:"))
                    .ok_or(INVALID)?;
                // Preserve input order and repeated pictures while sharing bounded decoding.
                let mut decoded = Vec::new();
                images::collect(
                    &json!({"type":"input_image","image_url":url}),
                    &mut decoded,
                    &mut total,
                )
                .map_err(|_| INVALID)?;
                let picture = decoded.pop().ok_or(INVALID)?;
                let media = match imagesize::image_type(&picture.bytes).map_err(|_| INVALID)? {
                    imagesize::ImageType::Png => "image/png",
                    imagesize::ImageType::Jpeg => "image/jpeg",
                    imagesize::ImageType::Gif => "image/gif",
                    imagesize::ImageType::Webp => "image/webp",
                    _ => return Err(INVALID),
                };
                if media != picture.media {
                    return Err(INVALID);
                }
                pictures.push(picture);
            }
        } else if !path.ends_with(CODEX_IMAGE_GENERATIONS_PATH)
            || source.get("images").is_some_and(|v| !v.is_null())
        {
            return Err(INVALID);
        }
        Ok(Self {
            endpoint: format!(
                "https://bps.openai.com/basispoints/api/images/{}",
                if edit { "edits" } else { "generations" }
            ),
            fields,
            pictures,
        })
    }
}

impl CodexBackendClient {
    pub(crate) async fn post_excel_image(
        &self,
        image: &PreparedImage,
        context: CodexRequestContext<'_>,
    ) -> Result<CodexBackendJsonResponse, CodexClientError> {
        let profile = self.profile.snapshot();
        let mut headers = super::request_headers(context, &profile.user_agent())?;
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        let trace = context
            .trace
            .cloned()
            .unwrap_or_default()
            .exchange("excel_http_json");
        trace.headers(
            "upstream.request.headers",
            json!({
                "method":"POST", "endpoint":image.endpoint
            }),
            headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        let started = std::time::Instant::now();
        let response = if image.pictures.is_empty() {
            self.send_profiled(&profile, false, |client| {
                client
                    .post(&image.endpoint)
                    .headers(headers)
                    .json(&image.fields)
            })
            .await?
        } else {
            headers.remove(header::CONTENT_TYPE);
            let mut form = multipart::Form::new();
            for (name, value) in &image.fields {
                form = form.text(
                    name.clone(),
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
            let name = if image.pictures.len() == 1 {
                "image"
            } else {
                "image[]"
            };
            for (index, picture) in image.pictures.iter().enumerate() {
                let part = multipart::Part::bytes(picture.bytes.clone())
                    .file_name(format!("picture-{index}.{}", picture.extension))
                    .mime_str(picture.media)
                    .map_err(CodexClientError::HttpJson)?;
                form = form.part(name, part);
            }
            self.send_profiled(&profile, false, |client| {
                client
                    .post(&image.endpoint)
                    .headers(headers)
                    .multipart(form)
            })
            .await?
        };
        let status = response.status();
        trace.headers(
            "upstream.response.headers",
            json!({"status":status.as_u16()}),
            response
                .headers()
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_bytes())),
        );
        let diagnostics = response_meta::diagnostics(Some(status.as_u16()), response.headers());
        let rate_limit_headers = response_meta::rate_limit_headers(response.headers());
        let rate_limit_observed_at = std::time::SystemTime::now();
        let mut response_metadata = response_meta::response_metadata(response.headers());
        response_metadata
            .client_headers
            .retain(|(name, _)| name != "x-codex-turn-state");
        let transport_metrics = CodexTransportMetrics {
            upstream_headers_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(i64::MAX)),
            ..CodexTransportMetrics::default()
        };
        if !status.is_success() {
            let retry_after_seconds = retry_after_seconds(response.headers(), None);
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|v| v.as_bytes().to_vec());
            let raw = read_error_response_body(response)
                .await
                .map_err(CodexClientError::HttpJson)?;
            return Err(CodexClientError::Upstream {
                status,
                body: String::from_utf8_lossy(&raw).into_owned(),
                client_response: Some(Box::new(CodexClientVisibleUpstreamResponse::new(
                    status,
                    content_type,
                    response_metadata.client_headers,
                    raw,
                ))),
                retry_after_seconds,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers: Vec::new(),
                rate_limit_headers,
                transport: CodexBackendTransport::HttpJson,
                transport_metrics: Box::new(transport_metrics),
                send_phase: CodexUpstreamSendPhase::AfterPayload,
            });
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(CodexClientError::HttpJson)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(invalid_response());
            }
            bytes.extend_from_slice(&chunk);
        }
        // A 200 error/empty object is not a generated picture.
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid_response())?;
        if value.get("error").is_some_and(|v| !v.is_null())
            || value
                .get("data")
                .and_then(Value::as_array)
                .is_none_or(|items| {
                    items.is_empty()
                        || items.iter().any(|item| {
                            item.get("b64_json")
                                .and_then(Value::as_str)
                                .is_none_or(str::is_empty)
                        })
                })
        {
            return Err(invalid_response());
        }
        Ok(CodexBackendJsonResponse {
            body: Bytes::from(bytes),
            set_cookie_headers: Vec::new(),
            rate_limit_headers,
            rate_limit_observed_at,
            diagnostics,
            response_metadata,
            transport_metrics,
        })
    }
}

fn invalid_response() -> CodexClientError {
    CodexClientError::InvalidSse(gateway_protocol::openai::sse::SseError::ParseError(
        "Excel image backend returned an invalid or oversized response".into(),
    ))
}
