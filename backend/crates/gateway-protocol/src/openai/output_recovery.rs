//! Buffered-only recovery from already collected, borrowed Responses wire events.

use serde_json::{Value, json};

/// Supplements a terminal response without changing its status or envelope.
///
/// Nonempty terminal output is authoritative; only empty tool input may be filled
/// from the same call. Otherwise raw done items win over delta reconstruction.
/// The caller still owns terminal/failure classification and execution finalization.
pub fn recover_response_output<'a>(
    response: &mut Value,
    events: impl IntoIterator<Item = (&'a str, &'a Value)>,
) {
    if !response.is_object()
        || response["status"]
            .as_str()
            .is_some_and(|status| !matches!(status, "completed" | "incomplete"))
        || response.get("error").is_some_and(|error| !error.is_null())
    {
        return;
    }
    let authoritative = response["output"]
        .as_array()
        .is_some_and(|output| !output.is_empty());
    if authoritative
        && !response["output"]
            .as_array()
            .into_iter()
            .flatten()
            .any(needs_tool_input)
    {
        return;
    }

    let mut items = Vec::<Item<'_>>::new();
    for (kind, event) in events {
        if matches!(kind, "error" | "response.failed" | "response.cancelled")
            || matches!(
                event["type"].as_str(),
                Some("error" | "response.failed" | "response.cancelled")
            )
        {
            return;
        }
        observe(&mut items, kind, event);
    }

    if authoritative {
        for (index, output) in response["output"]
            .as_array_mut()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if needs_tool_input(output) {
                let identity = Identity::new(output, Some(index as u64));
                if let Some(index) = locate(&items, &identity, false) {
                    items[index].supplement_input(output);
                }
            }
        }
        return;
    }

    // Stable sort keeps arrival order for missing or tied output indices. Missing
    // indices are never coerced to zero, so unrelated raw items cannot overwrite.
    items.sort_by_key(|item| {
        (
            item.identity.index.is_none(),
            item.identity.index.unwrap_or_default(),
        )
    });
    let output: Vec<_> = items
        .iter()
        .filter_map(|item| {
            if let Some(done) = item.done {
                let mut output = done.clone();
                item.supplement_input(&mut output);
                Some(output)
            } else if items
                .iter()
                .any(|other| other.done.is_some() && item.identity.same_slot(other.identity))
            {
                // A complete item owns its slot even when a preceding partial
                // reported a conflicting call ID. Never replay that stale call.
                None
            } else {
                item.reconstruct()
            }
        })
        .collect();
    if !output.is_empty() {
        response["output"] = Value::Array(output);
    }
}

#[derive(Clone, Copy, Default)]
struct Identity<'a> {
    index: Option<u64>,
    id: Option<&'a str>,
    call_id: Option<&'a str>,
    name: Option<&'a str>,
    kind: Option<&'a str>,
}

impl<'a> Identity<'a> {
    fn new(item: &'a Value, index: Option<u64>) -> Self {
        Self {
            index,
            id: nonempty(&item["id"]),
            call_id: nonempty(&item["call_id"]),
            name: nonempty(&item["name"]),
            kind: nonempty(&item["type"]),
        }
    }

    fn event(event: &'a Value, kind: Option<&'a str>) -> Self {
        let mut identity = Self::new(&event["item"], event["output_index"].as_u64());
        identity.id = identity.id.or_else(|| nonempty(&event["item_id"]));
        identity.call_id = identity.call_id.or_else(|| nonempty(&event["call_id"]));
        identity.name = identity.name.or_else(|| nonempty(&event["name"]));
        identity.kind = identity.kind.or(kind);
        identity
    }

    fn compatible(self, other: Self) -> bool {
        [
            (self.id, other.id),
            (self.call_id, other.call_id),
            (self.name, other.name),
            (self.kind, other.kind),
        ]
        .into_iter()
        .all(|(left, right)| left.zip(right).is_none_or(|(left, right)| left == right))
    }

    fn merge(&mut self, other: Self) {
        self.index = other.index.or(self.index);
        self.id = other.id.or(self.id);
        self.call_id = other.call_id.or(self.call_id);
        self.name = other.name.or(self.name);
        self.kind = other.kind.or(self.kind);
    }

    fn has_key(self) -> bool {
        self.index.is_some() || self.id.is_some() || self.call_id.is_some()
    }

    fn same_slot(self, other: Self) -> bool {
        self.id.is_some() && self.id == other.id
            || self.call_id.is_some() && self.call_id == other.call_id
            || self.index.is_some() && self.index == other.index
    }
}

fn locate(items: &[Item<'_>], identity: &Identity<'_>, keyless: bool) -> Option<usize> {
    let compatible = |item: &Item<'_>| identity.compatible(item.identity);
    for key in 0..3 {
        if let Some(index) = items.iter().position(|item| {
            compatible(item)
                && match key {
                    0 => identity.id.is_some() && identity.id == item.identity.id,
                    1 => identity.call_id.is_some() && identity.call_id == item.identity.call_id,
                    _ => identity.index.is_some() && identity.index == item.identity.index,
                }
        }) {
            return Some(index);
        }
    }
    if !keyless || identity.has_key() {
        return None;
    }
    // Keyless deltas are usable only with one unambiguous item of that kind.
    // Raw keyless done items instead retain their independent arrival order.
    let mut matches = items
        .iter()
        .enumerate()
        .filter(|(_, item)| compatible(item));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

#[derive(Default)]
struct Text<'a> {
    deltas: Vec<&'a str>,
    done: Option<&'a str>,
}

impl<'a> Text<'a> {
    fn observe(&mut self, text: &'a Value, done: bool) {
        if let Some(text) = nonempty(text) {
            if done {
                self.done = Some(text);
            } else if self.done.is_none() {
                self.deltas.push(text);
            }
        }
    }

    fn value(&self) -> Option<String> {
        self.done
            .map(str::to_owned)
            .or_else(|| (!self.deltas.is_empty()).then(|| self.deltas.concat()))
    }
}

struct Part<'a> {
    field: &'static str,
    index: u64,
    kind: &'static str,
    text: Text<'a>,
}

