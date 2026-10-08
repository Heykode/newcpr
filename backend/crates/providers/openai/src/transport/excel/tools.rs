use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use jsonschema::Validator;
use serde_json::{Map, Value, json};

use super::{
    CLIENT_TOOL_INSTRUCTIONS, EXTERNAL_CLIENT_INSTRUCTIONS, ExcelRequestError, envelope,
    structured::NoExternalSchemas,
};

const MAX_SCHEMA_BYTES: usize = 1024 * 1024;

pub(super) fn is_catalog_delta(item: &Value) -> bool {
    match item.get("type").and_then(Value::as_str) {
        Some("additional_tools") => true,
        Some("tool_search_output") => {
            matches!(item.get("status"), None | Some(Value::Null))
                || item.get("status").and_then(Value::as_str) == Some("completed")
        }
        _ => false,
    }
}

// Match Sub2API #139: annotations can change, execution constraints cannot.
// Keep the first (explicit/inherited) declaration ahead of historical additions.
fn tool_definition(value: &Value) -> Value {
    let mut definition = value.as_object().expect("validated tool object").clone();
    definition.remove("description");
    definition.remove("defer_loading");
    if value.get("type").and_then(Value::as_str) == Some("function") {
        for key in ["parameters", "inputSchema", "input_schema"] {
            definition.remove(key);
        }
        if let Some(schema) = ["parameters", "inputSchema", "input_schema"]
            .into_iter()
            .find_map(|key| value.get(key).filter(|value| !value.is_null()))
        {
            definition.insert("parameters".into(), schema.clone());
        }
    }
    Value::Object(definition)
}

#[derive(Clone)]
struct ToolSpec {
    name: String,
    namespace: Option<String>,
    kind: String,
    schema: Value,
    validator: Option<Arc<Validator>>,
    catalog: Value,
    declaration: Value,
}

#[derive(Clone, Default)]
enum ToolChoice {
    #[default]
    Auto,
    Required,
    Named(String),
    Allowed {
        names: BTreeSet<String>,
        required: bool,
        wire: Value,
    },
}

#[derive(Clone, Default)]
pub(crate) struct ClientTools {
    specs: BTreeMap<String, ToolSpec>,
    serial: bool,
    unavailable: BTreeSet<String>,
    choice: ToolChoice,
}

impl ClientTools {
    pub(super) fn catalog(&self) -> Value {
        let mut values: Vec<Value> = self
            .specs
            .values()
            .map(|spec| {
                // Persist the declaration, not the lossy model-facing catalog: later
                // duplicates must still compare strictness and unknown constraints.
                let value = spec.declaration.clone();
                match &spec.namespace {
                    Some(namespace) => json!({"type":"namespace","name":namespace,"tools":[value]}),
                    None => value,
                }
            })
            .collect();
        values.extend(self.unavailable.iter().map(|kind| json!({"type":kind})));
        Value::Array(values)
    }

    pub(super) fn contains(&self, name: &str) -> bool {
        self.specs.contains_key(name)
    }

    pub(super) fn with_choice(mut self, choice: Option<&Value>) -> Result<Self, ExcelRequestError> {
        if choice.and_then(Value::as_str) == Some("none") {
            self.specs.clear();
            self.unavailable.clear();
            self.choice = ToolChoice::Auto;
        } else {
            self.choice = self.parse_choice(choice)?;
        }
        Ok(self)
    }

