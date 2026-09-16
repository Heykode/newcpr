use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::{ChatConversionError as Error, Result, nonempty, object, only_keys, string};

pub(super) fn convert(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
) -> Result<BTreeSet<String>> {
    let mut tools = Vec::new();
    let mut names = BTreeMap::new();
    let mut builtins = BTreeSet::new();
    if let Some(value) = source.get("tools").filter(|v| !v.is_null()) {
        let definitions = value.as_array().ok_or_else(|| Error::invalid("tools"))?;
        for (index, tool) in definitions.iter().enumerate() {
            let path = format!("tools[{index}]");
            let raw = object(tool, &path)?;
            let kind = string(&tool["type"], &format!("{path}.type"))?;
            let converted = match kind {
                "function" | "custom" => {
                    let converted = if kind == "function" {
                        function_tool(raw, &path)?
                    } else {
                        custom_tool(raw, &path)?
                    };
                    let name_path = if kind == "custom" && !raw.contains_key("custom") {
                        format!("{path}.name")
                    } else {
                        format!("{path}.{kind}.name")
                    };
                    let name = nonempty(converted.get("name").unwrap_or(&Value::Null), &name_path)?;
                    if names.insert(name.to_owned(), kind).is_some() {
                        return Err(Error::invalid(name_path));
                    }
                    converted
                }
                "image_generation" | "web_search" => {
                    if !builtins.insert(kind) {
                        return Err(Error::invalid(format!("{path}.type")));
                    }
                    if kind == "image_generation" {
                        image_tool(raw, &path)?;
                    } else {
                        search_tool(raw, &path)?;
                    }
                    empty_function(raw, &path)?;
                    let mut converted = raw.clone();
                    converted.remove("function");
                    converted
                }
                _ => return Err(Error::unsupported(format!("{path}.type"))),
            };
            tools.push(Value::Object(converted));
        }
    }
    if let Some(search) = source.get("web_search_options").filter(|v| !v.is_null()) {
        // Explicit tools and legacy options each define a complete search configuration.
        if !builtins.insert("web_search") {
            return Err(Error::invalid("web_search_options"));
        }
        let options = object(search, "web_search_options")?;
        only_keys(
            options,
            &["search_context_size", "user_location"],
            "web_search_options",
        )?;
        let mut converted = options.clone();
        if let Some(size) = options.get("search_context_size")
            && !size
                .as_str()
                .is_some_and(|s| ["low", "medium", "high"].contains(&s))
        {
            return Err(Error::invalid("web_search_options.search_context_size"));
        }
        if let Some(location) = options.get("user_location").filter(|v| !v.is_null()) {
            let location_object = object(location, "web_search_options.user_location")?;
            only_keys(
                location_object,
                &["type", "approximate"],
                "web_search_options.user_location",
            )?;
            if location["type"] != "approximate" {
                return Err(Error::invalid("web_search_options.user_location.type"));
            }
            let approximate = object(
                &location["approximate"],
                "web_search_options.user_location.approximate",
            )?;
            only_keys(
                approximate,
                &["city", "country", "region", "timezone"],
                "web_search_options.user_location.approximate",
            )?;
            if approximate.values().any(|v| !v.is_string()) {
                return Err(Error::invalid(
                    "web_search_options.user_location.approximate",
                ));
            }
            let mut location = approximate.clone();
            location.insert("type".into(), json!("approximate"));
            converted.insert("user_location".into(), Value::Object(location));
        }
        converted.insert("type".into(), json!("web_search"));
        tools.push(Value::Object(converted));
    }
    if let Some(choice) = source.get("tool_choice").filter(|v| !v.is_null()) {
        let converted = match choice.as_str() {
            Some("auto" | "none" | "required") => choice.clone(),
            Some(_) => return Err(Error::invalid("tool_choice")),
            None => {
                let raw = object(choice, "tool_choice")?;
                match choice["type"].as_str() {
                    Some(kind @ ("function" | "custom")) => {
                        let flat_custom = kind == "custom" && !raw.contains_key("custom");
                        let path = if kind == "custom" {
                            empty_function(raw, "tool_choice")?;
                            if flat_custom {
                                only_keys(raw, &["type", "name", "function"], "tool_choice")?;
                                "tool_choice"
                            } else {
                                only_keys(raw, &["type", "custom", "function"], "tool_choice")?;
                                "tool_choice.custom"
                            }
                        } else {
                            only_keys(raw, &["type", "function"], "tool_choice")?;
                            "tool_choice.function"
                        };
                        let named = if flat_custom {
                            raw
                        } else {
                            let named = object(&choice[kind], path)?;
                            only_keys(named, &["name"], path)?;
                            named
                        };
                        let name = nonempty(
                            named.get("name").unwrap_or(&Value::Null),
                            &format!("{path}.name"),
                        )?;
                        let declared_kind = names
                            .get(name)
                            .copied()
                            .ok_or_else(|| Error::invalid("tool_choice"))?;
                        if kind == "custom" && declared_kind != "custom" {
                            return Err(Error::invalid("tool_choice"));
                        }
                        json!({"type":declared_kind,"name":name})
                    }
                    Some(kind @ ("image_generation" | "web_search")) => {
                        only_keys(raw, &["type", "function"], "tool_choice")?;
                        empty_function(raw, "tool_choice")?;
                        if !builtins.contains(kind) {
                            return Err(Error::invalid("tool_choice"));
                        }
                        json!({"type":kind})
                    }
                    _ => return Err(Error::invalid("tool_choice")),
                }
            }
        };
        if tools.is_empty() && choice != "none" && choice != "auto" {
            return Err(Error::invalid("tool_choice"));
        }
        target.insert("tool_choice".into(), converted);
    }
    if source.contains_key("tools") || !tools.is_empty() {
        target.insert("tools".into(), Value::Array(tools));
    }
    Ok(names
        .into_iter()
        .filter_map(|(name, kind)| (kind == "custom").then_some(name))
        .collect())
}

