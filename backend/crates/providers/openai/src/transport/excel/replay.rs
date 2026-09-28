use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{MAX_PROVIDER_REPLAY_BYTES, ProviderReplayPort},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{ClientTools, ExcelRequestError, request::message, tools::canonical_history_call};
use futures::StreamExt;

const STORE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_SNAPSHOT_DECODED_BYTES: usize = 128 * 1024 * 1024;
const SNAPSHOT_ENCODING: &str = "cpr-excel-replay-zstd-v1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayRecord {
    version: u8,
    owner: String,
    conversation: String,
    input: Vec<Value>,
    native_calls: BTreeMap<String, Value>,
    #[serde(default)]
    client_calls: BTreeMap<String, Value>,
    #[serde(default)]
    tools: Option<Value>,
}

#[derive(Clone)]
pub(crate) struct ReplayCapture {
    store: Arc<dyn ProviderReplayPort>,
    record: ReplayRecord,
    persisted: Arc<AtomicBool>,
}

pub(crate) struct RestoredInput {
    pub(crate) input: Vec<Value>,
    pub(crate) conversation: String,
    pub(crate) native_calls: BTreeMap<String, Value>,
    pub(crate) capture: ReplayCapture,
    pub(crate) tools: ClientTools,
}

#[cfg(test)]
pub(crate) async fn restore(
    store: Arc<dyn ProviderReplayPort>,
    owner: String,
    conversation: String,
    previous_response_id: Option<&str>,
    source: &Map<String, Value>,
) -> Result<RestoredInput, ExcelRequestError> {
    restore_scoped(
        store,
        owner,
        conversation,
        previous_response_id,
        source,
        None,
    )
    .await
}

pub(crate) async fn restore_scoped(
    store: Arc<dyn ProviderReplayPort>,
    owner: String,
    conversation: String,
    previous_response_id: Option<&str>,
    source: &Map<String, Value>,
    session: Option<&str>,
) -> Result<RestoredInput, ExcelRequestError> {
    let mut record = if let Some(previous) = previous_response_id {
        let payload = load(store.as_ref(), &key(&owner, "response", previous))
            .await?
            .ok_or(ExcelRequestError::History)?;
        let record = tokio::task::spawn_blocking(move || decode_snapshot(payload))
            .await
            .map_err(|_| ExcelRequestError::History)??;
        if record.owner != owner || record.version != 1 {
            return Err(ExcelRequestError::History);
        }
        record
    } else {
        ReplayRecord {
            version: 1,
            owner,
            conversation,
            input: Vec::new(),
            native_calls: BTreeMap::new(),
            client_calls: BTreeMap::new(),
            tools: None,
        }
    };
    let delta = match source.get("input").ok_or(ExcelRequestError::Input)? {
        Value::String(text) => vec![message("user", text)],
        Value::Array(items) => items.clone(),
        _ => return Err(ExcelRequestError::Input),
    };
    let legacy_catalog = if record.tools.is_none() && previous_response_id.is_some() {
        Some(ClientTools::parse(json!({"input":record.input}).as_object().unwrap())?.catalog())
    } else {
        None
    };
    let (tools, catalog) = super::catalog::resolve(
        store.as_ref(),
        &record.owner,
        session,
        record.tools.as_ref().or(legacy_catalog.as_ref()),
        source,
    )
    .await?;
    record.tools = Some(catalog);
    // Cache lookup is an optimization for complete calls, with one request-wide budget.
    let ids: BTreeSet<_> = delta
        .iter()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
            )
        })
        .filter_map(|item| item["call_id"].as_str().map(str::to_owned))
        .collect();
    let mut receipts = BTreeMap::new();
    {
        let reads = futures::stream::iter(ids)
            .map(|id| {
                let store = Arc::clone(&store);
                let cache_key = key(&record.owner, &record.conversation, &id);
                async move {
                    let receipt = store.read_tool(&cache_key).await.ok().flatten();
                    (id, receipt)
                }
            })
            .buffer_unordered(16);
        tokio::pin!(reads);
        let _ = tokio::time::timeout(STORE_TIMEOUT, async {
            while let Some((id, receipt)) = reads.next().await {
                if let Some(receipt) = receipt {
                    receipts.insert(id, receipt.expose_to_provider().clone());
                }
            }
        })
        .await;
    }
    // previous_response_id explicitly means incremental input; never guess by text similarity.
    for (index, item) in delta.iter().enumerate() {
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some(
                "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
            )
        ) {
            let id = item
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or(ExcelRequestError::Input)?;
            let complete_call = matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            );
            let signature = complete_call
                .then(|| canonical_history_call(item))
                .transpose()?;
            let cached_matches = record.native_calls.contains_key(id)
                && signature
                    .as_ref()
                    .is_none_or(|signature| record.client_calls.get(id) == Some(signature));
            if !cached_matches {
                let payload = receipts.get(id).filter(|payload| {
                    payload.get("owner").and_then(Value::as_str) == Some(&record.owner)
                        && payload.get("conversation").and_then(Value::as_str)
                            == Some(&record.conversation)
                        && signature
                            .as_ref()
                            .is_none_or(|signature| payload.get("client") == Some(signature))
                });
                let native = payload
                    .and_then(|payload| payload.get("item"))
                    .filter(|item| item.get("call_id").and_then(Value::as_str) == Some(id))
                    .cloned();
                let native = match native {
                    Some(native) => native,
                    None if complete_call => tools.rebuild_history_call(item)?,
                    None => {
                        return Err(ExcelRequestError::HistoryInput {
                            input: record.input.len() + index,
                            kind: "missing_complete_tool_call",
                        });
                    }
                };
                record.native_calls.insert(id.into(), native);
            }
            if let Some(signature) = signature {
                record.client_calls.insert(id.into(), signature);
            }
        }
    }
    record.input.extend(delta);
    Ok(RestoredInput {
        input: record.input.clone(),
        conversation: record.conversation.clone(),
        native_calls: record.native_calls.clone(),
        capture: ReplayCapture {
            store,
            record,
            persisted: Default::default(),
        },
        tools,
    })
}

