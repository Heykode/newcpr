//! Account maintenance material stays in the private relogin store, never provider documents.
use super::relogin::{ReloginCredential, ReloginRequest};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSession {
    #[serde(default)]
    pub source_ip: Option<std::net::Ipv6Addr>,
    #[serde(default)]
    pub egress_mode: Option<gateway_core::provider_ports::egress::EgressMode>,
    pub email: String,
    pub device_id: String,
    #[serde(default)]
    pub session_id: String,
    pub cookies: Vec<WebCookie>,
    pub verified_at: DateTime<Utc>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub expires: Option<i64>,
    #[serde(default)]
    pub host_only: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaintenanceAction {
    EditMaterial,
    CheckSession,
    Devices,
    Workspaces,
    Security,
    RevokeDevice,
    LogoutAll,
    LogoutAndRelogin,
    Authorize,
    ForceLogin,
    SwitchWorkspace,
    PrepareTotp,
    ActivateTotp,
    DisableTotp,
}
impl MaintenanceAction {
    pub fn destructive(self) -> bool {
        matches!(
            self,
            Self::RevokeDevice
                | Self::LogoutAll
                | Self::LogoutAndRelogin
                | Self::PrepareTotp
                | Self::ActivateTotp
                | Self::DisableTotp
        )
    }
    pub fn authorizes(self) -> bool {
        matches!(
            self,
            Self::Authorize | Self::ForceLogin | Self::SwitchWorkspace | Self::LogoutAndRelogin
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::EditMaterial => "修改本地登录资料",
            Self::CheckSession => "检测会话",
            Self::Devices => "读取设备",
            Self::Workspaces => "读取工作区",
            Self::Security => "核对安全状态",
            Self::RevokeDevice => "退出设备",
            Self::LogoutAll => "退出全部设备",
            Self::LogoutAndRelogin => "退出并重登",
            Self::Authorize => "重新授权",
            Self::ForceLogin => "密码与 2FA 重登",
            Self::SwitchWorkspace => "切换工作区并授权",
            Self::PrepareTotp => "准备 2FA",
            Self::ActivateTotp => "激活 2FA",
            Self::DisableTotp => "停用旧 2FA",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReloginEgressView {
    pub kind: String,
    pub label: String,
    pub source: String,
    pub account_id: Option<String>,
    pub proxy_id: Option<String>,
    pub address: Option<String>,
    pub exit_ip: Option<String>,
    pub latency_ms: Option<u64>,
    pub checked_at: Option<DateTime<Utc>>,
    pub source_ip: Option<std::net::Ipv6Addr>,
    pub mode: Option<gateway_core::provider_ports::egress::EgressMode>,
}

// Secrets and sensitive identifiers must not acquire a Debug implementation.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceCommand {
    pub id: String,
    pub revision: u64,
    pub action: MaintenanceAction,
    pub target: Option<String>,
    pub device_id_hash: Option<String>,
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MaintenanceView {
    pub action: Option<MaintenanceAction>,
    pub state: String,
    pub message: String,
    pub checked_at: Option<DateTime<Utc>>,
    pub session_state: String,
    pub session_checked_at: Option<DateTime<Utc>>,
    pub devices: Vec<MaintenanceDevice>,
    pub workspaces: Vec<MaintenanceWorkspace>,
    pub totp_enabled: Option<bool>,
    pub password_enabled: Option<bool>,
    pub pending_totp: bool,
    pub uncertain: bool,
    pub login_paused: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceDevice {
    pub id: String,
    pub session_id: Option<String>,
    pub device_id_hash: Option<String>,
    pub name: String,
    pub platform: String,
    pub current: bool,
    pub trusted: bool,
    pub last_active_at: Option<i64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaintenanceWorkspace {
    pub id: String,
    pub name: String,
    pub plan_type: Option<String>,
    pub available: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PendingTotp {
    pub secret: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MaterialVersion {
    pub password: String,
    pub mfa_secret: String,
    pub saved_at: DateTime<Utc>,
    #[serde(default)]
    pub pending_totp: Option<PendingTotp>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct MaintenanceState {
    #[serde(default)]
    pub import_egress: Option<ReloginImportEgress>,
    #[serde(default)]
    pub last_activity: Option<super::relogin::ReloginAttempt>,
    #[serde(default)]
    pub uncertain: bool,
    #[serde(default)]
    pub login_paused: bool,
    #[serde(default)]
    pub view: MaintenanceView,
    #[serde(default)]
    pub session: Option<WebSession>,
    #[serde(default)]
    pub pending_totp: Option<PendingTotp>,
    #[serde(default)]
    pub material_history: Vec<MaterialVersion>,
}
impl MaintenanceState {
    /// Shared precondition for automatic recovery and every explicit queue entry point.
    pub fn blocks_login(&self) -> bool {
        self.uncertain
            || matches!(self.view.state.as_str(), "running" | "uncertain")
            || self.pending_totp.is_some()
            || self.view.totp_enabled == Some(false)
    }
}

pub struct MaintenanceRequest {
    pub login: ReloginRequest,
    pub action: MaintenanceAction,
    pub target: Option<String>,
    pub device_id_hash: Option<String>,
    pub pending_totp: Option<PendingTotp>,
}
#[derive(Clone)]
pub struct MaintenanceOutcome {
    pub view: MaintenanceView,
    pub session: Option<WebSession>,
    pub credential: Option<ReloginCredential>,
    pub pending_totp: Option<PendingTotp>,
    pub new_secret: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceMaterial {
    pub revision: u64,
    pub needs_resolution: bool,
    pub password: String,
    pub mfa_secret: String,
    pub pending_totp_secret: Option<String>,
    pub history: Vec<MaterialVersion>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialUpdate {
    pub id: String,
    pub revision: u64,
    pub password: String,
    pub mfa_secret: String,
    #[serde(default)]
    pub confirm_maintenance_resolution: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ReloginImportEgress {
    pub proxy_id: Option<String>,
    pub mode: Option<Option<gateway_core::provider_ports::egress::EgressMode>>,
}
