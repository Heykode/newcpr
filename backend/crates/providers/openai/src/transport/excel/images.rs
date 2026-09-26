//! Position-aware Responses image references and bounded image normalization.

use std::collections::{BTreeMap, BTreeSet};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use gateway_protocol::openai::sse::SseError;
use reqwest::{header, multipart};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{
    ExcelRequestError,
    image_cache::{AssetReceipt, cached_file_id, valid_file_id},
    replay::ReplayCapture,
};
use crate::transport::{
    CodexBackendClient, CodexBackendTransport, CodexClientError, CodexRequestContext,
    CodexTransportMetrics,
    client::{CodexClientVisibleUpstreamResponse, read_error_response_body, retry_after_seconds},
    diagnostics::CodexUpstreamSendPhase,
    profile::CodexWireProfile,
    response_meta,
};

#[derive(Clone, Copy)]
pub(crate) struct ImageLimits {
    pub(crate) single: usize,
    pub(crate) total: usize,
    pub(crate) count: usize,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self::from(gateway_core::routing::RequestTuning::default())
    }
}

impl From<gateway_core::routing::RequestTuning> for ImageLimits {
    fn from(value: gateway_core::routing::RequestTuning) -> Self {
        Self {
            single: value.excel_image_max_bytes.clamp(1, 20 * 1024 * 1024) as usize,
            total: value.excel_image_total_bytes.clamp(1, 32 * 1024 * 1024) as usize,
            count: value.excel_image_max_count.clamp(1, 4096) as usize,
        }
    }
}

fn is_inline(url: &str) -> bool {
    url.get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
}

pub(crate) fn has_user_inline(body: &Map<String, Value>) -> bool {
    user_contents(body.get("input").unwrap_or(&Value::Null)).any(|content| {
        content.as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("type").and_then(Value::as_str) == Some("input_image")
                    && part
                        .get("image_url")
                        .and_then(Value::as_str)
                        .is_some_and(is_inline)
            })
        })
    })
}

pub(super) fn is_user_message(value: &Value) -> bool {
    value.get("role").and_then(Value::as_str) == Some("user")
        && matches!(
            value.get("type").and_then(Value::as_str),
            None | Some("message")
        )
}

fn user_contents(input: &Value) -> impl Iterator<Item = &Value> {
    input
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| is_user_message(item))
        .filter_map(|item| item.get("content"))
}

pub(crate) fn has_images(body: &Map<String, Value>) -> bool {
    image_contents(body.get("input").unwrap_or(&Value::Null)).any(|content| {
        content.as_array().is_some_and(|parts| {
            parts
                .iter()
                .any(|part| part.get("type").and_then(Value::as_str) == Some("input_image"))
        })
    })
}

fn image_contents(input: &Value) -> impl Iterator<Item = &Value> {
    input.as_array().into_iter().flatten().filter_map(|item| {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call_output" | "custom_tool_call_output") => item.get("output"),
            None | Some("message") => item.get("content"),
            _ => None,
        }
    })
}

/// Validate the source before any upload or relay. Never echo image contents or IDs.
#[cfg(test)]
pub(crate) fn validate(body: &Map<String, Value>) -> Result<(), ExcelRequestError> {
    validate_with_limits(body, true, ImageLimits::default())
}

/// The relay reserves its global memory budget before decoding image bytes.
#[cfg(test)]
pub(crate) fn validate_references(body: &Map<String, Value>) -> Result<(), ExcelRequestError> {
    validate_with_limits(body, false, ImageLimits::default())
}

