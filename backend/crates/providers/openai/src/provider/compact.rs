//! Explicit compaction uses the account-bound HTTP JSON execution lifecycle.

use gateway_core::{
    metering::{CalculatedCost, Usage},
    operation::CompactRequest,
};

use super::*;
use crate::transport::{
    endpoints::CODEX_RESPONSES_COMPACT_PATH,
    usage::{OpenAiBillingUsage, openai_billing_breakdown},
};

impl CodexProvider {
    pub(super) async fn execute_compact(
        &self,
        compact: &CompactRequest,
        candidate: &ProviderCandidate,
        context: AttemptContext,
    ) -> Result<ProviderStream, ProviderError> {
        let Some(model) = candidate.upstream_model() else {
            return Err(compact_error(
                ProviderErrorKind::InvalidRequest,
                UpstreamSendState::NotSent,
                "compact requires model-scoped routing",
            ));
        };
        let body = compact_body(compact, model)?;
        let session_affinity = crate::credential::derive_codex_compact_session_affinity(
            compact.payload(),
            context.client_api_key_ref(),
        );
        self.execute_raw_json_endpoint(
            context,
            RawJsonEndpointRequest {
                response_origin: self.compact_url.clone(),
                endpoint_path: CODEX_RESPONSES_COMPACT_PATH,
                body,
                image_turn_id: None,
                turn_metadata: None,
                session_affinity,
                upstream_model: Some(model.clone()),
            },
        )
        .await
    }
}

fn compact_body(compact: &CompactRequest, model: &UpstreamModelId) -> Result<Bytes, ProviderError> {
    let invalid = || {
        compact_error(
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::NotSent,
            "compact requires an OpenAI JSON object with model and input",
        )
    };
    if compact.payload().protocol() != PROVIDER_NAME {
        return Err(invalid());
    }
    let mut body: Map<String, Value> =
        serde_json::from_slice(compact.payload().body()).map_err(|_| invalid())?;
    // Defense in depth for internal callers that bypass the HTTP decoder.
    if body
        .get("previous_response_id")
        .is_some_and(|value| !value.is_null())
    {
        return Err(compact_error(
            ProviderErrorKind::InvalidRequest,
            UpstreamSendState::NotSent,
            "previous_response_id is not supported for compact; send the complete input history",
        ));
    }
    if !matches!(body.get("input"), Some(Value::String(_) | Value::Array(_)))
        || body
            .get("model")
            .and_then(Value::as_str)
            .is_none_or(|model| model.trim().is_empty())
        || ["stream", "use_websocket", "background"]
            .iter()
            .any(|field| body.contains_key(*field))
    {
        return Err(invalid());
    }
    if body.get("model").and_then(Value::as_str) == Some(model.as_str()) {
        return Ok(compact.payload().body().clone());
    }
    body.insert("model".to_owned(), Value::String(model.as_str().to_owned()));
    serde_json::to_vec(&body)
        .map(Bytes::from)
        .map_err(|_| invalid())
}

/// Validate only the result envelope; compaction items remain opaque wire bytes.
pub(super) fn compact_response_metering(
    request: &[u8],
    response: &[u8],
    model: Option<&UpstreamModelId>,
) -> Result<(Option<Usage>, Option<CalculatedCost>), ProviderError> {
    let invalid = || {
        compact_error(
            ProviderErrorKind::Protocol,
            UpstreamSendState::Sent,
            "upstream compact response is missing a valid compaction output",
        )
    };
    let body: Value = serde_json::from_slice(response).map_err(|_| invalid())?;
    if body.get("error").is_some_and(|error| !error.is_null())
        || !body
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("compaction")
                        && item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty())
                })
            })
    {
        return Err(invalid());
    }
    let Some(raw) = body.get("usage").filter(|value| value.is_object()) else {
        return Ok((None, None));
    };
    let usage = Usage {
        input_tokens: raw.get("input_tokens").and_then(Value::as_u64),
        output_tokens: raw.get("output_tokens").and_then(Value::as_u64),
        cached_tokens: raw
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64),
        cache_write_tokens: raw
            .pointer("/input_tokens_details/cache_write_tokens")
            .and_then(Value::as_u64),
        reasoning_tokens: raw
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64),
        total_tokens: raw.get("total_tokens").and_then(Value::as_u64),
        ..Usage::new()
    };
    // Unknown counters must not become zero-valued billing facts or guessed cost.
    let cost = (|| {
        let billing = OpenAiBillingUsage::new(
            usage.input_tokens?,
            usage.output_tokens?,
            usage.cached_tokens?,
            usage.cache_write_tokens?,
        );
        let request: Value = serde_json::from_slice(request).ok()?;
        let tier = body
            .get("service_tier")
            .or_else(|| request.get("service_tier"))
            .and_then(Value::as_str);
        Some(openai_billing_breakdown(model?.as_str(), billing, tier)?.calculated_cost())
    })();
    Ok((Some(usage), cost))
}

fn compact_error(
    kind: ProviderErrorKind,
    send_state: UpstreamSendState,
    message: &'static str,
) -> ProviderError {
    provider_error(kind, send_state).with_client_visible_upstream_error(
        ClientVisibleUpstreamError::new(
            message,
            Some("compact_request_failed".to_owned()),
            Some(
                if kind == ProviderErrorKind::InvalidRequest {
                    "invalid_request_error"
                } else {
                    "server_error"
                }
                .to_owned(),
            ),
        ),
    )
}
