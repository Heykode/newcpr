use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::{TimeDelta, Utc};
use futures::{StreamExt as _, future::BoxFuture};
use gateway_core::{
    account::{
        AccountStatusFacts, CredentialState, OpaqueProviderData, ProviderAccountId, QuotaEvidence,
        QuotaState, resolve_account_status,
    },
    engine::probe::{
        AccountProbe, AccountProbeError, AccountProbeErrorSource, AccountProbeRequest,
        AccountProbeResult,
    },
    error::{ClientVisibleUpstreamError, GatewayError, GatewayErrorKind},
    operation::{GenerateRequest, Operation, ProtocolPayload},
    routing::ProviderKind,
    upstream::UpstreamSendState,
};

use gateway_admin::{
    AdminServices,
    model::{
        AdminError, MutationContext, Revision,
        accounts::{
            AccountConnectionTestEvent, AccountCost, AccountCumulativeCost, AccountListQuery,
            AccountPage, AccountPageItem, AccountRecord, AccountRuntimeSnapshot, AccountSummary,
            AccountUpdateResult, AccountUsage, AccountUsageWindowQuery, AccountUsageWindowResult,
            AccountsUpdateResult, BatchUpdateAccounts, DeleteAccounts, UpdateAccount,
        },
        observability::TimeRange,
        provider_credentials::{
            AuthorizationCommit, AuthorizationCommitGuard, AuthorizationCredentialCommit,
            AuthorizationMutationTarget, AuthorizationStarted, CompleteAuthorization,
            ConsumeProviderResetCredit, CredentialCommitGuard, CredentialDetails,
            CredentialImportCommit, CredentialImportResult, CredentialListQuery,
            CredentialMutationResult, CredentialPage, CredentialRotationCommit,
            PendingAuthorizationMutation, PrepareCredentialImport, PrepareCredentialRefresh,
            PrepareCredentialRotation, PreparedAuthorizationCommit,
            PreparedAuthorizationCredential, PreparedCredentialCreate, PreparedCredentialImport,
            PreparedCredentialRotation, PreparedCredentialRotationFacts, ProviderDocument,
            ProviderExport, ProviderExportCredentialInput, ProviderModels,
            ProviderProfileActivityInsights, ProviderProfileStatistics,
            ProviderProfileStatisticsSummary, ProviderQuota, ProviderQuotaRequest,
            ProviderQuotaWindow, ProviderResetCreditResult, ProviderSubscription,
            QuotaLocalUsageAttribution,
        },
        quota_forecast_sampling::{QuotaForecastHistory, QuotaForecastUsage},
        quota_learning::{QuotaLearningEstimate, QuotaLearningObservation, QuotaLearningSource},
        settings::{
            AdminApiKey, AdminApiKeyMutation, ReplaceRuntimeSettings, RotationStrategy,
            RuntimeSettings,
        },
    },
    ports::{
        provider::{
            ProviderAdmin, ProviderAdminError, ProviderAdminErrorKind, ProviderAdminRegistry,
        },
        store::{
            AccountStore, AdminStoreError, AdminStoreErrorKind, AdminStoreResult, SettingsStore,
        },
    },
};
use serde_json::{Map, json};

pub(super) type EventLog = Arc<Mutex<Vec<&'static str>>>;

pub(super) struct FakeProviderAdmin {
    kind: ProviderKind,
    events: EventLog,
    failure: Mutex<Option<ProviderAdminError>>,
    quota_failure: Mutex<Option<ProviderAdminErrorKind>>,
    pending: Arc<Mutex<Option<PendingAuthorizationMutation>>>,
    retry_authorization_after_abort: Mutex<bool>,
    export_inputs: Mutex<Vec<ProviderExportCredentialInput>>,
    import_account_ids: Mutex<Vec<String>>,
    import_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
    quota_requests: Mutex<Vec<ProviderQuotaRequest>>,
    quota: Mutex<ProviderQuota>,
    quota_refresh_account: Mutex<Option<(Arc<FakeAccountStore>, AccountRecord)>>,
    current_credential_revision: Mutex<Revision>,
    pub(super) rotation_update: Mutex<Option<(Arc<FakeAccountStore>, AccountRecord)>>,
    pub(super) rotation_store: Mutex<Option<Arc<FakeAccountStore>>>,
    reset_credit_commands: Mutex<Vec<ConsumeProviderResetCredit>>,
    profile_result: Mutex<Result<ProviderProfileStatistics, ProviderAdminErrorKind>>,
    subscription_result: Mutex<Result<Option<ProviderSubscription>, ProviderAdminErrorKind>>,
    personal_info_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    pub(super) relogin_result: Mutex<Option<gateway_admin::model::relogin::ReloginCredential>>,
    pub(super) relogin_delay: Mutex<std::time::Duration>,
    pub(super) relogin_requests: Mutex<Vec<Option<String>>>,
    pub(super) relogin_active: std::sync::atomic::AtomicUsize,
    pub(super) relogin_peak: std::sync::atomic::AtomicUsize,
}

