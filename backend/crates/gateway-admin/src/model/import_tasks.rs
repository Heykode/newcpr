//! Process-local import inputs and credential-free progress snapshots.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use gateway_core::{account::ProviderAccountId, routing::ProviderKind};
use uuid::Uuid;

use super::{MutationContext, provider_credentials::ImportCredentials};

pub const MAX_IMPORT_TASK_ITEMS: usize = 200;

pub struct ImportTaskInput {
    pub provider: ProviderKind,
    pub command: ImportCredentials,
}

pub struct SubmitImportTask {
    pub submission_id: Uuid,
    /// Digest of the complete wire input, never a logged or persisted request.
    pub fingerprint: [u8; 32],
    pub context: MutationContext,
    pub items: Vec<ImportTaskInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportItemStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
    Skipped,
}

impl ImportItemStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImportTaskItem {
    pub index: usize,
    pub provider: ProviderKind,
    pub status: ImportItemStatus,
    pub account_ids: Vec<ProviderAccountId>,
    pub account_emails: BTreeMap<ProviderAccountId, Option<String>>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ImportTaskCounts {
    pub pending: usize,
    pub running: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub unknown: usize,
    pub skipped: usize,
    pub imported_accounts: usize,
}

#[derive(Debug, Clone)]
pub struct ImportTaskSummary {
    pub task_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub stop_requested: bool,
    pub total: usize,
    pub counts: ImportTaskCounts,
}

#[derive(Debug, Clone)]
pub struct ImportTaskDetail {
    pub summary: ImportTaskSummary,
    pub items: Vec<ImportTaskItem>,
}
