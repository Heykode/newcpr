//! Fence rejected Excel credentials while the existing state write is pending or unavailable.

use super::*;
use gateway_core::account::CredentialRevision;

const EXCEL_AUTH_RECOVERY_WINDOW: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct ExcelAuthBlock {
    revision: CredentialRevision,
    observed_at: SystemTime,
    revoked: bool,
    until: Option<tokio::time::Instant>,
}

impl CodexCredentialSelector {
    /// Record a confirmed Excel HTTP 401 using the existing credential policy.
    /// The bounded state write outlives cancellation; only a fixed reason is stored.
    pub async fn record_excel_authentication_failure(
        self: &Arc<Self>,
        account: &ProviderAccount,
        revoked: bool,
        diagnostic: bool,
    ) {
        if !diagnostic && !account.enabled() {
            return;
        }
        let revoked = self.block_excel_authentication(account, revoked);
        let selector = self.clone();
        let account = account.clone();
        let failure = if revoked {
            CodexAccountFailure::CredentialRevoked
        } else {
            CodexAccountFailure::CredentialExpired
        };
        let _ = tokio::spawn(async move {
            let message = Some("Excel upstream authentication failed".to_owned());
            let persist = async {
                if diagnostic {
                    selector.record_diagnostic_failure(&account, failure, message).await
                } else {
                    selector.record_failure(&account, failure, message).await
                }
            };
            if !matches!(tokio::time::timeout(Duration::from_secs(2), persist).await, Ok(Ok(()))) {
                tracing::warn!(account_id = %account.id(), "Excel HTTP 401 state write did not complete; rejected credentials remain locally blocked");
            }
        }).await;
    }

    fn block_excel_authentication(&self, account: &ProviderAccount, revoked: bool) -> bool {
        let mut blocks = self
            .excel_auth_blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let revoked = revoked
            || blocks
                .get(account.id())
                .is_some_and(|current| current.revision == account.revision() && current.revoked);
        let block = ExcelAuthBlock {
            revision: account.revision(),
            observed_at: SystemTime::now(),
            revoked,
            until: (!revoked && account.has_refresh_token())
                .then(|| tokio::time::Instant::now() + EXCEL_AUTH_RECOVERY_WINDOW),
        };
        let current = blocks.entry(account.id().clone()).or_insert(block);
        if current.revision <= block.revision {
            *current = block;
        }
        revoked
    }

    pub(super) fn excel_auth_block(&self, account: &ProviderAccount) -> Option<ExcelAuthBlock> {
        self.excel_auth_blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(account.id())
            .copied()
            .filter(|block| {
                account.revision() <= block.revision
                    && block
                        .until
                        .is_none_or(|until| until > tokio::time::Instant::now())
            })
    }

    pub(super) fn retain_excel_auth_blocks(&self, accounts: &[ProviderAccount]) {
        let mut blocks = self
            .excel_auth_blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if blocks.is_empty() {
            return;
        }
        let revisions = accounts
            .iter()
            .map(|account| (account.id(), account.revision()))
            .collect::<HashMap<_, _>>();
        blocks.retain(|id, block| {
            revisions
                .get(id)
                .is_some_and(|revision| *revision <= block.revision)
                && block
                    .until
                    .is_none_or(|until| until > tokio::time::Instant::now())
        });
    }

    pub(super) fn clear_excel_auth_block(
        &self,
        account: &ProviderAccount,
        expected: ExcelAuthBlock,
    ) {
        let mut blocks = self
            .excel_auth_blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if blocks.get(account.id()) == Some(&expected) {
            blocks.remove(account.id());
        }
    }
}