impl ReplayCapture {
    pub(super) fn native_calls(&self) -> &BTreeMap<String, Value> {
        &self.record.native_calls
    }

    pub(crate) fn is_persisted(&self) -> bool {
        self.persisted.load(Ordering::Acquire)
    }

    pub(super) fn image_cache(
        &self,
        endpoint: &str,
        picture: &super::images::Picture,
    ) -> super::image_cache::AssetCache {
        super::image_cache::AssetCache::new(
            Arc::clone(&self.store),
            &self.record.owner,
            endpoint,
            picture,
        )
    }

    pub(crate) async fn commit(
        &self,
        response: &Value,
        tools: &ClientTools,
    ) -> Result<(), ExcelRequestError> {
        if response.get("status").and_then(Value::as_str) != Some("completed") {
            return Err(ExcelRequestError::History);
        }
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(ExcelRequestError::History)?;
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or(ExcelRequestError::History)?;
        let mut record = self.record.clone();
        let output_start = record.input.len();
        let mut calls = Vec::new();
        for item in output {
            let mut replay_item = item.clone();
            if matches!(
                item.get("type").and_then(Value::as_str),
                Some("function_call" | "custom_tool_call")
            ) {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .ok_or(ExcelRequestError::History)?;
                let converted = tools.convert_call(item)?;
                let signature = canonical_history_call(&converted)?;
                if !matches!(
                    item.get("name").and_then(Value::as_str),
                    Some(
                        "run_officejs"
                            | "functions.run_officejs"
                            | "update_plan"
                            | "functions.update_plan"
                    )
                ) {
                    replay_item = tools.rebuild_history_call(&converted)?;
                }
                record
                    .native_calls
                    .insert(call_id.into(), replay_item.clone());
                record
                    .client_calls
                    .insert(call_id.into(), signature.clone());
                calls.push((call_id.to_owned(), replay_item.clone(), signature));
            }
            record.input.push(replay_item);
        }
        prune_compacted_history(&mut record, output_start);
        // Small receipts survive even when this full response cannot fit its cache.
        let writes = futures::stream::iter(calls).map(|(call_id, item, signature)| {
            let cache_key = key(&record.owner, &record.conversation, &call_id);
            let store = Arc::clone(&self.store);
            let payload = OpaqueProviderData::new(
                json!({"owner":record.owner,"conversation":record.conversation,"item":item,"client":signature})
                    .as_object().expect("receipt object").clone(),
            );
            async move { let _ = store.write_tool(&cache_key, &payload).await; }
        }).buffer_unordered(16).collect::<Vec<_>>();
        let _ = tokio::time::timeout(STORE_TIMEOUT, writes).await;
        let response_key = key(&record.owner, "response", id);
        let payload = tokio::task::spawn_blocking(move || encode_snapshot(&record))
            .await
            .map_err(|_| ExcelRequestError::History)??;
        let Some(payload) = payload else {
            tracing::debug!("Excel response history exceeds optional cache capacity");
            return Ok(());
        };
        write(self.store.as_ref(), &response_key, payload).await?;
        self.persisted.store(true, Ordering::Release);
        Ok(())
    }
}

