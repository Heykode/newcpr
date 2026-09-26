//! Account-selected billing classification; upstream totals are never increased.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use serde_json::{Value, json};

const WRITE_PATHS: &[&str] = &[
    "/input_tokens_details/cache_write_tokens",
    "/prompt_tokens_details/cache_write_tokens",
    "/input_tokens_details/cache_creation_tokens",
    "/prompt_tokens_details/cache_creation_tokens",
    "/cache_write_tokens",
    "/cache_creation_input_tokens",
    "/cache_write_input_tokens",
    "/cache_creation_tokens",
    "/cache_creation/ephemeral_5m_input_tokens",
    "/cache_creation/ephemeral_1h_input_tokens",
];
const OTHER_PATHS: &[&str] = &[
    "/input_tokens",
    "/prompt_tokens",
    "/output_tokens",
    "/completion_tokens",
    "/total_tokens",
    "/input_tokens_details/cached_tokens",
    "/prompt_tokens_details/cached_tokens",
    "/output_tokens_details/reasoning_tokens",
];

#[derive(Clone, Default)]
pub(crate) struct ExcelUsagePolicy {
    as_input: bool,
    original: Arc<Mutex<Option<Value>>>,
    repair_failure: Arc<Mutex<Option<Value>>>,
    repairing: Arc<AtomicBool>,
}