impl FakeProviderAdmin {
    pub(super) fn new(kind: &str, events: EventLog) -> Arc<Self> {
        Arc::new(Self {
            kind: ProviderKind::new(kind).expect("provider kind"),
            events,
            failure: Mutex::new(None),
            quota_failure: Mutex::new(None),
            pending: Arc::new(Mutex::new(None)),
            retry_authorization_after_abort: Mutex::new(false),
            export_inputs: Mutex::new(Vec::new()),
            import_account_ids: Mutex::new(vec!["acct_prepared".to_owned()]),
            import_gate: Mutex::new(None),
            quota_requests: Mutex::new(Vec::new()),
            quota: Mutex::new(empty_quota()),
            quota_refresh_account: Mutex::new(None),
            current_credential_revision: Mutex::new(revision(1)),
            rotation_update: Mutex::new(None),
            rotation_store: Mutex::new(None),
            reset_credit_commands: Mutex::new(Vec::new()),
            profile_result: Mutex::new(Ok(empty_profile_statistics())),
            subscription_result: Mutex::new(Ok(None)),
            personal_info_barrier: Mutex::new(None),
            relogin_result: Mutex::new(None),
            relogin_delay: Mutex::new(std::time::Duration::ZERO),
            relogin_requests: Mutex::new(Vec::new()),
            relogin_active: std::sync::atomic::AtomicUsize::new(0),
            relogin_peak: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    pub(super) fn block_imports(&self) -> Arc<tokio::sync::Semaphore> {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        *self.import_gate.lock().expect("import gate") = Some(gate.clone());
        gate
    }

    pub(super) fn fail_next(&self, kind: ProviderAdminErrorKind) {
        *self.failure.lock().expect("provider failure") = Some(ProviderAdminError::new(kind));
    }

    async fn wait_personal_info_queries(&self) {
        let barrier = self.personal_info_barrier.lock().unwrap().clone();
        if let Some(barrier) = barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
    }

    pub(super) fn fail_next_with_message(&self, kind: ProviderAdminErrorKind, message: &str) {
        *self.failure.lock().expect("provider failure") =
            Some(ProviderAdminError::new(kind).with_message(message));
    }

    pub(super) fn fail_next_with_public_message(
        &self,
        kind: ProviderAdminErrorKind,
        message: &'static str,
    ) {
        *self.failure.lock().expect("provider failure") = Some(
            ProviderAdminError::new(kind)
                .with_message("upstream body containing a secret token")
                .with_public_message(message),
        );
    }

    pub(super) fn fail_next_quota(&self, kind: ProviderAdminErrorKind) {
        *self.quota_failure.lock().expect("provider quota failure") = Some(kind);
    }

    pub(super) fn set_import_account_ids(&self, account_ids: &[&str]) {
        *self
            .import_account_ids
            .lock()
            .expect("provider import account IDs") = account_ids
            .iter()
            .map(|account_id| (*account_id).to_owned())
            .collect();
    }

    pub(super) fn quota_requests(&self) -> Vec<ProviderQuotaRequest> {
        self.quota_requests
            .lock()
            .expect("provider quota requests")
            .clone()
    }

    fn reset_credit_commands(&self) -> Vec<ConsumeProviderResetCredit> {
        self.reset_credit_commands
            .lock()
            .expect("provider reset-credit commands")
            .clone()
    }

    pub(super) fn set_quota(&self, quota: ProviderQuota) {
        *self.quota.lock().expect("provider quota") = quota;
    }

    pub(super) fn pending(&self) -> Option<PendingAuthorizationMutation> {
        self.pending.lock().expect("pending authorization").clone()
    }

    pub(super) fn retry_authorization_after_abort(&self) {
        *self
            .retry_authorization_after_abort
            .lock()
            .expect("authorization retry") = true;
    }

    pub(super) fn set_current_credential_revision(&self, revision: Revision) {
        *self
            .current_credential_revision
            .lock()
            .expect("current credential revision") = revision;
    }

    fn export_inputs(&self) -> Vec<ProviderExportCredentialInput> {
        self.export_inputs.lock().expect("export inputs").clone()
    }

    fn record(&self, event: &'static str) {
        self.events.lock().expect("provider events").push(event);
    }

    fn require_available(&self) -> Result<(), ProviderAdminError> {
        match self.failure.lock().expect("provider failure").take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn prepared_rotation(&self, account: &AccountRecord) -> PreparedCredentialRotation {
        PreparedCredentialRotation::new(
            PreparedCredentialRotationFacts {
                preserve_profile: false,
                account_id: ProviderAccountId::new(account.id.clone()).expect("account ID"),
                provider_kind: account.provider_kind.clone(),
                expected_credential_revision: *self
                    .current_credential_revision
                    .lock()
                    .expect("current credential revision"),
                replacement_identity: None,
                name: account.name.clone(),
                email: account.email.clone(),
                plan_type: account.plan_type.clone(),
                provider_material: document(),
                has_refresh_token: account.has_refresh_token,
                access_token_expires_at: account
                    .access_token_expires_at
                    .map(|expires_at| expires_at + TimeDelta::hours(1)),
                next_refresh_at: account.next_refresh_at,
            },
            Box::new(RecordingGuard::new(self.events.clone())),
        )
    }
}

#[async_trait]
impl ProviderAdmin for FakeProviderAdmin {
    async fn relogin(
        &self,
        request: gateway_admin::model::relogin::ReloginRequest,
    ) -> Result<gateway_admin::model::relogin::ReloginCredential, ProviderAdminError> {
        use std::sync::atomic::Ordering;
        self.relogin_requests
            .lock()
            .unwrap()
            .push(request.workspace_id);
        let active = self.relogin_active.fetch_add(1, Ordering::SeqCst) + 1;
        self.relogin_peak.fetch_max(active, Ordering::SeqCst);
        struct Active<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for Active<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _active = Active(&self.relogin_active);
        let delay = *self.relogin_delay.lock().unwrap();
        tokio::time::sleep(delay).await;
        let mut credential = self
            .relogin_result
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| ProviderAdminError::new(ProviderAdminErrorKind::Unsupported))?;
        credential.email = request.email;
        Ok(credential)
    }

    fn provider_kind(&self) -> &ProviderKind {
        &self.kind
    }

    async fn profile_statistics(
        &self,
        _: &ProviderAccountId,
    ) -> Result<ProviderProfileStatistics, ProviderAdminError> {
        self.record("provider.profile_statistics");
        self.wait_personal_info_queries().await;
        self.profile_result
            .lock()
            .unwrap()
            .clone()
            .map_err(ProviderAdminError::new)
    }

    async fn subscription(
        &self,
        _: &ProviderAccountId,
    ) -> Result<Option<ProviderSubscription>, ProviderAdminError> {
        self.record("provider.subscription");
        self.wait_personal_info_queries().await;
        self.subscription_result
            .lock()
            .unwrap()
            .clone()
            .map_err(ProviderAdminError::new)
    }

    fn plan_type_display(&self, plan_type: &str) -> String {
        format!("{} display: {plan_type}", self.kind)
    }

    async fn account_unavailable(&self, _: &ProviderAccountId) {
        self.record("provider.account_unavailable");
    }

    async fn account_facts_changed(&self, _: &[ProviderAccountId]) {
        self.record("provider.account_facts_changed");
    }

    fn connection_test_operation(
        &self,
        model: &gateway_core::routing::UpstreamModelId,
        input: &str,
    ) -> Result<gateway_core::operation::Operation, ProviderAdminError> {
        let payload = ProtocolPayload::json_object(
            "openai",
            Map::from_iter([
                ("model".to_owned(), json!(model.as_str())),
                ("input".to_owned(), json!(input)),
                ("stream".to_owned(), json!(true)),
                ("store".to_owned(), json!(false)),
            ]),
        )
        .map_err(|_| ProviderAdminError::new(ProviderAdminErrorKind::Invalid))?;
        Ok(Operation::Generate(GenerateRequest::from_protocol_payload(
            payload,
        )))
    }

    fn dashboard_wire_profile(
        &self,
    ) -> Option<gateway_admin::model::observability::DashboardWireProfile> {
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
        _command: PrepareCredentialImport,
    ) -> Result<PreparedCredentialImport, ProviderAdminError> {
        self.record("provider.prepare_import");
        let gate = self.import_gate.lock().expect("import gate").clone();
        if let Some(gate) = gate {
            gate.acquire().await.expect("import permit").forget();
        }
        self.require_available()?;
        let account_ids = self
            .import_account_ids
            .lock()
            .expect("provider import account IDs")
            .clone();
        Ok(PreparedCredentialImport {
            provider_kind: self.kind.clone(),
            credentials: account_ids
                .into_iter()
                .map(|account_id| {
                    let mut prepared =
                        prepared_create_with_id(self.kind.clone(), &account_id, "prepared-import");
                    if let Some(credential) = self.relogin_result.lock().unwrap().as_ref() {
                        prepared.email = Some(credential.email.clone());
                        prepared.upstream_user_id = Some(credential.user_id.clone());
                        prepared.upstream_account_id = Some(credential.workspace_id.clone());
                        prepared.plan_type = Some(credential.plan_type.clone());
                    }
                    prepared
                })
                .collect(),
        })
    }

    async fn start_authorization(
        &self,
        pending: PendingAuthorizationMutation,
    ) -> Result<AuthorizationStarted, ProviderAdminError> {
        self.record("provider.start_authorization");
        self.require_available()?;
        *self.pending.lock().expect("pending authorization") = Some(pending);
        Ok(AuthorizationStarted {
            flow_id: "flow-test".to_owned(),
            authorization_url: "https://example.invalid/oauth".to_owned(),
            expires_at: Utc::now() + TimeDelta::minutes(10),
        })
    }

    async fn complete_authorization(
        &self,
        command: CompleteAuthorization,
    ) -> Result<PreparedAuthorizationCommit, ProviderAdminError> {
        self.record("provider.complete_authorization");
        self.require_available()?;
        let pending = self
            .pending
            .lock()
            .expect("pending authorization")
            .take()
            .ok_or_else(unsupported)?;
        if !pending.owner_binding().matches_context(&command.context) {
            return Err(ProviderAdminError::new(ProviderAdminErrorKind::NotFound));
        }
        let retry_pending = (*self
            .retry_authorization_after_abort
            .lock()
            .expect("authorization retry"))
        .then(|| pending.clone());
        let credential = match pending.target() {
            AuthorizationMutationTarget::Create { name } => {
                PreparedAuthorizationCredential::Create(prepared_create(self.kind.clone(), name))
            }
            AuthorizationMutationTarget::Reauthorize { account_id } => {
                let mut account = account_record(self.kind.as_str());
                account.id = account_id.as_str().to_owned();
                PreparedAuthorizationCredential::Reauthorize(self.prepared_rotation(&account))
            }
        };
        let prepared = PreparedAuthorizationCommit::new(pending, credential);
        Ok(match retry_pending {
            Some(pending) => {
                prepared.with_authorization_guard(Box::new(RetryableAuthorizationGuard::new(
                    self.events.clone(),
                    Arc::clone(&self.pending),
                    pending,
                )))
            }
            None => prepared,
        })
    }

    async fn prepare_rotation(
        &self,
        command: PrepareCredentialRotation,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        self.record("provider.prepare_rotation");
        self.require_available()?;
        if let Some((store, account)) = self.rotation_update.lock().unwrap().take() {
            store.set_accounts(vec![account]);
        }
        if let Some(store) = self.rotation_store.lock().unwrap().as_ref() {
            let current = store
                .accounts
                .lock()
                .unwrap()
                .iter()
                .find(|account| account.id == command.account.id)
                .cloned()
                .ok_or_else(|| ProviderAdminError::new(ProviderAdminErrorKind::NotFound))?;
            self.set_current_credential_revision(current.credential_revision);
            return Ok(self.prepared_rotation(&current));
        }
        Ok(self.prepared_rotation(&command.account))
    }

    async fn prepare_refresh(
        &self,
        command: PrepareCredentialRefresh,
    ) -> Result<PreparedCredentialRotation, ProviderAdminError> {
        self.record("provider.prepare_refresh");
        self.require_available()?;
        Ok(self.prepared_rotation(&command.account))
    }

    async fn quota(
        &self,
        request: ProviderQuotaRequest,
    ) -> Result<ProviderQuota, ProviderAdminError> {
        if request.refresh {
            self.record("provider.quota");
            if let Some((store, account)) = self
                .quota_refresh_account
                .lock()
                .expect("quota refresh account")
                .take()
            {
                store.set_accounts(vec![account]);
            }
        }
        self.quota_requests
            .lock()
            .expect("provider quota requests")
            .push(request);
        if let Some(kind) = self
            .quota_failure
            .lock()
            .expect("provider quota failure")
            .take()
        {
            return Err(ProviderAdminError::new(kind));
        }
        Ok(self.quota.lock().expect("provider quota").clone())
    }

    async fn consume_reset_credit(
        &self,
        command: ConsumeProviderResetCredit,
    ) -> Result<ProviderResetCreditResult, ProviderAdminError> {
        self.record("provider.consume_reset_credit");
        self.reset_credit_commands
            .lock()
            .expect("provider reset-credit commands")
            .push(command);
        self.require_available()?;
        Ok(ProviderResetCreditResult {
            code: "reset".to_owned(),
            credit: None,
        })
    }

    fn quota_forecast_observation(
        &self,
        document: &ProviderDocument,
        _: &ProviderQuotaWindow,
    ) -> Option<gateway_admin::model::quota_forecast_sampling::QuotaForecastObservation> {
        let fields = document.expose_to_provider().expose_to_provider();
        Some(
            gateway_admin::model::quota_forecast_sampling::QuotaForecastObservation {
                used_percent: fields.get("percent")?.as_f64()?,
                reset_at: fields.get("reset")?.as_str()?.parse().ok()?,
                plan_type: fields.get("plan")?.as_str().map(str::to_owned),
            },
        )
    }

    async fn models(
        &self,
        _: &ProviderAccountId,
        _: bool,
    ) -> Result<ProviderModels, ProviderAdminError> {
        Ok(ProviderModels {
            models: Vec::new(),
            observed_at: None,
        })
    }

    async fn export_credentials(
        &self,
        credentials: Vec<ProviderExportCredentialInput>,
    ) -> Result<ProviderExport, ProviderAdminError> {
        self.record("provider.export");
        self.require_available()?;
        *self.export_inputs.lock().expect("export inputs") = credentials.clone();
        Ok(ProviderExport {
            provider_kind: self.kind.clone(),
            account_ids: credentials
                .into_iter()
                .map(|credential| {
                    ProviderAccountId::new(credential.account.id).expect("stored account ID")
                })
                .collect(),
            document: document(),
        })
    }
}

pub(super) struct FakeAccountStore {
    events: EventLog,
    accounts: Mutex<Vec<AccountRecord>>,
    account_after_probe: Mutex<Option<AccountRecord>>,
    fail_commit: Mutex<bool>,
    pub(super) rotation_updates: Mutex<Vec<AccountRecord>>,
    pub(super) batch_updates: Mutex<Vec<BatchUpdateAccounts>>,
    pub(super) rotation_attempts: Mutex<Vec<CredentialRotationCommit>>,
    pub(super) credential_detail_updates: Mutex<Vec<AccountRecord>>,
    audit_requests: Mutex<Vec<String>>,
    import_settings: Mutex<Vec<Option<gateway_admin::model::accounts::AccountImportSettings>>>,
    quota_window_usage: Mutex<Vec<AccountUsageWindowResult>>,
    quota_window_queries: Mutex<Vec<AccountUsageWindowQuery>>,
    quota_forecast_history: Mutex<QuotaForecastHistory>,
    quota_learning_observations: Mutex<Vec<QuotaLearningObservation>>,
    quota_learning_estimates: Mutex<Vec<QuotaLearningEstimate>>,
    quota_learning_failure: Mutex<bool>,
    cumulative_costs: Mutex<BTreeMap<String, Vec<AccountCumulativeCost>>>,
    cumulative_cost_queries: Mutex<Vec<Vec<String>>>,
    turn_states: Mutex<BTreeMap<String, gateway_admin::model::accounts::AccountTurnStateStatus>>,
    projection_override: Mutex<Option<gateway_core::account::AccountStatus>>,
}

impl FakeAccountStore {
    pub(super) fn new(kind: &str, events: EventLog) -> Arc<Self> {
        Self::with_account(account_record(kind), events)
    }

    fn with_account(account: AccountRecord, events: EventLog) -> Arc<Self> {
        Arc::new(Self {
            events,
            accounts: Mutex::new(vec![account]),
            account_after_probe: Mutex::new(None),
            fail_commit: Mutex::new(false),
            rotation_updates: Mutex::new(Vec::new()),
            batch_updates: Mutex::new(Vec::new()),
            rotation_attempts: Mutex::new(Vec::new()),
            credential_detail_updates: Mutex::new(Vec::new()),
            audit_requests: Mutex::new(Vec::new()),
            import_settings: Mutex::new(Vec::new()),
            quota_window_usage: Mutex::new(Vec::new()),
            quota_window_queries: Mutex::new(Vec::new()),
            quota_forecast_history: Mutex::new(QuotaForecastHistory::default()),
            quota_learning_observations: Mutex::new(Vec::new()),
            quota_learning_estimates: Mutex::new(Vec::new()),
            quota_learning_failure: Mutex::new(false),
            cumulative_costs: Mutex::new(BTreeMap::new()),
            cumulative_cost_queries: Mutex::new(Vec::new()),
            turn_states: Mutex::default(),
            projection_override: Mutex::default(),
        })
    }

    pub(super) fn fail_next_commit(&self) {
        *self.fail_commit.lock().expect("store failure") = true;
    }

    pub(super) fn import_settings(
        &self,
    ) -> Vec<Option<gateway_admin::model::accounts::AccountImportSettings>> {
        self.import_settings
            .lock()
            .expect("import settings")
            .clone()
    }

    pub(super) fn audit_requests(&self) -> Vec<String> {
        self.audit_requests.lock().expect("audit requests").clone()
    }

    pub(super) fn set_quota_window_usage(&self, usage: Vec<AccountUsageWindowResult>) {
        *self.quota_window_usage.lock().expect("quota window usage") = usage;
    }

    fn set_cumulative_costs(&self, account_id: &str, costs: Vec<AccountCumulativeCost>) {
        self.cumulative_costs
            .lock()
            .expect("cumulative costs")
            .insert(account_id.to_owned(), costs);
    }

    fn quota_window_queries(&self) -> Vec<AccountUsageWindowQuery> {
        self.quota_window_queries
            .lock()
            .expect("quota window queries")
            .clone()
    }

    pub(super) fn set_accounts(&self, accounts: Vec<AccountRecord>) {
        *self.accounts.lock().expect("accounts") = accounts;
    }

    fn set_account_after_probe(&self, account: AccountRecord) {
        *self
            .account_after_probe
            .lock()
            .expect("account after probe") = Some(account);
    }

    fn record(&self, event: &'static str) {
        self.events.lock().expect("store events").push(event);
    }

    fn require_commit(&self) -> AdminStoreResult<()> {
        let mut failure = self.fail_commit.lock().expect("store failure");
        if std::mem::take(&mut *failure) {
            Err(store_unavailable())
        } else {
            Ok(())
        }
    }

    fn record_context(&self, context: &MutationContext) {
        self.audit_requests
            .lock()
            .expect("audit requests")
            .push(context.request_id.clone());
    }

    fn page_item(account: AccountRecord) -> AccountPageItem {
        let facts = AccountStatusFacts {
            enabled: account.enabled,
            credential_state: account.credential_state,
            access_token_expires_at: account.access_token_expires_at.map(Into::into),
            quota: account.quota,
            rate_limited_until: None,
            last_error_reason: account.last_error_reason,
            last_error_message: account.last_error_message.clone(),
        };
        AccountPageItem {
            account,
            projection: resolve_account_status(&facts, std::time::SystemTime::now()),
            in_flight: None,
        }
    }
}

#[async_trait]
impl AccountStore for FakeAccountStore {
    async fn load_turn_state_status(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, gateway_admin::model::accounts::AccountTurnStateStatus>>
    {
        Ok(self
            .turn_states
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _)| account_ids.contains(id))
            .map(|(id, state)| (id.clone(), state.clone()))
            .collect())
    }

    async fn list_accounts(
        &self,
        _: AccountListQuery,
        _: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<AccountPage> {
        self.record("store.list_accounts");
        let accounts = self.accounts.lock().expect("accounts").clone();
        let total = accounts.len() as u64;
        Ok(AccountPage {
            config_revision: revision(1),
            items: accounts
                .into_iter()
                .map(|account| {
                    let mut item = Self::page_item(account);
                    if let Some(status) = *self.projection_override.lock().unwrap() {
                        item.projection.status = status;
                    }
                    item
                })
                .collect(),
            total,
            summary: AccountSummary {
                total,
                normal: 1,
                quota_exhausted: 0,
                rate_limited: 0,
                disabled: 0,
                error: 0,
            },
        })
    }

    async fn load_account(
        &self,
        account_id: &str,
        _: AccountRuntimeSnapshot,
    ) -> AdminStoreResult<Option<AccountPageItem>> {
        self.record("store.load_account");
        // probe 后的账号状态覆盖只对同一 id 生效；其余按账号列表查询。
        let account = self
            .account_after_probe
            .lock()
            .expect("account after probe")
            .clone()
            .filter(|account| account.id == account_id)
            .or_else(|| {
                self.accounts
                    .lock()
                    .expect("accounts")
                    .iter()
                    .find(|account| account.id == account_id)
                    .cloned()
            });
        Ok(account.map(|account| {
            let mut item = Self::page_item(account);
            if let Some(status) = *self.projection_override.lock().unwrap() {
                item.projection.status = status;
            }
            item
        }))
    }

    async fn load_account_usage(
        &self,
        _: TimeRange,
        _: &[String],
    ) -> AdminStoreResult<Vec<AccountUsage>> {
        Ok(Vec::new())
    }

    async fn load_account_cumulative_costs(
        &self,
        account_ids: &[String],
    ) -> AdminStoreResult<BTreeMap<String, Vec<AccountCumulativeCost>>> {
        self.cumulative_cost_queries
            .lock()
            .expect("cumulative cost queries")
            .push(account_ids.to_vec());
        let costs = self.cumulative_costs.lock().expect("cumulative costs");
        Ok(account_ids
            .iter()
            .filter_map(|id| costs.get(id).map(|costs| (id.clone(), costs.clone())))
            .collect())
    }

    async fn load_account_usage_by_windows(
        &self,
        windows: &[AccountUsageWindowQuery],
    ) -> AdminStoreResult<Vec<AccountUsageWindowResult>> {
        *self
            .quota_window_queries
            .lock()
            .expect("quota window queries") = windows.to_vec();
        Ok(self
            .quota_window_usage
            .lock()
            .expect("quota window usage")
            .clone())
    }

    async fn load_quota_forecast_history(
        &self,
        window: &AccountUsageWindowQuery,
    ) -> AdminStoreResult<QuotaForecastHistory> {
        self.quota_window_queries
            .lock()
            .expect("quota window queries")
            .push(window.clone());
        Ok(self
            .quota_forecast_history
            .lock()
            .expect("quota forecast history")
            .clone())
    }

    async fn record_quota_learning(
        &self,
        observations: &[QuotaLearningObservation],
    ) -> AdminStoreResult<Vec<QuotaLearningEstimate>> {
        *self.quota_learning_observations.lock().unwrap() = observations.to_vec();
        if *self.quota_learning_failure.lock().unwrap() {
            return Err(store_unavailable());
        }
        Ok(self.quota_learning_estimates.lock().unwrap().clone())
    }

    async fn list_credentials(
        &self,
        provider_kind: &ProviderKind,
        _: CredentialListQuery,
    ) -> AdminStoreResult<CredentialPage> {
        self.record("store.list_credentials");
        let accounts = self.accounts.lock().expect("accounts").clone();
        Ok(CredentialPage {
            config_revision: revision(1),
            items: accounts
                .into_iter()
                .filter(|account| &account.provider_kind == provider_kind)
                .collect(),
            next_cursor: None,
        })
    }

    async fn credential_details(
        &self,
        provider_kind: &ProviderKind,
        account_id: &ProviderAccountId,
    ) -> AdminStoreResult<Option<CredentialDetails>> {
        self.record("store.credential_details");
        {
            let mut updates = self.credential_detail_updates.lock().unwrap();
            if !updates.is_empty() {
                self.set_accounts(vec![updates.remove(0)]);
            }
        }
        let account = self
            .accounts
            .lock()
            .expect("accounts")
            .iter()
            .find(|account| {
                &account.provider_kind == provider_kind && account.id == account_id.as_str()
            })
            .cloned();
        Ok(account.map(|credential| CredentialDetails {
            config_revision: revision(1),
            credential,
        }))
    }

    async fn load_credentials_for_export(
        &self,
        provider_kind: &ProviderKind,
        account_ids: &[ProviderAccountId],
    ) -> AdminStoreResult<Vec<ProviderExportCredentialInput>> {
        self.record("store.load_credentials_for_export");
        let accounts = self.accounts.lock().expect("accounts").clone();
        if accounts.iter().any(|account| {
            &account.provider_kind != provider_kind
                || !account_ids
                    .iter()
                    .any(|account_id| account_id.as_str() == account.id)
        }) {
            return Err(AdminStoreError::new(
                AdminStoreErrorKind::NotFound,
                "test account",
                "credential not found",
            ));
        }
        Ok(accounts
            .into_iter()
            .map(|account| ProviderExportCredentialInput {
                account,
                provider_material: document(),
            })
            .collect())
    }

    async fn commit_credential_import(
        &self,
        command: CredentialImportCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult> {
        self.import_settings
            .lock()
            .expect("import settings")
            .push(command.settings);
        self.record("store.commit_import");
        self.record_context(context);
        self.require_commit()?;
        Ok(CredentialImportResult {
            config_revision: revision(2),
            credential_ids: command
                .prepared
                .credentials
                .into_iter()
                .map(|credential| credential.account_id)
                .collect(),
        })
    }

    async fn commit_new_credential_import(
        &self,
        command: CredentialImportCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialImportResult> {
        let additions: Vec<_> = command
            .prepared
            .credentials
            .iter()
            .map(|prepared| {
                let mut account = account_record(prepared.provider_kind.as_str());
                account.id = prepared.account_id.as_str().to_owned();
                account.email = prepared.email.clone();
                account.upstream_user_id = prepared.upstream_user_id.clone();
                account.upstream_account_id = prepared.upstream_account_id.clone();
                account.plan_type = prepared.plan_type.clone();
                account
            })
            .collect();
        let result = self.commit_credential_import(command, context).await?;
        self.accounts.lock().unwrap().extend(additions);
        Ok(result)
    }

    async fn commit_authorization(
        &self,
        command: AuthorizationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult> {
        self.import_settings
            .lock()
            .expect("import settings")
            .push(command.settings);
        self.record("store.commit_authorization");
        self.record_context(context);
        self.require_commit()?;
        let (account_id, credential_revision) = match command.credential {
            AuthorizationCredentialCommit::Create(credential) => (credential.account_id, None),
            AuthorizationCredentialCommit::Reauthorize(credential) => (
                credential.account_id,
                Some(revision(credential.expected_credential_revision.get() + 1)),
            ),
        };
        Ok(CredentialMutationResult {
            config_revision: revision(2),
            account_id,
            credential_revision,
        })
    }

    async fn commit_credential_rotation(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult> {
        self.record("store.commit_rotation");
        self.record_context(context);
        self.rotation_attempts.lock().unwrap().push(command.clone());
        self.require_commit()?;
        if command.relogin_operation_id.is_some() {
            let mut updates = self.rotation_updates.lock().unwrap();
            if !updates.is_empty() {
                self.set_accounts(vec![updates.remove(0)]);
            }
            let mut accounts = self.accounts.lock().unwrap();
            let account = accounts
                .iter_mut()
                .find(|account| account.id == command.prepared.account_id.as_str())
                .expect("relogin target");
            if account.credential_revision != command.prepared.expected_credential_revision {
                return Err(AdminStoreError::new(
                    AdminStoreErrorKind::Conflict,
                    "account",
                    "credential revision changed",
                ));
            }
            account.credential_revision = revision(account.credential_revision.get() + 1);
            account.turn_state_binding_revision = account.credential_revision;
            account.relogin_count += 1;
            account.last_relogin_at = Some(Utc::now());
            account.credential_state = CredentialState::Ready;
        }
        Ok(rotation_result(command))
    }

    async fn commit_credential_refresh(
        &self,
        command: CredentialRotationCommit,
        context: &MutationContext,
    ) -> AdminStoreResult<CredentialMutationResult> {
        self.record("store.commit_refresh");
        self.record_context(context);
        self.require_commit()?;
        Ok(rotation_result(command))
    }

    async fn update_account(
        &self,
        command: UpdateAccount,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult> {
        self.record("store.update_account");
        self.record_context(context);
        self.require_commit()?;
        Ok(AccountUpdateResult {
            config_revision: revision(2),
            account_id: ProviderAccountId::new(command.account_id).expect("account ID"),
        })
    }

    async fn recover_account(
        &self,
        account_id: &ProviderAccountId,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountUpdateResult> {
        self.record("store.recover_account");
        self.record_context(context);
        self.require_commit()?;
        let mut accounts = self.accounts.lock().expect("accounts");
        if let Some(account) = accounts
            .iter_mut()
            .find(|account| account.id == account_id.as_str())
        {
            account.enabled = true;
            account.credential_state = CredentialState::Ready;
            account.quota = QuotaState::allowed(std::time::SystemTime::now());
            account.last_error_reason = None;
            account.last_error_message = None;
        }
        Ok(AccountUpdateResult {
            config_revision: revision(2),
            account_id: account_id.clone(),
        })
    }

    async fn batch_update_accounts(
        &self,
        command: BatchUpdateAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<AccountsUpdateResult> {
        self.record("store.batch_update_accounts");
        self.record_context(context);
        self.require_commit()?;
        self.batch_updates.lock().unwrap().push(command.clone());
        if let Some(enabled) = command.enabled {
            for account in self.accounts.lock().expect("accounts").iter_mut() {
                if command.account_ids.contains(&account.id) {
                    account.enabled = enabled;
                }
            }
        }
        Ok(AccountsUpdateResult {
            config_revision: revision(2),
            account_ids: command
                .account_ids
                .into_iter()
                .map(|id| ProviderAccountId::new(id).expect("account ID"))
                .collect(),
        })
    }

    async fn delete_accounts(
        &self,
        _: DeleteAccounts,
        context: &MutationContext,
    ) -> AdminStoreResult<Revision> {
        self.record("store.delete");
        self.record_context(context);
        self.require_commit()?;
        Ok(revision(2))
    }

    async fn record_credential_export(
        &self,
        _: &[ProviderAccountId],
        context: &MutationContext,
    ) -> AdminStoreResult<()> {
        self.record("store.audit_export");
        self.record_context(context);
        Ok(())
    }
}

struct StaticSettingsStore;

#[async_trait]
impl SettingsStore for StaticSettingsStore {
    async fn load_runtime_settings(&self) -> AdminStoreResult<RuntimeSettings> {
        Ok(RuntimeSettings {
            turn_state_probe_proxy_id: None,
            turn_state_probe_concurrency: 3,
            config_revision: revision(1),
            disable_fast: false,
            turn_state_injection_enabled: false,
            turn_state_models: vec![
                gateway_core::routing::UpstreamModelId::new("gpt-6-astra".to_owned())
                    .expect("model"),
            ],
            responses_max_decompressed_body_bytes: 64 * 1024 * 1024,
            model_mappings: Default::default(),
            refresh_margin_seconds: 300,
            refresh_concurrency: 2,
            max_concurrent_per_account: 1,
            request_interval_ms: 0,
            rotation_strategy: RotationStrategy::Smart,
            min_codex_desktop_version: None,
            min_codex_cli_version: None,
            usage_retention_days: 30,
            ops_event_retention_days: 30,
            audit_retention_days: 30,
            request_tuning: Default::default(),
            updated_at: Utc::now(),
        })
    }

    async fn admin_api_key_exists(&self) -> AdminStoreResult<bool> {
        Err(store_unavailable())
    }

    async fn replace_runtime_settings(
        &self,
        _: ReplaceRuntimeSettings,
        _: &MutationContext,
    ) -> AdminStoreResult<RuntimeSettings> {
        Err(store_unavailable())
    }

    async fn replace_admin_api_key(
        &self,
        _: AdminApiKey,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(store_unavailable())
    }

    async fn delete_admin_api_key(
        &self,
        _: &MutationContext,
    ) -> AdminStoreResult<AdminApiKeyMutation> {
        Err(store_unavailable())
    }
}

struct RecordingGuard {
    events: EventLog,
    finished: bool,
}

impl RecordingGuard {
    fn new(events: EventLog) -> Self {
        Self {
            events,
            finished: false,
        }
    }
}

impl CredentialCommitGuard for RecordingGuard {
    fn finish(mut self: Box<Self>) {
        self.events
            .lock()
            .expect("guard events")
            .push("guard.finish");
        self.finished = true;
    }
}

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.events.lock().expect("guard events").push("guard.drop");
        }
    }
}

struct RetryableAuthorizationGuard {
    events: EventLog,
    pending: Arc<Mutex<Option<PendingAuthorizationMutation>>>,
    authorization: PendingAuthorizationMutation,
}

impl RetryableAuthorizationGuard {
    fn new(
        events: EventLog,
        pending: Arc<Mutex<Option<PendingAuthorizationMutation>>>,
        authorization: PendingAuthorizationMutation,
    ) -> Self {
        Self {
            events,
            pending,
            authorization,
        }
    }
}

#[async_trait]
impl AuthorizationCommitGuard for RetryableAuthorizationGuard {
    async fn commit(self: Box<Self>) -> Result<(), AdminError> {
        self.events
            .lock()
            .expect("authorization guard events")
            .push("authorization_guard.commit");
        Ok(())
    }

    async fn abort(self: Box<Self>) -> Result<(), AdminError> {
        let Self {
            events,
            pending,
            authorization,
        } = *self;
        *pending.lock().expect("pending authorization") = Some(authorization);
        events
            .lock()
            .expect("authorization guard events")
            .push("authorization_guard.abort");
        Ok(())
    }
}

#[test]
fn provider_registry_should_resolve_custom_kind_without_central_match() {
    let provider = FakeProviderAdmin::new("custom-provider", events());
    let registry = ProviderAdminRegistry::new([provider as Arc<dyn ProviderAdmin>])
        .expect("provider registry");
    let resolved = registry
        .require(&ProviderKind::new("custom-provider").expect("provider kind"))
        .expect("registered provider");
    assert_eq!(resolved.provider_kind().as_str(), "custom-provider");
}

#[test]
fn provider_registry_should_reject_duplicate_kind() {
    let first = FakeProviderAdmin::new("duplicate", events());
    let second = FakeProviderAdmin::new("duplicate", events());
    let result = ProviderAdminRegistry::new([
        first as Arc<dyn ProviderAdmin>,
        second as Arc<dyn ProviderAdmin>,
    ]);
    assert!(matches!(
        result,
        Err(error) if error.kind() == ProviderAdminErrorKind::Conflict
    ));
}

#[tokio::test]
async fn connection_test_should_probe_unavailable_account() {
    let provider = FakeProviderAdmin::new("xai", events());
    let mut account = account_record("xai");
    account.quota = QuotaState::exhausted(
        QuotaEvidence::UsageLimitReached,
        std::time::SystemTime::now(),
        None,
    );
    let store = FakeAccountStore::with_account(account, events());
    let services =
        accounts_service_with_probe(provider, store, Arc::new(FailingAccountProbe)).await;

    let events = services
        .accounts()
        .test_connection(gateway_admin::model::accounts::AccountConnectionTest {
            account_id: ProviderAccountId::new("acct_test").expect("account ID"),
            upstream_model: gateway_core::routing::UpstreamModelId::new("grok-4.5").expect("model"),
            endpoint: gateway_admin::model::accounts::ConnectionTestEndpoint::Responses,
            input_text: "Reply with exactly OK.".to_owned(),
            stream: true,
        })
        .await
        .expect("connection test stream")
        .collect::<Vec<_>>()
        .await;

    assert!(matches!(
        events.last(),
        Some(AccountConnectionTestEvent::Failed {
            source: AccountProbeErrorSource::Upstream,
            gateway_error_code: GatewayErrorKind::RateLimited,
            send_state: Some(UpstreamSendState::NotSent),
            message,
            provider_error_code: Some(code),
            provider_error_type: Some(error_type),
            ..
        }) if message == "included usage exhausted"
            && code == "usage_exhausted"
            && error_type == "invalid_request_error"
    ));
}

#[tokio::test]
async fn connection_test_rate_limited_probe_returns_provider_failure() {
    let events = events();
    let provider = FakeProviderAdmin::new("xai", events.clone());
    let store = FakeAccountStore::new("xai", events);
    let probe = Arc::new(FreeModelQuotaProbe {
        store: Arc::clone(&store),
    });
    let services = accounts_service_with_probe(provider, store, probe).await;

    let events = services
        .accounts()
        .test_connection(gateway_admin::model::accounts::AccountConnectionTest {
            account_id: ProviderAccountId::new("acct_test").expect("account ID"),
            upstream_model: gateway_core::routing::UpstreamModelId::new("grok-4.5").expect("model"),
            endpoint: gateway_admin::model::accounts::ConnectionTestEndpoint::Responses,
            input_text: "Reply with exactly OK.".to_owned(),
            stream: true,
        })
        .await
        .expect("connection test stream")
        .collect::<Vec<_>>()
        .await;

    assert!(matches!(
        events.last(),
        Some(AccountConnectionTestEvent::Failed {
            source: AccountProbeErrorSource::Provider,
            gateway_error_code: GatewayErrorKind::RateLimited,
            send_state: Some(UpstreamSendState::NotSent),
            message,
            ..
        })
        if message == "xAI free model quota is exhausted"
    ));
}

#[tokio::test]
async fn connection_test_should_preserve_disabled_account_status() {
    let provider = FakeProviderAdmin::new("xai", events());
    let mut account = account_record("xai");
    account.enabled = false;
    let store = FakeAccountStore::with_account(account, events());
    let services =
        accounts_service_with_probe(provider, store, Arc::new(FailingAccountProbe)).await;

    let events = services
        .accounts()
        .test_connection(gateway_admin::model::accounts::AccountConnectionTest {
            account_id: ProviderAccountId::new("acct_test").expect("account ID"),
            upstream_model: gateway_core::routing::UpstreamModelId::new("grok-4.5").expect("model"),
            endpoint: gateway_admin::model::accounts::ConnectionTestEndpoint::Responses,
            input_text: "Reply with exactly OK.".to_owned(),
            stream: true,
        })
        .await
        .expect("connection test stream")
        .collect::<Vec<_>>()
        .await;

    assert!(matches!(
        events.last(),
        Some(AccountConnectionTestEvent::Failed { .. })
    ));
}

#[tokio::test]
async fn accounts_export_should_pass_store_loaded_timestamps_and_material_to_provider() {
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let expected = store.accounts.lock().expect("accounts")[0].clone();
    accounts_service(provider.clone(), store)
        .await
        .accounts()
        .export(
            &context("export-complete-input"),
            vec![ProviderAccountId::new(expected.id.clone()).expect("account ID")],
        )
        .await
        .expect("export credentials");

    let inputs = provider.export_inputs();
    let input = inputs.first().expect("provider export input");
    assert_eq!(
        (
            input.account.created_at,
            input.account.updated_at,
            &input.provider_material,
        ),
        (expected.created_at, expected.updated_at, &document()),
    );
}

#[tokio::test]
async fn accounts_refresh_should_keep_guard_through_store_commit() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider, store.clone()).await;

    let result = services
        .accounts()
        .refresh(
            &context("refresh-request"),
            ProviderAccountId::new("acct_test").expect("account ID"),
        )
        .await
        .expect("refresh credential");
    assert_eq!(result.config_revision, revision(2));
    assert_eq!(result.account.account.provider_kind.as_str(), "openai");
    assert_eq!(
        result.account.projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal
    );

    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "provider.prepare_refresh",
            "store.commit_refresh",
            "guard.finish",
            "provider.account_facts_changed",
            "store.load_account",
        ]
    );
    assert_eq!(store.audit_requests(), ["refresh-request"]);
}

#[tokio::test]
async fn accounts_quota_refresh_should_return_the_updated_account_status() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let mut account = account_record("openai");
    account.quota = QuotaState::exhausted(
        QuotaEvidence::UsageLimitReached,
        std::time::SystemTime::now(),
        None,
    );
    let store = FakeAccountStore::with_account(account.clone(), events.clone());
    account.quota = QuotaState::allowed(std::time::SystemTime::now());
    *provider
        .quota_refresh_account
        .lock()
        .expect("quota refresh account") = Some((store.clone(), account));

    let result = accounts_service(provider, store)
        .await
        .accounts()
        .quota(
            &ProviderAccountId::new("acct_test").expect("account ID"),
            true,
        )
        .await
        .expect("refresh quota");

    assert_eq!(
        result.account.quota.access(),
        gateway_core::account::QuotaAccessState::Allowed
    );
    assert_eq!(
        result.projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal
    );
    assert_eq!(
        recorded(&events),
        ["store.load_account", "provider.quota", "store.load_account"]
    );
}

#[tokio::test]
async fn accounts_recover_disabled_preserves_errors_state_settings_and_quota() {
    for has_error in [false, true] {
        let events = events();
        let provider = FakeProviderAdmin::new("openai", events.clone());
        let mut account = account_record("openai");
        account.enabled = false;
        account.turn_state_injection_enabled = true;
        if has_error {
            account.credential_state = CredentialState::Invalid;
            account.last_error_reason =
                Some(gateway_core::account::AccountErrorReason::CredentialInvalid);
            account.last_error_message = Some("invalid credential".to_owned());
            account.quota = QuotaState::exhausted(
                QuotaEvidence::UsageLimitReached,
                std::time::SystemTime::now(),
                None,
            );
        }
        let store = FakeAccountStore::with_account(account.clone(), events.clone());
        let services = accounts_service(provider.clone(), store.clone()).await;
        let result = services
            .accounts()
            .recover(
                &context("enable-request"),
                ProviderAccountId::new("acct_test").unwrap(),
            )
            .await
            .unwrap();
        account.enabled = true;
        assert_eq!(result.account.account, account);
        assert_eq!(
            result.account.projection.status,
            if has_error {
                gateway_admin::model::accounts::AccountStatus::Error
            } else {
                gateway_admin::model::accounts::AccountStatus::Normal
            }
        );
        assert_eq!(
            recorded(&events),
            [
                "store.load_account",
                "store.batch_update_accounts",
                "provider.account_facts_changed",
                "store.load_account",
            ]
        );
        assert_eq!(store.audit_requests(), ["enable-request"]);
        let commands = store.batch_updates.lock().unwrap();
        assert_eq!(commands.len(), 1);
        assert!(commands[0].turn_state_injection_enabled.is_none());
        assert!(commands[0].custom_name.is_none());
        assert!(commands[0].outbound_proxy.is_none());
        let requests = provider.quota_requests();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].refresh);
    }
}

