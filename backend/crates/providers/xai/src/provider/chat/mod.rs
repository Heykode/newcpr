//! Chat 工具使用 Grok 的函数格式；原生 Responses 只做协议观测。
use super::*;
use gateway_protocol::chat_completions::{CHAT_PROTOCOL, ChatIdentity};
mod response;
mod tools;
pub(super) fn translator(
    generate: &GenerateRequest,
    model: &UpstreamModelId,
    context: &AttemptContext,
) -> impl gateway_core::engine::provider::NativeResponseTranslator {
    let original = generate.protocol_payload().body();
    let identity = ChatIdentity {
        id: format!("chatcmpl-{}", context.request_id().as_str()),
        created: SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        model: original
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(model.as_str())
            .to_owned(),
    };
    response::Translator::new(identity, original)
}