impl ExcelUsagePolicy {
    pub(super) fn clear_repair_usage(&self) {
        self.repairing.store(false, Ordering::Relaxed);
        self.repair_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    pub(super) fn record_repair_usage(&self, response: &Value, usage: &Value) {
        self.repairing.store(true, Ordering::Relaxed);
        let mut record = json!({"usage":usage});
        for field in ["id", "model", "service_tier"] {
            if let Some(value) = response.get(field) {
                record[field] = value.clone();
            }
        }
        *self
            .repair_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(record);
    }

    pub(crate) fn repair_started(&self) -> bool {
        self.repairing.load(Ordering::Relaxed)
    }

    pub(crate) fn take_failed_repair_usage(&self) -> Option<Value> {
        let mut record = self
            .repair_failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        self.normalize(&mut record);
        Some(record)
    }

    pub(crate) fn new(as_input: bool) -> Self {
        Self {
            as_input,
            ..Self::default()
        }
    }

    pub(crate) fn metadata(&self) -> Value {
        json!({
            "cacheCreationAsInput": self.as_input,
            "upstreamUsage": *self.original.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        })
    }

    pub(crate) fn normalize(&self, envelope: &mut Value) {
        for root in ["/usage", "/response/usage"] {
            let Some(usage) = envelope.pointer_mut(root).filter(|value| value.is_object()) else {
                continue;
            };
            let mut original = serde_json::Map::new();
            for path in OTHER_PATHS.iter().chain(WRITE_PATHS) {
                if let Some(value) = usage.pointer(path).and_then(Value::as_u64) {
                    original.insert((*path).to_owned(), value.into());
                }
            }
            // Preserve only typed measurements, never arbitrary upstream fields.
            if !original.is_empty() {
                let mut recorded = self
                    .original
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let values = recorded.get_or_insert_with(|| json!({}));
                if let Some(values) = values.as_object_mut() {
                    values.extend(original);
                }
            }
            let written = WRITE_PATHS[..8]
                .iter()
                .find_map(|path| usage.pointer(path).and_then(Value::as_u64))
                .or_else(|| {
                    let short = usage.pointer(WRITE_PATHS[8]).and_then(Value::as_u64);
                    let long = usage.pointer(WRITE_PATHS[9]).and_then(Value::as_u64);
                    (short.is_some() || long.is_some())
                        .then(|| short.unwrap_or(0).checked_add(long.unwrap_or(0)))
                        .flatten()
                });
            if self.as_input {
                for path in WRITE_PATHS {
                    if let Some(value) = usage.pointer_mut(path).filter(|value| value.is_u64()) {
                        *value = 0.into();
                    }
                }
            }
            // The canonical decoder and compact accounting share this field.
            // Keep aliases for clients; add a canonical measurement only if absent.
            if let Some(written) = written
                && usage.pointer(WRITE_PATHS[0]).is_none()
                && let Some(fields) = usage.as_object_mut()
            {
                let details = fields
                    .entry("input_tokens_details")
                    .or_insert_with(|| json!({}));
                if let Some(details) = details.as_object_mut() {
                    details.insert(
                        "cache_write_tokens".into(),
                        if self.as_input { 0 } else { written }.into(),
                    );
                }
            }
        }
    }

    pub(crate) fn project(&self, envelope: &mut Value) {
        // Compact reads the original completion, not the transformed SSE event.
        // Use a detached recorder so projection cannot replace upstream evidence.
        Self::new(self.as_input).normalize(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::usage::{OpenAiBillingUsage, openai_billing_breakdown};

    #[test]
    fn aliases_reach_canonical_accounting_and_partial_usage_keeps_original_evidence() {
        for as_input in [false, true] {
            let policy = ExcelUsagePolicy::new(as_input);
            let mut envelope = json!({"response":{"usage":{
                "input_tokens":1000,"output_tokens":50,
                "input_tokens_details":{"cached_tokens":100},
                "cache_creation":{"ephemeral_5m_input_tokens":75,"ephemeral_1h_input_tokens":125}
            }}});
            policy.normalize(&mut envelope);
            let usage =
                gateway_protocol::openai::events::extract_usage(&envelope["response"]).unwrap();
            assert_eq!(usage.cache_write_tokens, if as_input { 0 } else { 200 });
            assert_eq!(usage.input_tokens, 1000);
            assert_eq!(usage.cached_tokens, 100);
            policy.normalize(&mut json!({"usage":{"output_tokens":51}}));
            assert_eq!(
                policy.metadata()["upstreamUsage"]["/cache_creation/ephemeral_1h_input_tokens"],
                125
            );
            assert_eq!(policy.metadata()["upstreamUsage"]["/output_tokens"], 51);
        }
    }

    #[test]
    fn account_option_changes_classification_without_changing_totals_or_original() {
        let original = json!({"usage":{
            "input_tokens":1000,"output_tokens":50,"total_tokens":1050,
            "input_tokens_details":{"cached_tokens":100,"cache_write_tokens":200},
            "output_tokens_details":{"reasoning_tokens":20}
        }});
        for enabled in [false, true] {
            let policy = ExcelUsagePolicy::new(enabled);
            let mut response = original.clone();
            policy.normalize(&mut response);
            assert_eq!(response["usage"]["input_tokens"], 1000);
            assert_eq!(response["usage"]["output_tokens"], 50);
            assert_eq!(response["usage"]["total_tokens"], 1050);
            assert_eq!(
                response["usage"]["input_tokens_details"]["cached_tokens"],
                100
            );
            assert_eq!(
                response["usage"]["input_tokens_details"]["cache_write_tokens"],
                if enabled { 0 } else { 200 }
            );
            assert_eq!(
                policy.metadata()["upstreamUsage"]["/input_tokens_details/cache_write_tokens"],
                200
            );
            let billing = openai_billing_breakdown(
                "gpt-6-astra",
                OpenAiBillingUsage::new(1000, 50, 100, if enabled { 0 } else { 200 }),
                None,
            )
            .unwrap();
            assert_eq!(
                billing.input_amount().amount(),
                if enabled { "0.009" } else { "0.007" }.parse().unwrap()
            );
            assert_eq!(
                billing.cache_write_amount().amount(),
                if enabled { "0" } else { "0.0025" }.parse().unwrap()
            );
            if !enabled {
                assert_eq!(response, original);
            }
        }
    }

    #[test]
    fn all_supported_aliases_and_nested_stream_usage_are_projected_without_mutating_raw() {
        let source = json!({"response":{"usage":{
            "input_tokens":1000,"output_tokens":50,
            "input_tokens_details":{"cache_write_tokens":200,"cache_creation_tokens":200,"cached_tokens":100},
            "prompt_tokens_details":{"cache_write_tokens":200,"cache_creation_tokens":200},
            "cache_write_tokens":200,"cache_creation_input_tokens":200,
            "cache_write_input_tokens":200,"cache_creation_tokens":200,
            "cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":100},
            "unknown_secret":"must-not-be-recorded"
        }}});
        let policy = ExcelUsagePolicy::new(true);
        let mut projected = source.clone();
        policy.normalize(&mut projected);
        for path in WRITE_PATHS {
            assert_eq!(
                projected["response"]["usage"].pointer(path),
                Some(&json!(0))
            );
        }
        let evidence = policy.metadata();
        assert!(!evidence.to_string().contains("unknown_secret"));
        assert!(!evidence.to_string().contains("must-not-be-recorded"));
        policy.project(&mut projected);
        assert_eq!(policy.metadata(), evidence);
        assert_eq!(
            source["response"]["usage"]["cache_creation_input_tokens"],
            200
        );
        assert_eq!(
            projected["response"]["usage"]["input_tokens_details"]["cached_tokens"],
            100
        );
    }
}
