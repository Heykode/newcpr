use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::{CLIENT_TOOL_INSTRUCTIONS, EXTERNAL_CLIENT_INSTRUCTIONS, ExcelRequestError, envelope};

#[derive(Clone)]
struct ToolSpec {
    name: String,
    namespace: Option<String>,
    kind: String,
    schema: Value,
    catalog: Value,
}

#[derive(Clone, Default)]
pub(crate) struct ClientTools {
    specs: BTreeMap<String, ToolSpec>,
    serial: bool,
    unavailable: BTreeSet<String>,
}

impl ClientTools {
    pub(crate) fn parse(source: &Map<String, Value>) -> Result<Self, ExcelRequestError> {
        let serial = match source.get("parallel_tool_calls") {
            None | Some(Value::Null | Value::Bool(true)) => false,
            Some(Value::Bool(false)) => true,
            _ => return Err(ExcelRequestError::Tool),
        };
        let mut tools = Self {
            serial,
            ..Self::default()
        };
        match source.get("tool_choice") {
            Some(Value::String(choice)) if choice == "none" => return Ok(tools),
            None | Some(Value::Null) => {}
            Some(Value::String(choice)) if choice == "auto" => {}
            _ => return Err(ExcelRequestError::Tool),
        }
        if let Some(values) = source.get("tools") {
            tools.add(values, None, 0)?;
        }
        if let Some(input) = source.get("input").and_then(Value::as_array) {
            for item in input {
                if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                    tools.add(item.get("tools").ok_or(ExcelRequestError::Tool)?, None, 0)?;
                }
            }
        }
        Ok(tools)
    }

    fn add(
        &mut self,
        values: &Value,
        namespace: Option<&str>,
        depth: usize,
    ) -> Result<(), ExcelRequestError> {
        if depth > 8 {
            return Err(ExcelRequestError::Tool);
        }
        for value in values.as_array().ok_or(ExcelRequestError::Tool)? {
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .ok_or(ExcelRequestError::Tool)?;
            if matches!(
                kind,
                "web_search"
                    | "web_search_preview"
                    | "web_search_preview_2025_03_11"
                    | "web_search_2025_08_26"
                    | "tool_search"
                    | "image_generation"
                    | "file_search"
                    | "code_interpreter"
                    | "computer"
                    | "computer_use_preview"
                    | "mcp"
            ) {
                self.unavailable.insert(kind.into());
                continue;
            }
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or(ExcelRequestError::Tool)?;
            if kind == "namespace" {
                let nested =
                    namespace.map_or_else(|| name.to_owned(), |parent| format!("{parent}.{name}"));
                self.add(
                    value.get("tools").ok_or(ExcelRequestError::Tool)?,
                    Some(&nested),
                    depth + 1,
                )?;
                continue;
            }
            if !matches!(kind, "function" | "custom") || self.specs.len() >= 256 {
                return Err(ExcelRequestError::Tool);
            }
            let key = namespace.map_or_else(|| name.to_owned(), |ns| format!("{ns}.{name}"));
            let schema = value
                .get("parameters")
                .or_else(|| value.get("inputSchema"))
                .or_else(|| value.get("input_schema"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut catalog = json!({"type":kind,"name":key});
            if let Some(description) = value.get("description").filter(|v| v.is_string()) {
                catalog["description"] = description.clone();
            }
            if let Some(namespace) = namespace {
                catalog["namespace"] = namespace.into();
                catalog["tool"] = name.into();
            }
            if kind == "function" {
                catalog["parameters"] = schema.clone();
            } else if let Some(format) = value.get("format") {
                catalog["format"] = format.clone();
            }
            if let Some(previous) = self.specs.get(&key) {
                if previous.catalog != catalog {
                    return Err(ExcelRequestError::Tool);
                }
                continue;
            }
            if self
                .specs
                .insert(
                    key,
                    ToolSpec {
                        name: name.into(),
                        namespace: namespace.map(str::to_owned),
                        kind: kind.into(),
                        schema,
                        catalog,
                    },
                )
                .is_some()
            {
                return Err(ExcelRequestError::Tool);
            }
        }
        Ok(())
    }

    pub(crate) fn instructions(&self) -> String {
        let warning = if self.unavailable.is_empty() {
            String::new()
        } else {
            format!(
                "\nThe following declared hosted capabilities are unavailable on this route: {}. \
                 Do not call them or claim they ran. If required, explain the limitation; \
                 only use declared client tools for capabilities they actually provide.",
                self.unavailable
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        if self.specs.is_empty() {
            return format!("{EXTERNAL_CLIENT_INSTRUCTIONS}{warning}");
        }
        let catalog = Value::Array(self.specs.values().map(|s| s.catalog.clone()).collect());
        format!(
            "{CLIENT_TOOL_INSTRUCTIONS}{catalog}\nFor custom tools, prefer summary=cpr.custom/CATALOG_NAME and put exact raw input directly in code. \
             For other function tools, put exactly one catalog-tool JSON object in code. Never combine calls. {}{}{warning}",
            if self.serial {
                "Return at most one client tool call per response; wait for its result before requesting another."
            } else {
                "Independent client tools may be called in parallel, using a separate native run_officejs call for each."
            },
            self.function_code_instructions()
        )
    }

    fn supports_function_code(&self, name: &str) -> bool {
        !name.is_empty()
            && !name
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\'))
            && self.specs.get(name).is_some_and(|spec| {
                spec.kind == "function"
                    && spec.schema.get("type").and_then(Value::as_str) == Some("object")
                    && spec
                        .schema
                        .pointer("/properties/code/type")
                        .and_then(Value::as_str)
                        == Some("string")
            })
    }

    fn function_code_instructions(&self) -> String {
        let names = self
            .specs
            .keys()
            .filter(|name| self.supports_function_code(name))
            .cloned()
            .collect::<Vec<_>>();
        if names.is_empty() {
            return String::new();
        }
        format!(
            "\nFor catalog functions [{}], prefer summary={}EXACT_CATALOG_NAME, put the exact raw source text in code, and put one JSON object containing all remaining arguments in extended_summary (use {{}} when empty). Do not include code in that object. Do not wrap, escape again, repair or execute the source text here.",
            names.join(", "),
            envelope::FUNCTION_CODE_PREFIX
        )
    }

    pub(crate) fn unavailable(&self) -> &BTreeSet<String> {
        &self.unavailable
    }

    pub(crate) fn validate_call_count(&self, count: usize) -> Result<(), ExcelRequestError> {
        if self.serial && count > 1 {
            Err(ExcelRequestError::ToolCall)
        } else {
            Ok(())
        }
    }

    pub(crate) fn parallel_allowed(&self) -> bool {
        !self.serial
    }

    pub(crate) fn reminder(&self) -> Option<String> {
        (!self.specs.is_empty()).then(|| format!(
            "Reminder: use the outer native run_officejs transport; it never executes Office code here. Function tools use one JSON envelope unless the raw-code contract below applies. Custom tools use summary=cpr.custom/CATALOG_NAME and exact raw code input. The catalog names are: {}. Other native tools are unavailable.{}",
            self.specs.keys().cloned().collect::<Vec<_>>().join(", "),
            self.function_code_instructions()
        ))
    }

    pub(crate) fn convert_call(&self, native: &Value) -> Result<Value, ExcelRequestError> {
        let invalid = ExcelRequestError::ToolCall;
        let name = native.get("name").and_then(Value::as_str).ok_or(invalid)?;
        let transport = matches!(name, "run_officejs" | "functions.run_officejs");
        let (envelope, marked) = if transport {
            envelope::native_envelope(
                native,
                &|name| {
                    let (key, spec) = self
                        .specs
                        .get_key_value(name)
                        .or_else(|| self.specs.get_key_value(name.strip_prefix("functions.")?))?;
                    Some((key.clone(), spec.kind == "custom"))
                },
                &|name| self.supports_function_code(name),
            )?
        } else {
            (native.clone(), false)
        };
        let declared = envelope::name(&envelope)?;
        let qualified = native
            .get("namespace")
            .and_then(Value::as_str)
            .filter(|ns| !ns.is_empty())
            .map(|ns| format!("{ns}.{declared}"));
        let spec = self
            .specs
            .get(qualified.as_deref().unwrap_or(declared))
            .or_else(|| {
                (!transport)
                    .then(|| {
                        declared
                            .strip_prefix("functions.")
                            .and_then(|name| self.specs.get(name))
                    })
                    .flatten()
            })
            .ok_or(invalid)?;
        if (marked && spec.kind != "custom")
            || (!transport
                && native.get("type").and_then(Value::as_str)
                    != Some(if spec.kind == "custom" {
                        "custom_tool_call"
                    } else {
                        "function_call"
                    }))
        {
            return Err(invalid);
        }
        let call_id = native
            .get("call_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(invalid)?;
        let item_id = native
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| history_item_id(call_id));
        let mut result = json!({
            "type": if spec.kind == "function" {"function_call"} else {"custom_tool_call"},
            "id": item_id, "call_id":call_id, "name":spec.name, "status":"completed",
        });
        if let Some(namespace) = &spec.namespace {
            result["namespace"] = namespace.clone().into();
        }
        if spec.kind == "custom" {
            if envelope.get("arguments").is_some()
                || (envelope.get("input").is_some() && envelope.get("args").is_some())
            {
                return Err(invalid);
            }
            result["id"] =
                format!("ctc_{}", history_item_id(call_id).trim_start_matches("fc_")).into();
            result["input"] = envelope
                .get("input")
                .or_else(|| envelope.get("args"))
                .and_then(Value::as_str)
                .ok_or(invalid)?
                .into();
        } else {
            let raw = envelope::arguments_field(&envelope)?;
            let mut args: Value = if let Some(text) = raw.as_str() {
                serde_json::from_str(text).map_err(|_| invalid)?
            } else {
                raw.clone()
            };
            let mut preserve_encryption = !transport;
            if !transport && matches!(declared, "update_plan" | "functions.update_plan") {
                let normalized = normalize_plan(args.clone())?;
                preserve_encryption = normalized == args;
                args = normalized;
            }
            if !args.is_object() || !matches_schema(&args, &spec.schema, 0) {
                return Err(invalid);
            }
            result["arguments"] = args.to_string().into();
            // Missing differs from empty: collaboration clients otherwise treat plaintext as ciphertext.
            result["encrypted_function_args"] = if preserve_encryption {
                native
                    .get("encrypted_function_args")
                    .filter(|value| !value.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!([]))
            } else {
                json!([])
            };
        }
        Ok(result)
    }
}

pub(super) fn canonical_history_call(item: &Value) -> Result<Value, ExcelRequestError> {
    let invalid = ExcelRequestError::History;
    let call_id = item
        .get("call_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.trim() == *s)
        .ok_or(invalid)?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.trim() == *s)
        .ok_or(invalid)?;
    let namespace = match item.get("namespace") {
        None => "",
        Some(value) => value.as_str().filter(|s| s.trim() == *s).ok_or(invalid)?,
    };
    let mut call = json!({"type":item["type"],"call_id":call_id,"name":name,"namespace":namespace});
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => {
            let args =
                envelope::json_value(item.get("arguments").ok_or(invalid)?).map_err(|_| invalid)?;
            if !args.is_object() {
                return Err(invalid);
            }
            call["arguments"] = args;
        }
        Some("custom_tool_call") => {
            call["input"] = item
                .get("input")
                .filter(|v| v.is_string())
                .cloned()
                .ok_or(invalid)?;
        }
        _ => return Err(invalid),
    }
    Ok(call)
}

pub(super) fn rebuild_history_call(item: &Value) -> Result<Value, ExcelRequestError> {
    let call = canonical_history_call(item)?;
    let name = call["name"].as_str().ok_or(ExcelRequestError::History)?;
    let namespace = call["namespace"].as_str().unwrap_or_default();
    let name = if namespace.is_empty() {
        name.into()
    } else {
        format!("{namespace}.{name}")
    };
    let mut args = json!({"name":name});
    if call["type"] == "custom_tool_call" {
        args["input"] = call["input"].clone();
    } else {
        args["arguments"] = call["arguments"].clone();
    }
    let id = call["call_id"].as_str().ok_or(ExcelRequestError::History)?;
    Ok(json!({
        "type":"function_call","id":history_item_id(id),"call_id":id,"name":"run_officejs","status":"completed",
        "arguments":json!({"code":args.to_string(),"summary":"Replay a recorded client tool",
            "extended_summary":"Use this recorded result without repeating the tool.",
            "destructive":false,"references":[]}).to_string()
    }))
}

fn history_item_id(id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("fc_{}", &hex::encode(Sha256::digest(id.as_bytes()))[..40])
}

fn normalize_plan(args: Value) -> Result<Value, ExcelRequestError> {
    let items = args
        .get("plan")
        .and_then(Value::as_array)
        .ok_or(ExcelRequestError::ToolCall)?;
    let mut plan = Vec::with_capacity(items.len());
    for item in items {
        let step = item
            .get("step")
            .or_else(|| item.get("description"))
            .or_else(|| item.get("title"))
            .and_then(Value::as_str)
            .ok_or(ExcelRequestError::ToolCall)?;
        let status = match item.get("status").and_then(Value::as_str).unwrap_or("") {
            "pending" | "not_started" | "todo" | "planned" | "queued" | "blocked" => "pending",
            "in_progress" | "active" | "started" | "doing" | "current" => "in_progress",
            "completed" | "complete" | "done" | "finished" => "completed",
            _ => return Err(ExcelRequestError::ToolCall),
        };
        plan.push(json!({"step":step,"status":status}));
    }
    let mut result = json!({"plan":plan});
    if let Some(explanation) = args
        .get("explanation")
        .or_else(|| args.get("summary"))
        .filter(|v| v.is_string())
    {
        result["explanation"] = explanation.clone();
    }
    Ok(result)
}

fn matches_schema(value: &Value, schema: &Value, depth: usize) -> bool {
    if depth > 32 {
        return false;
    }
    let Some(schema) = schema.as_object() else {
        return true;
    };
    if let Some(types) = schema.get("type").and_then(Value::as_array) {
        return types.iter().any(|kind| {
            let mut alternative = schema.clone();
            alternative.insert("type".into(), kind.clone());
            matches_schema(value, &alternative.into(), depth + 1)
        });
    }
    let valid_type = match schema.get("type").and_then(Value::as_str) {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("boolean") => value.is_boolean(),
        Some("null") => value.is_null(),
        _ => true,
    };
    if !valid_type
        || schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.contains(value))
    {
        return false;
    }
    if let Some(object) = value.as_object() {
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .any(|key| !object.contains_key(key))
            })
        {
            return false;
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in object {
                match properties.get(key) {
                    Some(nested) if !matches_schema(value, nested, depth + 1) => return false,
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return false;
                    }
                    _ => {}
                }
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items"))
        && !items
            .iter()
            .all(|item| matches_schema(item, item_schema, depth + 1))
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_namespaced_tool_is_restored_without_executing_it() {
        let source = json!({"tools":[{"type":"namespace","name":"workspace","tools":[
            {"type":"function","name":"read","parameters":{"type":"object","required":["path"],
                "properties":{"path":{"type":"string"}},"additionalProperties":false}}
        ]}]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        let native = json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture","name":"run_officejs",
            "arguments":json!({"code":json!({"name":"workspace.read","arguments":{"path":"sample.txt"}}).to_string()}).to_string()});
        let output = tools.convert_call(&native).unwrap();
        assert_eq!(output["namespace"], "workspace");
        assert_eq!(output["name"], "read");
        assert_eq!(output["call_id"], native["call_id"]);
    }

    #[test]
    fn undeclared_native_tools_and_invalid_arguments_never_reach_client() {
        let source = json!({"tools":[{"type":"function","name":"read","parameters":{
            "type":"object","required":["path"],"properties":{"path":{"type":"string"}}}}]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        for (name, args) in [
            ("run_officejs", json!({"code":"not json"})),
            ("read", json!({"path":9})),
            ("delete", json!({})),
        ] {
            let native = json!({"type":"function_call","id":"fc_fixture","call_id":"call_fixture","name":name,"arguments":args.to_string()});
            assert_eq!(
                tools.convert_call(&native),
                Err(ExcelRequestError::ToolCall)
            );
        }
    }
}
