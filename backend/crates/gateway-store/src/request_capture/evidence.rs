//! Small, allowlisted decision records, independent of raw-body buffering.

use serde_json::{Value, json};

pub(super) const LIMIT: usize = 64;

pub(super) struct Evidence {
    pub attempt: u32,
    pub account: Option<String>,
    pub task_mask: u16,
    pub body: Value,
}

pub(super) fn summary(stage: &'static str, attempt: u32, data: &Value) -> Option<Value> {
    if !matches!(
        stage,
        "transport.selected"
            | "upstream.read.failed"
            | "upstream.completed"
            | "upstream.business_result"
            | "retry.decided"
            | "attempt.failed"
            | "downstream.write.failed"
            | "downstream.status"
            | "request.finished"
    ) {
        return None;
    }
    let mut result = json!({"stage":stage,"attempt":attempt});
    for key in [
        "status",
        "upstreamStatus",
        "connectionAgeMs",
        "connectionIdleMs",
        "delayMs",
        "waitMs",
    ] {
        if let Some(value) = data[key].as_u64() {
            result[key] = value.into();
        }
    }
    for key in [
        "retryable",
        "continuationRetry",
        "sameAccountRetry",
        "accountRotationRetry",
        "ordinaryRetry",
        "transportRecovery",
        "transientRetry",
        "downstreamCommitted",
        "reused",
    ] {
        if let Some(value) = data[key].as_bool() {
            result[key] = value.into();
        }
    }
    // Never copy free-form messages, upstream identifiers, headers or tool content.
    for key in [
        "kind",
        "errorKind",
        "outcome",
        "sendState",
        "failureReason",
        "lastEventType",
        "decision",
        "requirement",
    ] {
        if let Some(value) = data[key].as_str().and_then(safe_label) {
            result[key] = value.into();
        }
    }
    if let Some(code) = data.pointer("/diagnostic/code").and_then(Value::as_str) {
        if let Some(value) = safe_label(code) {
            result["code"] = value.into();
        } else if let Some(number) = code
            .strip_prefix("websocket_close_")
            .filter(|value| value.len() == 4)
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|value| (1000..=4999).contains(value))
        {
            result["closeCode"] = number.into();
        }
    }
    Some(result)
}

fn safe_label(value: &str) -> Option<&str> {
    matches!(
        value,
        "invalid_request"
            | "continuation_recovery_required"
            | "unsupported"
            | "unauthorized"
            | "permission_denied"
            | "rate_limited"
            | "quota_exhausted"
            | "account_capacity_unavailable"
            | "no_eligible_account"
            | "provider_infrastructure_unavailable"
            | "timeout"
            | "transport"
            | "protocol"
            | "unavailable"
            | "upstream_capacity_unavailable"
            | "cancelled"
            | "process_terminated"
            | "invalid_encrypted_content"
            | "unsupported_value"
            | "unsupported_parameter"
            | "server_error"
            | "usage_limit_reached"
            | "response.failed"
            | "response.incomplete"
            | "response.completed"
            | "response.created"
            | "response.in_progress"
            | "response.output_item.done"
            | "error"
            | "response.output_text.delta"
            | "response.function_call_arguments.delta"
            | "Failed"
            | "Incomplete"
            | "Succeeded"
            | "succeeded"
            | "Cancelled"
            | "failed"
            | "incomplete"
            | "completed"
            | "unverified"
            | "NotSent"
            | "Sent"
            | "Ambiguous"
            | "normal_close"
            | "peer_closed"
            | "invalid_event_json"
            | "reset_without_closing_handshake"
            | "unexpected_eof"
            | "connection_reset"
            | "idle_timeout"
            | "outbound_transport_error"
            | "tcp_reset"
            | "http_required"
            | "explicit_websocket_warmup"
            | "websocket_new_chain"
            | "exact_websocket_continuation"
            | "persisted_continuation"
            | "external_unknown"
            | "new_chain"
            | "http_large_request"
            | "ws_reused"
            | "ws_connected_fast"
            | "ws_exact_required"
            | "ws_required"
            | "http2_ws_budget_exhausted"
            | "http2_breaker_open"
            | "http2_pool_unavailable"
    )
    .then_some(value)
}
