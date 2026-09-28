//! Excel Responses adaptation derived from excel-codex-bridge v0.4.6 (Unlicense).
//! Account selection, authorization, egress and execution remain CPR-owned.
//! Feature: excel-upstream. Removal boundaries: docs/excel-removal.md.

mod catalog;
pub(crate) mod diagnostics;
pub(super) mod encrypted;
pub(crate) mod encrypted_content;
mod envelope;
mod history_messages;
#[cfg(test)]
mod history_tests;
mod image_cache;
pub(crate) mod image_generation;
pub(crate) mod image_relay;
#[cfg(test)]
mod image_tests;
pub(crate) mod images;
mod repair;
pub(crate) mod replay;
mod request;
mod stream;
mod structured;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_compat_tests;
mod tools;
pub(crate) mod usage;

pub(crate) use request::{ExcelRequestError, prepare_request, reasoning_effort};
pub(crate) use stream::transform_stream;
pub(crate) use stream::transform_stream_with_repair;
pub(crate) use structured::StructuredOutput;
pub(crate) use tools::ClientTools;

#[derive(Clone)]
pub(crate) struct ExcelPreparedRequest {
    pub(crate) exit_lease:
        Option<std::sync::Arc<dyn gateway_core::provider_ports::session_proxy::SessionProxyLease>>,
    pub(crate) body: serde_json::Map<String, serde_json::Value>,
    pub(crate) tools: ClientTools,
    pub(crate) structured: Option<StructuredOutput>,
    pub(crate) _image_lease: Option<std::sync::Arc<image_relay::ImageLease>>,
    pub(crate) image_limits: images::ImageLimits,
    pub(crate) completed: std::sync::Arc<std::sync::Mutex<Option<serde_json::Value>>>,
    pub(crate) usage: usage::ExcelUsagePolicy,
    pub(crate) replay: Option<replay::ReplayCapture>,
    pub(crate) endpoint: String,
}

pub(crate) const RESPONSES_URL: &str = "https://bps.openai.com/basispoints/api/responses";
pub(crate) const RESPONSES_PATH: &str = "/basispoints/api/responses";

pub(crate) fn request_headers(
    context: crate::transport::CodexRequestContext<'_>,
    user_agent: &str,
) -> Result<reqwest::header::HeaderMap, reqwest::header::InvalidHeaderValue> {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut headers = HeaderMap::new();
    let mut authorization = HeaderValue::from_str(context.authorization)?;
    authorization.set_sensitive(true);
    headers.insert("authorization", authorization);
    for (key, value) in [
        ("chatgpt-account-id", context.account_id.unwrap_or("")),
        ("x-openai-account-id", context.account_id.unwrap_or("")),
        ("user-agent", user_agent),
    ] {
        headers.insert(key, HeaderValue::from_str(value)?);
    }
    for (key, value) in [
        ("accept", "text/event-stream"),
        ("accept-encoding", "identity"),
        ("content-type", "application/json"),
        ("origin", "https://bps.openai.com"),
        ("x-basispoints-auth-mode", "chatgpt"),
        (
            "x-openai-internal-basispoints-client-agent-profile",
            "excel",
        ),
        ("x-openai-internal-basispoints-client-editor", "excel"),
        ("x-openai-internal-basispoints-client-host", "office"),
        ("x-openai-internal-basispoints-client-platform", "excel"),
        ("x-openai-internal-basispoints-client-platform-class", "PC"),
        (
            "x-openai-internal-basispoints-client-product",
            "basispoints-excel-plugin",
        ),
        ("x-openai-internal-basispoints-client-runtime", "desktop"),
        ("x-openai-internal-basispoints-office-host", "Excel"),
        ("x-openai-internal-basispoints-office-platform", "PC"),
        ("x-stainless-arch", "unknown"),
        ("x-stainless-lang", "js"),
        ("x-stainless-os", "Unknown"),
        ("x-stainless-package-version", "6.31.0"),
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime", "browser:chrome"),
    ] {
        headers.insert(key, HeaderValue::from_static(value));
    }
    Ok(headers)
}

pub(crate) const EXTERNAL_CLIENT_INSTRUCTIONS: &str = "This request comes from an external Responses client. \
    Return assistant text. Do not call Excel, Office, workbook or connector tools.";

pub(crate) const CLIENT_TOOL_INSTRUCTIONS: &str = "This request comes from an external Responses client. \
    Use only the client tools in the catalog below. There is no live Excel workbook for this request. \
    The proxy intercepts run_officejs as a transport and never executes Office code. \
    To call one client tool, call native run_officejs using the transport matching its catalog type. \
    FUNCTION: code must contain one serialized JSON object {\"name\":\"CATALOG_NAME\",\"arguments\":{...}}. \
    Arguments is an object, not an extra JSON string. \
    CUSTOM: set summary to exactly codex2api.custom/CATALOG_NAME and put the exact raw tool input directly in code. \
    Do not wrap custom input in another JSON object or add Markdown fences. \
    CATALOG_NAME includes its exact namespace. Outer arguments also include extended_summary, destructive=false and references=[]. \
    Never nest run_officejs inside code. Serialize outer native arguments with proper JSON escaping. \
    After receiving its result continue the task; do not repeat completed calls. \
    Tool results replayed under run_officejs are the named client tool's results. \
    When a tool is needed, emit its call in this response instead of only announcing it. \
    Do not call other native tools or claim that shell, filesystem or workspace access is unavailable \
    when a suitable catalog tool exists. If no tool is needed, answer as assistant text. Client tool catalog:\n";
