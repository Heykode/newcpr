use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    ChatConversionError as Error, Result,
    response::{self, ImageFingerprint, check_failure, string_value},
};

// Retain only a digest and length, not a second copy of streamed output.
#[derive(Debug, Default)]
struct Snapshot {
    length: usize,
    digest: Sha256,
}

impl Snapshot {
    fn append(&mut self, delta: &str) {
        self.length += delta.len();
        self.digest.update(delta.as_bytes());
    }

    fn suffix<'a>(&mut self, snapshot: &'a str) -> Result<&'a str> {
        // Some native terminal frames omit output already delivered as deltas.
        if snapshot.is_empty() && self.length > 0 {
            return Ok("");
        }
        let prefix = snapshot
            .get(..self.length)
            .ok_or_else(|| Error::output("output"))?;
        if Sha256::digest(prefix.as_bytes()) != self.digest.clone().finalize() {
            return Err(Error::output("output"));
        }
        let suffix = &snapshot[self.length..];
        self.append(suffix);
        Ok(suffix)
    }
}

#[derive(Debug, Default)]
struct TextState {
    id: Option<String>,
    parts: BTreeMap<(u64, &'static str), Snapshot>,
}

#[derive(Debug, Default)]
struct ToolState {
    arguments: Snapshot,
    done: bool,
    identity: BTreeMap<&'static str, String>,
}

impl ToolState {
    fn validate_identity(&mut self, event: &Value, item: &Value) -> Result<()> {
        for (field, value) in [
            ("id", &item["id"]),
            ("call_id", &event["call_id"]),
            ("call_id", &item["call_id"]),
            ("name", &item["name"]),
            ("type", &item["type"]),
        ] {
            let Some(value) = value.as_str().filter(|value| !value.is_empty()) else {
                continue;
            };
            match self.identity.get(field) {
                Some(previous) if previous != value => return Err(Error::output("output")),
                Some(_) => {}
                None => {
                    self.identity.insert(field, value.to_owned());
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ToolKey {
    Item(String),
    Output(String),
    Call(String),
}

/// Converts one Responses event at a time, without parsing or generating SSE bytes.
///
/// The caller must treat EOF with `!is_completed()` as failure, not emit success.
#[derive(Debug)]
pub struct ChatStreamEncoder {
    include_usage: bool,
    response_id: Option<String>,
    id: String,
    model: String,
    created: u64,
    completed: bool,
    failure: bool,
    tools: Vec<ToolState>,
    tool_keys: BTreeMap<ToolKey, usize>,
    current_tool: Option<usize>,
    last_images: BTreeMap<String, ImageFingerprint>,
    text_items: Vec<TextState>,
    text_keys: BTreeMap<ToolKey, usize>,
    current_text: Option<usize>,
    unindexed_text: BTreeSet<usize>,
}

impl ChatStreamEncoder {
    pub fn new(include_usage: bool) -> Self {
        Self {
            include_usage,
            response_id: None,
            id: format!("chatcmpl-{}", uuid::Uuid::new_v4().simple()),
            model: String::new(),
            created: 0,
            completed: false,
            failure: false,
            tools: Vec::new(),
            tool_keys: BTreeMap::new(),
            current_tool: None,
            last_images: BTreeMap::new(),
            text_items: Vec::new(),
            text_keys: BTreeMap::new(),
            current_text: None,
            unindexed_text: BTreeSet::new(),
        }
    }

    pub fn is_completed(&self) -> bool {
        self.completed
    }

    pub fn response_id(&self) -> Option<&str> {
        self.response_id.as_deref()
    }

    pub fn has_failure(&self) -> bool {
        self.failure
    }

    pub fn push(&mut self, event_type: Option<&str>, data: &Value) -> Result<Vec<Value>> {
        let result = self.push_event(event_type, data);
        if result.is_err() {
            self.failure = true;
        }
        result
    }

    fn push_event(&mut self, event_type: Option<&str>, data: &Value) -> Result<Vec<Value>> {
        if self.failure {
            return Err(Error::failed());
        }
        let kind = response::event_kind(event_type, data)?;
        if self.completed {
            return Ok(Vec::new());
        }
        let delta = match kind {
            "response.created" | "response.in_progress" | "response.queued" => {
                check_failure(&data["response"])?;
                self.metadata(&data["response"]);
                return Ok(Vec::new());
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let field = if kind == "response.refusal.delta" {
                    "refusal"
                } else {
                    "content"
                };
                self.text_delta(data, field, &data["delta"], false)?
            }
            "response.output_text.done" | "response.refusal.done" => {
                let (field, source) = if kind == "response.refusal.done" {
                    ("refusal", "refusal")
                } else {
                    ("content", "text")
                };
                self.text_delta(data, field, &data[source], true)?
            }
            "response.content_part.added" | "response.content_part.done" => {
                self.content_part(data, &data["part"])?
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let text = string_value(&data["delta"]);
                (!text.is_empty()).then(|| json!({"role":"assistant","reasoning_content":text}))
            }
            "response.reasoning_summary_text.done" | "response.reasoning_text.done" => {
                Some(json!({"role":"assistant","reasoning_content":"\n\n"}))
            }
            "response.output_item.added" | "response.output_item.done" => {
                return self.item(data, kind.ends_with(".done")).map(|deltas| {
                    deltas
                        .into_iter()
                        .map(|delta| self.chunk(delta, None))
                        .collect()
                });
            }
            "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done" => self.arguments(kind, data)?,
            "response.image_generation_call.partial_image" => self.image(
                &data["item_id"],
                &data["partial_image_b64"],
                data.get("output_format"),
            )?,
            "response.completed" | "response.incomplete" => return self.finish(data, kind),
            // Auxiliary events and unknown extension items do not imply completion.
            _ => None,
        };
        Ok(delta
            .into_iter()
            .map(|delta| self.chunk(delta, None))
            .collect())
    }

    fn metadata(&mut self, response: &Value) {
        if let Some(value) = response.get("id") {
            let id = string_value(value);
            self.id = response::chat_id(&id);
            self.response_id = Some(id.into_owned());
        }
        if let Some(value) = response.get("model") {
            self.model = string_value(value).into_owned();
        }
        if let Some(value) = response.get("created_at") {
            self.created = value.as_u64().unwrap_or_default();
        }
    }

    fn text_item(&mut self, event: &Value) -> Result<usize> {
        let id = event
            .get("item_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let output = event.get("output_index").and_then(Value::as_u64);
        let item_key = id.map(|id| ToolKey::Item(id.to_owned()));
        let output_key = output.map(|index| ToolKey::Output(index.to_string()));
        let mut known = None;
        for key in [item_key.as_ref(), output_key.as_ref()]
            .into_iter()
            .flatten()
        {
            if let Some(&index) = self.text_keys.get(key) {
                if known.is_some_and(|previous| previous != index) {
                    return Err(Error::output("output"));
                }
                known = Some(index);
            }
        }
        let index = known
            .or_else(|| {
                if (id.is_none() && output.is_none()) || self.text_keys.is_empty() {
                    self.current_text
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                self.text_items.push(TextState::default());
                self.text_items.len() - 1
            });
        if let Some(id) = id {
            let previous = &mut self.text_items[index].id;
            if previous.as_deref().is_some_and(|previous| previous != id) {
                return Err(Error::output("output"));
            }
            previous.get_or_insert_with(|| id.to_owned());
        }
        for key in [item_key, output_key].into_iter().flatten() {
            self.text_keys.entry(key).or_insert(index);
        }
        self.current_text = Some(index);
        Ok(index)
    }

    fn text_delta(
        &mut self,
        event: &Value,
        field: &'static str,
        value: &Value,
        snapshot: bool,
    ) -> Result<Option<Value>> {
        let text = string_value(value);
        let index = self.text_item(event)?;
        if !snapshot && event.get("content_index").and_then(Value::as_u64).is_none() {
            self.unindexed_text.insert(index);
        }
        let part = event
            .get("content_index")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let state = self.text_items[index]
            .parts
            .entry((part, field))
            .or_default();
        let text = if snapshot {
            state.suffix(&text)?
        } else {
            state.append(&text);
            &text
        };
        Ok((!text.is_empty()).then(|| json!({"role":"assistant", field:text})))
    }

    fn content_part(&mut self, event: &Value, part: &Value) -> Result<Option<Value>> {
        match part["type"].as_str() {
            Some("output_text") => self.text_delta(event, "content", &part["text"], true),
            Some("refusal") => self.text_delta(event, "refusal", &part["refusal"], true),
            _ => Ok(None),
        }
    }

    fn item(&mut self, event: &Value, done: bool) -> Result<Vec<Value>> {
        let item = &event["item"];
        let kind = item["type"].as_str().unwrap_or_default();
        if kind == "message" {
            let mut position = event.clone();
            if let Some(id) = item.get("id") {
                position["item_id"] = id.clone();
            }
            let text_index = self.text_item(&position)?;
            let mut deltas = Vec::new();
            if self.unindexed_text.contains(&text_index) {
                for (kind, field, source) in [
                    ("output_text", "content", "text"),
                    ("refusal", "refusal", "refusal"),
                ] {
                    let parts: Vec<_> = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|part| part["type"] == kind)
                        .collect();
                    if !parts.is_empty() {
                        let snapshot: String = parts
                            .iter()
                            .map(|part| string_value(&part[source]))
                            .collect();
                        deltas.extend(self.text_delta(
                            &position,
                            field,
                            &Value::String(snapshot),
                            true,
                        )?);
                    }
                }
                return Ok(deltas);
            }
            for (index, part) in item["content"].as_array().into_iter().flatten().enumerate() {
                position["content_index"] = json!(index);
                deltas.extend(self.content_part(&position, part)?);
            }
            return Ok(deltas);
        }
        if done && kind == "image_generation_call" {
            return Ok(self
                .image(&item["id"], &item["result"], item.get("output_format"))?
                .into_iter()
                .collect());
        }
        if !matches!(kind, "function_call" | "custom_tool_call") {
            return Ok(Vec::new());
        }
        let arguments = string_value(&item[tool_input_field(kind)]);
        if let Some(index) = self.find_tool(event, item)? {
            self.tools[index].validate_identity(event, item)?;
            self.register_tool(event, item, index);
            let state = &mut self.tools[index];
            if !done && arguments.is_empty() {
                return Ok(Vec::new());
            }
            let suffix = state.arguments.suffix(&arguments)?;
            state.done |= done;
            return Ok((!suffix.is_empty())
                .then(|| tool_arguments(index, suffix))
                .into_iter()
                .collect());
        }

        let index = self.tools.len();
        let mut state = ToolState {
            done,
            ..Default::default()
        };
        state.validate_identity(event, item)?;
        state.arguments.append(&arguments);
        self.tools.push(state);
        self.register_tool(event, item, index);
        self.current_tool = Some(index);
        Ok(vec![json!({
            "role":"assistant",
            "tool_calls":[{
                "index":index,"id":string_value(&item["call_id"]),"type":"function",
                "function":{"name":string_value(&item["name"]), "arguments":arguments}
            }]
        })])
    }

    fn arguments(&mut self, kind: &str, event: &Value) -> Result<Option<Value>> {
        let Some(index) = self.find_tool(event, &Value::Null)? else {
            return Ok(None);
        };
        let state = &mut self.tools[index];
        state.validate_identity(event, &Value::Null)?;
        let done = kind.ends_with(".done");
        if state.done {
            return Ok(None);
        }
        let field = if !done {
            "delta"
        } else if kind.starts_with("response.custom_tool_call_input.") {
            "input"
        } else {
            "arguments"
        };
        let input = string_value(&event[field]);
        let input = if done {
            state.arguments.suffix(&input)?
        } else {
            state.arguments.append(&input);
            &input
        };
        Ok((!input.is_empty()).then(|| tool_arguments(index, input)))
    }

    fn register_tool(&mut self, event: &Value, item: &Value, index: usize) {
        for value in [&event["item_id"], &item["id"]] {
            let id = string_value(value);
            if !id.is_empty() {
                self.tool_keys.insert(ToolKey::Item(id.into_owned()), index);
            }
        }
        if let Some(output) = event.get("output_index") {
            self.tool_keys
                .insert(ToolKey::Output(output.to_string()), index);
        }
        for value in [&event["call_id"], &item["call_id"]] {
            let id = string_value(value);
            if !id.is_empty() {
                self.tool_keys.insert(ToolKey::Call(id.into_owned()), index);
            }
        }
    }

    fn find_tool(&self, event: &Value, item: &Value) -> Result<Option<usize>> {
        let mut keys = Vec::new();
        for value in [&event["item_id"], &item["id"]] {
            if let Some(id) = value.as_str().filter(|id| !id.is_empty()) {
                keys.push(ToolKey::Item(id.to_owned()));
            }
        }
        for value in [&event["call_id"], &item["call_id"]] {
            if let Some(id) = value.as_str().filter(|id| !id.is_empty()) {
                keys.push(ToolKey::Call(id.to_owned()));
            }
        }
        if let Some(output) = event.get("output_index") {
            keys.push(ToolKey::Output(output.to_string()));
        }
        let mut found = None;
        for key in &keys {
            if let Some(&index) = self.tool_keys.get(key) {
                if found.is_some_and(|previous| previous != index) {
                    return Err(Error::output("output"));
                }
                found = Some(index);
            }
        }
        // Explicit new identities cannot inherit the current tool's arguments.
        Ok(found.or_else(|| keys.is_empty().then_some(self.current_tool).flatten()))
    }

    fn image(
        &mut self,
        item_id: &Value,
        result: &Value,
        format: Option<&Value>,
    ) -> Result<Option<Value>> {
        let result = string_value(result);
        if result.is_empty() {
            return Ok(None);
        }
        let image = response::parse_image_payload(&result, format, "image")?;
        let id = string_value(item_id);
        if !id.is_empty() {
            let fingerprint = image.fingerprint();
            if self.last_images.get(id.as_ref()) == Some(&fingerprint) {
                return Ok(None);
            }
            self.last_images.insert(id.into_owned(), fingerprint);
        }
        let mut delta = json!({"role":"assistant","images":null});
        delta["images"] = Value::Array(vec![image.entry(0)]);
        Ok(Some(delta))
    }

    fn finish(&mut self, event: &Value, kind: &str) -> Result<Vec<Value>> {
        let response = &event["response"];
        check_failure(response)?;
        let expected_status = if kind == "response.incomplete" {
            "incomplete"
        } else {
            "completed"
        };
        if response["status"].as_str() != Some(expected_status) {
            return Err(Error::output("status"));
        }
        self.metadata(response);
        let mut chunks = Vec::new();
        if let Some(output) = response.get("output").filter(|value| !value.is_null()) {
            let output = output.as_array().ok_or_else(|| Error::output("output"))?;
            let mut item_ids = std::collections::BTreeSet::new();
            let mut call_ids = std::collections::BTreeSet::new();
            for (index, item) in output.iter().enumerate() {
                if !matches!(
                    item["type"].as_str(),
                    Some("message" | "function_call" | "custom_tool_call")
                ) {
                    continue;
                }

                for (field, identities) in [("id", &mut item_ids), ("call_id", &mut call_ids)] {
                    if let Some(id) = item
                        .get(field)
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        && !identities.insert(id)
                    {
                        return Err(Error::output("output"));
                    }
                }
                let mut position = json!({"output_index":index, "item":item});
                let identified_tool = matches!(
                    item["type"].as_str(),
                    Some("function_call" | "custom_tool_call")
                ) && ["id", "call_id"].iter().any(|field| {
                    item.get(*field)
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                });
                let identified_message = item["type"] == "message"
                    && item["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .is_some_and(|id| {
                            self.text_keys.contains_key(&ToolKey::Item(id.to_owned()))
                                || self
                                    .text_keys
                                    .get(&ToolKey::Output(index.to_string()))
                                    .is_none_or(|previous| self.text_items[*previous].id.is_some())
                        });
                // A terminal ID wins over array order; unnamed earlier deltas can
                // acquire their first ID at the same position without being replayed.
                if identified_tool || identified_message {
                    position
                        .as_object_mut()
                        .expect("object")
                        .remove("output_index");
                }
                let deltas = self.item(&position, true)?;
                chunks.extend(deltas.into_iter().map(|delta| self.chunk(delta, None)));
            }
        }
        let finish = if kind == "response.incomplete" {
            match response
                .pointer("/incomplete_details/reason")
                .and_then(Value::as_str)
            {
                Some("max_tokens" | "max_output_tokens") => "length",
                Some("content_filter") => "content_filter",
                _ => "stop",
            }
        } else if self.tools.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        let mut terminal = self.chunk(json!({}), Some(finish));
        response::project_response_fields(response, &mut terminal)?;
        chunks.push(terminal);
        if self.include_usage {
            let mut chunk = self.chunk(json!({}), None);
            chunk["choices"] = json!([]);
            chunk["usage"] = response::chat_usage(response)?.unwrap_or(Value::Null);
            chunks.push(chunk);
        }
        self.completed = true;
        Ok(chunks)
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> Value {
        let mut chunk = json!({
            "id":self.id,"object":"chat.completion.chunk","created":self.created,"model":self.model,
            "choices":[{"index":0,"delta":null,"finish_reason":finish,"logprobs":null}]
        });
        chunk["choices"][0]["delta"] = delta;
        if self.include_usage {
            chunk["usage"] = Value::Null;
        }
        chunk
    }
}

fn tool_input_field(kind: &str) -> &'static str {
    if kind == "custom_tool_call" {
        "input"
    } else {
        "arguments"
    }
}

fn tool_arguments(index: usize, input: &str) -> Value {
    json!({"tool_calls":[{"index":index,"function":{"arguments":input}}]})
}
