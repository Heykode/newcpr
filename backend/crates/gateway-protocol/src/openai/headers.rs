//! Codex 头部规则在公共 HTTP 传输边界上保留本地下游身份隔离。
pub use crate::http::parse_retry_after_seconds;

/// 本地反代元数据与调度提示不作为上游业务扩展头。
#[must_use]
pub fn is_transport_managed_request_header(name: &str) -> bool {
    crate::http::is_transport_managed_request_header(name)
        || name.starts_with("cf-")
        || name.starts_with("x-forwarded-")
        || super::is_openai_scheduling_session_hint_header(name)
        || matches!(
            name,
            "forwarded" | "via" | "cdn-loop" | "x-real-ip" | "true-client-ip"
        )
}

/// 账号与安装身份不能随上游响应下发给客户端
#[must_use]
pub fn is_private_response_header(name: &str) -> bool {
    let name = name.trim();
    [
        "chatgpt-account-id",
        "chatgpt-organization-id",
        "chatgpt-org-id",
        "chatgpt-project-id",
        "openai-organization",
        "openai-project",
        "x-openai-organization",
        "x-openai-project",
        "x-codex-installation-id",
        "x-codex-turn-metadata",
    ]
    .iter()
    .any(|private| name.eq_ignore_ascii_case(private))
}