// NewAPI's typed envelope emits an empty function even for a raw custom tool.
pub(super) fn empty_function(raw: &Map<String, Value>, path: &str) -> Result<()> {
    if let Some(function) = raw.get("function") {
        let path = format!("{path}.function");
        let function = object(function, &path)?;
        only_keys(function, &["name"], &path)?;
        if function.get("name").is_some_and(|name| name != "") {
            return Err(Error::invalid(path));
        }
    }
    Ok(())
}

fn function_tool(raw: &Map<String, Value>, path: &str) -> Result<Map<String, Value>> {
    only_keys(raw, &["type", "function"], path)?;
    let function = object(
        raw.get("function").unwrap_or(&Value::Null),
        &format!("{path}.function"),
    )?;
    only_keys(
        function,
        &["name", "description", "parameters", "strict"],
        path,
    )?;
    if let Some(parameters) = function.get("parameters") {
        object(parameters, &format!("{path}.function.parameters"))?;
    }
    if let Some(description) = function.get("description") {
        string(description, &format!("{path}.function.description"))?;
    }
    if let Some(strict) = function.get("strict").filter(|v| !v.is_null())
        && !strict.is_boolean()
    {
        return Err(Error::invalid(format!("{path}.function.strict")));
    }
    let mut converted = function.clone();
    // Chat's omitted strict flag must not opt into Responses strict normalization.
    if converted.get("strict").is_none_or(Value::is_null) {
        converted.insert("strict".into(), json!(false));
    }
    converted.insert("type".into(), json!("function"));
    Ok(converted)
}

fn custom_tool(raw: &Map<String, Value>, path: &str) -> Result<Map<String, Value>> {
    empty_function(raw, path)?;
    let nested_path = format!("{path}.custom");
    let (custom, path) = if let Some(custom) = raw.get("custom") {
        only_keys(raw, &["type", "custom", "function"], path)?;
        let custom = object(custom, &nested_path)?;
        only_keys(custom, &["name", "description", "format"], &nested_path)?;
        (custom, nested_path.as_str())
    } else {
        only_keys(
            raw,
            &["type", "name", "description", "format", "function"],
            path,
        )?;
        (raw, path)
    };
    nonempty(
        custom.get("name").unwrap_or(&Value::Null),
        &format!("{path}.name"),
    )?;
    if let Some(description) = custom.get("description") {
        string(description, &format!("{path}.description"))?;
    }
    let mut converted = custom.clone();
    converted.remove("function");
    if let Some(format) = custom.get("format") {
        let path = format!("{path}.format");
        let raw = object(format, &path)?;
        match format["type"].as_str() {
            Some("text") => only_keys(raw, &["type"], &path)?,
            Some("grammar") => {
                let grammar_path = format!("{path}.grammar");
                let (grammar, grammar_path) = if let Some(grammar) = raw.get("grammar") {
                    only_keys(raw, &["type", "grammar"], &path)?;
                    let grammar = object(grammar, &grammar_path)?;
                    only_keys(grammar, &["syntax", "definition"], &grammar_path)?;
                    (grammar, grammar_path.as_str())
                } else {
                    only_keys(raw, &["type", "syntax", "definition"], &path)?;
                    (raw, path.as_str())
                };
                let syntax = string(
                    grammar.get("syntax").unwrap_or(&Value::Null),
                    &format!("{grammar_path}.syntax"),
                )?;
                if !["lark", "regex"].contains(&syntax) {
                    return Err(Error::invalid(format!("{grammar_path}.syntax")));
                }
                let definition = nonempty(
                    grammar.get("definition").unwrap_or(&Value::Null),
                    &format!("{grammar_path}.definition"),
                )?;
                converted.insert(
                    "format".into(),
                    json!({"type":"grammar","syntax":syntax,"definition":definition}),
                );
            }
            _ => return Err(Error::unsupported(path)),
        }
    }
    converted.insert("type".into(), json!("custom"));
    Ok(converted)
}