#[derive(Default)]
struct Item<'a> {
    identity: Identity<'a>,
    added: Option<&'a Value>,
    done: Option<&'a Value>,
    input: Text<'a>,
    parts: Vec<Part<'a>>,
}

impl Item<'_> {
    fn supplement_input(&self, output: &mut Value) {
        if !needs_tool_input(output) {
            return;
        }
        let field = input_field(output["type"].as_str());
        let input = self
            .done
            .and_then(|item| nonempty(&item[field]))
            .map(str::to_owned)
            .or_else(|| self.input.value())
            .or_else(|| {
                self.added
                    .and_then(|item| nonempty(&item[field]))
                    .map(str::to_owned)
            });
        if let Some(input) = input {
            output[field] = Value::String(input);
        }
    }

    fn reconstruct(&self) -> Option<Value> {
        let kind = self.identity.kind?;
        let tool = matches!(kind, "function_call" | "custom_tool_call");
        if self.parts.is_empty() && !(tool && self.added.is_some()) {
            return None;
        }
        let mut output = self.added.cloned().unwrap_or_else(|| json!({"type":kind}));
        for (field, value) in [
            ("id", self.identity.id),
            ("call_id", self.identity.call_id),
            ("name", self.identity.name),
        ] {
            if output.get(field).is_none()
                && let Some(value) = value
            {
                output[field] = json!(value);
            }
        }
        if kind == "message" && output.get("role").is_none() {
            output["role"] = json!("assistant");
        }
        for field in ["summary", "content"] {
            let mut parts: Vec<_> = self
                .parts
                .iter()
                .filter(|part| part.field == field)
                .collect();
            parts.sort_by_key(|part| part.index);
            let parts: Vec<_> = parts
                .into_iter()
                .filter_map(|part| {
                    let text = part.text.value()?;
                    let text_field = if part.kind == "refusal" {
                        "refusal"
                    } else {
                        "text"
                    };
                    Some(json!({"type":part.kind,text_field:text}))
                })
                .collect();
            if !parts.is_empty() {
                output[field] = Value::Array(parts);
            }
        }
        if tool && let Some(input) = self.input.value() {
            output[input_field(self.identity.kind)] = Value::String(input);
        } else {
            self.supplement_input(&mut output);
        }
        Some(output)
    }
}

fn observe<'a>(items: &mut Vec<Item<'a>>, kind: &str, event: &'a Value) {
    let (item_kind, part) = match kind {
        "response.output_item.added" | "response.output_item.done" => {
            if !event["item"].is_object() {
                return;
            }
            (None, None)
        }
        "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
            (Some("function_call"), None)
        }
        "response.custom_tool_call_input.delta" | "response.custom_tool_call_input.done" => {
            (Some("custom_tool_call"), None)
        }
        "response.output_text.delta" | "response.output_text.done" => {
            (Some("message"), Some(("content", "output_text", "text")))
        }
        "response.refusal.delta" | "response.refusal.done" => {
            (Some("message"), Some(("content", "refusal", "refusal")))
        }
        "response.reasoning_summary_text.delta" | "response.reasoning_summary_text.done" => {
            (Some("reasoning"), Some(("summary", "summary_text", "text")))
        }
        "response.reasoning_text.delta" | "response.reasoning_text.done" => (
            Some("reasoning"),
            Some(("content", "reasoning_text", "text")),
        ),
        _ => return,
    };
    let raw_item = kind.starts_with("response.output_item.");
    let done = kind.ends_with(".done");
    let text_field = if !done {
        "delta"
    } else {
        part.map_or_else(|| input_field(item_kind), |(_, _, field)| field)
    };
    if !raw_item && nonempty(&event[text_field]).is_none() {
        return;
    }
    let identity = Identity::event(event, item_kind);
    let index = locate(items, &identity, !raw_item).unwrap_or_else(|| {
        items.push(Item::default());
        items.len() - 1
    });
    let item = &mut items[index];
    item.identity.merge(identity);
    if raw_item {
        if done {
            item.done = Some(&event["item"]);
        } else {
            item.added = Some(&event["item"]);
        }
    } else if let Some((field, part_kind, _)) = part {
        let index_field = if field == "summary" {
            "summary_index"
        } else {
            "content_index"
        };
        let index = event[index_field].as_u64().unwrap_or_default();
        let position = item
            .parts
            .iter()
            .position(|part| part.field == field && part.index == index && part.kind == part_kind)
            .unwrap_or_else(|| {
                item.parts.push(Part {
                    field,
                    index,
                    kind: part_kind,
                    text: Text::default(),
                });
                item.parts.len() - 1
            });
        item.parts[position].text.observe(&event[text_field], done);
    } else {
        item.input.observe(&event[text_field], done);
    }
}

fn input_field(kind: Option<&str>) -> &'static str {
    if kind == Some("custom_tool_call") {
        "input"
    } else {
        "arguments"
    }
}

fn needs_tool_input(item: &Value) -> bool {
    let kind = item["type"].as_str();
    matches!(kind, Some("function_call" | "custom_tool_call"))
        && match item.get(input_field(kind)) {
            None | Some(Value::Null) => true,
            Some(Value::String(value)) => value.is_empty(),
            _ => false,
        }
}

fn nonempty(value: &Value) -> Option<&str> {
    value.as_str().filter(|value| !value.is_empty())
}
