//! Native credential probes reuse account leases, egress and provider health writes.

use async_trait::async_trait;
use chrono::Utc;
use futures::{StreamExt, stream};
use gateway_core::{
    account::{CredentialState, ProviderAccountId},
    lifecycle::CancellationToken,
    routing::{ProviderKind, UpstreamModelId},
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

use super::map_store_error;
use crate::{
    AccountsService, ReloginService,
    model::{
        AdminError, MutationContext,
        accounts::{
            AccountConnectionTest, AccountConnectionTestEvent, AccountRecord,
            AccountRuntimeSnapshot, ConnectionTestEndpoint,
        },
        provider_credentials::{CredentialListQuery, CredentialListWindow},
        token_guard::*,
    },
    ports::{store::AccountStore, token_guard::TokenGuardStore},
};

#[async_trait]
pub trait TokenGuardService: Send + Sync {
    async fn status(&self) -> Result<TokenGuardStatus, AdminError>;
    async fn configure(
        &self,
        config: TokenGuardConfig,
        context: &MutationContext,
    ) -> Result<(), AdminError>;
    async fn request_run(&self, context: &MutationContext) -> Result<(), AdminError>;
    async fn queue_relogin(
        &self,
        account_id: &str,
        context: &MutationContext,
    ) -> Result<(), AdminError>;
}

pub(crate) struct DefaultTokenGuardService {
    store: Option<Arc<dyn TokenGuardStore>>,
    accounts: Arc<dyn AccountStore>,
    service: Arc<dyn AccountsService>,
    relogin: Arc<dyn ReloginService>,
    cycle: Mutex<Option<Instant>>,
    control: Mutex<CancellationToken>,
    running: AtomicBool,
    queued: AtomicBool,
}

impl DefaultTokenGuardService {
    pub(crate) fn new(
        store: Option<Arc<dyn TokenGuardStore>>,
        accounts: Arc<dyn AccountStore>,
        service: Arc<dyn AccountsService>,
        relogin: Arc<dyn ReloginService>,
    ) -> Self {
        Self {
            store,
            accounts,
            service,
            relogin,
            cycle: Mutex::new(None),
            control: Mutex::new(CancellationToken::new()),
            running: AtomicBool::new(false),
            queued: AtomicBool::new(false),
        }
    }

    fn store(&self) -> Result<&dyn TokenGuardStore, AdminError> {
        self.store
            .as_deref()
            .ok_or_else(|| AdminError::unavailable("凭证守护未配置"))
    }

    pub(crate) async fn tick(&self, shutdown: &CancellationToken) -> Result<(), AdminError> {
        let Ok(mut last) = self.cycle.try_lock() else {
            return Ok(());
        };
        let store = self.store()?;
        let control = self.control.lock().await;
        let config = store
            .config()
            .await
            .map_err(|error| map_store_error(error, "token guard"))?;
        let cancellation = control.clone();
        drop(control);
        if !config.enabled {
            return Ok(());
        }
        if !self.queued.swap(false, Ordering::AcqRel)
            && last.is_some_and(|at| {
                at.elapsed() < Duration::from_secs(u64::from(config.interval_seconds))
            })
        {
            return Ok(());
        }
        let latest: BTreeMap<_, _> = store
            .latest()
            .await
            .map_err(|error| map_store_error(error, "token guard"))?
            .into_iter()
            .map(|event| (event.account_id.clone(), event.observed_at))
            .collect();
        let mut accounts = self
            .accounts
            .list_credentials(
                &ProviderKind::new("openai")
                    .map_err(|_| AdminError::internal("invalid provider"))?,
                CredentialListQuery {
                    enabled: None,
                    credential_state: None,
                    window: CredentialListWindow::All,
                },
            )
            .await
            .map_err(|error| map_store_error(error, "token guard"))?
            .items;
        accounts.retain(|account| {
            account.authentication_kind == "oauth"
                && (config.group_ids.is_empty()
                    || account
                        .groups
                        .iter()
                        .any(|group| config.group_ids.iter().any(|id| id == group.id.as_str())))
        });
        accounts.sort_by(|left, right| {
            latest
                .get(&left.id)
                .cmp(&latest.get(&right.id))
                .then_with(|| left.id.cmp(&right.id))
        });
        accounts.truncate(usize::from(config.max_per_cycle));
        self.running.store(true, Ordering::Release);
        let _running = RunningGuard(&self.running);
        let probes = stream::iter(accounts)
            .map(|account| self.probe(account, &config))
            .buffer_unordered(usize::from(config.concurrency));
        tokio::pin!(probes);
        loop {
            let event = tokio::select! {
                biased;
                () = shutdown.cancelled() => break,
                () = cancellation.cancelled() => break,
                event = probes.next() => event,
            };
            let Some(event) = event else { break };
            store
                .record(&event)
                .await
                .map_err(|error| map_store_error(error, "token guard"))?;
        }
        *last = Some(Instant::now());
        Ok(())
    }

    async fn probe(&self, account: AccountRecord, config: &TokenGuardConfig) -> TokenGuardEvent {
        let started = Instant::now();
        let mut event = TokenGuardEvent {
            account_id: account.id.clone(),
            observed_revision: account.credential_revision.get(),
            outcome: TokenGuardOutcome::Transient,
            reason: TokenGuardReason::RequestFailed,
            latency_ms: 0,
            observed_at: Utc::now(),
        };
        if let Some((outcome, reason)) = eligibility(&account) {
            event.outcome = outcome;
            event.reason = reason;
            return event;
        }
        // Re-read before admission so a queued account cannot bypass a new manual pause.
        let current = self
            .accounts
            .load_account(&account.id, AccountRuntimeSnapshot::default())
            .await;
        if !matches!(&current, Ok(Some(current)) if current.account.enabled
            && current.account.turn_state_binding_revision == account.turn_state_binding_revision
            && eligibility(&current.account).is_none())
        {
            event.outcome = TokenGuardOutcome::Skipped;
            event.reason = TokenGuardReason::AccountChanged;
            return event;
        }
        let result = tokio::time::timeout(
            Duration::from_secs(u64::from(config.timeout_seconds)),
            async {
                let mut events = self
                    .service
                    .test_connection(AccountConnectionTest {
                        account_id: ProviderAccountId::new(&account.id)
                            .map_err(|_| AdminError::invalid("invalid account"))?,
                        upstream_model: UpstreamModelId::new(&config.model)
                            .map_err(|_| AdminError::invalid("invalid model"))?,
                        endpoint: ConnectionTestEndpoint::Responses,
                        input_text: "Reply with one lowercase word: ok".into(),
                        stream: true,
                    })
                    .await?;
                while let Some(result) = events.next().await {
                    match result {
                        AccountConnectionTestEvent::Completed { .. } => return Ok(true),
                        AccountConnectionTestEvent::Failed { .. } => return Ok(false),
                        _ => {}
                    }
                }
                Ok::<_, AdminError>(false)
            },
        )
        .await;
        match result {
            Ok(Ok(true)) => {
                event.outcome = TokenGuardOutcome::Healthy;
                event.reason = TokenGuardReason::Completed;
            }
            Err(_) => {
                event.outcome = TokenGuardOutcome::TimedOut;
                event.reason = TokenGuardReason::Timeout;
            }
            _ => {}
        }
        // Only provider-persisted, typed credential facts can classify auth failure.
        // A generic 401/403 from a proxy or an unavailable model is not sufficient.
        if let Ok(Some(current)) = self
            .accounts
            .load_account(&account.id, AccountRuntimeSnapshot::default())
            .await
        {
            if current.account.turn_state_binding_revision != account.turn_state_binding_revision {
                event.outcome = TokenGuardOutcome::Skipped;
                event.reason = TokenGuardReason::AccountChanged;
            } else if let Some((outcome, reason)) = eligibility(&current.account) {
                event.outcome = outcome;
                event.reason = reason;
            }
        }
        event.latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        event.observed_at = Utc::now();
        event
    }
}

fn eligibility(account: &AccountRecord) -> Option<(TokenGuardOutcome, TokenGuardReason)> {
    use TokenGuardOutcome::{AuthRequired, Skipped};
    use TokenGuardReason::*;
    if !account.enabled {
        return Some((Skipped, AccountDisabled));
    }
    match account.credential_state {
        CredentialState::Banned => return Some((Skipped, AccountBanned)),
        CredentialState::Invalid => return Some((Skipped, CredentialInvalid)),
        CredentialState::Expired => return Some((AuthRequired, CredentialExpired)),
        CredentialState::Unknown | CredentialState::Ready => {}
    }
    account
        .quota
        .is_exhausted()
        .then_some((Skipped, QuotaExhausted))
}

struct RunningGuard<'a>(&'a AtomicBool);
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[async_trait]
impl TokenGuardService for DefaultTokenGuardService {
    async fn status(&self) -> Result<TokenGuardStatus, AdminError> {
        let store = self.store()?;
        Ok(TokenGuardStatus {
            config: store
                .config()
                .await
                .map_err(|error| map_store_error(error, "token guard"))?,
            running: self.running.load(Ordering::Acquire),
            queued: self.queued.load(Ordering::Acquire),
            events: store
                .events()
                .await
                .map_err(|error| map_store_error(error, "token guard"))?,
        })
    }

