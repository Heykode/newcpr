//! Independent acquisition state; password/TOTP material stays on the parent row.

use super::*;
use gateway_core::account::CredentialTarget;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReloginChannelState {
    pub automatic: bool,
    pub preferred_workspace_id: Option<String>,
    pub status: ReloginStatus,
    pub message: String,
    pub credential: Option<ReloginCredential>,
    pub target: Option<ReloginTarget>,
    pub automatic_job: bool,
    pub workspace_mode: ReloginWorkspaceMode,
    pub workspace_targets: Vec<ReloginTarget>,
    pub workspace_choices: Vec<ReloginWorkspaceChoice>,
    pub selected_workspace_id: Option<String>,
    pub manual_push_context: Option<super::super::MutationContext>,
    pub enrollment: Option<ReloginEnrollment>,
    pub automatic_attempts: u32,
    pub stop_reason: Option<ReloginStopReason>,
    pub automatic_started_at: Vec<DateTime<Utc>>,
    pub attempted_target: Option<ReloginTarget>,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub synced_at: Option<DateTime<Utc>>,
    pub last_attempt: Option<ReloginAttempt>,
}

impl Default for ReloginChannelState {
    fn default() -> Self {
        Self {
            automatic: false,
            preferred_workspace_id: None,
            status: ReloginStatus::Pending,
            message: String::new(),
            credential: None,
            target: None,
            automatic_job: false,
            workspace_mode: ReloginWorkspaceMode::Original,
            workspace_targets: Vec::new(),
            workspace_choices: Vec::new(),
            selected_workspace_id: None,
            manual_push_context: None,
            enrollment: None,
            automatic_attempts: 0,
            stop_reason: None,
            automatic_started_at: Vec::new(),
            attempted_target: None,
            next_attempt_at: None,
            synced_at: None,
            last_attempt: None,
        }
    }
}

impl ReloginEntry {
    pub fn channel_state(&self) -> ReloginChannelState {
        ReloginChannelState {
            automatic: self.automatic,
            preferred_workspace_id: self.preferred_workspace_id.clone(),
            status: self.status,
            message: self.message.clone(),
            credential: self.credential.clone(),
            target: self.target.clone(),
            automatic_job: self.automatic_job,
            workspace_mode: self.workspace_mode,
            workspace_targets: self.workspace_targets.clone(),
            workspace_choices: self.workspace_choices.clone(),
            selected_workspace_id: self.selected_workspace_id.clone(),
            manual_push_context: self.manual_push_context.clone(),
            enrollment: self.enrollment.clone(),
            automatic_attempts: self.automatic_attempts,
            stop_reason: self.stop_reason,
            automatic_started_at: self.automatic_started_at.clone(),
            attempted_target: self.attempted_target.clone(),
            next_attempt_at: self.next_attempt_at,
            synced_at: self.synced_at,
            last_attempt: self.last_attempt.clone(),
        }
    }

    /// Read a channel using the existing orchestration without changing its sibling.
    /// Persist projections through the service's channel-aware save, never directly.
    pub fn project_channel(mut self, target: CredentialTarget) -> Self {
        debug_assert_eq!(self.credential_target, CredentialTarget::Codex);
        if target == CredentialTarget::Excel {
            let state = self.excel.clone().unwrap_or_default();
            self.automatic = state.automatic;
            self.preferred_workspace_id = state.preferred_workspace_id;
            self.status = state.status;
            self.message = state.message;
            self.credential = state.credential;
            self.target = state.target;
            self.automatic_job = state.automatic_job;
            self.workspace_mode = state.workspace_mode;
            self.workspace_targets = state.workspace_targets;
            self.workspace_choices = state.workspace_choices;
            self.selected_workspace_id = state.selected_workspace_id;
            self.manual_push_context = state.manual_push_context;
            self.enrollment = state.enrollment;
            self.automatic_attempts = state.automatic_attempts;
            self.stop_reason = state.stop_reason;
            self.automatic_started_at = state.automatic_started_at;
            self.attempted_target = state.attempted_target;
            self.next_attempt_at = state.next_attempt_at;
            self.synced_at = state.synced_at;
            self.last_attempt = state.last_attempt;
        }
        self.credential_target = target;
        self
    }
}