#[tokio::test]
async fn accounts_recover_disabled_does_not_publish_failed_enable() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let mut account = account_record("openai");
    account.enabled = false;
    let store = FakeAccountStore::with_account(account.clone(), events.clone());
    store.fail_next_commit();
    let services = accounts_service(provider, store.clone()).await;
    let result = services
        .accounts()
        .recover(
            &context("enable-failed"),
            ProviderAccountId::new("acct_test").unwrap(),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(*store.accounts.lock().unwrap(), [account]);
    assert_eq!(
        recorded(&events),
        ["store.load_account", "store.batch_update_accounts"]
    );
}

#[tokio::test]
async fn accounts_recover_should_commit_facts_then_return_normal_account() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let mut account = account_record("openai");
    account.credential_state = CredentialState::Invalid;
    account.quota = QuotaState::exhausted(
        QuotaEvidence::UsageLimitReached,
        std::time::SystemTime::now(),
        None,
    );
    account.last_error_reason = Some(gateway_core::account::AccountErrorReason::CredentialInvalid);
    account.last_error_message = Some("invalid credential".to_owned());
    let store = FakeAccountStore::with_account(account, events.clone());
    let services = accounts_service(provider, store.clone()).await;

    let result = services
        .accounts()
        .recover(
            &context("recover-request"),
            ProviderAccountId::new("acct_test").expect("account ID"),
        )
        .await
        .expect("recover account");

    assert_eq!(result.config_revision, revision(2));
    assert!(result.account.account.enabled);
    assert_eq!(
        result.account.account.credential_state,
        CredentialState::Ready
    );
    assert_eq!(
        result.account.account.quota.access(),
        gateway_core::account::QuotaAccessState::Allowed
    );
    assert_eq!(
        result.account.projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal
    );
    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "store.recover_account",
            "provider.account_facts_changed",
            "store.load_account",
        ]
    );
    assert_eq!(store.audit_requests(), ["recover-request"]);
}

