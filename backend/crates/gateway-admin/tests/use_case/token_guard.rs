use super::accounts::{FakeAccountStore, FakeProviderAdmin, account_record, context};
use super::*;
use gateway_admin::{model::token_guard::*, ports::token_guard::TokenGuardStore};
use gateway_core::{
    account::{CredentialState, QuotaState},
    lifecycle::CancellationToken,
    task::{WorkerContribution, WorkerRunnable},
};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

#[derive(Default)]
struct MemoryGuard {
    config: Mutex<TokenGuardConfig>,
    events: Mutex<Vec<TokenGuardEvent>>,
}
#[async_trait]
impl TokenGuardStore for MemoryGuard {
    async fn audit_run(&self, _: &MutationContext) -> AdminStoreResult<()> {
        Ok(())
    }
    async fn config(&self) -> AdminStoreResult<TokenGuardConfig> {
        Ok(self.config.lock().unwrap().clone())
    }
    async fn configure(
        &self,
        config: &TokenGuardConfig,
        _: &MutationContext,
    ) -> AdminStoreResult<()> {
        *self.config.lock().unwrap() = config.clone();
        Ok(())
    }
    async fn latest(&self) -> AdminStoreResult<Vec<TokenGuardEvent>> {
        self.events().await
    }
    async fn events(&self) -> AdminStoreResult<Vec<TokenGuardEvent>> {
        Ok(self.events.lock().unwrap().clone())
    }
    async fn record(&self, event: &TokenGuardEvent) -> AdminStoreResult<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

#[derive(Default)]
struct GuardProbe {
    calls: AtomicUsize,
    pending: bool,
    release: Option<Arc<tokio::sync::Semaphore>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    account_ids: Mutex<Vec<String>>,
}
impl AccountProbe for GuardProbe {
    fn probe(
        &self,
        request: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move {
            assert_eq!(request.upstream_model.as_str(), "gpt-6-astra");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.account_ids
                .lock()
                .unwrap()
                .push(request.account_id.as_str().into());
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            let _active = ProbeActive(&self.active);
            if let Some(release) = &self.release {
                release.acquire().await.unwrap().forget();
            }
            if self.pending {
                std::future::pending::<()>().await;
            }
            Ok(AccountProbeResult {
                text: vec!["ok".into()],
                upstream_response_model: None,
            })
        })
    }
}

struct ProbeActive<'a>(&'a AtomicUsize);
impl Drop for ProbeActive<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn bundle(
    accounts: Vec<gateway_admin::model::accounts::AccountRecord>,
    store: Arc<MemoryGuard>,
    probe: Arc<GuardProbe>,
) -> (gateway_admin::AdminBundle, Arc<FakeAccountStore>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let account_store = FakeAccountStore::new("openai", log.clone());
    account_store.set_accounts(accounts);
    let bundle = AdminHarness::new()
        .accounts(account_store.clone())
        .provider(FakeProviderAdmin::new("openai", log))
        .token_guard(store)
        .probe(probe)
        .build_bundle()
        .await;
    (bundle, account_store)
}

fn start(
    bundle: &mut gateway_admin::AdminBundle,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let task = bundle
        .take_worker_contributions()
        .into_iter()
        .find_map(|entry| {
            if let WorkerContribution::Registration(registration) = entry
                && registration.id.owner() == "admin_token_guard"
                && let WorkerRunnable::Daemon { task, .. } = registration.runnable
            {
                Some(task)
            } else {
                None
            }
        })
        .unwrap();
    let cancel = CancellationToken::new();
    let shutdown = cancel.clone();
    (
        cancel,
        tokio::spawn(async move {
            task.run(shutdown).await.unwrap();
        }),
    )
}