fn enum_field(raw: &Map<String, Value>, key: &str, allowed: &[&str], path: &str) -> Result<()> {
    if let Some(value) = raw.get(key)
        && !value.as_str().is_some_and(|value| allowed.contains(&value))
    {
        return Err(Error::invalid(format!("{path}.{key}")));
    }
    Ok(())
}

fn image_tool(raw: &Map<String, Value>, path: &str) -> Result<()> {
    only_keys(
        raw,
        &[
            "type",
            "function",
            "action",
            "background",
            "input_fidelity",
            "input_image_mask",
            "model",
            "moderation",
            "output_compression",
            "output_format",
            "partial_images",
            "quality",
            "size",
        ],
        path,
    )?;
    enum_field(raw, "action", &["auto", "generate", "edit"], path)?;
    enum_field(raw, "background", &["auto", "opaque", "transparent"], path)?;
    enum_field(raw, "moderation", &["auto", "low"], path)?;
    enum_field(raw, "output_format", &["png", "jpeg", "webp"], path)?;
    enum_field(
        raw,
        "quality",
        &["auto", "low", "medium", "high", "xhigh", "max"],
        path,
    )?;
    if raw.get("input_fidelity").is_some_and(|v| !v.is_null()) {
        enum_field(raw, "input_fidelity", &["low", "high"], path)?;
    }
    if let Some(model) = raw.get("model") {
        nonempty(model, &format!("{path}.model"))?;
    }
    for (key, max) in [("output_compression", 100), ("partial_images", 3)] {
        if let Some(value) = raw.get(key)
            && !value.as_u64().is_some_and(|n| n <= max)
        {
            return Err(Error::invalid(format!("{path}.{key}")));
        }
    }
    if let Some(size) = raw.get("size") {
        let path = format!("{path}.size");
        let size = string(size, &path)?;
        if size != "auto" {
            // Model-specific size limits belong to the upstream, not this adapter.
            if !size.split_once('x').is_some_and(|(width, height)| {
                [width, height].iter().all(|dimension| {
                    dimension.bytes().all(|c| c.is_ascii_digit())
                        && dimension.bytes().any(|c| c != b'0')
                })
            }) {
                return Err(Error::invalid(path));
            }
        }
    }
    if raw.get("background").is_some_and(|v| v == "transparent")
        && raw.get("output_format").is_some_and(|v| v == "jpeg")
    {
        return Err(Error::invalid(format!("{path}.output_format")));
    }
    if let Some(mask) = raw.get("input_image_mask") {
        let path = format!("{path}.input_image_mask");
        let mask = object(mask, &path)?;
        only_keys(mask, &["image_url", "file_id"], &path)?;
        if mask.len() != 1 {
            return Err(Error::invalid(path));
        }
        for key in ["image_url", "file_id"] {
            if let Some(value) = mask.get(key) {
                nonempty(value, &format!("{path}.{key}"))?;
            }
        }
    }
    Ok(())
}

fn search_tool(raw: &Map<String, Value>, path: &str) -> Result<()> {
    only_keys(
        raw,
        &[
            "type",
            "function",
            "search_context_size",
            "user_location",
            "filters",
            "external_web_access",
        ],
        path,
    )?;
    enum_field(raw, "search_context_size", &["low", "medium", "high"], path)?;
    if let Some(value) = raw.get("external_web_access")
        && !value.is_boolean()
    {
        return Err(Error::invalid(format!("{path}.external_web_access")));
    }
    if let Some(location) = raw.get("user_location").filter(|v| !v.is_null()) {
        let path = format!("{path}.user_location");
        let location = object(location, &path)?;
        only_keys(
            location,
            &["type", "city", "country", "region", "timezone"],
            &path,
        )?;
        enum_field(location, "type", &["approximate"], &path)?;
        for key in ["city", "country", "region", "timezone"] {
            if let Some(value) = location.get(key).filter(|v| !v.is_null()) {
                string(value, &format!("{path}.{key}"))?;
            }
        }
    }
    if let Some(filters) = raw.get("filters").filter(|v| !v.is_null()) {
        let path = format!("{path}.filters");
        let filters = object(filters, &path)?;
        only_keys(filters, &["allowed_domains"], &path)?;
        if let Some(domains) = filters.get("allowed_domains").filter(|v| !v.is_null()) {
            let path = format!("{path}.allowed_domains");
            let domains = domains.as_array().ok_or_else(|| Error::invalid(&path))?;
            for (index, domain) in domains.iter().enumerate() {
                nonempty(domain, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}