    pub(crate) fn has_client_tools(&self) -> bool {
        !self.specs.is_empty()
    }

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
        if source.get("tool_choice").and_then(Value::as_str) == Some("none") {
            return Ok(tools);
        }
        if let Some(values) = source.get("tools") {
            tools.add(values, None, 0)?;
        }
        if let Some(input) = source.get("input").and_then(Value::as_array) {
            for item in input {
                if is_catalog_delta(item) {
                    tools.add(item.get("tools").ok_or(ExcelRequestError::Tool)?, None, 0)?;
                }
            }
        }
        tools.choice = tools.parse_choice(source.get("tool_choice"))?;
        Ok(tools)
    }

    fn parse_choice(&self, choice: Option<&Value>) -> Result<ToolChoice, ExcelRequestError> {
        let invalid = ExcelRequestError::Tool;
        let Some(choice) = choice.filter(|value| !value.is_null()) else {
            return Ok(ToolChoice::Auto);
        };
        match choice.as_str() {
            Some("auto") => return Ok(ToolChoice::Auto),
            Some("required") if !self.specs.is_empty() => return Ok(ToolChoice::Required),
            Some(_) => return Err(invalid),
            None => {}
        }
        let fields = choice.as_object().ok_or(invalid)?;
        if fields.get("type").and_then(Value::as_str) == Some("allowed_tools") {
            if fields
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "mode" | "tools"))
            {
                return Err(invalid);
            }
            let required = match fields.get("mode").and_then(Value::as_str) {
                Some("auto") => false,
                Some("required") => true,
                _ => return Err(invalid),
            };
            let names = fields
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(invalid)?
                .iter()
                .map(|selector| self.selected_name(selector))
                .collect::<Result<BTreeSet<_>, _>>()?;
            if required && names.is_empty() {
                return Err(invalid);
            }
            return Ok(ToolChoice::Allowed {
                names,
                required,
                wire: choice.clone(),
            });
        }
        Ok(ToolChoice::Named(self.selected_name(choice)?))
    }

    fn selected_name(&self, selector: &Value) -> Result<String, ExcelRequestError> {
        let invalid = ExcelRequestError::Tool;
        let fields = selector.as_object().ok_or(invalid)?;
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "name" | "namespace"))
        {
            return Err(invalid);
        }
        let kind = fields.get("type").and_then(Value::as_str).ok_or(invalid)?;
        let name = fields.get("name").and_then(Value::as_str).ok_or(invalid)?;
        let name = match fields.get("namespace") {
            None => name.to_owned(),
            Some(Value::String(namespace)) if !namespace.is_empty() => {
                format!("{namespace}.{name}")
            }
            _ => return Err(invalid),
        };
        let spec = self.specs.get(&name).ok_or(invalid)?;
        if spec.kind != kind {
            return Err(invalid);
        }
        Ok(name)
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
            if !matches!(kind, "function" | "custom") {
                return Err(ExcelRequestError::Tool);
            }
            let key = namespace.map_or_else(|| name.to_owned(), |ns| format!("{ns}.{name}"));
            let schema = value
                .get("parameters")
                .filter(|value| !value.is_null())
                .or_else(|| value.get("inputSchema").filter(|value| !value.is_null()))
                .or_else(|| value.get("input_schema").filter(|value| !value.is_null()))
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
                if previous.name != name
                    || previous.namespace.as_deref() != namespace
                    || tool_definition(&previous.declaration) != tool_definition(value)
                {
                    return Err(ExcelRequestError::Tool);
                }
                continue;
            }
            // Repeated declarations do not consume another unique-tool slot.
            if self.specs.len() >= 256 {
                return Err(ExcelRequestError::Tool);
            }
            let validator = if kind == "function" {
                if serde_json::to_vec(&schema)
                    .map_err(|_| ExcelRequestError::Tool)?
                    .len()
                    > MAX_SCHEMA_BYTES
                {
                    return Err(ExcelRequestError::Tool);
                }
                Some(Arc::new(
                    jsonschema::draft202012::options()
                        .with_retriever(NoExternalSchemas)
                        .should_validate_formats(true)
                        .build(&schema)
                        .map_err(|_| ExcelRequestError::Tool)?,
                ))
            } else {
                None
            };
            if self
                .specs
                .insert(
                    key,
                    ToolSpec {
                        name: name.into(),
                        namespace: namespace.map(str::to_owned),
                        kind: kind.into(),
                        schema,
                        validator,
                        catalog,
                        declaration: value.clone(),
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
            return format!(
                "{EXTERNAL_CLIENT_INSTRUCTIONS}{}{warning}",
                self.choice_instructions()
            );
        }
        let catalog = Value::Array(self.specs.values().map(|s| s.catalog.clone()).collect());
        format!(
            "{CLIENT_TOOL_INSTRUCTIONS}{catalog}\nFor custom tools, prefer summary=codex2api.custom/CATALOG_NAME and put exact raw input directly in code. \
             For other function tools, put exactly one catalog-tool JSON object in code. Never combine calls. {}{}{}{}{warning}",
            if self.serial {
                "Return at most one client tool call per response; wait for its result before requesting another."
            } else {
                "Independent client tools may be called in parallel, using a separate native run_officejs call for each."
            },
            self.function_code_instructions(),
            self.function_cmd_instructions(),
            self.choice_instructions()
        )
    }

    fn choice_instructions(&self) -> String {
        match &self.choice {
            ToolChoice::Auto => String::new(),
            ToolChoice::Required => "\nThe client requires at least one declared client tool call in this response, unless you explicitly refuse the request.".into(),
            ToolChoice::Named(name) => format!("\nThe client requires exactly one call to catalog tool {name}; do not substitute another tool. An explicit refusal is allowed."),
            ToolChoice::Allowed { names, required, .. } => format!(
                "\nOnly these exact catalog tools may be called in this response: {}. Other catalog entries are for history only. {}",
                serde_json::to_string(names).expect("tool names serialize"),
                if *required { "Call at least one allowed tool unless explicitly refusing the request." }
                else { "Calling a tool is optional; an empty allowed list forbids all tool calls." }
            ),
        }
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

    fn supports_function_cmd(&self, name: &str) -> bool {
        (name == "exec_command" || name.ends_with(".exec_command"))
            && !self.supports_function_code(name)
            && !name
                .chars()
                .any(|ch| ch.is_whitespace() || ch.is_control() || matches!(ch, '/' | '\\'))
            && self.specs.get(name).is_some_and(|spec| {
                spec.kind == "function"
                    && spec.schema.get("type").and_then(Value::as_str) == Some("object")
                    && spec
                        .schema
                        .pointer("/properties/cmd/type")
                        .and_then(Value::as_str)
                        == Some("string")
            })
    }

    pub(super) fn rebuild_history_call(&self, item: &Value) -> Result<Value, ExcelRequestError> {
        let mut native = rebuild_history_call(item)?;
        let call = canonical_history_call(item)?;
        let namespace = call["namespace"].as_str().unwrap_or_default();
        let name = call["name"].as_str().ok_or(ExcelRequestError::History)?;
        let name = if namespace.is_empty() {
            name.to_owned()
        } else {
            format!("{namespace}.{name}")
        };
        let Some(spec) = self.specs.get(&name) else {
            return Ok(native);
        };
        let mut outer = envelope::json_value(&native["arguments"])?;
        if spec.kind == "custom" && call["type"] == "custom_tool_call" {
            outer["summary"] = format!("codex2api.custom/{name}").into();
            outer["code"] = call["input"].clone();
        } else if call["type"] == "function_call"
            && self.supports_function_code(&name)
            && let Some(code) = call["arguments"]
                .get("code")
                .filter(|value| value.is_string())
        {
            let mut metadata = call["arguments"].clone();
            metadata
                .as_object_mut()
                .ok_or(ExcelRequestError::History)?
                .remove("code");
            outer["summary"] = format!("{}{name}", envelope::FUNCTION_CODE_PREFIX).into();
            outer["code"] = code.clone();
            outer["extended_summary"] = metadata.to_string().into();
        } else if call["type"] == "function_call"
            && self.supports_function_cmd(&name)
            && let Some(cmd) = call["arguments"]
                .get("cmd")
                .filter(|value| value.is_string())
        {
            let mut metadata = call["arguments"].clone();
            metadata
                .as_object_mut()
                .ok_or(ExcelRequestError::History)?
                .remove("cmd");
            outer["summary"] = format!("{}{name}", envelope::FUNCTION_CMD_PREFIX).into();
            outer["code"] = cmd.clone();
            outer["extended_summary"] = metadata.to_string().into();
        } else {
            return Ok(native);
        }
        native["arguments"] = outer.to_string().into();
        // Historical calls need not satisfy today's schema, but transport sizes still apply.
        envelope::native_envelope(
            &native,
            &|name| {
                self.specs
                    .get(name)
                    .map(|spec| (name.into(), spec.kind == "custom"))
            },
            &|name| self.supports_function_code(name),
            &|name| self.supports_function_cmd(name),
        )?;
        Ok(native)
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

    fn function_cmd_instructions(&self) -> String {
        let names = self
            .specs
            .keys()
            .filter(|name| self.supports_function_cmd(name))
            .cloned()
            .collect::<Vec<_>>();
        if names.is_empty() {
            return String::new();
        }
        format!(
            "\nFor catalog functions [{}], prefer summary={}EXACT_CATALOG_NAME, put the exact raw shell command in code and one JSON object with all remaining arguments in extended_summary ({{}} when empty). Do not include cmd in that object. Do not wrap, escape again, repair or execute the command here.",
            names.join(", "),
            envelope::FUNCTION_CMD_PREFIX
        )
    }

    pub(crate) fn unavailable(&self) -> &BTreeSet<String> {
        &self.unavailable
    }

    pub(crate) fn validate_call_count(&self, count: usize) -> Result<(), ExcelRequestError> {
        if (self.serial && count > 1)
            || matches!(self.choice, ToolChoice::Required) && count == 0
            || matches!(self.choice, ToolChoice::Allowed { required: true, .. }) && count == 0
            || matches!(self.choice, ToolChoice::Named(_)) && count != 1
        {
            Err(ExcelRequestError::ToolCall)
        } else {
            Ok(())
        }
    }

    pub(crate) fn validate_completion(&self, output: &[Value]) -> Result<(), ExcelRequestError> {
        let count = output
            .iter()
            .filter(|item| {
                matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("function_call" | "custom_tool_call")
                )
            })
            .count();
        let refusal = output.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some("refusal")
                                && part
                                    .get("refusal")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| !text.is_empty())
                        })
                    })
        });
        if count == 0 && refusal {
            return Ok(());
        }
        self.validate_call_count(count)
    }

    pub(crate) fn parallel_allowed(&self) -> bool {
        !self.serial
    }

    pub(crate) fn project_choice(&self, response: &mut Value) {
        match &self.choice {
            ToolChoice::Auto => {}
            ToolChoice::Required => response["tool_choice"] = "required".into(),
            ToolChoice::Allowed { wire, .. } => response["tool_choice"] = wire.clone(),
            ToolChoice::Named(key) => {
                if let Some(spec) = self.specs.get(key) {
                    let mut choice = json!({"type":spec.kind,"name":spec.name});
                    if let Some(namespace) = &spec.namespace {
                        choice["namespace"] = namespace.clone().into();
                    }
                    response["tool_choice"] = choice;
                }
            }
        }
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
                &|name| self.supports_function_cmd(name),
            )?
        } else {
            (native.clone(), false)
        };
        let declared = envelope::name(&envelope)?;
        // Only a direct client call owns its namespace. The transport wrapper does not.
        let qualified = (!transport)
            .then(|| native.get("namespace"))
            .flatten()
            .and_then(Value::as_str)
            .filter(|ns| !ns.is_empty())
            .map(|ns| format!("{ns}.{declared}"));
        let (key, spec) = self
            .specs
            .get_key_value(qualified.as_deref().unwrap_or(declared))
            .or_else(|| {
                (!transport && qualified.is_none())
                    .then(|| {
                        declared
                            .strip_prefix("functions.")
                            .and_then(|name| self.specs.get_key_value(name))
                    })
                    .flatten()
            })
            .ok_or(ExcelRequestError::UnknownTool)?;
        if match &self.choice {
            ToolChoice::Named(required) => key != required,
            ToolChoice::Allowed { names, .. } => !names.contains(key),
            _ => false,
        } {
            return Err(invalid);
        }
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
            if !args.is_object()
                || !spec
                    .validator
                    .as_ref()
                    .is_some_and(|validator| validator.is_valid(&args))
            {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tools_at_limit() -> Vec<Value> {
        (0..256)
            .map(|index| {
                json!({"type":"function","name":format!("fixture_{index:03}"),
                    "parameters":{"type":"object"}})
            })
            .collect()
    }

    #[test]
    fn tool_limit_counts_unique_declarations_not_repeated_entries() {
        let declarations = tools_at_limit();
        let source = json!({"tools":declarations});
        let baseline = ClientTools::parse(source.as_object().unwrap()).unwrap();
        for duplicate_index in [0, 127, 255] {
            let mut repeated = declarations.clone();
            repeated.push(declarations[duplicate_index].clone());
            let source = json!({"tools":repeated});
            let parsed = ClientTools::parse(source.as_object().unwrap()).unwrap();
            assert_eq!(parsed.specs.len(), 256);
            assert_eq!(parsed.catalog(), baseline.catalog());
            assert_eq!(parsed.instructions(), baseline.instructions());
        }
    }

    #[test]
    fn tool_limit_preserves_current_declaration_over_historical_annotations() {
        let mut declarations = tools_at_limit();
        declarations[127]["strict"] = json!(true);
        declarations[127]["description"] = json!("Current");
        let mut historical = declarations[127].clone();
        historical["description"] = json!("Historical");
        historical["defer_loading"] = json!(true);
        historical.as_object_mut().unwrap().remove("parameters");
        historical["inputSchema"] = json!({"type":"object"});
        let source = json!({"tools":declarations,"input":[
            {"type":"additional_tools","tools":[historical]}
        ]});
        let parsed = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert_eq!(parsed.catalog(), json!(declarations));
        assert!(!parsed.instructions().contains("Historical"));
        let restored = json!({"tools":parsed.catalog(),"input":source["input"]});
        let restored = ClientTools::parse(restored.as_object().unwrap()).unwrap();
        assert_eq!(restored.catalog(), parsed.catalog());
        assert_eq!(restored.instructions(), parsed.instructions());
    }

    #[test]
    fn tool_limit_still_rejects_new_unique_tools_and_execution_conflicts() {
        let declarations = tools_at_limit();
        let mut extra = declarations.clone();
        extra.push(json!({"type":"function","name":"extra","parameters":{}}));
        let source = json!({"tools":extra});
        assert!(matches!(
            ClientTools::parse(source.as_object().unwrap()),
            Err(ExcelRequestError::Tool)
        ));
        for (field, value) in [
            ("parameters", json!({"type":"string"})),
            ("strict", json!(true)),
            ("type", json!("custom")),
            ("future_execution_constraint", json!(false)),
        ] {
            let mut conflicting = declarations[0].clone();
            conflicting[field] = value;
            let source = json!({"tools":declarations,"input":[
                {"type":"additional_tools","tools":[conflicting]}
            ]});
            assert!(
                matches!(
                    ClientTools::parse(source.as_object().unwrap()),
                    Err(ExcelRequestError::Tool)
                ),
                "{field}"
            );
        }
    }

    #[test]
    fn tool_limit_deduplicates_namespaced_tools_without_merging_identities() {
        let declarations = tools_at_limit();
        let source = json!({"tools":[
            {"type":"namespace","name":"workspace","tools":declarations},
            {"type":"namespace","name":"workspace","tools":[declarations[0]]}
        ]});
        let parsed = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert_eq!(parsed.specs.len(), 256);
        let collision = json!({"tools":parsed.catalog(),"input":[
            {"type":"additional_tools","tools":[
                {"type":"function","name":"workspace.fixture_000","parameters":{"type":"object"}}
            ]}
        ]});
        assert!(matches!(
            ClientTools::parse(collision.as_object().unwrap()),
            Err(ExcelRequestError::Tool)
        ));
    }

    #[test]
    fn duplicate_function_tools_normalize_aliases_without_relaxing_arguments() {
        let schema = json!({"type":"object","properties":{"path":{"type":"string"}},
            "required":["path"],"additionalProperties":false});
        for alias in ["parameters", "inputSchema", "input_schema"] {
            let current = json!({"type":"function","name":"read","description":"Current",
                "parameters":schema,"strict":true,"encrypted":false});
            let historical = json!({"type":"function","name":"read","description":"Older",
                alias:schema,"strict":true,"encrypted":false,"defer_loading":true});
            let source = json!({"tools":[current.clone(),historical]});
            let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
            assert_eq!(tools.catalog(), json!([current]));
            assert!(!tools.instructions().contains("Older"));
            let native = json!({"type":"function_call","name":"read","call_id":"call_fixture",
                "arguments":"{\"path\":12}"});
            assert_eq!(
                tools.convert_call(&native),
                Err(ExcelRequestError::ToolCall)
            );
        }
    }

    #[test]
    fn duplicate_tool_definitions_keep_unknown_constraints_and_namespace_identity() {
        let current = json!({"type":"custom","name":"patch","description":"Current",
            "format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}});
        for (field, value) in [
            ("type", json!("function")),
            ("format", json!({"type":"text"})),
            ("strict", json!(true)),
            ("parameters", json!({"type":"object"})),
            ("encrypted", json!(true)),
            ("future_execution_constraint", json!({"allowed":false})),
        ] {
            let mut conflicting = current.clone();
            conflicting[field] = value;
            let source = json!({"tools":[current.clone(),conflicting]});
            assert!(
                matches!(
                    ClientTools::parse(source.as_object().unwrap()),
                    Err(ExcelRequestError::Tool)
                ),
                "{field}"
            );
        }
        let collision = json!({"tools":[
            {"type":"custom","name":"workspace.patch"},
            {"type":"namespace","name":"workspace","tools":[{"type":"custom","name":"patch"}]}
        ]});
        assert!(ClientTools::parse(collision.as_object().unwrap()).is_err());
    }

    #[test]
    fn function_schema_alias_precedence_matches_reference_null_handling() {
        let schema = json!({"type":"object","properties":{"value":{"type":"integer"}}});
        let source = json!({"tools":[
            {"type":"function","name":"read","parameters":null,"inputSchema":schema},
            {"type":"function","name":"read","input_schema":schema}
        ]});
        let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
        assert_eq!(tools.specs["read"].schema, schema);
        assert_ne!(
            tool_definition(&json!({"type":"function","name":"read"})),
            tool_definition(&json!({"type":"function","name":"read","parameters":{}}))
        );
        assert_eq!(
            tool_definition(
                &json!({"type":"function","name":"read","parameters":schema,"inputSchema":{"unused":true}})
            ),
            tool_definition(&json!({"type":"function","name":"read","parameters":schema}))
        );
    }

    #[test]
    fn hosted_omit_policy_distinguishes_none_forced_and_client_named_tools() {
        for kind in [
            "web_search",
            "web_search_preview",
            "web_search_preview_2025_03_11",
            "web_search_2025_08_26",
            "image_generation",
        ] {
            for external in [Value::Null, json!(true), json!(false)] {
                let mut hosted = json!({"type":kind});
                if !external.is_null() {
                    hosted["external_web_access"] = external;
                }
                for choice in [json!("auto"), json!("none"), json!({"type":kind})] {
                    let source = json!({"tools":[hosted.clone(),{"type":"function","name":"web_search_client"}],"tool_choice":choice});
                    let parsed = ClientTools::parse(source.as_object().unwrap());
                    if choice.is_object() {
                        assert!(matches!(parsed, Err(ExcelRequestError::Tool)));
                    } else {
                        let tools = parsed.unwrap();
                        assert_eq!(
                            tools.instructions().contains("unavailable on this route"),
                            choice != "none"
                        );
                        assert_eq!(tools.has_client_tools(), choice != "none");
                    }
                }
            }
        }
        for (kind, name) in [
            ("function", "web_search_client"),
            ("custom", "image_generation_client"),
        ] {
            let source = json!({"tools":[{"type":kind,"name":name}],"tool_choice":{"type":kind,"name":name}});
            let tools = ClientTools::parse(source.as_object().unwrap()).unwrap();
            assert!(tools.has_client_tools());
            assert!(!tools.instructions().contains("unavailable on this route"));
        }
    }

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
                Err(if name == "delete" {
                    ExcelRequestError::UnknownTool
                } else {
                    ExcelRequestError::ToolCall
                })
            );
        }
    }
}