async fn wait_until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn token_guard_native_probe_preserves_pauses_bans_quotas_and_credentials() {
    let mut healthy = account_record("openai");
    healthy.authentication_kind = "oauth".into();
    healthy.credential_state = CredentialState::Ready;
    healthy.enabled = true;
    let mut disabled = healthy.clone();
    disabled.id = "guard_disabled".into();
    disabled.enabled = false;
    let mut banned = healthy.clone();
    banned.id = "guard_banned".into();
    banned.credential_state = CredentialState::Banned;
    let mut expired = healthy.clone();
    expired.id = "guard_expired".into();
    expired.credential_state = CredentialState::Expired;
    let mut exhausted = healthy.clone();
    exhausted.id = "guard_exhausted".into();
    exhausted.quota = QuotaState::exhausted(
        gateway_core::account::QuotaEvidence::UsageLimitReached,
        std::time::SystemTime::now(),
        None,
    );
    let originals = vec![healthy, disabled, banned, expired, exhausted];
    let store = Arc::new(MemoryGuard::default());
    let probe = Arc::new(GuardProbe::default());
    let (mut bundle, accounts) = bundle(originals.clone(), store.clone(), probe.clone()).await;
    let services = bundle.services();
    assert!(
        !services
            .token_guard()
            .status()
            .await
            .unwrap()
            .config
            .enabled
    );
    assert!(
        services
            .token_guard()
            .request_run(&context("guard_run"))
            .await
            .is_err()
    );
    services
        .token_guard()
        .configure(
            TokenGuardConfig {
                enabled: true,
                ..Default::default()
            },
            &context("guard_enable"),
        )
        .await
        .unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_until(|| store.events.lock().unwrap().len() == 5).await;
    cancel.cancel();
    worker.await.unwrap();
    let events = store.events.lock().unwrap();
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.outcome == TokenGuardOutcome::Healthy)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.outcome == TokenGuardOutcome::AuthRequired)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.outcome == TokenGuardOutcome::Skipped)
            .count(),
        3
    );
    assert_eq!(*accounts.accounts.lock().unwrap(), originals);
    assert!(accounts.rotation_updates.lock().unwrap().is_empty());
    assert!(accounts.batch_updates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn token_guard_disable_cancels_pending_probe_without_making_up_a_failure() {
    let mut account = account_record("openai");
    account.enabled = true;
    account.authentication_kind = "oauth".into();
    account.credential_state = CredentialState::Ready;
    let store = Arc::new(MemoryGuard::default());
    let probe = Arc::new(GuardProbe {
        pending: true,
        ..Default::default()
    });
    let (mut bundle, _) = bundle(vec![account], store.clone(), probe.clone()).await;
    let services = bundle.services();
    services
        .token_guard()
        .configure(
            TokenGuardConfig {
                enabled: true,
                ..Default::default()
            },
            &context("guard_enable"),
        )
        .await
        .unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_until(|| probe.calls.load(Ordering::SeqCst) == 1).await;
    services
        .token_guard()
        .configure(TokenGuardConfig::default(), &context("guard_disable"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while services.token_guard().status().await.unwrap().running {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(store.events.lock().unwrap().is_empty());
    cancel.cancel();
    worker.await.unwrap();
}

#[tokio::test]
async fn token_guard_bounds_concurrency_and_visits_oldest_accounts_before_repeating() {
    let accounts = (0..5)
        .map(|index| {
            let mut account = account_record("openai");
            account.id = format!("acct_guard_{index}");
            account.enabled = true;
            account.authentication_kind = "oauth".into();
            account.credential_state = CredentialState::Ready;
            account
        })
        .collect();
    let store = Arc::new(MemoryGuard::default());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let probe = Arc::new(GuardProbe {
        release: Some(release.clone()),
        ..Default::default()
    });
    let (mut bundle, _) = bundle(accounts, store.clone(), probe.clone()).await;
    let services = bundle.services();
    services
        .token_guard()
        .configure(
            TokenGuardConfig {
                enabled: true,
                concurrency: 2,
                max_per_cycle: 3,
                ..Default::default()
            },
            &context("guard_enable"),
        )
        .await
        .unwrap();
    let (cancel, worker) = start(&mut bundle);
    wait_until(|| probe.calls.load(Ordering::SeqCst) == 2).await;
    assert_eq!(probe.active.load(Ordering::SeqCst), 2);
    release.add_permits(3);
    wait_until(|| store.events.lock().unwrap().len() == 3).await;
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    services
        .token_guard()
        .request_run(&context("guard_run"))
        .await
        .unwrap();
    wait_until(|| probe.calls.load(Ordering::SeqCst) == 5).await;
    assert_eq!(
        &probe.account_ids.lock().unwrap()[3..5],
        &["acct_guard_3", "acct_guard_4"]
    );
    services
        .token_guard()
        .configure(TokenGuardConfig::default(), &context("guard_disable"))
        .await
        .unwrap();
    wait_until(|| probe.active.load(Ordering::SeqCst) == 0).await;
    assert_eq!(store.events.lock().unwrap().len(), 3);
    cancel.cancel();
    worker.await.unwrap();
}