pub(crate) fn validate_with_limits(
    body: &Map<String, Value>,
    decode: bool,
    limits: ImageLimits,
) -> Result<(), ExcelRequestError> {
    fn visit(
        value: &Value,
        tool_output: bool,
        user: bool,
        decode: bool,
        limits: ImageLimits,
    ) -> Result<(), ExcelRequestError> {
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, tool_output, user, decode, limits)?;
                }
            }
            Value::Object(fields)
                if fields.get("type").and_then(Value::as_str) == Some("input_image") =>
            {
                if !tool_output && !user {
                    return Err(ExcelRequestError::ImageInput(
                        "images must be in user messages or tool results",
                    ));
                }
                if fields.get("detail").is_some_and(|detail| {
                    !detail.is_null()
                        && !matches!(detail.as_str(), Some("auto" | "low" | "high" | "original"))
                }) {
                    return Err(ExcelRequestError::ImageInput(
                        "detail must be auto, low, high or original",
                    ));
                }
                if fields.get("file_id").is_some_and(|id| !id.is_null())
                    && fields.get("image_url").is_some_and(|url| !url.is_null())
                {
                    return Err(ExcelRequestError::ImageInput(
                        "provide only one of file_id or image_url",
                    ));
                }
                if tool_output
                    && fields
                        .get("file_id")
                        .is_some_and(|id| id.as_str().is_some_and(|value| !value.trim().is_empty()))
                {
                    return Err(ExcelRequestError::ImageInput(
                        "tool image file_id is unsupported; return the original image as Base64 or HTTPS image_url",
                    ));
                }
                if fields.get("file_id").is_some_and(|id| {
                    !id.is_null() && id.as_str().is_none_or(|value| value.trim().is_empty())
                }) {
                    return Err(ExcelRequestError::ImageInput(
                        "file_id must be a non-empty string",
                    ));
                }
                if fields
                    .get("file_id")
                    .is_some_and(|id| id.as_str().is_some_and(|value| !valid_file_id(value)))
                {
                    return Err(ExcelRequestError::ImageInput("file_id is invalid"));
                }
                if fields.get("file_id").is_some_and(|id| !id.is_null()) {
                    return Ok(());
                }
                let raw = fields.get("image_url").and_then(Value::as_str).ok_or(
                    ExcelRequestError::ImageInput("provide file_id or an image_url"),
                )?;
                if is_inline(raw) {
                    validate_data_url(raw, decode, limits)?;
                    return Ok(());
                }
                let valid = raw.trim() == raw
                    && raw
                        .get(..8)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
                    && !raw[8..].starts_with('/')
                    && !raw.contains('\\')
                    && !raw.chars().any(char::is_control)
                    && url::Url::parse(raw).is_ok_and(|url| {
                        url.scheme() == "https"
                            && url.host_str().is_some_and(|host| !host.is_empty())
                            && url.username().is_empty()
                            && url.password().is_none()
                    });
                if !valid {
                    return Err(ExcelRequestError::ImageInput(
                        "provide an absolute HTTPS image_url without embedded credentials",
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }
    decoded_budget(
        image_contents(body.get("input").unwrap_or(&Value::Null)),
        limits,
    )
    .map_err(|_| ExcelRequestError::ImageInput("inline image input is invalid"))?;
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let tool = matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call_output" | "custom_tool_call_output")
        );
        visit(
            item.get(if tool { "output" } else { "content" })
                .unwrap_or(&Value::Null),
            tool,
            is_user_message(item),
            decode,
            limits,
        )?;
    }
    Ok(())
}

pub(super) struct Picture {
    pub(super) url: String,
    pub(super) media: &'static str,
    pub(super) extension: &'static str,
    pub(super) bytes: Vec<u8>,
}

