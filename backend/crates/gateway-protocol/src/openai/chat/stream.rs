use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{
    ChatConversionError as Error, Result,
    response::{self, ImageFingerprint, check_failure, string_value},
};

#[derive(Debug, Default)]
struct ToolState {
    arguments_emitted: bool,
    done: bool,
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
                let text = string_value(&data["delta"]);
                (!text.is_empty()).then(|| json!({"role":"assistant", field:text}))
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let text = string_value(&data["delta"]);
                (!text.is_empty()).then(|| json!({"role":"assistant","reasoning_content":text}))
            }
            "response.reasoning_summary_text.done" | "response.reasoning_text.done" => {
                Some(json!({"role":"assistant","reasoning_content":"\n\n"}))
            }
            "response.output_item.added" | "response.output_item.done" => {
                self.item(data, kind.ends_with(".done"))?
            }
            "response.function_call_arguments.delta"
            | "response.function_call_arguments.done"
            | "response.custom_tool_call_input.delta"
            | "response.custom_tool_call_input.done" => self.arguments(kind, data),
            "response.image_generation_call.partial_image" => self.image(
                &data["item_id"],
                &data["partial_image_b64"],
                data.get("output_format"),
            )?,
            "response.completed" | "response.incomplete" => return self.finish(data, kind),
            // Includes annotations and done snapshots. Only mapped deltas are forwarded;
            // reconstructing terminal text would require retaining the entire reply.
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

    fn item(&mut self, event: &Value, done: bool) -> Result<Option<Value>> {
        let item = &event["item"];
        let kind = item["type"].as_str().unwrap_or_default();
        if done && kind == "image_generation_call" {
            return self.image(&item["id"], &item["result"], item.get("output_format"));
        }
        if !matches!(kind, "function_call" | "custom_tool_call") {
            return Ok(None);
        }
        let arguments = || string_value(&item[tool_input_field(kind)]);
        if done && let Some(index) = self.find_tool(event, item) {
            let state = &mut self.tools[index];
            if state.done {
                return Ok(None);
            }
            state.done = true;
            if state.arguments_emitted {
                return Ok(None);
            }
            state.arguments_emitted = true;
            let input = arguments();
            return Ok((!input.is_empty()).then(|| tool_arguments(index, &input)));
        }

        let index = self.tools.len();
        self.tools.push(ToolState {
            arguments_emitted: done,
            done,
        });
        self.register_tool(event, item, index);
        Ok(Some(json!({
            "role":"assistant",
            "tool_calls":[{
                "index":index,"id":string_value(&item["call_id"]),"type":"function",
                "function":{
                    "name":string_value(&item["name"]),
                    "arguments":if done {arguments()} else {std::borrow::Cow::Borrowed("")}
                }
            }]
        })))
    }

    fn arguments(&mut self, kind: &str, event: &Value) -> Option<Value> {
        let index = self.find_tool(event, &Value::Null)?;
        let state = &mut self.tools[index];
        let done = kind.ends_with(".done");
        if state.done || (done && state.arguments_emitted) {
            return None;
        }
        let field = if !done {
            "delta"
        } else if kind.starts_with("response.custom_tool_call_input.") {
            "input"
        } else {
            "arguments"
        };
        let input = string_value(&event[field]);
        if done {
            // CPA's argument-done fallback is distinct from output-item completion.
            state.arguments_emitted = true;
        }
        if input.is_empty() {
            return None;
        }
        state.arguments_emitted = true;
        Some(tool_arguments(index, &input))
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
        self.current_tool = Some(index);
    }

    fn find_tool(&self, event: &Value, item: &Value) -> Option<usize> {
        let mut has_key = false;
        for value in [&event["item_id"], &item["id"]] {
            let id = string_value(value);
            if !id.is_empty() {
                has_key = true;
                if let Some(index) = self.tool_keys.get(&ToolKey::Item(id.into_owned())) {
                    return Some(*index);
                }
            }
        }
        if let Some(output) = event.get("output_index") {
            has_key = true;
            if let Some(index) = self.tool_keys.get(&ToolKey::Output(output.to_string())) {
                return Some(*index);
            }
        }
        for value in [&event["call_id"], &item["call_id"]] {
            let id = string_value(value);
            if !id.is_empty() {
                has_key = true;
                if let Some(index) = self.tool_keys.get(&ToolKey::Call(id.into_owned())) {
                    return Some(*index);
                }
            }
        }
        // An explicit new key is not the current call. In particular, a second
        // done-only item must be announced rather than consumed by the first.
        if has_key { None } else { self.current_tool }
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
        if self.response_id.is_none() {
            self.metadata(response);
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
        let mut chunks = vec![self.chunk(json!({}), Some(finish))];
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
