//! Supported CLI/Exec UA grammar, independent of transport.

use super::{CodexWireProfile, CodexWireProfileError, safe_token};

pub const DEFAULT_USER_AGENT: &str = "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color";

/// Parse only a supported CLI/Exec grammar, retaining the exact safe header value.
pub(super) fn parse(
    value: &str,
    desktop: &CodexWireProfile,
) -> Result<CodexWireProfile, CodexWireProfileError> {
    if value.len() > 512 || !value.is_ascii() || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(CodexWireProfileError::Unsafe);
    }
    let (product, tail) = value
        .split_once(" (")
        .ok_or(CodexWireProfileError::InvalidSyntax)?;
    let (originator, version) = product
        .split_once('/')
        .ok_or(CodexWireProfileError::InvalidSyntax)?;
    if !matches!(originator, "codex-tui" | "codex_cli_rs" | "codex_exec")
        || semver::Version::parse(version).is_err()
    {
        return Err(CodexWireProfileError::Incoherent);
    }
    let (target, tail) = tail
        .split_once(") ")
        .ok_or(CodexWireProfileError::InvalidSyntax)?;
    let (terminal, suffix) = tail
        .split_once(" (")
        .map_or((tail, None), |(terminal, suffix)| (terminal, Some(suffix)));
    if !safe_token(terminal) {
        return Err(CodexWireProfileError::Incoherent);
    }
    if let Some(suffix) = suffix {
        let (client, client_version) = suffix
            .strip_suffix(')')
            .and_then(|suffix| suffix.split_once("; "))
            .ok_or(CodexWireProfileError::InvalidSyntax)?;
        if client != originator || client_version != version {
            return Err(CodexWireProfileError::Incoherent);
        }
    }
    let (os_type, target) = ["Mac OS", "Windows", "Linux", "Ubuntu"]
        .into_iter()
        .find_map(|os| {
            target
                .strip_prefix(&format!("{os} "))
                .map(|rest| (os, rest))
        })
        .ok_or(CodexWireProfileError::InvalidSyntax)?;
    let (os_version, arch) = target
        .split_once("; ")
        .ok_or(CodexWireProfileError::InvalidSyntax)?;
    let architecture_os = if os_type == "Ubuntu" {
        "Linux"
    } else {
        os_type
    };
    if !safe_token(os_version)
        || !os_version
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_digit)
        || !super::valid_architecture(architecture_os, arch)
    {
        return Err(CodexWireProfileError::Incoherent);
    }
    Ok(CodexWireProfile {
        raw_user_agent: Some(value.to_owned()),
        originator: originator.to_owned(),
        codex_version: version.to_owned(),
        os_type: os_type.to_owned(),
        os_version: os_version.to_owned(),
        arch: arch.to_owned(),
        terminal: terminal.to_owned(),
        verified_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
        ..desktop.clone()
    })
}
