//! OpenAI Responses 的透明 wire adapter 与 Core canonical facts 投影。

mod error;
mod http;
mod keepalive;
mod request;
mod response;
pub mod websocket;

pub use error::{ProtocolError, ProtocolErrorBody, RequestDecodeError, ResponseEncodeError};
pub(crate) use http::responses;
pub(super) use http::{PendingExecution, request_client_context};
pub use http::{collect_execution_response, stream_execution_response};
pub(in crate::openai) use http::{collect_execution_response_as, stream_execution_response_as};
pub(in crate::openai) use request::decompress_request_body_limit;
pub(crate) use request::passthrough_request_headers;
pub use request::{
    ContinuationIntent, DecodedResponsesRequest, OpenAiRequestHeaders, ResponsesRequestMetadata,
    decode_request_with_headers,
};
pub use response::OpenAiResponsesEncoder;
pub(crate) use websocket::responses_websocket;
pub use websocket::{ResponseCreateFrameError, decode_response_create_with_context};

use gateway_core::event::ProviderResponseHeader;

pub(super) fn response_connection_options(headers: &[ProviderResponseHeader]) -> Vec<String> {
    headers
        .iter()
        .filter(|header| header.name().trim().eq_ignore_ascii_case("connection"))
        .filter_map(|header| std::str::from_utf8(header.value()).ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

pub(super) fn response_header_is_forwardable(name: &str, connection_options: &[String]) -> bool {
    gateway_protocol::http::response_header_is_forwardable(name, connection_options)
        && !gateway_protocol::openai::is_private_response_header(name)
        && !name.trim().eq_ignore_ascii_case("x-goog-api-key")
}
