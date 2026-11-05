//! Provider-owned UA validation and coherent control-plane projections.

use gateway_admin::model::user_agent::{OutboundUserAgentView, ProviderUserAgentOverride};
use gateway_admin::ports::provider::{ProviderAdminError, ProviderAdminErrorKind};

use crate::transport::profile::{
    CodexWireProfileError, CodexWireProfileOverride, CodexWireProfileSettings,
    CodexWireProfileState,
};

pub(crate) fn current(profile: &CodexWireProfileState) -> OutboundUserAgentView {
    let mut result = view(profile.settings_snapshot());
    result.profile_options = profile.selection_options().ok();
    if let ProviderUserAgentOverride::Configuration { document } = &result.selection {
        result.profile_preview = profile.preview_selection(document).ok();
    }
    result
}

pub(crate) fn preview(
    profile: &CodexWireProfileState,
    selection: &ProviderUserAgentOverride,
) -> Result<OutboundUserAgentView, ProviderAdminError> {
    let candidate = profile.frozen();
    if let ProviderUserAgentOverride::Configuration { document } = selection {
        candidate.preview_selection(document).map_err(|error| {
            use crate::transport::profile::selection::ClientProfileError;
            ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(
                match error {
                    ClientProfileError::Invalid => "客户端身份字段或版本组合不合法",
                    ClientProfileError::InvalidInstallationId => "安装 ID 必须是带连字符的 UUID",
                    ClientProfileError::InvalidUserAgent => {
                        "User-Agent 必须是 1 至 4096 字节的单行 ASCII 文本，且首尾不能含空白"
                    }
                    ClientProfileError::CompanionHeadersRequired => {
                        "无法识别 User-Agent，请补充 originator 和有效的 Core version"
                    }
                    ClientProfileError::CompanionHeadersConflict => {
                        "originator 或 Core version 与 User-Agent 不一致"
                    }
                    ClientProfileError::ReleaseUnavailable => {
                        "此客户端、平台与架构尚无已核验发布版本，请选择固定版本或稍后重试"
                    }
                },
            )
        })?;
    }
    candidate
        .apply_user_agent_override(selection)
        .map_err(map_error)?;
    Ok(current(&candidate))
}

pub(crate) fn apply(
    profile: &CodexWireProfileState,
    selection: &ProviderUserAgentOverride,
) -> Result<OutboundUserAgentView, ProviderAdminError> {
    profile
        .apply_user_agent_override(selection)
        .map_err(map_error)?;
    Ok(current(profile))
}

fn view(settings: CodexWireProfileSettings) -> OutboundUserAgentView {
    let selection = match settings.mode {
        CodexWireProfileOverride::Configuration { document, .. } => {
            ProviderUserAgentOverride::Configuration { document }
        }
        CodexWireProfileOverride::Default => ProviderUserAgentOverride::Default,
        CodexWireProfileOverride::Custom(profile) => ProviderUserAgentOverride::Custom {
            user_agent: profile.user_agent(),
        },
        CodexWireProfileOverride::CustomAutoVersion(profile) => {
            ProviderUserAgentOverride::CustomAutoVersion {
                user_agent: profile.user_agent(),
            }
        }
    };
    let profile = settings.effective_profile;
    OutboundUserAgentView {
        profile_options: None,
        profile_preview: None,
        verified: matches!(selection, ProviderUserAgentOverride::Default),
        selection,
        default_user_agent: settings.default_user_agent,
        effective_user_agent: settings.effective_user_agent,
        effective_desktop_user_agent: settings.effective_desktop_user_agent,
        core_version: profile.codex_version,
        desktop_version: settings.effective_desktop_version,
        os_type: profile.os_type,
        os_version: profile.os_version,
        arch: profile.arch,
        terminal: profile.terminal,
        default_verified_at: settings.default_verified_at,
    }
}

fn map_error(error: CodexWireProfileError) -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Invalid).with_public_message(match error {
        CodexWireProfileError::Unsafe => "UA 最多 512 字节，不能包含换行或控制字符",
        CodexWireProfileError::InvalidSyntax => {
            "请填写完整 UA：Codex Desktop、codex-tui 或 codex_cli_rs"
        }
        CodexWireProfileError::Incoherent => "UA 的客户端名称、版本、系统或架构不配套",
    })
}