#[tokio::test]
async fn accounts_update_should_commit_then_release_disabled_account_and_publish_facts() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider, store.clone()).await;

    let result = services
        .accounts()
        .update(
            &context("update-request"),
            UpdateAccount {
                custom_name: None,
                outbound_proxy: None,
                account_id: "acct_test".to_owned(),
                enabled: false,
                turn_state_injection_enabled: Some(false),
                concurrency_limit: None,
                weight: gateway_core::account::AccountWeight::DEFAULT,
                group_ids: Vec::new(),
            },
        )
        .await
        .expect("update account");

    assert_eq!(result.config_revision, revision(2));
    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "store.update_account",
            "provider.account_unavailable",
            "provider.account_facts_changed",
        ]
    );
    assert_eq!(store.audit_requests(), ["update-request"]);
}

#[tokio::test]
async fn accounts_update_should_not_notify_provider_when_store_commit_fails() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    store.fail_next_commit();
    let services = accounts_service(provider, store).await;

    services
        .accounts()
        .update(
            &context("update-failure"),
            UpdateAccount {
                custom_name: None,
                outbound_proxy: None,
                account_id: "acct_test".to_owned(),
                enabled: false,
                turn_state_injection_enabled: Some(false),
                concurrency_limit: None,
                weight: gateway_core::account::AccountWeight::DEFAULT,
                group_ids: Vec::new(),
            },
        )
        .await
        .expect_err("failed account update");

    assert_eq!(
        recorded(&events),
        ["store.load_account", "store.update_account"]
    );
}

#[tokio::test]
async fn accounts_batch_update_should_commit_once_and_notify_each_provider() {
    let events = events();
    let openai = FakeProviderAdmin::new("openai", events.clone());
    let xai = FakeProviderAdmin::new("xai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    let mut openai_account = account_record("openai");
    openai_account.id = "acct_openai".to_owned();
    let mut xai_account = account_record("xai");
    xai_account.id = "acct_xai".to_owned();
    store.set_accounts(vec![openai_account, xai_account]);
    let services = super::AdminHarness::new()
        .accounts(store.clone())
        .settings(Arc::new(StaticSettingsStore))
        .provider(openai)
        .provider(xai)
        .probe(Arc::new(SuccessfulAccountProbe))
        .build()
        .await;

    let result = services
        .accounts()
        .batch_update(
            &context("batch-update-request"),
            BatchUpdateAccounts {
                custom_name: None,
                outbound_proxy: None,
                account_ids: vec!["acct_openai".to_owned(), "acct_xai".to_owned()],
                enabled: Some(false),
                turn_state_injection_enabled: None,
                concurrency_limit: None,
                weight: Some(gateway_core::account::AccountWeight::DEFAULT),
                group_ids: Some(Vec::new()),
            },
        )
        .await
        .expect("batch update accounts");

    assert_eq!(result.config_revision, revision(2));
    assert_eq!(result.account_ids.len(), 2);
    assert_eq!(store.audit_requests(), ["batch-update-request"]);
    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "store.load_account",
            "store.batch_update_accounts",
            "provider.account_unavailable",
            "provider.account_facts_changed",
            "provider.account_unavailable",
            "provider.account_facts_changed",
        ]
    );
}

#[tokio::test]
async fn accounts_list_should_return_complete_directory_semantics() {
    let provider = FakeProviderAdmin::new("openai", events());
    let mut stored = account_record("openai");
    stored.plan_type = Some("  self_serve_business_prolite  ".to_owned());
    let store = FakeAccountStore::with_account(stored, events());
    let services = accounts_service(provider, store).await;
    let page = services
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("complete account directory");

    assert_eq!(page.summary.total, 1);
    assert_eq!(page.summary.normal, 1);
    let account = page.items.first().expect("account item");
    assert_eq!(account.account.provider_kind.as_str(), "openai");
    assert_eq!(
        account.projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal
    );
    assert_eq!(
        (
            account.account.plan_type.as_deref(),
            account.plan_type_display.as_deref(),
        ),
        (
            Some("  self_serve_business_prolite  "),
            Some("OpenaiDisplaySelfServeBusinessProlite"),
        )
    );

    let detail = services
        .accounts()
        .quota(
            &ProviderAccountId::new("acct_test").expect("account ID"),
            false,
        )
        .await
        .expect("account detail");
    assert_eq!(detail.plan_type_display, account.plan_type_display);
    assert_eq!(detail.account.plan_type, account.account.plan_type);
    assert!(account.cumulative_costs.is_empty());
    assert!(detail.cumulative_costs.is_empty());
    assert!(account.usage.is_none());
    assert!(detail.usage.is_none());
}