    async fn configure(
        &self,
        config: TokenGuardConfig,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        config.validate()?;
        let mut control = self.control.lock().await;
        self.store()?
            .configure(&config, context)
            .await
            .map_err(|error| map_store_error(error, "token guard"))?;
        control.cancel();
        *control = CancellationToken::new();
        self.queued.store(config.enabled, Ordering::Release);
        Ok(())
    }

    async fn request_run(&self, context: &MutationContext) -> Result<(), AdminError> {
        let _control = self.control.lock().await;
        if !self
            .store()?
            .config()
            .await
            .map_err(|error| map_store_error(error, "token guard"))?
            .enabled
        {
            return Err(AdminError::conflict("请先开启凭证守护"));
        }
        self.store()?
            .audit_run(context)
            .await
            .map_err(|error| map_store_error(error, "token guard"))?;
        self.queued.store(true, Ordering::Release);
        Ok(())
    }

    async fn queue_relogin(
        &self,
        account_id: &str,
        context: &MutationContext,
    ) -> Result<(), AdminError> {
        let actions = self
            .relogin
            .account_actions(&[account_id.to_owned()])
            .await?;
        let action = actions
            .into_iter()
            .find(|action| action.account_id == account_id)
            .ok_or_else(|| AdminError::not_found("未找到该账号的重登资料"))?;
        let target = action
            .target
            .ok_or_else(|| AdminError::conflict("账号重登目标待确认"))?;
        self.relogin
            .queue_account(&action.entry_id, action.revision, &target, context)
            .await
    }
}
