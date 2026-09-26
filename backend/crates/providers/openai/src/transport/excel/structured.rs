//! Prompt-based output formats with local, network-free terminal validation.

use std::sync::Arc;

use jsonschema::{Retrieve, Uri, Validator};
use serde_json::{Map, Value, json};

use super::ExcelRequestError;

const MAX_SCHEMA_BYTES: usize = 1024 * 1024;
const MAX_ANSWER_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct StructuredOutput {
    format: Value,
    validator: Option<Arc<Validator>>,
}

pub(super) struct NoExternalSchemas;

impl Retrieve for NoExternalSchemas {
    fn retrieve(&self, _: &Uri<String>) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schemas are disabled".into())
    }
}

impl StructuredOutput {
    pub(crate) fn parse(source: &Map<String, Value>) -> Result<Option<Self>, ExcelRequestError> {
        let invalid = ExcelRequestError::Format;
        let Some(text) = source.get("text").filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let text = text.as_object().ok_or(invalid)?;
        let Some(format) = text.get("format").filter(|value| !value.is_null()) else {
            return Ok(None);
        };
        let fields = format.as_object().ok_or(invalid)?;
        let kind = fields.get("type").and_then(Value::as_str).ok_or(invalid)?;
        if kind == "text" && fields.len() == 1 {
            return Ok(None);
        }
        if !matches!(kind, "json_object" | "json_schema")
            || fields.keys().any(|key| {
                key != "type"
                    && (kind != "json_schema"
                        || !matches!(key.as_str(), "name" | "schema" | "strict" | "description"))
            })
        {
            return Err(invalid);
        }
        let validator = if kind == "json_schema" {
            let name = fields.get("name").and_then(Value::as_str).ok_or(invalid)?;
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
                || fields
                    .get("strict")
                    .is_some_and(|value| !value.is_boolean())
                || fields
                    .get("description")
                    .is_some_and(|value| !value.is_string())
            {
                return Err(invalid);
            }
            let schema = fields
                .get("schema")
                .filter(|value| value.is_object())
                .ok_or(invalid)?;
            if serde_json::to_vec(schema).map_err(|_| invalid)?.len() > MAX_SCHEMA_BYTES {
                return Err(invalid);
            }
            Some(Arc::new(
                jsonschema::draft202012::options()
                    .with_retriever(NoExternalSchemas)
                    .should_validate_formats(true)
                    .build(schema)
                    .map_err(|_| invalid)?,
            ))
        } else {
            None
        };
        Ok(Some(Self {
            format: format.clone(),
            validator,
        }))
    }

    pub(crate) fn instructions(&self) -> String {
        format!(
            "The final assistant answer must be exactly one JSON {} without Markdown fences or surrounding prose. \
             Tool calls and explicit refusals remain separate protocol items. The gateway validates the \
             final answer against this requested format before delivery: {}",
            if self.validator.is_some() {
                "value"
            } else {
                "object"
            },
            self.format,
        )
    }

    pub(crate) fn project(&self, response: &mut Value) {
        if !response.get("text").is_some_and(Value::is_object) {
            response["text"] = json!({});
        }
        response["text"]["format"] = self.format.clone();
    }

    pub(crate) fn validate(&self, response: &Value) -> Result<(), ExcelRequestError> {
        let invalid = ExcelRequestError::Format;
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or(invalid)?;
        let mut answer = String::new();
        let mut tool = false;
        let mut refusal = false;
        for item in output {
            let kind = item.get("type").and_then(Value::as_str);
            tool |= matches!(kind, Some("function_call" | "custom_tool_call"));
            if kind != Some("message") {
                continue;
            }
            for part in item
                .get("content")
                .and_then(Value::as_array)
                .ok_or(invalid)?
            {
                match part.get("type").and_then(Value::as_str) {
                    Some("output_text") => {
                        let text = part.get("text").and_then(Value::as_str).ok_or(invalid)?;
                        if answer.len().saturating_add(text.len()) > MAX_ANSWER_BYTES {
                            return Err(invalid);
                        }
                        answer.push_str(text);
                    }
                    Some("refusal") => refusal = true,
                    _ => return Err(invalid),
                }
            }
        }
        if tool || (refusal && answer.is_empty()) {
            return Ok(());
        }
        let value: Value = serde_json::from_str(&answer).map_err(|_| invalid)?;
        if self
            .validator
            .as_ref()
            .map_or(!value.is_object(), |schema| !schema.is_valid(&value))
        {
            return Err(invalid);
        }
        Ok(())
    }
}

pub(super) fn is_message_event(kind: &str, item: Option<&Value>) -> bool {
    kind.starts_with("response.output_text.")
        || kind.starts_with("response.refusal.")
        || kind.starts_with("response.content_part.")
        || (kind.starts_with("response.output_item.")
            && item
                .and_then(|item| item.get("type"))
                .and_then(Value::as_str)
                == Some("message"))
}

pub(super) fn message_events(item: &Value, index: usize) -> Vec<Value> {
    let mut started = item.clone();
    started["content"] = json!([]);
    started["status"] = "in_progress".into();
    let mut events =
        vec![json!({"type":"response.output_item.added","output_index":index,"item":started})];
    if let Some(content) = item.get("content").and_then(Value::as_array) {
        for (content_index, part) in content.iter().enumerate() {
            let refusal = part.get("type").and_then(Value::as_str) == Some("refusal");
            let (field, prefix) = if refusal {
                ("refusal", "response.refusal")
            } else {
                ("text", "response.output_text")
            };
            let mut empty = part.clone();
            empty[field] = "".into();
            let mut done = json!({"type":format!("{prefix}.done")});
            done[field] = part[field].clone();
            for mut event in [
                json!({"type":"response.content_part.added","part":empty}),
                json!({"type":format!("{prefix}.delta"),"delta":part[field]}),
                done,
                json!({"type":"response.content_part.done","part":part}),
            ] {
                event["item_id"] = item["id"].clone();
                event["output_index"] = index.into();
                event["content_index"] = content_index.into();
                events.push(event);
            }
        }
    }
    events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    events
}