fn prune_compacted_history(record: &mut ReplayRecord, output_start: usize) {
    // Only a genuine successful upstream output can replace our stored window.
    // Incoming explicit compact windows are already canonical and stay untouched.
    let Some(index) = record
        .input
        .iter()
        .enumerate()
        .skip(output_start)
        .filter(|(_, item)| {
            item.get("type").and_then(Value::as_str) == Some("compaction")
                && item
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|value| !value.trim().is_empty())
        })
        .map(|(index, _)| index)
        .next_back()
    else {
        return;
    };
    let mut pending = BTreeSet::new();
    for item in &record.input[..index] {
        let Some(id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call" | "custom_tool_call") => {
                pending.insert(id);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                pending.remove(id);
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return;
    }
    // Do not orphan a result or repeated call whose native identity precedes the marker.
    let old_ids: BTreeSet<_> = record.input[..index]
        .iter()
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .collect();
    if record.input[index..]
        .iter()
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .any(|id| old_ids.contains(id))
    {
        return;
    }
    let mut position = 0;
    record.input.retain(|item| {
        let keep = position >= index
            || matches!(
                item.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            )
            || item.get("type").and_then(Value::as_str) == Some("additional_tools");
        position += 1;
        keep
    });
    let retained: BTreeSet<_> = record
        .input
        .iter()
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .collect();
    record
        .native_calls
        .retain(|id, _| retained.contains(id.as_str()));
    record
        .client_calls
        .retain(|id, _| retained.contains(id.as_str()));
}

fn encode_snapshot(record: &ReplayRecord) -> Result<Option<Value>, ExcelRequestError> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let bytes = serde_json::to_vec(record).map_err(|_| ExcelRequestError::History)?;
    if bytes.len() <= MAX_PROVIDER_REPLAY_BYTES {
        return serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ExcelRequestError::History);
    }
    if bytes.len() > MAX_SNAPSHOT_DECODED_BYTES {
        return Ok(None);
    }
    let compressed = zstd::bulk::compress(&bytes, 1).map_err(|_| ExcelRequestError::History)?;
    let payload = json!({
        "encoding": SNAPSHOT_ENCODING,
        "decoded_bytes": bytes.len(),
        "data": STANDARD.encode(compressed),
    });
    if serde_json::to_vec(&payload)
        .map_err(|_| ExcelRequestError::History)?
        .len()
        > MAX_PROVIDER_REPLAY_BYTES
    {
        return Ok(None);
    }
    Ok(Some(payload))
}

fn decode_snapshot(payload: Map<String, Value>) -> Result<ReplayRecord, ExcelRequestError> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    if !payload.contains_key("encoding") {
        return serde_json::from_value(Value::Object(payload))
            .map_err(|_| ExcelRequestError::History);
    }
    if payload.get("encoding").and_then(Value::as_str) != Some(SNAPSHOT_ENCODING) {
        return Err(ExcelRequestError::History);
    }
    let size = payload
        .get("decoded_bytes")
        .and_then(Value::as_u64)
        .filter(|size| *size > 0 && *size <= MAX_SNAPSHOT_DECODED_BYTES as u64)
        .ok_or(ExcelRequestError::History)? as usize;
    let encoded = payload
        .get("data")
        .and_then(Value::as_str)
        .filter(|data| data.len() <= MAX_PROVIDER_REPLAY_BYTES)
        .ok_or(ExcelRequestError::History)?;
    let compressed = STANDARD
        .decode(encoded)
        .map_err(|_| ExcelRequestError::History)?;
    let bytes =
        zstd::bulk::decompress(&compressed, size).map_err(|_| ExcelRequestError::History)?;
    if bytes.len() != size {
        return Err(ExcelRequestError::History);
    }
    serde_json::from_slice(&bytes).map_err(|_| ExcelRequestError::History)
}

fn key(owner: &str, category: &str, id: &str) -> String {
    hex::encode(Sha256::digest(
        json!(["cpr-excel-replay-v1", owner, category, id])
            .to_string()
            .as_bytes(),
    ))
}

