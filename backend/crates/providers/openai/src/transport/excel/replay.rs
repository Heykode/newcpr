use std::{collections::BTreeMap, sync::Arc, time::Duration};

use gateway_core::{
    account::OpaqueProviderData,
    provider_ports::{MAX_PROVIDER_REPLAY_BYTES, ProviderReplayPort},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    ClientTools, ExcelRequestError,
    request::message,
    tools::{canonical_history_call, rebuild_history_call},
};

const STORE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_NATIVE_CALLS: usize = 512;

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
}

#[derive(Clone)]
pub(crate) struct ReplayCapture {
    store: Arc<dyn ProviderReplayPort>,
    record: ReplayRecord,
}

pub(crate) struct RestoredInput {
    pub(crate) input: Vec<Value>,
    pub(crate) conversation: String,
    pub(crate) native_calls: BTreeMap<String, Value>,
    pub(crate) capture: ReplayCapture,
}

pub(crate) async fn restore(
    store: Arc<dyn ProviderReplayPort>,
    owner: String,
    conversation: String,
    previous_response_id: Option<&str>,
    input: &Value,
) -> Result<RestoredInput, ExcelRequestError> {
    let mut record = if let Some(previous) = previous_response_id {
        let payload = load(store.as_ref(), &key(&owner, "response", previous))
            .await?
            .ok_or(ExcelRequestError::History)?;
        let record: ReplayRecord = serde_json::from_value(Value::Object(payload))
            .map_err(|_| ExcelRequestError::History)?;
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
        }
    };
    let delta = match input {
        Value::String(text) => vec![message("user", text)],
        Value::Array(items) => items.clone(),
        _ => return Err(ExcelRequestError::Input),
    };
    // previous_response_id explicitly means incremental input; never guess by text similarity.
    for item in &delta {
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
                let payload = load(
                    store.as_ref(),
                    &key(&record.owner, &record.conversation, id),
                )
                .await?;
                let payload = payload.as_ref().filter(|payload| {
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
                    None if complete_call => rebuild_history_call(item)?,
                    None => return Err(ExcelRequestError::History),
                };
                record.native_calls.insert(id.into(), native);
            }
            if let Some(signature) = signature {
                record.client_calls.insert(id.into(), signature);
            }
        }
    }
    record.input.extend(delta);
    validate_record(&record)?;
    Ok(RestoredInput {
        input: record.input.clone(),
        conversation: record.conversation.clone(),
        native_calls: record.native_calls.clone(),
        capture: ReplayCapture { store, record },
    })
}

impl ReplayCapture {
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
                    replay_item = rebuild_history_call(&converted)?;
                }
                record
                    .native_calls
                    .insert(call_id.into(), replay_item.clone());
                record
                    .client_calls
                    .insert(call_id.into(), signature.clone());
                calls.push((call_id, replay_item.clone(), signature));
            }
            record.input.push(replay_item);
        }
        validate_record(&record)?;
        for (call_id, item, signature) in calls {
            write(
                    self.store.as_ref(),
                    &key(&record.owner, &record.conversation, call_id),
                    json!({"owner":record.owner,"conversation":record.conversation,"item":item,"client":signature}),
                )
                .await?;
        }
        let payload = serde_json::to_value(&record).map_err(|_| ExcelRequestError::History)?;
        write(
            self.store.as_ref(),
            &key(&record.owner, "response", id),
            payload,
        )
        .await
    }
}

fn validate_record(record: &ReplayRecord) -> Result<(), ExcelRequestError> {
    if record.native_calls.len() > MAX_NATIVE_CALLS
        || record.input.len() > 4096
        || serde_json::to_vec(record)
            .map_err(|_| ExcelRequestError::History)?
            .len()
            > MAX_PROVIDER_REPLAY_BYTES
    {
        return Err(ExcelRequestError::History);
    }
    Ok(())
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
    use super::*;

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
    async fn excel_replay_rejects_failed_unknown_oversized_and_conflicting_records() {
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
            .is_err()
        );
    }
}