/// Upper bound without decoding or copying data URLs, for global relay admission.
fn decoded_budget<'a>(
    values: impl Iterator<Item = &'a Value>,
    limits: ImageLimits,
) -> Result<usize, CodexClientError> {
    fn visit<'a>(
        value: &'a Value,
        urls: &mut BTreeSet<&'a str>,
        count: &mut usize,
        limits: ImageLimits,
    ) -> Result<(), CodexClientError> {
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, urls, count, limits)?;
                }
            }
            Value::Object(fields) => {
                if fields.get("type").and_then(Value::as_str) == Some("input_image") {
                    *count += 1;
                    if *count > limits.count {
                        return Err(invalid("Excel image count limit exceeded"));
                    }
                }
                if fields.get("type").and_then(Value::as_str) == Some("input_image")
                    && let Some(url) = fields
                        .get("image_url")
                        .and_then(Value::as_str)
                        .filter(|url| is_inline(url))
                {
                    urls.insert(url);
                    if url.len() > limits.single * 4 / 3 + 128 {
                        return Err(invalid("Excel inline image input limit exceeded"));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut urls = BTreeSet::new();
    let mut count = 0;
    for value in values {
        visit(value, &mut urls, &mut count, limits)?;
    }
    let mut total = 0usize;
    for url in urls {
        let (_, data) = url
            .split_once(',')
            .ok_or_else(|| invalid("Malformed image data URL"))?;
        let size = data.len().div_ceil(4) * 3
            - data
                .bytes()
                .rev()
                .take(2)
                .take_while(|byte| *byte == b'=')
                .count();
        if size == 0 || size > limits.single {
            return Err(invalid("Excel inline image input limit exceeded"));
        }
        total += size;
    }
    if total > limits.total {
        return Err(invalid("Excel inline image input limit exceeded"));
    }
    Ok(total)
}

#[cfg(test)]
pub(super) fn decoded_budget_user(value: &Value) -> Result<usize, CodexClientError> {
    decoded_budget_user_with_limits(value, ImageLimits::default())
}

pub(super) fn decoded_budget_user_with_limits(
    value: &Value,
    limits: ImageLimits,
) -> Result<usize, CodexClientError> {
    decoded_budget(user_contents(value), limits)
}

#[cfg(test)]
pub(super) fn collect_user(
    value: &Value,
    pictures: &mut Vec<Picture>,
) -> Result<(), CodexClientError> {
    collect_user_with_limits(value, pictures, ImageLimits::default())
}

pub(super) fn collect_user_with_limits(
    value: &Value,
    pictures: &mut Vec<Picture>,
    limits: ImageLimits,
) -> Result<(), CodexClientError> {
    let mut total = 0;
    for content in user_contents(value) {
        collect_with_limits(content, pictures, &mut total, limits)?;
    }
    Ok(())
}

pub(super) fn collect(
    value: &Value,
    pictures: &mut Vec<Picture>,
    total: &mut usize,
) -> Result<(), CodexClientError> {
    collect_with_limits(value, pictures, total, ImageLimits::default())
}

fn collect_with_limits(
    value: &Value,
    pictures: &mut Vec<Picture>,
    total: &mut usize,
    limits: ImageLimits,
) -> Result<(), CodexClientError> {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_with_limits(item, pictures, total, limits)?;
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("input_image")
                && let Some(url) = object
                    .get("image_url")
                    .and_then(Value::as_str)
                    .filter(|url| is_inline(url))
            {
                if pictures.iter().any(|picture| picture.url == url) {
                    return Ok(());
                }
                if pictures.len() >= limits.count || url.len() > limits.single * 4 / 3 + 128 {
                    return Err(invalid("Excel inline image input limit exceeded"));
                }
                let (metadata, data) = url
                    .split_once(',')
                    .ok_or_else(|| invalid("Malformed image data URL"))?;
                let (media, extension) = match metadata.to_ascii_lowercase().as_str() {
                    "data:image/png;base64" => ("image/png", "png"),
                    "data:image/jpeg;base64" => ("image/jpeg", "jpg"),
                    "data:image/gif;base64" => ("image/gif", "gif"),
                    "data:image/webp;base64" => ("image/webp", "webp"),
                    _ => {
                        return Err(invalid(
                            "Excel inline images support PNG, JPEG, GIF and WebP data URLs",
                        ));
                    }
                };
                let bytes = STANDARD
                    .decode(data)
                    .map_err(|_| invalid("Malformed image base64"))?;
                *total = total.saturating_add(bytes.len());
                if bytes.is_empty() || bytes.len() > limits.single || *total > limits.total {
                    return Err(invalid("Excel inline image input limit exceeded"));
                }
                pictures.push(Picture {
                    url: url.into(),
                    media,
                    extension,
                    bytes,
                });
                return Ok(());
            }
        }
        _ => {}
    }
    Ok(())
}

pub(crate) struct UploadedImages {
    pub(crate) body: Map<String, Value>,
    receipts: Vec<AssetReceipt>,
}

impl UploadedImages {
    pub(crate) async fn observe_error(&self, error: &CodexClientError) {
        if rejected_attachment(error) {
            for receipt in &self.receipts {
                receipt.invalidate().await;
            }
        }
    }
}

fn rejected_attachment(error: &CodexClientError) -> bool {
    let CodexClientError::Upstream { status, body, .. } = error else {
        return false;
    };
    if !matches!(status.as_u16(), 400 | 404 | 422) {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    matches!(
        value.pointer("/error/code").and_then(Value::as_str),
        Some(
            "invalid_file_id"
                | "file_not_found"
                | "file_expired"
                | "attachment_not_found"
                | "attachment_expired"
        )
    )
}

pub(crate) async fn upload_inline_with_limits(
    client: &CodexBackendClient,
    profile: &CodexWireProfile,
    context: CodexRequestContext<'_>,
    endpoint: &str,
    body: &Map<String, Value>,
    replay: Option<&ReplayCapture>,
    limits: ImageLimits,
) -> Result<UploadedImages, CodexClientError> {
    static UPLOADS: std::sync::LazyLock<tokio::sync::Semaphore> =
        std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(32));
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let _permit = UPLOADS
            .acquire()
            .await
            .map_err(|_| invalid("Excel attachment admission unavailable"))?;
        validate_with_limits(body, true, limits)
            .map_err(|_| invalid("Excel image input is invalid"))?;
        upload_admitted(client, profile, context, endpoint, body, replay, limits).await
    })
    .await
    .map_err(|_| invalid("Excel attachment upload timed out"))?
}

