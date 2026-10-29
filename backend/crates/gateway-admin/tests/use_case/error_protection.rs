use super::accounts::{FakeAccountStore, FakeProviderAdmin, StaticSettingsStore, account_record};
use super::*;
use gateway_core::{
    account::{AccountErrorProtection, CredentialRevision},
    lifecycle::CancellationToken,
    provider_ports::AccountProtectionState,
    task::{WorkerContribution, WorkerRunnable},
};
use std::sync::atomic::{AtomicBool, Ordering};

struct PolicyStore(bool);
#[async_trait]
impl SettingsStore for PolicyStore {
    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        let mut settings = StaticSettingsStore.load_runtime_settings().await?;
        settings.request_tuning.account_error_protection = Some(AccountErrorProtection {
            enabled: self.0,
            ..Default::default()
        });
        Ok(settings)
    }
    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        unreachable!()
    }
    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        unreachable!()
    }
    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        unreachable!()
    }
    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        unreachable!()
    }
}

#[derive(Default)]
struct ProtectionRuntime {
    claim: Mutex<Option<AccountProtectionState>>,
    finishes: Mutex<Vec<bool>>,
}
#[async_trait]
impl AccountRuntimeStore for ProtectionRuntime {
    async fn active_rate_limits(&self) -> AdminStoreResult<AccountRuntimeSnapshot> {
        Ok(Default::default())
    }
    async fn account_runtime(&self, _: &[String]) -> AdminStoreResult<AccountRuntimeSnapshot> {
        Ok(Default::default())
    }
    async fn due_protections(&self) -> AdminStoreResult<Vec<AccountProtectionState>> {
        Ok(self.claim.lock().unwrap().take().into_iter().collect())
    }
    async fn finish_protection(
        &self,
        _: &AccountProtectionState,
        postpone: Option<DateTime<Utc>>,
    ) -> AdminStoreResult<bool> {
        self.finishes.lock().unwrap().push(postpone.is_some());
        Ok(true)
    }
}

struct ProtectionProbe {
    mode: &'static str,
    started: AtomicBool,
    dropped: AtomicBool,
}
struct ProbeDrop<'a>(&'a AtomicBool);
impl Drop for ProbeDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl AccountProbe for ProtectionProbe {
    fn probe(
        &self,
        request: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move {
            assert_eq!(request.account_id.as_str(), "acct_test");
            assert_eq!(request.upstream_model.as_str(), "fixture-model");
            self.started.store(true, Ordering::SeqCst);
            let _drop = ProbeDrop(&self.dropped);
            match self.mode {
                "success" => Ok(AccountProbeResult {
                    text: vec!["OK".into()],
                }),
                "pending" | "cancel" => std::future::pending().await,
                _ => Err(
                    GatewayError::new(GatewayErrorKind::UpstreamUnavailable, "synthetic").into(),
                ),
            }
        })
    }
}

#[tokio::test(start_paused = true)]
async fn recovery_preserves_disabled_accounts_and_handles_probe_outcomes_and_cancellation() {
    for (mode, should_probe, expected) in [
        ("success", true, Some(false)),
        ("failure", true, Some(true)),
        ("pending", true, Some(true)),
        ("cancel", true, None),
        ("disabled", false, None),
        ("revision", false, Some(false)),
        ("policy_off", false, Some(false)),
    ] {
        let log = Arc::new(Mutex::new(vec![]));
        let store = FakeAccountStore::new("openai", log.clone());
        let mut account = account_record("openai");
        account.enabled = mode != "disabled";
        store.set_accounts(vec![account]);
        let runtime = Arc::new(ProtectionRuntime::default());
        *runtime.claim.lock().unwrap() = Some(AccountProtectionState {
            account_id: ProviderAccountId::new("acct_test").unwrap(),
            credential_revision: CredentialRevision::new(if mode == "revision" { 2 } else { 1 })
                .unwrap(),
            generation: "synthetic-generation".into(),
            until: std::time::SystemTime::now(),
            duration_seconds: 300,
            requires_probe: true,
            probe_model: Some("fixture-model".into()),
            rules: vec![],
        });
        let probe = Arc::new(ProtectionProbe {
            mode,
            started: AtomicBool::new(false),
            dropped: AtomicBool::new(false),
        });
        let mut bundle = AdminHarness::new()
            .accounts(store)
            .account_runtime(runtime.clone())
            .settings(Arc::new(PolicyStore(mode != "policy_off")))
            .provider(FakeProviderAdmin::new("openai", log))
            .probe(probe.clone())
            .build_bundle()
            .await;
        let registration = bundle
            .take_worker_contributions()
            .into_iter()
            .find_map(|worker| match worker {
                WorkerContribution::Registration(registration)
                    if registration.id.owner() == "admin_error_protection" =>
                {
                    Some(registration)
                }
                _ => None,
            })
            .unwrap();
        let WorkerRunnable::Daemon { task, .. } = registration.runnable else {
            panic!("daemon")
        };
        let cancel = CancellationToken::new();
        let child = cancel.clone();
        let worker = tokio::spawn(async move {
            task.run(child.into()).await.unwrap();
        });
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(probe.started.load(Ordering::SeqCst), should_probe, "{mode}");
        if mode == "pending" {
            tokio::time::advance(std::time::Duration::from_secs(46)).await;
        }
        if mode == "cancel" {
            cancel.cancel();
        }
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            *runtime.finishes.lock().unwrap(),
            expected.into_iter().collect::<Vec<_>>(),
            "{mode}"
        );
        if should_probe {
            assert!(probe.dropped.load(Ordering::SeqCst), "{mode}");
        }
        cancel.cancel();
        worker.await.unwrap();
    }
}
