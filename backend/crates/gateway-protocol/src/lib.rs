//! 网关协议编解码器。
//!
//! 本 crate 只处理 wire contract，不包含 Provider 认证、路由、重试或传输策略。

/// OpenAI wire contract。
pub mod openai;

/// Anthropic Messages wire contract.
pub mod anthropic;
/// Direct Chat conversion for each supported upstream protocol.
pub mod chat_completions;
/// Protocol-specific request facts.
pub mod request;

mod capabilities;
pub mod genai;
pub mod peer;