async fn upload_admitted(
    client: &CodexBackendClient,
    profile: &CodexWireProfile,
    context: CodexRequestContext<'_>,
    endpoint: &str,
    body: &Map<String, Value>,
    replay: Option<&ReplayCapture>,
    limits: ImageLimits,
) -> Result<UploadedImages, CodexClientError> {
    let mut value = Value::Object(body.clone());
    let mut pictures = Vec::new();
    collect_user_with_limits(
        value.get("input").unwrap_or(&Value::Null),
        &mut pictures,
        limits,
    )?;
    let upload_url = format!(
        "{}/attachments",
        endpoint
            .rsplit_once('/')
            .ok_or_else(|| invalid("Invalid Excel endpoint"))?
            .0
    );
    let mut replacements = BTreeMap::new();
    let mut receipts = Vec::new();
    for picture in pictures {
        let cache = replay.map(|replay| replay.image_cache(endpoint, &picture));
        let _guard = match &cache {
            Some(cache) => cache.lock().await,
            None => None,
        };
        let hit = match &cache {
            Some(cache) => cache.read().await,
            None => None,
        };
        if let Some(payload) = hit {
            replacements.insert(picture.url, cached_file_id(&payload).unwrap().to_owned());
            receipts.push(cache.expect("cache hit has an owner").receipt(payload));
            continue;
        }
        let mut headers = super::request_headers(context, &profile.user_agent())?;
        headers.remove(header::CONTENT_TYPE);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            "copilot-vision-request",
            header::HeaderValue::from_static("true"),
        );
        let digest = hex::encode(Sha256::digest(&picture.bytes));
        let part = multipart::Part::bytes(picture.bytes)
            .file_name(format!("picture-{}.{}", &digest[..12], picture.extension))
            .mime_str(picture.media)
            .map_err(CodexClientError::HttpJson)?;
        let form = multipart::Form::new()
            .text("purpose", "vision")
            .part("file", part);
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
        let id = ["openai_file_id", "file_id", "id"]
            .into_iter()
            .filter_map(|field| data.get(field).and_then(Value::as_str))
            .find(|id| valid_file_id(id))
            .ok_or_else(|| invalid("Excel attachment response has no file ID"))?;
        let id = if let Some(cache) = cache {
            let payload = cache.store(id).await;
            let id = cached_file_id(&payload).unwrap().to_owned();
            receipts.push(cache.receipt(payload));
            id
        } else {
            id.to_owned()
        };
        replacements.insert(picture.url, id);
    }
    if let Some(items) = value.get_mut("input").and_then(Value::as_array_mut) {
        for item in items.iter_mut().filter(|item| is_user_message(item)) {
            if let Some(content) = item.get_mut("content") {
                rewrite_user_images(content, &replacements);
            }
        }
    }
    let body = value
        .as_object()
        .cloned()
        .ok_or_else(|| invalid("Invalid Excel body"))?;
    Ok(UploadedImages { body, receipts })
}

fn rewrite_user_images(value: &mut Value, replacements: &BTreeMap<String, String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                rewrite_user_images(item, replacements);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("function_call_output")
                || object.get("type").and_then(Value::as_str) == Some("custom_tool_call_output")
            {
                return;
            }
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
            }
        }
        _ => {}
    }
}

fn validate_data_url(
    raw: &str,
    decode: bool,
    limits: ImageLimits,
) -> Result<(), ExcelRequestError> {
    let (metadata, encoded) = raw
        .split_once(',')
        .ok_or(ExcelRequestError::ImageInput("malformed image data URL"))?;
    let metadata = metadata.to_ascii_lowercase();
    if !matches!(
        metadata.as_str(),
        "data:image/png;base64"
            | "data:image/jpeg;base64"
            | "data:image/gif;base64"
            | "data:image/webp;base64"
    ) {
        return Err(ExcelRequestError::ImageInput(
            "inline images support PNG, JPEG, GIF and WebP data URLs",
        ));
    }
    if !decode {
        return Ok(());
    }
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| ExcelRequestError::ImageInput("malformed image base64"))?;
    if bytes.is_empty() || bytes.len() > limits.single {
        return Err(ExcelRequestError::ImageInput(
            "inline image size is invalid",
        ));
    }
    let actual = match imagesize::image_type(&bytes) {
        Ok(imagesize::ImageType::Png) => "data:image/png;base64",
        Ok(imagesize::ImageType::Jpeg) => "data:image/jpeg;base64",
        Ok(imagesize::ImageType::Gif) => "data:image/gif;base64",
        Ok(imagesize::ImageType::Webp) => "data:image/webp;base64",
        _ => return Err(ExcelRequestError::ImageInput("invalid image bytes")),
    };
    let size = imagesize::blob_size(&bytes)
        .map_err(|_| ExcelRequestError::ImageInput("invalid image dimensions"))?;
    if metadata != actual
        || size.width == 0
        || size.height == 0
        || size.width > 16_384
        || size.height > 16_384
        || size.width.saturating_mul(size.height) > 40_000_000
    {
        return Err(ExcelRequestError::ImageInput(
            "invalid image format or dimensions",
        ));
    }
    Ok(())
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
        let input = json!([{"type":"input_image","image_url":"data:image/png;base64,AQID"}]);
        let mut pictures = Vec::new();
        collect(&input, &mut pictures, &mut 0).unwrap();
        assert_eq!(pictures[0].bytes, [1, 2, 3]);
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