async fn load(
    store: &dyn ProviderReplayPort,
    key: &str,
) -> Result<Option<Map<String, Value>>, ExcelRequestError> {
    tokio::time::timeout(STORE_TIMEOUT, store.read(key))
        .await
        .map_err(|_| ExcelRequestError::History)?
        .map_err(|_| ExcelRequestError::History)
        .map(|value| value.map(|value| value.expose_to_provider().clone()))
}

async fn write(
    store: &dyn ProviderReplayPort,
    key: &str,
    value: Value,
) -> Result<(), ExcelRequestError> {
    let Value::Object(fields) = value else {
        return Err(ExcelRequestError::History);
    };
    let payload = OpaqueProviderData::new(fields);
    tokio::time::timeout(STORE_TIMEOUT, store.write(key, &payload))
        .await
        .map_err(|_| ExcelRequestError::History)?
        .map_err(|_| ExcelRequestError::History)
}

#[cfg(test)]
mod tests {
    use super::super::tests::MemoryReplay;
    use super::super::tools::rebuild_history_call;
    use super::*;

    async fn restore(
        store: Arc<dyn ProviderReplayPort>,
        owner: String,
        conversation: String,
        previous: Option<&str>,
        input: &Value,
    ) -> Result<RestoredInput, ExcelRequestError> {
        super::restore(
            store,
            owner,
            conversation,
            previous,
            json!({"input":input}).as_object().unwrap(),
        )
        .await
    }

    #[tokio::test]
    async fn excel_history_rebuild_uses_catalog_and_keeps_cached_native_calls() {
        for custom in [false, true] {
            let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
            let code = "  let price = '$1';\r\n\ttext(price);\n";
            let tool = if custom {
                json!({"type":"custom","name":"exec"})
            } else {
                json!({"type":"function","name":"exec","parameters":{"type":"object",
                    "properties":{"code":{"type":"string"},"timeout":{"type":"integer"}}}})
            };
            let call = if custom {
                json!({"type":"custom_tool_call","name":"exec","namespace":"client",
                    "call_id":"call_fixture","input":code})
            } else {
                json!({"type":"function_call","name":"exec","namespace":"client",
                    "call_id":"call_fixture","arguments":json!({"code":code,"timeout":123}).to_string()})
            };
            let source = json!({"tools":[{"type":"namespace","name":"client","tools":[tool]}],
                "input":[call,{"type":if custom {"custom_tool_call_output"} else {"function_call_output"},
                    "call_id":"call_fixture","output":"recorded"}]});
            let first = super::restore(
                store.clone(),
                "owner".into(),
                "thread".into(),
                None,
                source.as_object().unwrap(),
            )
            .await
            .unwrap();
            let native = &first.native_calls["call_fixture"];
            let outer = super::super::envelope::json_value(&native["arguments"]).unwrap();
            assert_eq!(outer["code"], code);
            assert_eq!(
                outer["summary"],
                if custom {
                    "codex2api.custom/client.exec"
                } else {
                    "codex2api.function_code/client.exec"
                }
            );
            let restored_call = first.tools.convert_call(native).unwrap();
            assert_eq!(
                canonical_history_call(&restored_call).unwrap(),
                canonical_history_call(&call).unwrap()
            );
            let mut cached = native.clone();
            cached["provider_extension"] = "preserve".into();
            first
                .capture
                .commit(
                    &json!({"id":"resp_fixture","status":"completed","output":[cached.clone()]}),
                    &first.tools,
                )
                .await
                .unwrap();
            let next = super::restore(
                store,
                "owner".into(),
                "thread".into(),
                None,
                source.as_object().unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(next.native_calls["call_fixture"], cached);
        }
    }

    #[tokio::test]
    async fn excel_missing_call_diagnostic_contains_position_not_payload() {
        let source = json!({"input":[message("user","continue"),{
            "type":"function_call_output","call_id":"PRIVATE_ID","output":"PRIVATE_RESULT"}]});
        let error = super::restore(
            Arc::new(MemoryReplay::default()),
            "owner".into(),
            "thread".into(),
            None,
            source.as_object().unwrap(),
        )
        .await
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("input[1]"));
        assert!(error.contains("matching complete tool call"));
        assert!(!error.contains("PRIVATE"));
    }

