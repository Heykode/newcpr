//! Shared optional filters for usage, analytics and error investigations.

use super::*;

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestSearchQuery {
    pub account_ids: Option<String>,
    pub account_search: Option<String>,
    pub group_id: Option<String>,
    pub requested_model: Option<String>,
    pub upstream_model: Option<String>,
    pub upstream_mode: Option<String>,
    pub client_transport: Option<String>,
    pub upstream_transport: Option<String>,
    #[serde(default, deserialize_with = "query_number")]
    pub client_status_code: Option<i64>,
    #[serde(default, deserialize_with = "query_number")]
    pub upstream_status_code: Option<i64>,
    pub client_ip: Option<String>,
    #[serde(default, deserialize_with = "query_number")]
    pub min_latency_ms: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    pub max_latency_ms: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    pub min_first_token_ms: Option<u64>,
    #[serde(default, deserialize_with = "query_number")]
    pub max_first_token_ms: Option<u64>,
    pub cache_match: Option<String>,
    pub failure_kind: Option<String>,
    pub error_code: Option<String>,
    pub error_phase: Option<String>,
    pub recovery: Option<String>,
    pub error_scope: Option<String>,
}

// Flattened URL fields are buffered as strings by serde; JSON tests use numbers.
fn query_number<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + std::str::FromStr,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Value<T> {
        Number(T),
        Text(String),
    }
    Option::<Value<T>>::deserialize(deserializer)?
        .map(|value| match value {
            Value::Number(value) => Ok(value),
            Value::Text(value) => value
                .parse()
                .map_err(|_| serde::de::Error::custom("invalid numeric filter")),
        })
        .transpose()
}

impl RequestSearchQuery {
    pub fn filter(&self) -> Result<domain::RequestSearchFilter, WireValidationError> {
        let account_ids: Vec<String> = self
            .account_ids
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .collect();
        if account_ids.len() > 50 {
            return Err(WireValidationError::new("accountIds"));
        }
        for (field, value) in [
            ("accountSearch", &self.account_search),
            ("groupId", &self.group_id),
            ("requestedModel", &self.requested_model),
            ("upstreamModel", &self.upstream_model),
            ("failureKind", &self.failure_kind),
            ("errorCode", &self.error_code),
            ("errorPhase", &self.error_phase),
        ] {
            if value
                .as_deref()
                .is_some_and(|v| v.len() > 256 || v.chars().any(char::is_control))
            {
                return Err(WireValidationError::new(field));
            }
        }
        if account_ids
            .iter()
            .any(|id| id.len() > 256 || id.chars().any(char::is_control))
        {
            return Err(WireValidationError::new("accountIds"));
        }
        for (field, value, allowed) in [
            (
                "upstreamMode",
                &self.upstream_mode,
                &["excel", "codex", "unknown"][..],
            ),
            (
                "clientTransport",
                &self.client_transport,
                &["http", "http_sse", "websocket"][..],
            ),
            (
                "upstreamTransport",
                &self.upstream_transport,
                &["http", "http_sse", "websocket", "unknown"][..],
            ),
            (
                "cacheMatch",
                &self.cache_match,
                &["hit", "miss", "unknown"][..],
            ),
            (
                "recovery",
                &self.recovery,
                &["recovered", "unrecovered"][..],
            ),
            (
                "errorScope",
                &self.error_scope,
                &["requests", "events", "all"][..],
            ),
        ] {
            if value
                .as_deref()
                .is_some_and(|v| !v.is_empty() && !allowed.contains(&v))
            {
                return Err(WireValidationError::new(field));
            }
        }
        for (field, min, max) in [
            ("latencyMs", self.min_latency_ms, self.max_latency_ms),
            (
                "firstTokenMs",
                self.min_first_token_ms,
                self.max_first_token_ms,
            ),
        ] {
            if min.zip(max).is_some_and(|(min, max)| min > max)
                || min.is_some_and(|v| v > i64::MAX as u64)
                || max.is_some_and(|v| v > i64::MAX as u64)
            {
                return Err(WireValidationError::new(field));
            }
        }
        let client_ip = non_empty(self.client_ip.clone());
        if client_ip
            .as_deref()
            .is_some_and(|ip| ip.parse::<std::net::IpAddr>().is_err())
        {
            return Err(WireValidationError::new("clientIp"));
        }
        Ok(domain::RequestSearchFilter {
            account_ids,
            client_ip,
            account_search: non_empty(self.account_search.clone()),
            group_id: non_empty(self.group_id.clone()),
            requested_model: non_empty(self.requested_model.clone()),
            upstream_model: non_empty(self.upstream_model.clone()),
            upstream_mode: non_empty(self.upstream_mode.clone()),
            client_transport: non_empty(self.client_transport.clone()),
            upstream_transport: non_empty(self.upstream_transport.clone()),
            client_status_code: parse_status(self.client_status_code)?,
            upstream_status_code: parse_status(self.upstream_status_code)?,
            min_latency_ms: self.min_latency_ms,
            max_latency_ms: self.max_latency_ms,
            min_first_token_ms: self.min_first_token_ms,
            max_first_token_ms: self.max_first_token_ms,
            cache_match: non_empty(self.cache_match.clone()),
            failure_kind: non_empty(self.failure_kind.clone()),
            error_code: non_empty(self.error_code.clone()),
            error_phase: non_empty(self.error_phase.clone()),
            recovery: non_empty(self.recovery.clone()),
            error_scope: non_empty(self.error_scope.clone()),
        })
    }
}