#[tokio::test]
async fn accounts_list_and_detail_should_resolve_effective_concurrency_for_mixed_accounts() {
    let provider = FakeProviderAdmin::new("openai", events());
    let stored_accounts = [None, Some(1), Some(8), Some(u32::MAX)]
        .into_iter()
        .enumerate()
        .map(|(index, limit)| {
            let mut account = account_record("openai");
            account.id = format!("acct_capacity_{index}");
            account.enabled = index % 2 == 0;
            account.concurrency_limit = limit.map(|value| {
                gateway_core::account::AccountConcurrencyLimit::new(value)
                    .expect("positive account override")
            });
            account
        })
        .collect::<Vec<_>>();
    let store = FakeAccountStore::with_account(stored_accounts[0].clone(), events());
    store.set_accounts(stored_accounts.clone());
    let services = accounts_service(provider, Arc::clone(&store)).await;

    let page = services
        .accounts()
        .list(account_list_query())
        .await
        .expect("account directory");
    assert_eq!(page.items.len(), stored_accounts.len());
    for (item, stored) in page.items.iter().zip(&stored_accounts) {
        let expected = stored.concurrency_limit.map_or(1, |limit| limit.get());
        assert_eq!(item.effective_concurrency_limit.get(), expected);
        assert_eq!(&item.account, stored);
        let detail = services
            .accounts()
            .quota(
                &ProviderAccountId::new(stored.id.clone()).expect("account ID"),
                false,
            )
            .await
            .expect("account detail");
        assert_eq!(
            detail.effective_concurrency_limit,
            item.effective_concurrency_limit
        );
        assert_eq!(&detail.account, stored);
    }
    assert_eq!(
        *store.accounts.lock().expect("stored accounts"),
        stored_accounts
    );
}

#[tokio::test]
async fn accounts_should_fill_missing_plan_from_quota_without_overriding_known_subtypes() {
    for (stored_plan, quota_plan, expected, expected_display) in [
        (None, Some("free"), Some("free"), Some("OpenaiDisplayFree")),
        (
            Some("  "),
            Some("free"),
            Some("free"),
            Some("OpenaiDisplayFree"),
        ),
        (
            Some("unknown"),
            Some("free"),
            Some("free"),
            Some("OpenaiDisplayFree"),
        ),
        (
            Some("self_serve_business_prolite"),
            Some("team"),
            Some("self_serve_business_prolite"),
            Some("OpenaiDisplaySelfServeBusinessProlite"),
        ),
        (None, None, None, None),
    ] {
        let provider = FakeProviderAdmin::new("openai", events());
        provider.set_quota(ProviderQuota {
            plan_type: quota_plan.map(str::to_owned),
            ..empty_quota()
        });
        let mut stored = account_record("openai");
        stored.plan_type = stored_plan.map(str::to_owned);
        let store = FakeAccountStore::with_account(stored, events());
        let services = accounts_service(provider, store).await;
        let page = services
            .accounts()
            .list(AccountListQuery {
                page: 1,
                page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
                provider_kind: None,
                group_filter: None,
                search: None,
                status: None,
                plan_type: None,
                sort: None,
            })
            .await
            .expect("account list");
        let account = page.items.first().expect("account item");
        assert_eq!(account.account.plan_type.as_deref(), expected);
        assert_eq!(account.plan_type_display.as_deref(), expected_display);
        for refresh in [false, true] {
            let detail = services
                .accounts()
                .quota(
                    &ProviderAccountId::new("acct_test").expect("account ID"),
                    refresh,
                )
                .await
                .expect("account quota detail");
            assert_eq!(detail.account.plan_type.as_deref(), expected);
            assert_eq!(detail.plan_type_display, account.plan_type_display);
        }
    }
}

#[tokio::test]
async fn accounts_list_should_degrade_quota_failure_to_empty_window_without_dropping_page() {
    let events = events();
    let openai = FakeProviderAdmin::new("openai", events.clone());
    openai.set_quota(ProviderQuota {
        plan_type: None,
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "primary".to_owned(),
            group: "shortTerm".to_owned(),
            label: "5小时限额".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(5 * 60 * 60),
            used_percent: Some(97.0),
            reset_at: Some(Utc::now() + TimeDelta::hours(1)),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
        limit_reached: false,
        provider_data: None,
    });
    let failing = FakeProviderAdmin::new("xai", events.clone());
    failing.fail_next_quota(ProviderAdminErrorKind::Invalid);
    let store = FakeAccountStore::new("openai", events.clone());
    let mut xai_account = account_record("xai");
    xai_account.id = "acct_bad_xai".to_owned();
    store.set_accounts(vec![account_record("openai"), xai_account]);
    store.set_cumulative_costs("acct_test", vec![cumulative_cost("USD", "100")]);
    store.set_cumulative_costs("acct_bad_xai", vec![cumulative_cost("USD", "105")]);
    let services = super::AdminHarness::new()
        .accounts(store.clone())
        .settings(Arc::new(StaticSettingsStore))
        .provider(openai)
        .provider(failing)
        .probe(Arc::new(SuccessfulAccountProbe))
        .build()
        .await;

    let page = services
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("one failing quota must not fail the directory page");

    assert_eq!(page.items.len(), 2);
    let by_id = page
        .items
        .iter()
        .map(|item| (item.account.id.as_str(), item))
        .collect::<std::collections::HashMap<_, _>>();
    let healthy = by_id.get("acct_test").expect("healthy account item");
    assert_eq!(healthy.quota.windows.len(), 1);
    assert_eq!(healthy.quota.windows[0].key, "primary");
    assert_eq!(
        healthy.cumulative_costs,
        vec![cumulative_cost("USD", "100")]
    );
    let degraded = by_id.get("acct_bad_xai").expect("degraded account item");
    assert!(degraded.quota.windows.is_empty());
    assert!(degraded.quota.observed_at.is_none());
    assert!(degraded.quota.provider_data.is_none());
    assert!(degraded.usage.is_none());
    assert_eq!(
        degraded.cumulative_costs,
        vec![cumulative_cost("USD", "105")]
    );
    assert_eq!(page.summary.total, 2);
}

#[tokio::test]
async fn account_cumulative_costs_should_remain_independent_of_window_usage() {
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let reset_at = Utc::now() + TimeDelta::hours(1);
    let mut window_usage = quota_local_usage("acct_test", 4_330_000);
    window_usage.costs = vec![account_cost("USD", "5")];
    store.set_quota_window_usage(vec![AccountUsageWindowResult {
        account_id: "acct_test".to_owned(),
        key: "primary".to_owned(),
        usage: window_usage.clone(),
    }]);
    provider.set_quota(ProviderQuota {
        windows: vec![ProviderQuotaWindow {
            key: "primary".to_owned(),
            group: "shortTerm".to_owned(),
            label: "5h".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(5 * 60 * 60),
            used_percent: Some(25.0),
            reset_at: Some(reset_at),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
        ..empty_quota()
    });
    let services = accounts_service(provider.clone(), store.clone()).await;
    let account_id = ProviderAccountId::new("acct_test").expect("account ID");

    for amount in ["100", "105", "10000000000.0000000001"] {
        let costs = vec![
            cumulative_cost("USD", amount),
            cumulative_cost("CNY", "0.1204956"),
        ];
        store.set_cumulative_costs("acct_test", costs.clone());
        let page = services
            .accounts()
            .list(account_list_query())
            .await
            .expect("account directory");
        let item = &page.items[0];
        assert_eq!(item.cumulative_costs, costs);
        assert!(item.usage.is_none());
        assert_eq!(
            item.quota.windows[0].local_usage.as_ref(),
            Some(&window_usage)
        );
        assert_eq!(item.quota.windows[0].used_percent, Some(25.0));
        assert_eq!(item.quota.windows[0].reset_at, Some(reset_at));

        for refresh in [false, true] {
            let detail = services
                .accounts()
                .quota(&account_id, refresh)
                .await
                .expect("account quota detail");
            assert_eq!(detail.cumulative_costs, costs);
            assert!(detail.usage.is_none());
            assert_eq!(detail.quota, item.quota);
        }
    }
    assert_eq!(
        *store
            .cumulative_cost_queries
            .lock()
            .expect("cumulative cost queries"),
        vec![vec!["acct_test".to_owned()]; 9]
    );
    assert!(
        provider
            .quota_requests()
            .iter()
            .all(|request| request.rolling_usage.is_none())
    );
}

#[tokio::test]
async fn account_cumulative_costs_should_survive_missing_quota_and_preserve_real_zero() {
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let mut zero_account = account_record("openai");
    zero_account.id = "acct_zero".to_owned();
    let mut unknown_account = account_record("openai");
    unknown_account.id = "acct_unknown".to_owned();
    store.set_accounts(vec![
        account_record("openai"),
        zero_account,
        unknown_account,
    ]);
    store.set_cumulative_costs("acct_test", vec![cumulative_cost("USD", "105")]);
    store.set_cumulative_costs("acct_zero", vec![cumulative_cost("USD", "0")]);
    store.set_cumulative_costs("acct_other", vec![cumulative_cost("USD", "999")]);
    let services = accounts_service(provider, store.clone()).await;
    let page = services
        .accounts()
        .list(account_list_query())
        .await
        .expect("account list without quotas");
    let expected = [
        ("acct_test", vec![cumulative_cost("USD", "105")]),
        ("acct_zero", vec![cumulative_cost("USD", "0")]),
        ("acct_unknown", vec![]),
    ];
    for (item, (account_id, costs)) in page.items.iter().zip(expected) {
        assert_eq!(item.account.id, account_id);
        assert_eq!(item.cumulative_costs, costs);
        assert!(item.usage.is_none());
        assert!(item.quota.windows.is_empty());
        for refresh in [false, true] {
            let detail = services
                .accounts()
                .quota(
                    &ProviderAccountId::new(account_id).expect("account ID"),
                    refresh,
                )
                .await
                .expect("account detail without quota");
            assert_eq!(detail.cumulative_costs, costs);
            assert!(detail.usage.is_none());
            assert!(detail.quota.windows.is_empty());
        }
    }
    assert_eq!(
        store
            .cumulative_cost_queries
            .lock()
            .expect("cumulative cost queries")[0],
        ["acct_test", "acct_zero", "acct_unknown"]
    );
    assert!(store.quota_window_queries().is_empty());
}

#[tokio::test]
async fn accounts_list_should_prefer_credential_error_over_quota_exhaustion() {
    let provider = FakeProviderAdmin::new("openai", events());
    let mut account = account_record("openai");
    account.quota = QuotaState::exhausted(
        QuotaEvidence::ProviderDenied,
        std::time::SystemTime::now(),
        None,
    );
    account.access_token_expires_at = Some(Utc::now() - TimeDelta::minutes(5));
    let store = FakeAccountStore::with_account(account, events());

    let page = accounts_service(provider, store)
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("usage-limited account directory");

    assert_eq!(
        page.items.first().expect("account item").projection.status,
        gateway_admin::model::accounts::AccountStatus::Error,
    );
}

#[tokio::test]
async fn accounts_list_should_map_unknown_credential_to_error_not_normal() {
    // Unknown 不可调度，Admin 不得显示为 normal。
    let provider = FakeProviderAdmin::new("openai", events());
    let mut account = account_record("openai");
    account.credential_state = CredentialState::Unknown;
    let store = FakeAccountStore::with_account(account, events());

    let page = accounts_service(provider, store)
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("unknown account directory");

    assert_eq!(
        page.items.first().expect("account item").projection.status,
        gateway_admin::model::accounts::AccountStatus::Error,
    );
}

#[tokio::test]
async fn accounts_list_should_not_derive_rate_limited_from_provider_quota_view() {
    let provider = FakeProviderAdmin::new("openai", events());
    provider.set_quota(ProviderQuota {
        plan_type: None,
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "primary".to_owned(),
            group: "shortTerm".to_owned(),
            label: "5小时限额".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(5 * 60 * 60),
            used_percent: Some(100.0),
            reset_at: Some(Utc::now() + TimeDelta::hours(1)),
            limit_reached: true,
            local_usage: None,
            provider_data: None,
        }],
        limit_reached: true,
        provider_data: None,
    });
    let store = FakeAccountStore::new("openai", events());

    let page = accounts_service(provider, store)
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("account directory with display quota cooldown");

    assert_eq!(
        page.items.first().expect("account item").projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal,
    );
}

#[tokio::test]
async fn account_state_readiness_respects_runtime_cooldown_without_hiding_cached_metadata() {
    use gateway_admin::model::accounts::{
        AccountStatus, AccountTurnStateModelStatus, AccountTurnStateSlotStatus,
        AccountTurnStateStatus,
    };
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let id = account_record("openai").id;
    let captured_at = Utc::now();
    let expires_at = captured_at + TimeDelta::hours(1);
    let model = AccountTurnStateModelStatus {
        model: "model-a".into(),
        refresh_status: "ready".into(),
        probe_attempts: 11,
        probe_total_attempts: 11,
        probe_cooldown_until: None,
        probe_retry_from_upstream: None,
        probe_http_status: None,
        probe_error_code: None,
        probe_returned_length: None,
        successful_probe_attempt: Some(2),
        last_probe_reason: None,
        active: Some(AccountTurnStateSlotStatus {
            chars: 292,
            captured_at: Some(captured_at),
            expires_at,
        }),
        standby: None,
    };
    store.turn_states.lock().unwrap().insert(
        id.clone(),
        AccountTurnStateStatus {
            enabled: true,
            required_models: vec!["model-a".into()],
            ready_models: vec![("model-a".into(), expires_at)],
            models: vec![model.clone()],
        },
    );
    let services = accounts_service(provider, store.clone()).await;
    for status in [
        AccountStatus::Normal,
        AccountStatus::RateLimited,
        AccountStatus::Error,
        AccountStatus::QuotaExhausted,
        AccountStatus::Disabled,
        AccountStatus::Normal,
    ] {
        *store.projection_override.lock().unwrap() = Some(status);
        let page = services
            .accounts()
            .list(account_list_query())
            .await
            .unwrap();
        let detail = services
            .accounts()
            .quota(&ProviderAccountId::new(id.clone()).unwrap(), false)
            .await
            .unwrap();
        for state in [
            page.items[0].turn_state.as_ref().unwrap(),
            detail.turn_state.as_ref().unwrap(),
        ] {
            assert!(state.enabled);
            assert_eq!(state.required_models, ["model-a"]);
            assert_eq!(
                state.ready_models.len(),
                usize::from(status == AccountStatus::Normal)
            );
            assert_eq!(state.models.as_slice(), std::slice::from_ref(&model));
        }
    }
}