    #[tokio::test]
    async fn compaction_prunes_only_successful_upstream_history_and_preserves_scope() {
        let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
        let tools = ClientTools::parse(
            json!({"tools":[{"type":"function","name":"read"}]})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let initial = restore(
            store.clone(),
            "owner".into(),
            "thread".into(),
            None,
            &json!([
                message("developer", "Keep instructions."),
                message("user", "Remember 521."),
                {"type":"function_call","name":"read","call_id":"old","arguments":"{}"},
                {"type":"function_call_output","call_id":"old","output":"ok"}
            ]),
        )
        .await
        .unwrap();
        let compact =
            json!({"type":"compaction","encrypted_content":"opaque-fixture","id":"cmp_fixture"});
        let next_call = rebuild_history_call(
            &json!({"type":"function_call","name":"read","call_id":"next","arguments":"{}"}),
        )
        .unwrap();
        initial
            .capture
            .commit(
                &json!({
                    "id":"resp_compact","status":"completed","output":[compact, next_call]
                }),
                &tools,
            )
            .await
            .unwrap();
        let next = restore(
            store.clone(),
            "owner".into(),
            "new-anchor".into(),
            Some("resp_compact"),
            &json!([{"type":"function_call_output","call_id":"next","output":"done"}]),
        )
        .await
        .unwrap();
        assert_eq!(next.input.len(), 4);
        assert_eq!(next.input[0], message("developer", "Keep instructions."));
        assert_eq!(next.input[1], compact);
        assert_eq!(next.conversation, "thread");
        assert_eq!(next.native_calls.len(), 1);
        assert!(next.native_calls.contains_key("next"));
        assert!(
            restore(
                store,
                "other".into(),
                "thread".into(),
                Some("resp_compact"),
                &json!("next")
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn compaction_never_discards_pending_tools_or_unverified_input() {
        let compact = json!({"type":"compaction","encrypted_content":"opaque-fixture"});
        let mut record = ReplayRecord {
            version: 1,
            owner: "owner".into(),
            conversation: "thread".into(),
            input: vec![message("user", "keep"), compact.clone()],
            native_calls: BTreeMap::new(),
            client_calls: BTreeMap::new(),
            tools: None,
        };
        prune_compacted_history(&mut record, 2);
        assert_eq!(record.input.len(), 2);
        record.input[1]["encrypted_content"] = "".into();
        prune_compacted_history(&mut record, 1);
        assert_eq!(record.input.len(), 2);
        record.input = vec![
            message("user", "keep"),
            json!({"type":"function_call","call_id":"pending"}),
            compact,
        ];
        prune_compacted_history(&mut record, 2);
        assert_eq!(record.input.len(), 3);
    }

    #[tokio::test]
    async fn excel_replay_is_incremental_and_scoped_to_owner_and_conversation() {
        let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
        let first = restore(
            store.clone(),
            "owner-a".into(),
            "thread-a".into(),
            None,
            &json!("read file"),
        )
        .await
        .unwrap();
        let native = json!({"type":"function_call","name":"run_officejs","id":"fc_native","call_id":"call_fixture",
            "arguments":json!({"code":json!({"name":"read","arguments":{"path":"sample.txt"}}).to_string()}).to_string()});
        first
            .capture
            .commit(
                &json!({"id":"resp_fixture","status":"completed","output":[native]}),
                &ClientTools::parse(
                    json!({"tools":[{"type":"function","name":"read"}]})
                        .as_object()
                        .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let delta =
            json!([{"type":"function_call_output","call_id":"call_fixture","output":"contents"}]);
        let next = restore(
            store.clone(),
            "owner-a".into(),
            "new-anchor".into(),
            Some("resp_fixture"),
            &delta,
        )
        .await
        .unwrap();
        assert_eq!(next.input.len(), 3);
        assert_eq!(next.conversation, "thread-a");
        assert_eq!(next.native_calls["call_fixture"]["name"], "run_officejs");
        assert!(
            restore(
                store.clone(),
                "owner-b".into(),
                "thread-a".into(),
                Some("resp_fixture"),
                &delta
            )
            .await
            .is_err()
        );
        assert!(
            restore(
                store.clone(),
                "owner-a".into(),
                "thread-b".into(),
                None,
                &delta
            )
            .await
            .is_err()
        );
        let complete = json!([first.input[0], {"type":"function_call","name":"read","call_id":"call_fixture","arguments":"{\"path\":\"sample.txt\"}"}, delta[0]]);
        let full = restore(store, "owner-a".into(), "thread-a".into(), None, &complete)
            .await
            .unwrap();
        assert_eq!(full.input.len(), 3);
        assert_eq!(full.native_calls["call_fixture"]["id"], "fc_native");
    }

    #[tokio::test]
    async fn complete_history_over_cache_capacity_still_prepares_and_completes() {
        let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
        let first = restore(
            store.clone(),
            "owner".into(),
            "thread".into(),
            None,
            &json!("x".repeat(MAX_PROVIDER_REPLAY_BYTES + 1)),
        )
        .await
        .unwrap();
        assert_eq!(
            first.input[0]["content"][0]["text"].as_str().unwrap().len(),
            MAX_PROVIDER_REPLAY_BYTES + 1
        );
        first
            .capture
            .commit(
                &json!({"id":"resp_large","status":"completed","output":[]}),
                &ClientTools::default(),
            )
            .await
            .unwrap();
        assert!(first.capture.is_persisted());
        let next = restore(
            store.clone(),
            "owner".into(),
            "thread".into(),
            Some("resp_large"),
            &json!("next"),
        )
        .await
        .unwrap();
        assert_eq!(next.input[0], first.input[0]);
        assert!(
            restore(
                store,
                "other-owner".into(),
                "thread".into(),
                Some("resp_large"),
                &json!("next")
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn complete_tool_history_rebuilds_when_cache_is_unavailable() {
        let store: Arc<dyn ProviderReplayPort> =
            Arc::new(gateway_core::provider_ports::UnavailableProviderReplay);
        let source = json!({
            "tools":[{"type":"function","name":"read"}],
            "input":[
                {"role":"user","content":"read"},
                {"type":"function_call","name":"read","call_id":"call_fixture","arguments":"{\"path\":\"sample.txt\"}"},
                {"type":"function_call_output","call_id":"call_fixture","output":"contents"}
            ]
        });
        let restored = restore_scoped(
            store.clone(),
            "owner".into(),
            "thread".into(),
            None,
            source.as_object().unwrap(),
            Some("thread"),
        )
        .await
        .unwrap();
        assert_eq!(
            restored.native_calls["call_fixture"]["name"],
            "run_officejs"
        );
        assert_eq!(restored.input.len(), 3);
        let missing = json!({"input":[{"type":"function_call_output","call_id":"call_fixture","output":"contents"}]});
        assert!(
            restore_scoped(
                store,
                "owner".into(),
                "thread".into(),
                None,
                missing.as_object().unwrap(),
                None
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn compressed_snapshots_reject_unknown_encoding_truncation_and_bombs() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let data = STANDARD.encode(zstd::bulk::compress(b"{}", 1).unwrap());
        for payload in [
            json!({"encoding":"unknown","decoded_bytes":2,"data":data}),
            json!({"encoding":SNAPSHOT_ENCODING,"decoded_bytes":1,"data":data}),
            json!({"encoding":SNAPSHOT_ENCODING,"decoded_bytes":3,"data":data}),
            json!({"encoding":SNAPSHOT_ENCODING,"decoded_bytes":MAX_SNAPSHOT_DECODED_BYTES + 1,"data":data}),
            json!({"encoding":SNAPSHOT_ENCODING,"decoded_bytes":2,"data":"broken"}),
            json!({"encoding":SNAPSHOT_ENCODING,"decoded_bytes":2,"data":data}),
        ] {
            assert!(decode_snapshot(payload.as_object().unwrap().clone()).is_err());
        }
    }

    #[tokio::test]
    async fn excel_replay_rejects_failed_unknown_and_conflicting_records() {
        let store: Arc<dyn ProviderReplayPort> = Arc::new(MemoryReplay::default());
        let first = restore(
            store.clone(),
            "owner".into(),
            "thread".into(),
            None,
            &json!("hello"),
        )
        .await
        .unwrap();
        assert!(
            first
                .capture
                .commit(
                    &json!({"id":"resp_fixture","status":"failed","output":[]}),
                    &ClientTools::default()
                )
                .await
                .is_err()
        );
        assert!(
            restore(
                store.clone(),
                "owner".into(),
                "thread".into(),
                Some("resp_fixture"),
                &json!("next")
            )
            .await
            .is_err()
        );
        let response = json!({"id":"resp_fixture","status":"completed","output":[message("assistant","hello")]});
        first
            .capture
            .commit(&response, &ClientTools::default())
            .await
            .unwrap();
        first
            .capture
            .commit(&response, &ClientTools::default())
            .await
            .unwrap();
        let other = json!({"id":"resp_fixture","status":"completed","output":[message("assistant","different")]});
        assert!(
            first
                .capture
                .commit(&other, &ClientTools::default())
                .await
                .is_err()
        );
        assert!(
            restore(
                store,
                "owner".into(),
                "thread".into(),
                None,
                &Value::Array(vec![message("user", "x"); 4097])
            )
            .await
            .is_ok()
        );
    }
}
