//! Reference attachment fallback with bounded input and no silent picture removal.

use std::collections::BTreeMap;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use gateway_protocol::openai::sse::SseError;
use reqwest::{header, multipart};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::transport::{
    CodexBackendClient, CodexBackendTransport, CodexClientError, CodexRequestContext,
    CodexTransportMetrics,
    client::{CodexClientVisibleUpstreamResponse, read_error_response_body, retry_after_seconds},
    diagnostics::CodexUpstreamSendPhase,
    profile::CodexWireProfile,
    response_meta,
};

const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 6 * 1024 * 1024;
const MAX_IMAGES: usize = 16;

pub(crate) fn needs_attachment(error: &CodexClientError) -> bool {
    let CodexClientError::Upstream { status, body, .. } = error else {
        return false;
    };
    if !matches!(status.as_u16(), 400 | 422) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    matches!(
        value.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_image_url" | "unsupported_image_url" | "inline_image_unsupported")
    )
}

pub(crate) fn has_inline(body: &Map<String, Value>) -> bool {
    body.get("input").is_some_and(contains_inline)
}

fn contains_inline(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_inline),
        Value::Object(object) => {
            object.get("type").and_then(Value::as_str) == Some("input_image")
                && object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .is_some_and(|url| url.starts_with("data:"))
                || object.values().any(contains_inline)
        }
        _ => false,
    }
}

pub(super) struct Picture {
    pub(super) url: String,
    pub(super) media: &'static str,
    extension: &'static str,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn collect(
    value: &Value,
    pictures: &mut Vec<Picture>,
    total: &mut usize,
) -> Result<(), CodexClientError> {
    match value {
        Value::Array(items) => {
            for item in items {
                collect(item, pictures, total)?;
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(url) = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .filter(|url| url.starts_with("data:"))
            {
                if pictures.iter().any(|picture| picture.url == url) {
                    return Ok(());
                }
                if pictures.len() >= MAX_IMAGES || url.len() > MAX_IMAGE_BYTES * 4 / 3 + 128 {
                    return Err(invalid("Excel attachment input limit exceeded"));
                }
                let (metadata, data) = url
                    .split_once(',')
                    .ok_or_else(|| invalid("Malformed image data URL"))?;
                let (media, extension) = match metadata {
                    "data:image/png;base64" => ("image/png", "png"),
                    "data:image/jpeg;base64" => ("image/jpeg", "jpg"),
                    "data:image/gif;base64" => ("image/gif", "gif"),
                    "data:image/webp;base64" => ("image/webp", "webp"),
                    _ => {
                        return Err(invalid(
                            "Excel attachments support PNG, JPEG, GIF and WebP data URLs",
                        ));
                    }
                };
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| invalid("Malformed image base64"))?;
                *total = total.saturating_add(bytes.len());
                if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES || *total > MAX_TOTAL_BYTES {
                    return Err(invalid("Excel attachment input limit exceeded"));
                }
                pictures.push(Picture {
                    url: url.into(),
                    media,
                    extension,
                    bytes,
                });
                return Ok(());
            }
            for item in object.values() {
                collect(item, pictures, total)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) async fn upload_inline(
    client: &CodexBackendClient,
    profile: &CodexWireProfile,
    context: CodexRequestContext<'_>,
    endpoint: &str,
    body: &Map<String, Value>,
) -> Result<Map<String, Value>, CodexClientError> {
    let mut value = Value::Object(body.clone());
    let mut pictures = Vec::new();
    collect(
        value.get("input").unwrap_or(&Value::Null),
        &mut pictures,
        &mut 0,
    )?;
    let upload_url = format!(
        "{}/attachments",
        endpoint
            .rsplit_once('/')
            .ok_or_else(|| invalid("Invalid Excel endpoint"))?
            .0
    );
    let mut replacements = BTreeMap::new();
    for picture in pictures {
        let mut headers = super::request_headers(context, &profile.user_agent())?;
        headers.remove(header::CONTENT_TYPE);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        let digest = hex::encode(Sha256::digest(&picture.bytes));
        let part = multipart::Part::bytes(picture.bytes)
            .file_name(format!("picture-{}.{}", &digest[..12], picture.extension))
            .mime_str(picture.media)
            .map_err(CodexClientError::HttpJson)?;
        let form = multipart::Form::new().part("file", part);
        let response = client
            .send_profiled(profile, false, |http| {
                http.post(&upload_url).headers(headers).multipart(form)
            })
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let diagnostics = response_meta::diagnostics(Some(status.as_u16()), response.headers());
            let rate_limit_headers = response_meta::rate_limit_headers(response.headers());
            let retry_after_seconds = retry_after_seconds(response.headers(), None);
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .map(|value| value.as_bytes().to_vec());
            let mut client_headers = response_meta::client_headers(response.headers());
            client_headers.retain(|(name, _)| name != "x-codex-turn-state");
            let raw = read_error_response_body(response)
                .await
                .map_err(CodexClientError::HttpJson)?;
            return Err(CodexClientError::Upstream {
                status,
                body: String::from_utf8_lossy(&raw).into_owned(),
                client_response: Some(Box::new(CodexClientVisibleUpstreamResponse::new(
                    status,
                    content_type,
                    client_headers,
                    raw,
                ))),
                retry_after_seconds,
                diagnostics: Box::new(diagnostics),
                set_cookie_headers: Vec::new(),
                rate_limit_headers,
                transport: CodexBackendTransport::HttpSse,
                transport_metrics: Box::new(CodexTransportMetrics::default()),
                send_phase: CodexUpstreamSendPhase::AfterPayload,
            });
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(CodexClientError::HttpJson)?;
            if bytes.len() + chunk.len() > 64 * 1024 {
                return Err(invalid("Excel attachment response is too large"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("Excel attachment response is not JSON"))?;
        let id = data
            .get("openai_file_id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty() && id.len() <= 512)
            .ok_or_else(|| invalid("Excel attachment response has no file ID"))?;
        replacements.insert(picture.url, id.to_owned());
    }
    rewrite(&mut value, &replacements);
    value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("Invalid Excel body"))
}

fn rewrite(value: &mut Value, replacements: &BTreeMap<String, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                rewrite(item, replacements);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(id) = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .and_then(|url| replacements.get(url))
            {
                let id = id.clone();
                object.remove("image_url");
                object.insert("file_id".into(), id.into());
                object.entry("detail").or_insert("auto".into());
            } else {
                for item in object.values_mut() {
                    rewrite(item, replacements);
                }
            }
        }
        _ => {}
    }
}

fn invalid(message: &str) -> CodexClientError {
    CodexClientError::InvalidSse(SseError::ParseError(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pictures_are_bounded_and_never_omitted() {
        let input = json!([{"type":"message","content":[{"type":"input_image","image_url":"data:image/png;base64,AQID"}]}]);
        let mut pictures = Vec::new();
        collect(&input, &mut pictures, &mut 0).unwrap();
        assert_eq!(pictures[0].bytes, [1, 2, 3]);
        let mut output = input.clone();
        rewrite(
            &mut output,
            &BTreeMap::from([(pictures[0].url.clone(), "file_fixture".into())]),
        );
        assert_eq!(output[0]["content"][0]["file_id"], "file_fixture");
        assert!(output[0]["content"][0].get("image_url").is_none());
        assert!(
            collect(
                &json!({"type":"input_image","image_url":"data:image/png;base64,!"}),
                &mut Vec::new(),
                &mut 0
            )
            .is_err()
        );
    }
}
