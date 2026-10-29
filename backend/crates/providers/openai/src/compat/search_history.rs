//! Native Codex search history compatibility.

use serde_json::{Map, Value, json};

use crate::transport::protocol::responses::CodexResponsesRequest;

// TODO(https://github.com/openai/codex/issues/51962): 官方修复历史搜索项与工具声明不匹配后，
// 用含 web_search_call 的普通请求和 Lite 压缩请求验证直连均可完成，再移除此补偿及对应合同测试
// 官方报告 Codex 0.163.0 本地压缩可保留搜索历史却使用空工具，补声明可消除对应 500
pub(crate) fn normalize_search_history(request: &mut CodexResponsesRequest, request_id: &str) {
    // NewCPR has no plugin header overrides; this is the effective wire flag.
    let responses_lite = request.responses_lite.as_deref() == Some("true");
    if normalize_body(request.body_mut(), responses_lite) {
        if !responses_lite {
            request.search_history_tool_index = request
                .body()
                .get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.len().checked_sub(1));
        }
        tracing::debug!(
            request_id,
            compatibility = "web_search_history",
            responses_lite,
            "OpenAI search history declaration supplemented"
        );
    }
}

fn normalize_body(body: &mut Map<String, Value>, lite: bool) -> bool {
    // 原生续接的历史声明可能仅在上游连接中，不能从本轮增量推断工具缺失
    if body
        .get("previous_response_id")
        .is_some_and(|id| !id.is_null())
    {
        return false;
    }
    let Some(input) = body.get("input").and_then(Value::as_array) else {
        return false;
    };
    if !input.iter().any(|item| item["type"] == "web_search_call") {
        return false;
    }
    let tools = match body.get("tools") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(tools)) => tools.as_slice(),
        _ => return false,
    };
    if declares_search(tools) {
        return false;
    }
    let mut has_tools = !tools.is_empty();
    for item in input {
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some("additional_tools" | "tool_search_output")
        ) {
            let Some(tools) = item.get("tools").and_then(Value::as_array) else {
                // 不修补非法工具结构，保留上游原有校验
                return false;
            };
            if declares_search(tools) {
                return false;
            }
            has_tools |= !tools.is_empty();
        }
    }
    let disable_tools = !has_tools;
    if disable_tools
        && !body
            .get("tool_choice")
            .is_none_or(|choice| matches!(choice.as_str(), Some("auto" | "none")))
    {
        return false;
    }
    let insert_at = input.len()
        - usize::from(
            input
                .last()
                .is_some_and(|item| item["type"] == "compaction_trigger"),
        );
    let tool = json!({"type":"web_search", "external_web_access":false});
    if lite {
        let Some(input) = body.get_mut("input").and_then(Value::as_array_mut) else {
            return false;
        };
        // 独立追加声明，不改写已有历史项及其 ID，且保留末尾压缩触发项的位置
        input.insert(
            insert_at,
            json!({"type":"additional_tools", "role":"developer", "tools":[tool]}),
        );
    } else if let Some(Value::Array(tools)) = body.get_mut("tools") {
        tools.push(tool);
    } else {
        body.insert("tools".to_owned(), json!([tool]));
    }
    // 无工具请求只获得历史解释声明；已有工具请求保留调用方的工具选择，新增搜索仅可读缓存
    if disable_tools {
        body.insert("tool_choice".to_owned(), json!("none"));
    }
    true
}

fn declares_search(tools: &[Value]) -> bool {
    tools.iter().any(|tool| {
        tool.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "web_search" || kind.starts_with("web_search_preview"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_continuations_invalid_tools_and_forced_absent_tools() {
        for extra in [
            json!({"previous_response_id":"resp_original"}),
            json!({"tools": {"future":true}}),
            json!({"tool_choice":"required"}),
            json!({"tool_choice":null}),
            json!({"tools":[{"type":"web_search_preview"}]}),
        ] {
            let mut body = json!({"input":[{"type":"web_search_call","id":"ws_original"}]})
                .as_object()
                .unwrap()
                .clone();
            body.extend(extra.as_object().unwrap().clone());
            let original = body.clone();
            assert!(!normalize_body(&mut body, true));
            assert_eq!(body, original);
        }
    }

    #[test]
    fn lite_preserves_history_compaction_order_and_is_idempotent() {
        for has_tools in [false, true] {
            let mut body = json!({"input":[{"type":"web_search_call","id":"ws_original"},
                {"type":"compaction_trigger"}]})
            .as_object()
            .unwrap()
            .clone();
            if has_tools {
                body.insert("tools".into(), json!([{"type":"function","name":"echo"}]));
                body.insert("tool_choice".into(), json!("required"));
            }
            assert!(normalize_body(&mut body, true));
            assert_eq!(body["input"][0]["id"], "ws_original");
            assert_eq!(body["input"][1]["type"], "additional_tools");
            assert_eq!(body["input"][2]["type"], "compaction_trigger");
            assert_eq!(
                body["tool_choice"],
                if has_tools { "required" } else { "none" }
            );
            let once = body.clone();
            assert!(!normalize_body(&mut body, true));
            assert_eq!(body, once);
        }
    }
}
