//! Safe semantic errors for Excel HTTP-200 streams, never HTTP rejection evidence.
use serde_json::{Value, json};

pub(super) fn normalize(kind: &str, payload: &mut Value) {
    if !matches!(kind, "error" | "response.failed" | "response.cancelled") {
        return;
    }
    let detail = payload
        .pointer("/response/error")
        .filter(|v| v.is_object())
        .or_else(|| payload.get("error").filter(|v| v.is_object()))
        .unwrap_or(payload);
    let raw_code = detail["code"].as_str().unwrap_or_default();
    let code = if identifier_status(raw_code).is_some() {
        raw_code
    } else if kind == "response.cancelled" {
        "basispoints_upstream_cancelled"
    } else {
        "basispoints_upstream_error"
    };
    let status = [
        payload.get("status"),
        payload.get("status_code"),
        detail.get("status"),
        detail.get("status_code"),
        payload.pointer("/response/status_code"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_u64)
    .find(|n| (400..=599).contains(n))
    .or_else(|| identifier_status(raw_code))
    .or_else(|| identifier_status(detail["type"].as_str().unwrap_or_default()))
    .unwrap_or(502);
    let error_type = match status {
        401 => "authentication_error",
        403 => "permission_error",
        429 => "rate_limit_error",
        400..=499 => "invalid_request_error",
        _ => "server_error",
    };
    let error = json!({"status":status,"code":code,"type":error_type,
        "message":format!("Excel BPS upstream failure: {code}; request was not replayed")});
    let mut output = json!({"status":status});
    if let Some(usage) = payload.get("usage").filter(|v| v.is_object()) {
        output["usage"] = usage.clone();
    }
    if kind == "error" {
        output["error"] = error;
        // Metering checkpoints from correction streams are response-local.
        if let Some(response) = payload.get("response").filter(|v| v.is_object()) {
            output["response"] = response.clone();
            output["response"]["error"] = output["error"].clone();
        }
    } else {
        output["response"] = payload.get("response").filter(|v| v.is_object()).cloned()
            .unwrap_or_else(|| json!({"status":if kind == "response.cancelled" {"cancelled"} else {"failed"},"output":[]}));
        output["response"]["error"] = error;
    }
    *payload = output;
}

fn identifier_status(code: &str) -> Option<u64> {
    Some(match code {
        "invalid_request"
        | "invalid_request_error"
        | "bad_request_error"
        | "invalid_argument"
        | "invalid_value"
        | "unsupported_value"
        | "invalid_prompt"
        | "context_length_exceeded"
        | "string_above_max_length"
        | "previous_response_not_found"
        | "thinking_signature_invalid"
        | "invalid_encrypted_content"
        | "cyber_policy"
        | "content_policy_violation" => 400,
        "authentication_error" | "invalid_api_key" | "token_expired" => 401,
        "permission_error" | "permission_denied" | "basispoints_model_access_changed" => 403,
        "model_not_found" | "model_not_found_error" => 404,
        "message_too_big" => 413,
        "rate_limit_error"
        | "rate_limit_exceeded"
        | "insufficient_quota"
        | "usage_limit_reached" => 429,
        "server_error" | "internal_server_error" => 500,
        "overloaded_error" | "service_unavailable" => 503,
        "api_error"
        | "basispoints_upstream_error"
        | "basispoints_upstream_cancelled"
        | "basispoints_protocol_error" => 502,
        _ => return None,
    })
}
