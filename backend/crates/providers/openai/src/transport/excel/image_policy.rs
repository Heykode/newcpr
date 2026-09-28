//! Excel-only adaptation of Sub2API #190. Persistent state contains digests only.

use super::{
    ExcelRequestError,
    images::{ImageLimits, validate_with_limits},
};
use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::ProviderReplayPort,
    routing::{ExcelImageLimitPolicy, RequestTuning},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

const STORE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    count: usize,
    digest: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    prefix: Progress,
    split: usize,
    window: Progress,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    progress: Progress,
    checkpoints: Vec<Checkpoint>,
    warned: bool,
}

pub(crate) struct ImagePolicy {
    relay: Arc<super::image_relay::ImageRelay>,
    origin: Option<String>,
    tuning: RequestTuning,
    store: Arc<dyn ProviderReplayPort>,
    key: String,
    previous: Option<OpaqueProviderData>,
    saved: Saved,
    pub(crate) source: Map<String, Value>,
    pub(crate) split: Option<usize>,
    pub(crate) explicit_compact: bool,
}

fn error(kind: &'static str) -> ExcelRequestError {
    ExcelRequestError::ImagePolicy(kind)
}

// Reference #190's strict transport compatibility check, not an authorization gate.
fn supported_client(ua: &str) -> bool {
    const CLIENTS: &[&str] = &[
        "codex_cli_rs",
        "codex-tui",
        "codex_vscode",
        "codex_vscode_copilot",
        "codex_app",
        "codex_chatgpt_desktop",
        "codex_atlas",
        "codex_exec",
        "codex_sdk_ts",
    ];
    let ua = ua.trim().to_ascii_lowercase();
    if ua.starts_with("codex ")
        || CLIENTS.iter().any(|name| {
            ua.strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    {
        return true;
    }
    ua.rsplit_once('(')
        .and_then(|(_, trailer)| trailer.split_once(')'))
        .map(|(inner, _)| inner.split(';').next().unwrap_or("").trim())
        .is_some_and(|name| CLIENTS.contains(&name) || name.starts_with("codex "))
}
fn progress(input: &[Value]) -> Progress {
    Progress {
        count: input.len(),
        digest: hex::encode(Sha256::digest(
            serde_json::to_vec(input).expect("JSON values serialize"),
        )),
    }
}
fn matches(input: &[Value], p: &Progress) -> bool {
    p.count <= input.len() && progress(&input[..p.count]).digest == p.digest
}

pub(crate) fn count(input: &[Value]) -> usize {
    input
        .iter()
        .filter_map(|v| match v["type"].as_str() {
            None | Some("message" | "agent_message") => v.get("content"),
            Some("function_call_output" | "custom_tool_call_output") => v.get("output"),
            _ => None,
        })
        .filter_map(Value::as_array)
        .flatten()
        .filter(|v| v["type"] == "input_image")
        .count()
}

fn reconcile(
    mut input: Vec<Value>,
    checkpoints: &[Checkpoint],
) -> Result<Vec<Value>, ExcelRequestError> {
    for cp in checkpoints {
        let n = cp.prefix.count;
        let w = cp.window.count;
        if n == 0 || cp.split == 0 || cp.split >= n || w == 0 {
            return Err(error("invalid_checkpoint"));
        }
        if !matches(&input, &cp.prefix) || input.len() == n {
            continue;
        }
        if !matches(&input[n..], &cp.window) {
            return Err(error("history_window_mismatch"));
        }
        input = input[n..n + w]
            .iter()
            .chain(&input[cp.split..n])
            .chain(&input[n + w..])
            .cloned()
            .collect();
    }
    Ok(input)
}

fn split(input: &[Value], previous: &Progress) -> Result<usize, ExcelRequestError> {
    if previous.count > 0 && previous.count < input.len() && matches(input, previous) {
        return Ok(previous.count);
    }
    let mut i = input.len();
    while i > 0
        && input[i - 1]["role"] == "user"
        && matches!(input[i - 1]["type"].as_str(), None | Some("message"))
    {
        i -= 1;
    }
    if i < input.len() {
        return Ok(i);
    }
    let mut needed = BTreeSet::new();
    while i > 0
        && matches!(
            input[i - 1]["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        )
    {
        let id = input[i - 1]["call_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(error("ambiguous_tool_batch"))?;
        if !needed.insert(id) {
            return Err(error("ambiguous_tool_batch"));
        }
        i -= 1;
    }
    if needed.is_empty() {
        return Err(error("history_boundary_unavailable"));
    }
    while i > 0 && !needed.is_empty() {
        let item = &input[i - 1];
        if item["type"] == "reasoning" {
            i -= 1;
            continue;
        }
        if !matches!(
            item["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        ) || !item["call_id"].as_str().is_some_and(|id| needed.remove(id))
        {
            return Err(error("ambiguous_tool_batch"));
        }
        i -= 1;
    }
    if !needed.is_empty() {
        return Err(error("incomplete_tool_batch"));
    }
    Ok(i)
}

pub(crate) async fn prepare(
    store: Arc<dyn ProviderReplayPort>,
    owner: &str,
    session: Option<&str>,
    ua: Option<&str>,
    source: &mut Map<String, Value>,
    tuning: RequestTuning,
    relay: Arc<super::image_relay::ImageRelay>,
) -> Result<Option<Arc<ImagePolicy>>, ExcelRequestError> {
    if tuning.excel_image_limit_policy == ExcelImageLimitPolicy::Off {
        return Ok(None);
    }
    let Some(original) = source.get("input").and_then(Value::as_array) else {
        return Ok(None);
    };
    let limits = ImageLimits::from(tuning);
    // Raw history still pays the full byte/format budget. Only count may be reduced later.
    super::images::validate_raw_budget(source, limits)?;
    validate_with_limits(
        source,
        true,
        ImageLimits {
            count: usize::MAX,
            ..limits
        },
    )?;
    let compact = original.iter().any(|v| v["type"] == "compaction_trigger");
    let n = count(original);
    let needs_state = !compact
        && match tuning.excel_image_limit_policy {
            ExcelImageLimitPolicy::AutoCompact => n > limits.count,
            ExcelImageLimitPolicy::Warn => {
                n >= limits
                    .count
                    .saturating_sub(tuning.excel_image_warning_remaining as usize)
            }
            ExcelImageLimitPolicy::Off => false,
        };
    let Some(session) = session.filter(|s| !s.is_empty()) else {
        return if needs_state {
            Err(error("state_unavailable"))
        } else {
            Ok(None)
        };
    };
    let key = hex::encode(Sha256::digest(
        json!([
            "excel-image-policy-v1",
            owner,
            session,
            tuning.excel_image_limit_policy,
            limits.count,
            tuning.excel_image_warning_remaining,
            tuning.excel_image_compact_reserve
        ])
        .to_string(),
    ));
    let previous = match tokio::time::timeout(STORE_TIMEOUT, store.read_image_policy(&key)).await {
        Ok(Ok(value)) => value,
        _ if compact => return Ok(None),
        _ => return Err(error("state_unavailable")),
    };
    let saved: Saved = previous
        .as_ref()
        .map(|v| serde_json::from_value(Value::Object(v.expose_to_provider().clone())))
        .transpose()
        .map_err(|_| error("invalid_checkpoint"))?
        .unwrap_or_default();
    if saved.checkpoints.len() > 16 {
        return Err(error("invalid_checkpoint"));
    }
    let input = reconcile(original.clone(), &saved.checkpoints)?;
    source.insert("input".into(), input.clone().into());
    let n = count(&input);
    let mut state = ImagePolicy {
        origin: relay.relay_origin(),
        relay,
        tuning,
        store,
        key,
        previous,
        saved,
        source: source.clone(),
        split: None,
        explicit_compact: compact,
    };
    if compact {
        return Ok(Some(Arc::new(state)));
    }
    match tuning.excel_image_limit_policy {
        ExcelImageLimitPolicy::Warn => {
            let reserve = tuning.excel_image_compact_reserve as usize;
            let warning = tuning.excel_image_warning_remaining as usize;
            if reserve == 0 || reserve >= warning || warning >= limits.count {
                return Err(error("invalid_configuration"));
            }
            if n > limits.count - reserve {
                return Err(error("limit_reached"));
            }
            if n >= limits.count - warning && !state.saved.warned {
                // Competing requests must not both claim the first warning.
                state.saved.warned = true;
                match state.save(&state.saved).await {
                    Ok(()) => {
                        return Err(ExcelRequestError::ImageWarning {
                            remaining: limits.count - reserve - n,
                        });
                    }
                    Err(ExcelRequestError::ImagePolicy("state_conflict")) => {
                        let winner = tokio::time::timeout(
                            STORE_TIMEOUT,
                            state.store.read_image_policy(&state.key),
                        )
                        .await
                        .map_err(|_| error("state_unavailable"))?
                        .map_err(|_| error("state_unavailable"))?;
                        let saved: Saved = winner
                            .as_ref()
                            .map(|v| {
                                serde_json::from_value(Value::Object(
                                    v.expose_to_provider().clone(),
                                ))
                            })
                            .transpose()
                            .map_err(|_| error("invalid_checkpoint"))?
                            .ok_or(error("state_conflict"))?;
                        if !saved.warned {
                            return Err(error("state_conflict"));
                        }
                        state.previous = winner;
                        state.saved = saved;
                    }
                    Err(failure) => return Err(failure),
                }
            }
        }
        ExcelImageLimitPolicy::AutoCompact if n > limits.count => {
            let supported = ua.is_some_and(supported_client);
            if !supported {
                return Err(error("unsupported_client"));
            }
            let boundary = split(&input, &state.saved.progress)?;
            if count(&input[boundary..]) > limits.count {
                return Err(error("new_batch_too_large"));
            }
            if boundary == 0 || count(&input[..boundary]) > limits.count {
                return Err(error("history_too_large"));
            }
            if state.saved.checkpoints.len() >= 16 {
                return Err(error("checkpoint_limit"));
            }
            state.split = Some(boundary);
        }
        _ => {}
    }
    Ok(Some(Arc::new(state)))
}

impl ImagePolicy {
    pub(crate) async fn stage(
        &self,
        mut body: Map<String, Value>,
    ) -> Result<
        (
            Map<String, Value>,
            Option<Arc<super::image_relay::ImageLease>>,
        ),
        ExcelRequestError,
    > {
        let Some(origin) = self
            .origin
            .clone()
            .filter(|_| super::images::has_user_inline(&body))
        else {
            return Ok((body, None));
        };
        let relay = Arc::clone(&self.relay);
        let tuning = self.tuning;
        let scope = self.key.clone();
        tokio::task::spawn_blocking(move || {
            let lease = relay.stage_with_origin(&mut body, tuning, &scope, &origin)?;
            Ok((body, lease))
        })
        .await
        .map_err(|_| ExcelRequestError::ImageRelay)?
    }
    async fn save(&self, saved: &Saved) -> Result<(), ExcelRequestError> {
        let value = serde_json::to_value(saved).map_err(|_| error("invalid_checkpoint"))?;
        let payload = OpaqueProviderData::new(
            value
                .as_object()
                .cloned()
                .ok_or(error("invalid_checkpoint"))?,
        );
        match tokio::time::timeout(
            STORE_TIMEOUT,
            self.store
                .compare_exchange_image_policy(&self.key, self.previous.as_ref(), &payload),
        )
        .await
        {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(error("state_conflict")),
            _ => Err(error("state_unavailable")),
        }
    }

    pub(crate) fn input(&self) -> &[Value] {
        self.source["input"].as_array().expect("validated history")
    }

    pub(crate) async fn checkpoint(&self, window: &[Value]) -> Result<(), ExcelRequestError> {
        let mut saved = self.saved.clone();
        saved.progress = progress(self.input());
        saved.checkpoints.push(Checkpoint {
            prefix: saved.progress.clone(),
            split: self.split.ok_or(error("history_boundary_unavailable"))?,
            window: progress(window),
        });
        self.save(&saved).await
    }

    pub(crate) async fn finish(&self) {
        if self.split.is_some() {
            return;
        } // The pre-delivery checkpoint already committed.
        let mut saved = self.saved.clone();
        saved.progress = progress(self.input());
        if self.explicit_compact {
            saved.checkpoints.clear();
            saved.warned = false;
        }
        let _ = self.save(&saved).await;
    }
}

pub(crate) fn compact_window(response: &Value) -> Result<Vec<Value>, ExcelRequestError> {
    if response["status"] != "completed" {
        return Err(error("compaction_incomplete"));
    }
    let items = response["output"]
        .as_array()
        .ok_or(error("compaction_missing_window"))?;
    let mut markers = 0;
    for item in items {
        match item["type"].as_str() {
            Some("compaction")
                if item["encrypted_content"]
                    .as_str()
                    .is_some_and(|v| !v.trim().is_empty()) =>
            {
                markers += 1
            }
            Some("message") => {}
            _ => return Err(error("compaction_invalid_window")),
        }
    }
    if markers != 1 {
        return Err(error("compaction_invalid_window"));
    }
    Ok(items.clone())
}
