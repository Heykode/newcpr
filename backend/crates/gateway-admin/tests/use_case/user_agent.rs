use super::*;

use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

use gateway_admin::model::{
    AdminErrorKind, MutationActor,
    user_agent::{OutboundUserAgentView, ProviderUserAgentOverride},
};
use gateway_core::{
    lifecycle::CancellationToken,
    task::{
        ScheduledTask, WorkerContribution, WorkerCycleContext, WorkerId, WorkerKind, WorkerRunnable,
    },
};
use tokio::sync::Notify;

#[derive(Clone, Copy, Default)]
enum CommitReply {
    #[default]
    Success,
    Lost,
    Pending,
}

#[derive(Default)]
struct IdentityStore {
    selection: Mutex<ProviderUserAgentOverride>,
    reply: Mutex<CommitReply>,
    reads: AtomicUsize,
    commits: AtomicUsize,
    fail_read: AtomicBool,
    hold_read: AtomicBool,
    release_read: Notify,
}

#[async_trait]
impl SettingsStore for IdentityStore {
    async fn load_user_agent_override(
        &self,
        provider: &ProviderKind,
    ) -> AdminStoreResult<ProviderUserAgentOverride> {
        assert_eq!(provider.as_str(), "identity-fixture");
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_read.load(Ordering::SeqCst) {
            return Err(unavailable("identity read"));
        }
        let selection = self.selection.lock().expect("selection").clone();
        if self.hold_read.load(Ordering::SeqCst) {
            self.release_read.notified().await;
        }
        Ok(selection)
    }

    async fn replace_user_agent_override(
        &self,
        provider: &ProviderKind,
        selection: ProviderUserAgentOverride,
        _: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        assert_eq!(provider.as_str(), "identity-fixture");
        *self.selection.lock().expect("selection") = selection;
        self.commits.fetch_add(1, Ordering::SeqCst);
        let reply = *self.reply.lock().expect("reply");
        match reply {
            CommitReply::Success => Ok(Revision::new(2).expect("revision")),
            CommitReply::Lost => Err(unavailable("commit reply lost")),
            CommitReply::Pending => futures::future::pending().await,
        }
    }

    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        Err(unavailable("settings"))
    }

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        Err(unavailable("admin API key"))
    }

    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        Err(unavailable("settings"))
    }

    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unavailable("admin API key"))
    }

    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(unavailable("admin API key"))
    }
}

struct IdentityProvider {
    kind: ProviderKind,
    selection: Mutex<ProviderUserAgentOverride>,
    applied: Mutex<Vec<ProviderUserAgentOverride>>,
}

impl IdentityProvider {
    fn new() -> Self {
        Self {
            kind: ProviderKind::new("identity-fixture").expect("provider kind"),
            selection: Mutex::default(),
            applied: Mutex::default(),
        }
    }

    fn view(selection: ProviderUserAgentOverride) -> OutboundUserAgentView {
        OutboundUserAgentView {
            effective_user_agent: selection
                .custom_user_agent()
                .unwrap_or("fixture")
                .to_owned(),
            selection,
            default_user_agent: "fixture".to_owned(),
            effective_desktop_user_agent: "fixture-desktop".to_owned(),
            core_version: "1".to_owned(),
            desktop_version: "1".to_owned(),
            os_type: "fixture".to_owned(),
            os_version: "1".to_owned(),
            arch: "fixture".to_owned(),
            terminal: "fixture".to_owned(),
            verified: false,
            default_verified_at: Utc::now(),
        }
    }
}