#[tokio::test]
async fn accounts_list_should_not_derive_exhaustion_from_provider_quota_view() {
    let provider = FakeProviderAdmin::new("openai", events());
    provider.set_quota(ProviderQuota {
        plan_type: None,
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "primary".to_owned(),
            group: "shortTerm".to_owned(),
            label: "5小时限额".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(5 * 60 * 60),
            used_percent: Some(100.0),
            reset_at: Some(Utc::now() + TimeDelta::hours(1)),
            limit_reached: true,
            local_usage: None,
            provider_data: None,
        }],
        limit_reached: true,
        provider_data: None,
    });
    let store = FakeAccountStore::new("openai", events());

    let page = accounts_service(provider, store)
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("account directory with display quota limit");

    assert_eq!(
        page.items.first().expect("account item").projection.status,
        gateway_admin::model::accounts::AccountStatus::Normal,
    );
}

#[tokio::test]
async fn quota_forecast_reads_raw_snapshot_and_limits_usage_to_observation_time() {
    let provider = FakeProviderAdmin::new("openai", events());
    let now = Utc::now();
    let observed = now - TimeDelta::hours(1);
    let reset = now + TimeDelta::days(1);
    provider.set_quota(ProviderQuota {
        observed_at: Some(observed),
        limit_reached: true,
        windows: vec![ProviderQuotaWindow {
            key: "week".to_owned(),
            group: "shortTerm".to_owned(),
            label: "周额度".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(7 * 86_400),
            used_percent: Some(20.0),
            reset_at: Some(reset),
            limit_reached: true,
            // Provider 自带的统计不保证与快照同一时间，预测必须重新采样。
            local_usage: Some(quota_local_usage("acct_test", 999_999)),
            provider_data: None,
        }],
        ..empty_quota()
    });
    let mut account = account_record("openai");
    account.created_at = now - TimeDelta::days(60);
    let store = FakeAccountStore::with_account(account, events());
    store.set_quota_window_usage(vec![AccountUsageWindowResult {
        account_id: "acct_test".to_owned(),
        key: "week".to_owned(),
        usage: quota_local_usage("acct_test", 1_000),
    }]);
    store.quota_forecast_history.lock().unwrap().usage = QuotaForecastUsage {
        request_count: 10,
        tokens: 1_000,
        known_cost_count: 10,
        ..Default::default()
    };
    let report = accounts_service(provider.clone(), store.clone())
        .await
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .expect("quota forecast");
    assert_eq!(report.account_id, "acct_test");
    assert_eq!(report.forecasts[0].estimated_tokens, Some(5_000));
    assert_eq!(
        report.forecasts[0].source.as_ref().unwrap().observed_at,
        Some(observed)
    );
    let queries = store.quota_window_queries();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].range.start, reset - TimeDelta::days(7));
    assert_eq!(queries[0].range.end, observed);
    let requests = provider.quota_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].refresh);
    assert!(requests[0].rolling_usage.is_none());
}

