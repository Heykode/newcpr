//! Provider-owned UA validation and coherent control-plane projections.

use gateway_admin::model::user_agent::{OutboundUserAgentView, ProviderUserAgentOverride};
use gateway_admin::ports::provider::{ProviderAdminError, ProviderAdminErrorKind};

use crate::transport::profile::{
    CodexWireProfileError, CodexWireProfileOverride, CodexWireProfileSettings,
    CodexWireProfileState,
};

pub(crate) fn current(profile: &CodexWireProfileState) -> OutboundUserAgentView {
    view(profile.settings_snapshot())
}

pub(crate) fn preview(
    profile: &CodexWireProfileState,
    selection: &ProviderUserAgentOverride,
) -> Result<OutboundUserAgentView, ProviderAdminError> {
    let candidate = CodexWireProfileState::new(profile.default_snapshot());
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
        CodexWireProfileOverride::Default => ProviderUserAgentOverride::Default,
        CodexWireProfileOverride::Custom(profile) => ProviderUserAgentOverride::Custom {
            user_agent: profile.user_agent(),
        },
    };
    let profile = settings.effective_profile;
    OutboundUserAgentView {
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
