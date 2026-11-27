//! Chat 入口与交付，协议转换由所选 Provider 完成。

pub(in crate::openai) mod delivery;
mod http;
pub(in crate::openai) use delivery::chat_error_frame;
pub(crate) use http::chat_completions;