fn learning_quota(now: chrono::DateTime<Utc>) -> ProviderQuota {
    ProviderQuota {
        plan_type: Some("future-pro-2027".to_owned()),
        observed_at: Some(now - TimeDelta::minutes(1)),
        windows: vec![ProviderQuotaWindow {
            key: "week".to_owned(),
            group: "shortTerm".to_owned(),
            label: "Weekly".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(604_800),
            used_percent: Some(25.0),
            reset_at: Some(now + TimeDelta::days(6)),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
        ..empty_quota()
    }
}

#[tokio::test]
async fn dynamic_forecast_never_records_plan_samples_even_for_new_plan_names() {
    for (stored_plan, quota_plan, _expected) in [
        (None, Some("future-pro-2027"), Some("future-pro-2027")),
        (
            Some("unknown"),
            Some("future-pro-2027"),
            Some("future-pro-2027"),
        ),
        (
            Some(" future-business-pro "),
            Some("business"),
            Some("future-business-pro"),
        ),
        (Some("plus"), Some("unknown"), Some("plus")),
        (None, Some("unknown"), None),
    ] {
        let now = Utc::now();
        let provider = FakeProviderAdmin::new("openai", events());
        let mut quota = learning_quota(now);
        quota.plan_type = quota_plan.map(str::to_owned);
        provider.set_quota(quota);
        let mut account = account_record("openai");
        account.created_at = now - TimeDelta::hours(1);
        account.plan_type = stored_plan.map(str::to_owned);
        let store = FakeAccountStore::with_account(account, events());
        accounts_service(provider.clone(), store.clone())
            .await
            .accounts()
            .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
            .await
            .unwrap();
        let observations = store.quota_learning_observations.lock().unwrap();
        assert!(observations.is_empty());
        assert!(
            provider
                .quota_requests
                .lock()
                .unwrap()
                .iter()
                .all(|item| !item.refresh)
        );
        assert!(store.audit_requests().is_empty());
        assert!(store.cumulative_cost_queries.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn dynamic_forecast_does_not_inherit_old_plan_or_short_window_limits() {
    let now = Utc::now();
    let provider = FakeProviderAdmin::new("openai", events());
    let mut quota = learning_quota(now);
    let mut short = quota.windows[0].clone();
    short.key = "primary".to_owned();
    short.window_seconds = Some(5 * 3600);
    short.reset_at = Some(now + TimeDelta::hours(4));
    short.used_percent = Some(50.0);
    quota.windows.push(short);
    provider.set_quota(quota);
    let mut account = account_record("openai");
    // Even a snapshot predating the local import can use an existing Plan estimate.
    account.created_at = now;
    account.plan_type = None;
    let store = FakeAccountStore::with_account(account, events());
    *store.quota_learning_estimates.lock().unwrap() = vec![
        QuotaLearningEstimate {
            account_id: "acct_test".to_owned(),
            window_key: "week".to_owned(),
            window_minutes: 10_080,
            effective_limit_usd: Some(1_200.0),
            source: QuotaLearningSource::PlanAverage,
            sample_count: 3,
        },
        QuotaLearningEstimate {
            account_id: "acct_test".to_owned(),
            window_key: "primary".to_owned(),
            window_minutes: 300,
            effective_limit_usd: Some(100.0),
            source: QuotaLearningSource::PlanAverage,
            sample_count: 3,
        },
    ];
    let services = accounts_service(provider, store.clone()).await;
    let report = services
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    assert!(
        report
            .forecasts
            .iter()
            .all(|item| item.estimated_usd.is_none() && item.remaining_usd.is_none())
    );
    assert!(!store.quota_window_queries().is_empty());
    assert!(store.quota_learning_observations.lock().unwrap().is_empty());

    store.quota_learning_estimates.lock().unwrap().truncate(1);
    let report = services
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    assert!(
        report
            .forecasts
            .iter()
            .all(|item| item.estimated_usd.is_none())
    );
}

#[tokio::test]
async fn dynamic_forecast_does_not_read_learning_storage_or_use_history_as_current_cost() {
    for (request_count, known_cost_count, unavailable_cost_count, pending_request_count) in
        [(2, 1, 1, 0), (1, 1, 0, 1)]
    {
        let now = Utc::now();
        let provider = FakeProviderAdmin::new("openai", events());
        provider.set_quota(learning_quota(now));
        let mut account = account_record("openai");
        account.created_at = now - TimeDelta::days(7);
        account.plan_type = None;
        let store = FakeAccountStore::with_account(account, events());
        *store.quota_forecast_history.lock().unwrap() = QuotaForecastHistory {
            usage: QuotaForecastUsage {
                request_count,
                known_cost_count,
                unavailable_cost_count,
                usd: 5.0,
                ..Default::default()
            },
            pending_request_count,
            ..Default::default()
        };
        *store.quota_learning_estimates.lock().unwrap() = vec![QuotaLearningEstimate {
            account_id: "acct_test".to_owned(),
            window_key: "week".to_owned(),
            window_minutes: 10_080,
            effective_limit_usd: None,
            source: QuotaLearningSource::Learning,
            sample_count: 0,
        }];
        let services = accounts_service(provider, store.clone()).await;
        let report = services
            .accounts()
            .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
            .await
            .unwrap();
        assert!(store.quota_learning_observations.lock().unwrap().is_empty());
        assert!(
            report
                .forecasts
                .iter()
                .all(|item| item.estimated_usd.is_none() && item.remaining_usd.is_none())
        );
        *store.quota_learning_failure.lock().unwrap() = true;
        assert!(
            services
                .accounts()
                .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
                .await
                .is_ok()
        );
    }
}

#[tokio::test]
async fn quota_learning_absent_bindings_never_fall_back_to_old_usd_estimates() {
    for plan in [Some("future-pro-2027"), Some("unknown"), None] {
        let now = Utc::now();
        let provider = FakeProviderAdmin::new("openai", events());
        let mut quota = learning_quota(now);
        quota.plan_type = plan.map(str::to_owned);
        provider.set_quota(quota);
        let mut account = account_record("openai");
        account.created_at = now - TimeDelta::days(8);
        account.plan_type = None;
        let store = FakeAccountStore::with_account(account, events());
        *store.quota_forecast_history.lock().unwrap() = QuotaForecastHistory {
            usage: QuotaForecastUsage {
                request_count: 1,
                known_cost_count: 1,
                tokens: 100,
                usd: 5.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let report = accounts_service(provider, store)
            .await
            .accounts()
            .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
            .await
            .unwrap();
        assert_eq!(report.forecasts[0].estimated_tokens, Some(400));
        assert!(report.forecasts.iter().all(|forecast| {
            forecast.estimated_usd.is_none() && forecast.remaining_usd.is_none()
        }));
    }
}

#[tokio::test]
async fn dynamic_forecast_ignores_legacy_estimates_without_current_window_usage() {
    let now = Utc::now();
    for mode in ["expired", "future", "missing", "before-window", "too-long"] {
        let provider = FakeProviderAdmin::new("openai", events());
        let mut quota = learning_quota(now);
        match mode {
            "expired" => quota.windows[0].reset_at = Some(now - TimeDelta::seconds(1)),
            "future" => quota.observed_at = Some(now + TimeDelta::days(1)),
            "missing" => quota.observed_at = None,
            "before-window" => quota.observed_at = Some(now - TimeDelta::days(8)),
            "too-long" => quota.windows[0].window_seconds = Some(33 * 86_400),
            _ => unreachable!(),
        }
        provider.set_quota(quota.clone());
        let store = FakeAccountStore::new("openai", events());
        *store.quota_learning_estimates.lock().unwrap() = vec![QuotaLearningEstimate {
            account_id: "acct_test".to_owned(),
            window_key: "week".to_owned(),
            window_minutes: (quota.windows[0].window_seconds.unwrap() / 60) as i32,
            effective_limit_usd: Some(100.0),
            source: QuotaLearningSource::PlanAverage,
            sample_count: 3,
        }];
        let report = accounts_service(provider, store.clone())
            .await
            .accounts()
            .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
            .await
            .unwrap();
        assert!(store.quota_learning_observations.lock().unwrap().is_empty());
        assert!(
            store
                .quota_window_queries()
                .iter()
                .all(|query| query.account_id == "acct_test")
        );
        assert!(report.forecasts.iter().all(|forecast| {
            forecast.estimated_usd.is_none() && forecast.remaining_usd.is_none()
        }));
        assert!(
            report
                .learned_windows
                .iter()
                .all(|window| window.remaining_usd.is_none())
        );
    }
}

#[tokio::test]
async fn quota_forecast_does_not_query_usage_without_a_current_snapshot() {
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let report = accounts_service(provider.clone(), store.clone())
        .await
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .expect("empty forecast");
    assert!(
        report
            .forecasts
            .iter()
            .all(|item| item.estimated_tokens.is_none())
    );
    assert!(store.quota_window_queries().is_empty());
    assert!(!provider.quota_requests.lock().unwrap()[0].refresh);
}

#[tokio::test]
async fn quota_forecast_mid_cycle_sampling_accepts_small_reset_jitter_but_not_a_changed_bucket() {
    use gateway_admin::model::quota_forecast_sampling::QuotaForecastHistoryPoint;
    let now = Utc::now();
    let observed = now - TimeDelta::minutes(1);
    let reset = now + TimeDelta::days(1);
    let added = now - TimeDelta::hours(5);
    let provider = FakeProviderAdmin::new("openai", events());
    provider.set_quota(ProviderQuota {
        plan_type: Some("pro".to_owned()),
        observed_at: Some(observed),
        windows: vec![ProviderQuotaWindow {
            key: "week".to_owned(),
            group: "shortTerm".to_owned(),
            label: "周额度".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(604_800),
            used_percent: Some(40.0),
            reset_at: Some(reset),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
        ..empty_quota()
    });
    let mut account = account_record("openai");
    account.created_at = added;
    let store = FakeAccountStore::with_account(account, events());
    let make_point = |hours, percent, tokens, delta| {
        QuotaForecastHistoryPoint {
        started_at: now - TimeDelta::hours(hours) - TimeDelta::seconds(10),
        completed_at: now - TimeDelta::hours(hours),
        usage: QuotaForecastUsage { request_count: tokens / 100, tokens, ..Default::default() },
        provider_observation: ProviderDocument::new(OpaqueProviderData::new(serde_json::json!({
            "percent": percent, "reset": (reset + TimeDelta::seconds(delta)).to_rfc3339(), "plan": "pro",
        }).as_object().unwrap().clone())),
    }
    };
    *store.quota_forecast_history.lock().unwrap() = QuotaForecastHistory {
        points: vec![
            make_point(3, 20.0, 1_000, 1),
            make_point(2, 30.0, 1_500, -1),
        ],
        usage: QuotaForecastUsage {
            request_count: 20,
            tokens: 2_000,
            ..Default::default()
        },
        pending_request_count: 1,
    };
    let services = accounts_service(provider, store.clone()).await;
    let result = services
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    assert!(result.forecasts[0].unavailable_reason.is_none());
    assert_eq!(result.forecasts[0].estimated_tokens, Some(5_000));
    assert_eq!(result.forecasts[0].remaining_tokens, Some(3_000));
    assert_eq!(
        store.quota_window_queries()[0].range.start,
        reset - TimeDelta::days(7)
    );
    store
        .quota_forecast_history
        .lock()
        .unwrap()
        .points
        .push(make_point(1, 35.0, 1_700, 3_600));
    let result = services
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    assert!(result.forecasts[0].estimated_tokens.is_none());
    assert!(
        result.forecasts[0]
            .unavailable_reason
            .unwrap()
            .contains("不连续")
    );
    // 新额度段已有足够观测后恢复预测，但总量不能带回重置前的累计用量。
    let mut first = make_point(1, 5.0, 1_750, 0);
    first.completed_at = now - TimeDelta::minutes(45);
    first.started_at = first.completed_at - TimeDelta::seconds(10);
    let mut next = make_point(1, 20.0, 1_900, 0);
    next.completed_at = now - TimeDelta::minutes(20);
    next.started_at = next.completed_at - TimeDelta::seconds(10);
    store
        .quota_forecast_history
        .lock()
        .unwrap()
        .points
        .extend([first, next]);
    let result = services
        .accounts()
        .quota_forecast(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    let cycle = &result.forecasts[0];
    assert!(cycle.unavailable_reason.is_none());
    assert_eq!(cycle.source.as_ref().unwrap().tokens, Some(250));
    assert_eq!(cycle.remaining_tokens, Some(429));
    assert_eq!(cycle.estimated_tokens, Some(679));
}

#[tokio::test]
async fn accounts_list_should_attach_local_usage_to_quota_windows() {
    let provider = FakeProviderAdmin::new("openai", events());
    let reset_at = Utc::now() + TimeDelta::hours(1);
    provider.set_quota(ProviderQuota {
        plan_type: None,
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![ProviderQuotaWindow {
            key: "primary".to_owned(),
            group: "shortTerm".to_owned(),
            label: "5小时限额".to_owned(),
            limit_id: None,
            limit_name: None,
            role: None,
            local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
            window_seconds: Some(5 * 60 * 60),
            used_percent: Some(97.0),
            reset_at: Some(reset_at),
            limit_reached: false,
            local_usage: None,
            provider_data: None,
        }],
        limit_reached: false,
        provider_data: None,
    });
    let store = FakeAccountStore::new("openai", events());
    store.set_quota_window_usage(vec![AccountUsageWindowResult {
        account_id: "acct_test".to_owned(),
        key: "primary".to_owned(),
        usage: quota_local_usage("acct_test", 4_330_000),
    }]);

    let page = accounts_service(provider, store.clone())
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("quota window usage");

    let item = &page.items[0];
    let usage = item.quota.windows[0]
        .local_usage
        .as_ref()
        .expect("quota window local usage");
    assert_eq!(usage.total_tokens, Some(4_330_000));
    // 短期额度条保留自己的本地用量，但不作为周/月统计面板的回退。
    assert!(item.usage.is_none());

    let queries = store.quota_window_queries();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].account_id, "acct_test");
    assert_eq!(queries[0].key, "primary");
    assert_eq!(queries[0].range.start, reset_at - TimeDelta::hours(5));
    assert_eq!(queries[0].range.end, reset_at);
}

#[tokio::test]
async fn accounts_list_and_quota_refresh_should_select_the_same_weekly_or_monthly_usage() {
    let provider = FakeProviderAdmin::new("openai", events());
    let store = FakeAccountStore::new("openai", events());
    let services = accounts_service(provider.clone(), store.clone()).await;
    let account_id = ProviderAccountId::new("acct_test").expect("account id");
    let reset_at = Utc::now() + TimeDelta::days(1);
    let short = ProviderQuotaWindow {
        key: "short".to_owned(),
        group: "shortTerm".to_owned(),
        label: "5小时限额".to_owned(),
        limit_id: None,
        limit_name: None,
        role: None,
        local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
        window_seconds: Some(18_000),
        used_percent: Some(50.0),
        reset_at: Some(reset_at),
        limit_reached: false,
        local_usage: None,
        provider_data: None,
    };
    let week = ProviderQuotaWindow {
        key: "week".to_owned(),
        label: "周额度".to_owned(),
        window_seconds: Some(7 * 86_400),
        ..short.clone()
    };
    let month = ProviderQuotaWindow {
        key: "month".to_owned(),
        group: "monthly".to_owned(),
        label: "月额度".to_owned(),
        window_seconds: Some(30 * 86_400),
        ..short.clone()
    };
    for (windows, selected_key, tokens, duration) in [
        (vec![short.clone(), month.clone(), week], "week", 700, 7),
        (vec![short, month], "month", 3000, 30),
    ] {
        provider.set_quota(ProviderQuota {
            windows,
            ..empty_quota()
        });
        store.set_quota_window_usage(
            [("short", 50), ("week", 700), ("month", 3000)]
                .into_iter()
                .map(|(key, total)| AccountUsageWindowResult {
                    account_id: account_id.to_string(),
                    key: key.to_owned(),
                    usage: quota_local_usage(account_id.as_str(), total),
                })
                .collect(),
        );
        let page = services
            .accounts()
            .list(AccountListQuery {
                page: 1,
                page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
                provider_kind: None,
                group_filter: None,
                search: None,
                status: None,
                plan_type: None,
                sort: None,
            })
            .await
            .expect("list accounts");
        let refreshed = services
            .accounts()
            .quota(&account_id, true)
            .await
            .expect("refresh quota");
        let expected = quota_local_usage(account_id.as_str(), tokens);
        for item in [&page.items[0], &refreshed] {
            let usage = item.usage.as_ref().expect("selected window usage");
            assert_eq!(usage.total_tokens, expected.total_tokens);
            assert_eq!(
                item.quota
                    .usage_window()
                    .map(|(window, _)| window.key.as_str()),
                Some(selected_key)
            );
        }
        let queries = store.quota_window_queries();
        let selected = queries
            .iter()
            .find(|query| query.key == selected_key)
            .expect("selected query");
        assert_eq!(selected.range.start, reset_at - TimeDelta::days(duration));
        assert_eq!(selected.range.end, reset_at);
    }
}

#[tokio::test]
async fn accounts_list_should_not_attach_account_usage_to_model_specific_quota_windows() {
    let provider = FakeProviderAdmin::new("openai", events());
    let reset_at = Utc::now() + TimeDelta::days(7);
    provider.set_quota(ProviderQuota {
        plan_type: None,
        observed_at: Some(Utc::now()),
        refresh_token_expires_at: None,
        windows: vec![
            ProviderQuotaWindow {
                key: "core-primary".to_owned(),
                group: "weekly".to_owned(),
                label: "周额度".to_owned(),
                limit_id: Some("codex".to_owned()),
                limit_name: None,
                role: None,
                local_usage_attribution: QuotaLocalUsageAttribution::AccountWide,
                window_seconds: Some(7 * 24 * 60 * 60),
                used_percent: Some(1.0),
                reset_at: Some(reset_at),
                limit_reached: false,
                local_usage: None,
                provider_data: None,
            },
            ProviderQuotaWindow {
                key: "codex-bengalfox-primary".to_owned(),
                group: "weekly".to_owned(),
                label: "周额度".to_owned(),
                limit_id: Some("codex_bengalfox".to_owned()),
                limit_name: Some("GPT-5.3-Codex-Spark".to_owned()),
                role: None,
                local_usage_attribution: QuotaLocalUsageAttribution::Unavailable,
                window_seconds: Some(7 * 24 * 60 * 60),
                used_percent: Some(0.0),
                reset_at: Some(reset_at),
                limit_reached: false,
                local_usage: None,
                provider_data: None,
            },
        ],
        limit_reached: false,
        provider_data: None,
    });
    let store = FakeAccountStore::new("openai", events());
    store.set_quota_window_usage(vec![
        AccountUsageWindowResult {
            account_id: "acct_test".to_owned(),
            key: "core-primary".to_owned(),
            usage: quota_local_usage("acct_test", 12_818_806),
        },
        AccountUsageWindowResult {
            account_id: "acct_test".to_owned(),
            key: "codex-bengalfox-primary".to_owned(),
            usage: quota_local_usage("acct_test", 127_926),
        },
    ]);

    let page = accounts_service(provider, store)
        .await
        .accounts()
        .list(AccountListQuery {
            page: 1,
            page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
            provider_kind: None,
            group_filter: None,
            search: None,
            status: None,
            plan_type: None,
            sort: None,
        })
        .await
        .expect("quota window usage");

    let local_tokens = page.items[0]
        .quota
        .windows
        .iter()
        .map(|window| {
            (
                window.key.as_str(),
                window
                    .local_usage
                    .as_ref()
                    .and_then(|usage| usage.total_tokens),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        local_tokens,
        [
            ("core-primary", Some(12_818_806)),
            ("codex-bengalfox-primary", None),
        ],
    );
    assert_eq!(
        page.items[0]
            .usage
            .as_ref()
            .and_then(|usage| usage.total_tokens),
        Some(12_818_806),
    );
}

#[tokio::test]
async fn accounts_refresh_provider_failure_should_not_call_store_commit() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    provider.fail_next(ProviderAdminErrorKind::Unavailable);
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider, store).await;

    services
        .accounts()
        .refresh(
            &context("refresh-provider-error"),
            ProviderAccountId::new("acct_test").expect("account ID"),
        )
        .await
        .expect_err("Provider preparation must fail");

    assert_eq!(
        recorded(&events),
        ["store.load_account", "provider.prepare_refresh"]
    );
}

#[tokio::test]
async fn reset_credit_oauth_refresh_should_reuse_the_exact_consume_command() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    provider.fail_next(ProviderAdminErrorKind::CredentialRefreshRequired);
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider.clone(), store).await;
    let redeem_request_id =
        uuid::Uuid::parse_str("8fbf302d-11df-4bd5-82e4-08e4b3df7874").expect("UUID v4");
    let command = ConsumeProviderResetCredit {
        account_id: ProviderAccountId::new("acct_test").expect("account ID"),
        credit_id: Some("credit_1".to_owned()),
        redeem_request_id,
    };

    let result = services
        .accounts()
        .consume_reset_credit(&context("reset-credit-refresh"), command.clone())
        .await
        .expect("consume after OAuth refresh");

    assert_eq!(result.code, "reset");
    assert_eq!(provider.reset_credit_commands(), [command.clone(), command]);
    assert!(recorded(&events).contains(&"provider.prepare_refresh"));
    assert!(recorded(&events).contains(&"store.commit_refresh"));
}

#[tokio::test]
async fn reset_credit_unknown_result_should_keep_a_stable_admin_kind() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    provider.fail_next(ProviderAdminErrorKind::Ambiguous);
    let store = FakeAccountStore::new("openai", events);
    let services = accounts_service(provider, store).await;
    let command = ConsumeProviderResetCredit {
        account_id: ProviderAccountId::new("acct_test").expect("account ID"),
        credit_id: Some("credit_1".to_owned()),
        redeem_request_id: uuid::Uuid::parse_str("244e790c-42a3-4ec9-a45d-a32b218bc8ac")
            .expect("UUID v4"),
    };

    let error = services
        .accounts()
        .consume_reset_credit(&context("reset-credit-ambiguous"), command)
        .await
        .expect_err("ambiguous consume result must remain classified");

    assert_eq!(
        error.kind(),
        gateway_admin::model::AdminErrorKind::UpstreamResultUnknown
    );
    assert_eq!(
        error.to_string(),
        "上游执行结果未知，请刷新状态后再决定是否重试"
    );
}

#[tokio::test]
async fn accounts_refresh_should_not_expose_the_provider_failure_message() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let upstream_message = "Your refresh token has already been used.";
    provider.fail_next_with_message(ProviderAdminErrorKind::Conflict, upstream_message);
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider, store).await;

    let error = services
        .accounts()
        .refresh(
            &context("refresh-provider-message"),
            ProviderAccountId::new("acct_test").expect("account ID"),
        )
        .await
        .expect_err("Provider preparation must fail");

    assert_eq!(error.kind(), gateway_admin::model::AdminErrorKind::Conflict);
    assert_eq!(error.to_string(), "Provider 资源状态冲突，请刷新后重试");
    assert!(!error.to_string().contains(upstream_message));
    assert_eq!(
        recorded(&events),
        ["store.load_account", "provider.prepare_refresh"]
    );
}

#[tokio::test]
async fn accounts_refresh_store_failure_should_drop_guard_after_commit_attempt() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    store.fail_next_commit();
    let services = accounts_service(provider, store).await;

    services
        .accounts()
        .refresh(
            &context("refresh-store-error"),
            ProviderAccountId::new("acct_test").expect("account ID"),
        )
        .await
        .expect_err("Store commit must fail");

    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "provider.prepare_refresh",
            "store.commit_refresh",
            "guard.drop",
        ]
    );
}