#[async_trait]
impl ProviderAdmin for IdentityProvider {
    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }

    fn outbound_user_agent(&self) -> Result<OutboundUserAgentView, ProviderAdminError> {
        Ok(Self::view(
            self.selection.lock().expect("selection").clone(),
        ))
    }

    fn preview_outbound_user_agent(
        &self,
        selection: &ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, ProviderAdminError> {
        if selection.custom_user_agent() == Some("invalid") {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::Invalid));
        }
        Ok(Self::view(selection.clone()))
    }

    fn apply_outbound_user_agent(
        &self,
        selection: ProviderUserAgentOverride,
    ) -> Result<OutboundUserAgentView, ProviderAdminError> {
        let view = self.preview_outbound_user_agent(&selection)?;
        self.applied
            .lock()
            .expect("applied")
            .push(selection.clone());
        *self.selection.lock().expect("selection") = selection;
        Ok(view)
    }

    async fn account_unavailable(&self, _: &ProviderAccountId) {}

    fn connection_test_operation(
        &self,
        _: &gateway_core::routing::UpstreamModelId,
        _: &str,
    ) -> Result<gateway_core::operation::Operation, ProviderAdminError> {
        Err(unsupported_provider())
    }

    fn dashboard_wire_profile(&self) -> Option<DashboardWireProfile> {
        None
    }

    fn calculated_billing(
        &self,
        _: &gateway_admin::model::observability::ProviderBillingInput,
    ) -> Result<
        Option<gateway_admin::model::observability::CalculatedBillingBreakdown>,
        ProviderAdminError,
    > {
        Ok(None)
    }

    async fn prepare_import(
        &self,
        _: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn start_authorization(
        &self,
        _: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn complete_authorization(
        &self,
        _: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn prepare_rotation(
        &self,
        _: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn prepare_refresh(
        &self,
        _: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn quota(
        &self,
        _: gateway_admin::model::provider_credentials::ProviderQuotaRequest,
    ) -> Result<ProviderQuota, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn models(
        &self,
        _: &ProviderAccountId,
        _: bool,
    ) -> Result<ProviderModels, ProviderAdminError> {
        Err(unsupported_provider())
    }

    async fn export_credentials(
        &self,
        _: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError> {
        Err(unsupported_provider())
    }
}

struct Fixture {
    services: AdminServices,
    store: Arc<IdentityStore>,
    provider: Arc<IdentityProvider>,
    worker: WorkerId,
    task: Box<dyn ScheduledTask>,
}

impl Fixture {
    async fn new() -> Self {
        let store = Arc::new(IdentityStore::default());
        let provider = Arc::new(IdentityProvider::new());
        let mut bundle = AdminHarness::new()
            .settings(store.clone())
            .provider(provider.clone())
            .build_bundle()
            .await;
        let services = bundle.services();
        let mut registrations =
            bundle
                .take_worker_contributions()
                .into_iter()
                .filter_map(|contribution| match contribution {
                    WorkerContribution::Registration(registration)
                        if registration.id.owner() == "admin_user_agent" =>
                    {
                        Some(registration)
                    }
                    _ => None,
                });
        let registration = registrations.next().expect("identity worker registered");
        assert!(registrations.next().is_none());
        assert!(bundle.take_worker_contributions().is_empty());
        assert_eq!(
            registration.id.kind(),
            WorkerKind::RuntimeSnapshotReconciliation
        );
        let WorkerRunnable::Scheduled {
            schedule,
            lease,
            task,
        } = registration.runnable
        else {
            panic!("identity reconciliation must be scheduled");
        };
        assert!(
            lease.is_none(),
            "each process must reconcile its local projection"
        );
        assert_eq!(schedule.interval(), Duration::from_secs(5));
        assert_eq!(schedule.initial_backoff(), Duration::from_secs(1));
        assert_eq!(schedule.maximum_backoff(), Duration::from_secs(5));
        Self {
            services,
            store,
            provider,
            worker: registration.id,
            task,
        }
    }

    fn context(&self, cancellation: CancellationToken) -> WorkerCycleContext {
        WorkerCycleContext::new(self.worker.clone(), None, cancellation)
    }

    async fn cycle(&self) {
        self.task
            .run_cycle(self.context(CancellationToken::new()))
            .await
            .expect("reconcile");
    }

    fn selection(&self) -> ProviderUserAgentOverride {
        self.services
            .outbound_user_agent()
            .load(&self.provider.kind)
            .expect("runtime view")
            .selection
    }

    fn applied(&self) -> Vec<ProviderUserAgentOverride> {
        self.provider.applied.lock().expect("applied").clone()
    }
}

fn mutation() -> MutationContext {
    MutationContext {
        actor: MutationActor::System,
        request_id: "identity-test".to_owned(),
    }
}

fn qx() -> ProviderUserAgentOverride {
    ProviderUserAgentOverride::Custom {
        user_agent: "fixture-cli".to_owned(),
    }
}

#[tokio::test]
async fn committed_selection_with_lost_reply_recovers_without_restart_for_all_modes() {
    let fixture = Fixture::new().await;
    *fixture.store.reply.lock().expect("reply") = CommitReply::Lost;
    for selection in [
        qx(),
        ProviderUserAgentOverride::Custom {
            user_agent: "fixture-custom".to_owned(),
        },
        ProviderUserAgentOverride::Default,
    ] {
        let previous = fixture.selection();
        let error = fixture
            .services
            .outbound_user_agent()
            .replace(&mutation(), &fixture.provider.kind, selection.clone())
            .await
            .expect_err("unknown commit reply must not claim save success");
        assert_eq!(error.kind(), AdminErrorKind::Unavailable);
        assert_eq!(
            *fixture.store.selection.lock().expect("committed selection"),
            selection
        );
        assert_eq!(fixture.selection(), previous, "GET stays memory-only");
        fixture.cycle().await;
        assert_eq!(fixture.selection(), selection);
        let applications = fixture.applied();
        fixture.cycle().await;
        assert_eq!(
            fixture.applied(),
            applications,
            "unchanged cycles do not republish"
        );
    }
    assert_eq!(fixture.store.commits.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn cancellation_after_commit_recovers_on_the_next_cycle_without_restart() {
    let fixture = Fixture::new().await;
    *fixture.store.reply.lock().expect("reply") = CommitReply::Pending;
    let context = mutation();
    let mut save = Box::pin(fixture.services.outbound_user_agent().replace(
        &context,
        &fixture.provider.kind,
        qx(),
    ));
    assert!(futures::poll!(save.as_mut()).is_pending());
    assert_eq!(fixture.store.commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        *fixture.store.selection.lock().expect("committed selection"),
        qx()
    );
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    drop(save);
    fixture.cycle().await;
    assert_eq!(fixture.selection(), qx());
    assert_eq!(fixture.applied(), vec![qx()]);
}

#[tokio::test]
async fn delayed_old_reconciliation_read_cannot_overtake_a_newer_save() {
    let fixture = Fixture::new().await;
    *fixture.store.selection.lock().expect("selection") = qx();
    fixture.store.hold_read.store(true, Ordering::SeqCst);
    let mut cycle = fixture
        .task
        .run_cycle(fixture.context(CancellationToken::new()));
    assert!(futures::poll!(cycle.as_mut()).is_pending());
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), 1);
    let newer = ProviderUserAgentOverride::Custom {
        user_agent: "newer".to_owned(),
    };
    let context = mutation();
    let mut save = Box::pin(fixture.services.outbound_user_agent().replace(
        &context,
        &fixture.provider.kind,
        newer.clone(),
    ));
    assert!(futures::poll!(save.as_mut()).is_pending());
    assert_eq!(
        fixture.store.commits.load(Ordering::SeqCst),
        0,
        "read must own the save fence"
    );
    fixture.store.hold_read.store(false, Ordering::SeqCst);
    fixture.store.release_read.notify_one();
    cycle.await.expect("older cycle");
    assert_eq!(fixture.selection(), qx());
    save.await.expect("newer save");
    assert_eq!(fixture.selection(), newer);
    assert_eq!(fixture.applied(), vec![qx(), newer.clone()]);
    fixture.cycle().await;
    assert_eq!(fixture.selection(), newer);
    assert_eq!(fixture.applied().len(), 2);
}

#[tokio::test]
async fn reconciliation_waits_for_pending_save_and_cancellation_releases_the_fence() {
    let fixture = Fixture::new().await;
    *fixture.store.reply.lock().expect("reply") = CommitReply::Pending;
    let context = mutation();
    let mut save = Box::pin(fixture.services.outbound_user_agent().replace(
        &context,
        &fixture.provider.kind,
        qx(),
    ));
    assert!(futures::poll!(save.as_mut()).is_pending());
    let mut cycle = fixture
        .task
        .run_cycle(fixture.context(CancellationToken::new()));
    assert!(futures::poll!(cycle.as_mut()).is_pending());
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), 0);
    drop(save);
    cycle.await.expect("recover after cancelled save");
    assert_eq!(fixture.selection(), qx());
}

#[tokio::test]
async fn failed_read_or_invalid_persisted_selection_keeps_last_valid_runtime() {
    let fixture = Fixture::new().await;
    *fixture.store.selection.lock().expect("selection") = qx();
    fixture.store.fail_read.store(true, Ordering::SeqCst);
    assert!(
        fixture
            .task
            .run_cycle(fixture.context(CancellationToken::new()))
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    assert!(fixture.applied().is_empty());
    fixture.store.fail_read.store(false, Ordering::SeqCst);
    *fixture.store.selection.lock().expect("selection") = ProviderUserAgentOverride::Custom {
        user_agent: "invalid".to_owned(),
    };
    assert!(
        fixture
            .task
            .run_cycle(fixture.context(CancellationToken::new()))
            .await
            .is_err()
    );
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    assert!(fixture.applied().is_empty());
    *fixture.store.selection.lock().expect("selection") = qx();
    fixture.cycle().await;
    assert_eq!(fixture.selection(), qx());
}

#[tokio::test]
async fn worker_cancellation_drops_delayed_read_without_publication_and_unlocks_save() {
    let fixture = Fixture::new().await;
    *fixture.store.selection.lock().expect("selection") = qx();
    fixture.store.hold_read.store(true, Ordering::SeqCst);
    let cancellation = CancellationToken::new();
    let mut cycle = fixture
        .task
        .run_cycle(fixture.context(cancellation.clone()));
    assert!(futures::poll!(cycle.as_mut()).is_pending());
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(1), cycle)
        .await
        .expect("bounded cancellation")
        .expect("cancelled");
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    assert!(fixture.applied().is_empty());
    tokio::time::timeout(
        Duration::from_secs(1),
        fixture
            .services
            .outbound_user_agent()
            .replace(&mutation(), &fixture.provider.kind, qx()),
    )
    .await
    .expect("mutation fence released")
    .expect("save");
    assert_eq!(fixture.selection(), qx());
}

#[tokio::test]
async fn timed_out_read_is_bounded_and_next_cycle_can_recover() {
    let fixture = Fixture::new().await;
    *fixture.store.selection.lock().expect("selection") = qx();
    fixture.store.hold_read.store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(
        Duration::from_secs(6),
        fixture
            .task
            .run_cycle(fixture.context(CancellationToken::new())),
    )
    .await
    .expect("cycle bounded by five seconds")
    .expect_err("timed out read");
    assert_eq!(
        error.as_safe_str(),
        "outbound user-agent reconciliation timed out"
    );
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    assert!(fixture.applied().is_empty());
    fixture.store.hold_read.store(false, Ordering::SeqCst);
    fixture.cycle().await;
    assert_eq!(fixture.selection(), qx());
}

#[tokio::test]
async fn preview_is_read_only_and_reconciliation_is_idempotent() {
    let fixture = Fixture::new().await;
    let selection = qx();
    let preview = fixture
        .services
        .outbound_user_agent()
        .preview(&fixture.provider.kind, &selection)
        .expect("preview");
    assert_eq!(preview.selection, qx());
    assert_eq!(fixture.selection(), ProviderUserAgentOverride::Default);
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.store.commits.load(Ordering::SeqCst), 0);
    assert!(fixture.applied().is_empty());
    *fixture.store.selection.lock().expect("selection") = selection;
    fixture.cycle().await;
    fixture.cycle().await;
    assert_eq!(fixture.selection(), qx());
    assert_eq!(fixture.applied(), vec![qx()]);
    assert_eq!(fixture.store.commits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn already_cancelled_worker_does_not_read_or_publish() {
    let fixture = Fixture::new().await;
    *fixture.store.selection.lock().expect("selection") = qx();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    fixture
        .task
        .run_cycle(fixture.context(cancellation))
        .await
        .expect("cancelled");
    assert_eq!(fixture.store.reads.load(Ordering::SeqCst), 0);
    assert!(fixture.applied().is_empty());
}

#[tokio::test]
async fn unified_save_preview_and_reconciliation_keep_the_complete_selection() {
    let fixture = Fixture::new().await;
    for user_agent in [None, Some("fixture-cli".to_owned())] {
        let selection = user_agent.map_or(ProviderUserAgentOverride::Default, |user_agent| {
            ProviderUserAgentOverride::Custom { user_agent }
        });
        let previous = fixture.selection();
        let preview = fixture
            .services
            .outbound_user_agent()
            .preview(&fixture.provider.kind, &selection)
            .unwrap();
        assert_eq!(preview.selection, selection);
        assert_eq!(fixture.selection(), previous);
        *fixture.store.reply.lock().expect("reply") = CommitReply::Lost;
        assert!(
            fixture
                .services
                .outbound_user_agent()
                .replace(&mutation(), &fixture.provider.kind, selection.clone())
                .await
                .is_err()
        );
        assert_eq!(fixture.selection(), previous);
        assert_eq!(
            *fixture.store.selection.lock().expect("persisted selection"),
            selection
        );
        fixture.cycle().await;
        assert_eq!(fixture.selection(), selection);
        *fixture.store.reply.lock().expect("reply") = CommitReply::Success;
        let saved = fixture
            .services
            .outbound_user_agent()
            .replace(&mutation(), &fixture.provider.kind, selection.clone())
            .await
            .unwrap();
        assert_eq!(saved.selection, selection);
    }
}