pub(super) fn events() -> EventLog {
    Arc::new(Mutex::new(Vec::new()))
}

pub(super) fn recorded(events: &EventLog) -> Vec<&'static str> {
    events.lock().expect("recorded events").clone()
}

pub(super) fn context(request_id: &str) -> MutationContext {
    MutationContext {
        actor: gateway_admin::model::MutationActor::AdminSession {
            admin_user_id: "admin-test".to_owned(),
        },
        request_id: request_id.to_owned(),
    }
}

pub(super) fn document() -> ProviderDocument {
    ProviderDocument::new(OpaqueProviderData::new(Default::default()))
}

fn empty_quota() -> ProviderQuota {
    ProviderQuota {
        plan_type: None,
        observed_at: None,
        refresh_token_expires_at: None,
        windows: Vec::new(),
        limit_reached: false,
        provider_data: None,
    }
}

pub(super) fn quota_local_usage(account_id: &str, total_tokens: u64) -> AccountUsage {
    AccountUsage {
        account_id: account_id.to_owned(),
        request_count: 1,
        success_count: 1,
        input_tokens: Some(total_tokens),
        output_tokens: Some(0),
        cached_tokens: Some(0),
        cache_write_tokens: Some(0),
        reasoning_tokens: Some(0),
        image_input_tokens: Some(0),
        image_output_tokens: Some(0),
        image_request_count: 0,
        image_request_failed_count: 0,
        total_tokens: Some(total_tokens),
        cost_coverage: Default::default(),
        costs: Vec::new(),
        last_used_at: Some(Utc::now()),
        request_buckets: Vec::new(),
        models: Vec::new(),
    }
}

fn account_cost(currency: &str, amount: &str) -> AccountCost {
    AccountCost {
        currency: currency.to_owned(),
        amount: amount.parse().expect("decimal cost"),
    }
}

fn cumulative_cost(currency: &str, amount: &str) -> AccountCumulativeCost {
    AccountCumulativeCost {
        currency: currency.to_owned(),
        amount: amount.parse().expect("cumulative cost amount"),
    }
}

fn account_list_query() -> AccountListQuery {
    AccountListQuery {
        page: 1,
        page_size: gateway_admin::model::PageSize::new(20).expect("page size"),
        provider_kind: None,
        group_filter: None,
        search: None,
        status: None,
        plan_type: None,
        sort: None,
    }
}

pub(super) fn account_record(kind: &str) -> AccountRecord {
    let now = Utc::now();
    AccountRecord {
        custom_name: None,
        outbound_proxy: None,
        id: "acct_test".to_owned(),
        provider_kind: ProviderKind::new(kind).expect("provider kind"),
        groups: Vec::new(),
        name: "test account".to_owned(),
        email: Some("test@example.invalid".to_owned()),
        upstream_user_id: Some("upstream-user".to_owned()),
        upstream_account_id: None,
        plan_type: Some("test".to_owned()),
        authentication_kind: "oauth".to_owned(),
        credential_revision: revision(1),
        turn_state_binding_revision: revision(1),
        relogin_count: 0,
        last_relogin_at: None,
        has_refresh_token: true,
        access_token_expires_at: Some(now + TimeDelta::hours(1)),
        next_refresh_at: Some(now + TimeDelta::minutes(30)),
        enabled: true,
        turn_state_injection_enabled: false,
        concurrency_limit: None,
        weight: gateway_core::account::AccountWeight::DEFAULT,
        credential_state: CredentialState::Ready,
        credential_observed_at: now,
        quota: QuotaState::allowed(now.into()),
        last_error_reason: None,
        last_error_message: None,
        created_at: now,
        updated_at: now,
    }
}

pub(super) fn revision(value: u64) -> Revision {
    Revision::new(value).expect("positive revision")
}

fn prepared_create(provider_kind: ProviderKind, name: &str) -> PreparedCredentialCreate {
    prepared_create_with_id(provider_kind, "acct_prepared", name)
}

fn prepared_create_with_id(
    provider_kind: ProviderKind,
    account_id: &str,
    name: &str,
) -> PreparedCredentialCreate {
    let now = Utc::now();
    PreparedCredentialCreate {
        outbound_proxy: None,
        account_id: ProviderAccountId::new(account_id).expect("prepared account ID"),
        provider_kind,
        name: name.to_owned(),
        email: Some("prepared@example.invalid".to_owned()),
        upstream_user_id: Some("prepared-user".to_owned()),
        upstream_account_id: None,
        plan_type: Some("test".to_owned()),
        authentication_kind: "oauth".to_owned(),
        provider_material: document(),
        has_refresh_token: true,
        access_token_expires_at: Some(now + TimeDelta::hours(1)),
        next_refresh_at: Some(now + TimeDelta::minutes(30)),
        enabled: true,
        credential_state: CredentialState::Ready,
        credential_observed_at: now,
    }
}

fn rotation_result(command: CredentialRotationCommit) -> CredentialMutationResult {
    CredentialMutationResult {
        config_revision: revision(2),
        account_id: command.prepared.account_id,
        credential_revision: Some(revision(
            command.prepared.expected_credential_revision.get() + 1,
        )),
    }
}

fn empty_profile_statistics() -> ProviderProfileStatistics {
    ProviderProfileStatistics {
        display_name: Some("Preview user".to_owned()),
        username: None,
        image_url: None,
        has_stats_error: false,
        summary: ProviderProfileStatisticsSummary {
            total_text_tokens: Some(123),
            peak_tokens: None,
            longest_task_duration_ms: None,
            current_streak_days: None,
            longest_streak_days: None,
        },
        daily_usage: None,
        activity_insights: ProviderProfileActivityInsights {
            fast_mode_percent: None,
            reasoning_effort: None,
            reasoning_effort_percent: None,
            skills_explored: None,
            total_skills_used: None,
            total_threads: None,
            invocations: None,
        },
    }
}

fn personal_info_subscription() -> ProviderSubscription {
    ProviderSubscription {
        starts_at: None,
        expires_at: Utc::now(),
        will_renew: Some(true),
        billing_period: Some("monthly".to_owned()),
        billing_currency: Some("USD".to_owned()),
        observed_at: Utc::now(),
    }
}

#[tokio::test]
async fn personal_info_should_query_both_parts_concurrently_once_on_every_request() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let subscription = personal_info_subscription();
    *provider.subscription_result.lock().unwrap() = Ok(Some(subscription.clone()));
    *provider.personal_info_barrier.lock().unwrap() = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let store = FakeAccountStore::new("openai", events.clone());
    let services = accounts_service(provider, store).await;
    let account_id = ProviderAccountId::new("acct_test").unwrap();

    for _ in 0..2 {
        let info = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            services.accounts().personal_info(&account_id),
        )
        .await
        .expect("两项查询应并发执行")
        .unwrap();
        assert_eq!(info.profile.unwrap(), empty_profile_statistics());
        assert_eq!(info.subscription, Some(subscription.clone()));
    }
    assert_eq!(
        recorded(&events),
        [
            "store.load_account",
            "provider.profile_statistics",
            "provider.subscription",
            "store.load_account",
            "store.load_account",
            "provider.profile_statistics",
            "provider.subscription",
            "store.load_account",
        ]
    );
}

#[tokio::test]
async fn personal_info_should_keep_profile_when_subscription_is_missing_or_fails() {
    for subscription in [Ok(None), Err(ProviderAdminErrorKind::Unavailable)] {
        let provider = FakeProviderAdmin::new("openai", events());
        *provider.subscription_result.lock().unwrap() = subscription;
        let services = accounts_service(provider, FakeAccountStore::new("openai", events())).await;
        let info = services
            .accounts()
            .personal_info(&ProviderAccountId::new("acct_test").unwrap())
            .await
            .unwrap();
        assert_eq!(info.profile.unwrap(), empty_profile_statistics());
        assert_eq!(info.subscription, None);
    }
}

#[tokio::test]
async fn personal_info_should_keep_subscription_when_profile_fails() {
    let provider = FakeProviderAdmin::new("openai", events());
    *provider.profile_result.lock().unwrap() = Err(ProviderAdminErrorKind::BadGateway);
    let subscription = personal_info_subscription();
    *provider.subscription_result.lock().unwrap() = Ok(Some(subscription.clone()));
    let services = accounts_service(provider, FakeAccountStore::new("openai", events())).await;
    let info = services
        .accounts()
        .personal_info(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap();
    assert_eq!(
        info.profile.unwrap_err().kind(),
        gateway_admin::model::AdminErrorKind::BadGateway
    );
    assert_eq!(info.subscription, Some(subscription));
}

#[tokio::test]
async fn personal_info_should_reject_missing_account_before_querying_provider() {
    let events = events();
    let provider = FakeProviderAdmin::new("openai", events.clone());
    let store = FakeAccountStore::new("openai", events.clone());
    store.set_accounts(Vec::new());
    let services = accounts_service(provider, store).await;
    let error = services
        .accounts()
        .personal_info(&ProviderAccountId::new("acct_test").unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.kind(), gateway_admin::model::AdminErrorKind::NotFound);
    assert_eq!(recorded(&events), ["store.load_account"]);
}

#[tokio::test]
async fn personal_info_should_discard_results_when_account_changes_during_query() {
    for change in 0..5 {
        let provider = FakeProviderAdmin::new("openai", events());
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        *provider.personal_info_barrier.lock().unwrap() = Some(barrier.clone());
        let store = FakeAccountStore::new("openai", events());
        let services = accounts_service(provider, store.clone()).await;
        let account_id = ProviderAccountId::new("acct_test").unwrap();
        let mutate = async {
            barrier.wait().await;
            let mut accounts = store.accounts.lock().unwrap().clone();
            match change {
                0 => accounts[0].credential_revision = revision(99),
                1 => accounts[0].upstream_account_id = Some("changed-account".to_owned()),
                2 => accounts[0].upstream_user_id = Some("changed-user".to_owned()),
                3 => accounts[0].provider_kind = ProviderKind::new("xai").unwrap(),
                _ => accounts.clear(),
            }
            store.set_accounts(accounts);
            barrier.wait().await;
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(services.accounts().personal_info(&account_id), mutate)
        })
        .await
        .unwrap();
        let expected = if change == 4 {
            gateway_admin::model::AdminErrorKind::NotFound
        } else {
            gateway_admin::model::AdminErrorKind::Conflict
        };
        assert_eq!(result.unwrap_err().kind(), expected);
    }
}

async fn accounts_service(
    provider: Arc<FakeProviderAdmin>,
    store: Arc<FakeAccountStore>,
) -> AdminServices {
    accounts_service_with_probe(provider, store, Arc::new(SuccessfulAccountProbe)).await
}

async fn accounts_service_with_probe(
    provider: Arc<FakeProviderAdmin>,
    store: Arc<FakeAccountStore>,
    probe: Arc<dyn AccountProbe>,
) -> AdminServices {
    super::AdminHarness::new()
        .accounts(store)
        .settings(Arc::new(StaticSettingsStore))
        .provider(provider)
        .probe(probe)
        .build()
        .await
}

struct SuccessfulAccountProbe;

impl AccountProbe for SuccessfulAccountProbe {
    fn probe(
        &self,
        _: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async {
            Ok(AccountProbeResult {
                text: vec!["OK".to_owned()],
            })
        })
    }
}

struct FailingAccountProbe;

impl AccountProbe for FailingAccountProbe {
    fn probe(
        &self,
        _: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async {
            let detail = ClientVisibleUpstreamError::new(
                "included usage exhausted",
                Some("usage_exhausted".to_owned()),
                Some("invalid_request_error".to_owned()),
            );
            Err(AccountProbeError::new(
                GatewayError::new(
                    GatewayErrorKind::RateLimited,
                    "upstream capacity is temporarily unavailable",
                )
                .with_client_visible_upstream_error(detail),
                AccountProbeErrorSource::Upstream,
                Some(UpstreamSendState::NotSent),
                None,
            ))
        })
    }
}

struct FreeModelQuotaProbe {
    store: Arc<FakeAccountStore>,
}

impl AccountProbe for FreeModelQuotaProbe {
    fn probe(
        &self,
        _: AccountProbeRequest,
    ) -> BoxFuture<'_, Result<AccountProbeResult, AccountProbeError>> {
        Box::pin(async move {
            self.store.set_account_after_probe(account_record("xai"));
            Err(AccountProbeError::new(
                GatewayError::new(
                    GatewayErrorKind::RateLimited,
                    "xAI free model quota is exhausted",
                ),
                AccountProbeErrorSource::Provider,
                Some(UpstreamSendState::NotSent),
                None,
            ))
        })
    }
}

fn store_unavailable() -> AdminStoreError {
    AdminStoreError::new(
        AdminStoreErrorKind::Unavailable,
        "test account",
        "unavailable",
    )
}

fn unsupported() -> ProviderAdminError {
    ProviderAdminError::new(ProviderAdminErrorKind::Unsupported)
}

pub(super) fn import_settings() -> gateway_admin::model::accounts::AccountImportSettings {
    gateway_admin::model::accounts::AccountImportSettings {
        custom_name: None,
        enabled: false,
        turn_state_injection_enabled: None,
        concurrency_limit: Some(
            gateway_core::account::AccountConcurrencyLimit::new(3).expect("concurrency"),
        ),
        weight: gateway_core::account::AccountWeight::new(7).expect("weight"),
        group_ids: vec![
            gateway_core::routing::AccountGroupId::new("grp_00000000000000000000000000000091")
                .expect("group ID"),
        ],
    }
}
