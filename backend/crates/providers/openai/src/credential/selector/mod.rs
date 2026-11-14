//! AttemptContext 驱动的 Codex 账号选择与 Redis lease port。

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gateway_core::account::{
    AccountCandidate, AccountCapacitySnapshot, AccountEligibilityPolicy, AccountErrorReason,
    AccountFeedbackStats, AccountModelAccessMode, AccountRuntimeSignals, AccountSchedulingBlocker,
    AccountSelectionContext, AccountSelector, AccountStatus, CredentialState,
    PreferredAccountSelection, ProviderAccount, ProviderAccountId, QuotaEvidence,
    ResponsesUpstream,
};
use gateway_core::engine::{AttemptContext, ContinuationAttempt};
use gateway_core::provider_ports::{
    ProviderConcurrencyPool, ProviderCooldownPort, ProviderLeaseAcquisition, ProviderLeaseGuard,
    ProviderLeasePort, ProviderLeaseRequest, ProviderSchedulingLeaseRequest,
    ProviderSessionAffinityBinding, ProviderSessionAffinityKey, ProviderSessionAffinityPort,
    ProviderSessionAlias, ProviderSessionExclusionPort, ProviderSessionExclusions,
    ProviderStoreError,
};
use gateway_core::routing::ProviderKind;
use gateway_core::runtime::AccountConcurrencyHandle;
use secrecy::ExposeSecret;
use thiserror::Error;
use url::Url;

use super::affinity::{CODEX_ROOT_SESSION_TTL, CodexSessionAffinity};
use super::catalog::CodexCredentialCatalogService;
use super::cookie::CodexCookiePolicy;
use super::quota::CodexCredentialQuotaService;
use super::repository::{
    CodexCredentialRepository, CredentialRepositoryError, only_quota_observation_advanced,
};
use super::security::CodexRuntimeAuthentication;
use super::types::{
    CODEX_AUTHENTICATION_KIND_OAUTH, CodexCookie, CodexCookieCaptureOutcome, RuntimeCodexCookie,
};

mod admission;
mod capacity_wait;
mod excel_auth;
mod model_eligibility;
mod projection;
mod route_state;

pub use capacity_wait::AccountWaitFailure;

const CLOUDFLARE_RECOVERY_STALE_AFTER: Duration = Duration::from_secs(60 * 60);
const CLOUDFLARE_CHALLENGE_BACKOFF: [Duration; 4] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(90),
    Duration::from_secs(120),
];
const CLOUDFLARE_PATH_BLOCK_THRESHOLD: u32 = 3;
// Binding must succeed before sending; tolerate brief storage jitter separately from policy hints.
const SESSION_AFFINITY_TIMEOUT: Duration = Duration::from_millis(500);
const CYBER_POLICY_TIMEOUT: Duration = Duration::from_millis(100);
const CYBER_POLICY_SESSION_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexAccountFailure {
    /// Access token 已被上游明确判定为过期或失效。
    CredentialExpired,
    /// Explicit revocation cannot recover through automatic refresh.
    CredentialRevoked,
    /// 账号需要完成身份验证后才能继续使用。
    IdentityVerificationRequired,
    /// 账号、workspace 或 organization 已被封禁或停用。
    Banned,
    /// 账号信用额度已耗尽。
    QuotaExhausted,
    /// 当前用量窗口已耗尽；到重置时间后可自动恢复。
    UsageLimitExhausted {
        /// 上游返回的窗口绝对重置时刻。
        reset_at: Option<SystemTime>,
    },
    /// 账号触发临时用量限制。
    RateLimited {
        /// 上游明确返回的最短冷却时长。
        retry_after: Option<Duration>,
    },
    /// Cloudflare challenge 要求账号进入递增冷却。
    CloudflareChallenge {
        /// 上游明确返回的最短冷却时长。
        retry_after: Option<Duration>,
    },
    /// Cloudflare 对当前上游路径返回空 404。
    CloudflarePathBlocked,
}

#[derive(Debug, Clone, Copy)]
struct RiskRecoveryState {
    challenge_count: u32,
    path_block_count: u32,
    observed_at: SystemTime,
}

#[derive(Debug, Clone, Copy)]
enum CookieRecovery {
    ExpireAt(SystemTime),
    Clear,
}

pub struct SelectCodexCredential<'a> {
    pub upstream_model: &'a str,
    pub request_url: &'a Url,
    pub attempt: &'a AttemptContext,
    pub session_affinity_key: Option<&'a ProviderSessionAffinityKey>,
    pub guardian: bool,
}

pub(crate) struct SelectCodexProviderEndpointCredential<'a> {
    pub request_url: &'a Url,
    pub attempt: &'a AttemptContext,
    pub session_affinity: Option<&'a CodexSessionAffinity>,
    /// 本端点的上游模型；提供后按账号模型权限过滤候选（如 live 语音）。
    pub upstream_model: Option<&'a str>,
    pub image_request: bool,
    pub search_request: bool,
    /// 本端点只接受 OAuth 凭据时排除 API Key 账号，避免混合池选中后必然失败。
    pub requires_oauth: bool,
}

struct CredentialSelectionInput<'a> {
    upstream_model: Option<&'a str>,
    responses_model: bool,
    image_request: bool,
    search_request: bool,
    requires_oauth: bool,
    request_url: &'a Url,
    attempt: &'a AttemptContext,
    session_affinity_key: Option<&'a ProviderSessionAffinityKey>,
    session_affinity_observation: Option<&'a CodexSessionAffinity>,
    cache_preference_disabled: std::sync::atomic::AtomicBool,
    guardian: bool,
}

impl CredentialSelectionInput<'_> {
    fn is_cache_preference(&self) -> bool {
        self.session_affinity_observation
            .is_some_and(CodexSessionAffinity::is_cache_preference)
    }

    fn discard_optional_cache_preference(&self) -> bool {
        let optional = self.is_cache_preference()
            && self
                .attempt
                .account_selection_policy()
                .openai_account_affinity()
                == gateway_core::account::AccountAffinity::Preferred;
        if optional {
            self.cache_preference_disabled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        optional
    }

    fn affinity_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.session_affinity_key.filter(|_| {
            !self
                .cache_preference_disabled
                .load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    fn follows_root(&self) -> bool {
        self.attempt
            .account_selection_policy()
            .openai_account_affinity()
            == gateway_core::account::AccountAffinity::Strict
            && !self.attempt.is_diagnostic_required_account()
            && self.attempt.required_account().is_none()
            && matches!(
                self.attempt.continuation_attempt(),
                ContinuationAttempt::None | ContinuationAttempt::ReplayAny
            )
            && self
                .session_affinity_observation
                .is_some_and(CodexSessionAffinity::follow_only)
    }

    fn reserved_concurrency(&self) -> u32 {
        if self.guardian
            && !self.attempt.is_quality_check()
            && !self.attempt.is_diagnostic_required_account()
        {
            self.attempt
                .account_selection_policy()
                .openai_guardian_reserved_concurrency()
        } else {
            0
        }
    }

    fn concurrency_pool(&self) -> ProviderConcurrencyPool {
        if self.reserved_concurrency() > 0 {
            ProviderConcurrencyPool::Reserved
        } else {
            ProviderConcurrencyPool::Shared
        }
    }
}

#[derive(Clone)]
pub(crate) struct CodexCyberPolicyScope {
    key: ProviderSessionAffinityKey,
    state: Option<ProviderSessionExclusions>,
}

fn session_policy_excludes_all(
    scope: Option<&CodexCyberPolicyScope>,
    candidates: &[AccountCandidate],
    pinned: Option<&ProviderAccountId>,
) -> bool {
    let Some(state) = scope.and_then(|scope| scope.state.as_ref()) else {
        return false;
    };
    let mut effective = candidates
        .iter()
        .filter(|candidate| pinned.is_none_or(|id| candidate.account.id() == id));
    effective.next().is_some_and(|first| {
        state.excluded_accounts().contains(first.account.id())
            && effective.all(|candidate| state.excluded_accounts().contains(candidate.account.id()))
    })
}

pub struct CodexCredentialSelector {
    provider_kind: ProviderKind,
    repository: CodexCredentialRepository,
    leases: Arc<dyn ProviderLeasePort>,
    session_affinity: Arc<dyn ProviderSessionAffinityPort>,
    session_exclusions: Arc<dyn ProviderSessionExclusionPort>,
    quota: Arc<CodexCredentialQuotaService>,
    catalog: Arc<CodexCredentialCatalogService>,
    cooldowns: Arc<dyn ProviderCooldownPort>,
    cookie_policy: CodexCookiePolicy,
    risk_recovery: Mutex<HashMap<String, RiskRecoveryState>>,
    excel_auth_blocks: Mutex<HashMap<ProviderAccountId, excel_auth::ExcelAuthBlock>>,
    account_feedback: Arc<AccountFeedbackStats>,
    account_concurrency: AccountConcurrencyHandle,
    scheduling_projection: super::scheduling_projection::SchedulingProjectionCache,
}

enum SessionAffinityLookup {
    Missing,
    Bound(ProviderSessionAffinityBinding),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AffinityEscapeReason {
    HardUnavailable,
    QuotaExhausted,
    Cooldown,
    LeaseSaturated,
    HigherPriority,
    PinnedAccount,
    SelectionInvariant,
}

impl AffinityEscapeReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::HardUnavailable => "hard_unavailable",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Cooldown => "cooldown",
            Self::LeaseSaturated => "lease_saturated",
            Self::HigherPriority => "higher_priority",
            Self::PinnedAccount => "pinned_account",
            Self::SelectionInvariant => "selection_invariant",
        }
    }
}

#[derive(Debug, Default)]
struct AffinitySelection {
    bound_account: Option<ProviderAccountId>,
    bound_binding: Option<ProviderSessionAffinityBinding>,
    preferred_account: Option<ProviderAccountId>,
    escape_reason: Option<AffinityEscapeReason>,
    inherited: bool,
}

impl AffinitySelection {
    fn preferred(binding: ProviderSessionAffinityBinding) -> Self {
        let account_id = binding.account_id().clone();
        Self {
            bound_account: Some(account_id.clone()),
            bound_binding: Some(binding),
            preferred_account: Some(account_id),
            escape_reason: None,
            inherited: false,
        }
    }

    fn escaped(binding: ProviderSessionAffinityBinding, reason: AffinityEscapeReason) -> Self {
        let account_id = binding.account_id().clone();
        Self {
            bound_account: Some(account_id),
            bound_binding: Some(binding),
            preferred_account: None,
            escape_reason: Some(reason),
            inherited: false,
        }
    }

    fn bound_account(&self) -> Option<&ProviderAccountId> {
        self.bound_account.as_ref()
    }

    fn bound_binding(&self) -> Option<&ProviderSessionAffinityBinding> {
        self.bound_binding.as_ref()
    }

    fn preferred_account(&self) -> Option<&ProviderAccountId> {
        self.preferred_account.as_ref()
    }

    fn escape(&mut self, reason: AffinityEscapeReason) {
        if self.bound_account.is_some() && self.escape_reason.is_none() {
            self.escape_reason = Some(reason);
            self.preferred_account = None;
        }
    }

    fn observe_preferred_selection(&mut self, selection: PreferredAccountSelection) {
        if self.bound_account.is_none() || self.escape_reason.is_some() {
            return;
        }
        match selection {
            PreferredAccountSelection::Hit => {}
            PreferredAccountSelection::Blocked(AccountSchedulingBlocker::ConcurrencyLimit) => {
                self.escape(AffinityEscapeReason::LeaseSaturated);
            }
            PreferredAccountSelection::Blocked(AccountSchedulingBlocker::RequestInterval) => {
                self.escape(AffinityEscapeReason::Cooldown);
            }
            PreferredAccountSelection::Blocked(AccountSchedulingBlocker::LowerWeight) => {
                self.escape(AffinityEscapeReason::HigherPriority);
            }
            PreferredAccountSelection::Blocked(
                AccountSchedulingBlocker::LocalAvailability
                | AccountSchedulingBlocker::Excluded
                | AccountSchedulingBlocker::OutsideClientScope,
            )
            | PreferredAccountSelection::Missing => {
                self.escape(AffinityEscapeReason::HardUnavailable);
            }
            PreferredAccountSelection::NotRequested => {
                self.escape(AffinityEscapeReason::SelectionInvariant);
            }
        }
    }

    fn observe_lease_busy(&mut self, account_id: &ProviderAccountId) {
        if self.bound_account.as_ref() == Some(account_id) {
            self.escape(AffinityEscapeReason::LeaseSaturated);
        }
    }

    fn telemetry(
        &self,
        selected_account: &ProviderAccountId,
        cache_preference: bool,
    ) -> AffinityTelemetry {
        AffinityTelemetry {
            affinity_hit: !self.inherited
                && self.bound_account.as_ref() == Some(selected_account)
                && self.escape_reason.is_none(),
            escape_reason: self.escape_reason,
            account_switch: !cache_preference
                && self
                    .bound_account
                    .as_ref()
                    .is_some_and(|bound| bound != selected_account),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct AffinityTelemetry {
    affinity_hit: bool,
    escape_reason: Option<AffinityEscapeReason>,
    account_switch: bool,
}

impl CodexCredentialSelector {
    #[must_use]
    // 选择器显式持有各能力边界，避免把 Provider 私有服务重新包装成通用容器。
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider_kind: ProviderKind,
        repository: CodexCredentialRepository,
        leases: Arc<dyn ProviderLeasePort>,
        session_affinity: Arc<dyn ProviderSessionAffinityPort>,
        session_exclusions: Arc<dyn ProviderSessionExclusionPort>,
        quota: Arc<CodexCredentialQuotaService>,
        catalog: Arc<CodexCredentialCatalogService>,
        cooldowns: Arc<dyn ProviderCooldownPort>,
        account_feedback: Arc<AccountFeedbackStats>,
        cookie_policy: CodexCookiePolicy,
    ) -> Self {
        Self {
            provider_kind,
            repository,
            leases,
            session_affinity,
            session_exclusions,
            quota,
            catalog,
            cooldowns,
            cookie_policy,
            risk_recovery: Mutex::new(HashMap::new()),
            excel_auth_blocks: Mutex::new(HashMap::new()),
            account_feedback,
            account_concurrency: AccountConcurrencyHandle::default(),
            scheduling_projection: Default::default(),
        }
    }

    /// Attach the existing runtime publication; request snapshots never publish limits.
    #[must_use]
    pub fn with_account_concurrency(mut self, handle: AccountConcurrencyHandle) -> Self {
        self.account_concurrency = handle;
        self
    }

    fn account_in_scope(
        &self,
        account: &ProviderAccount,
        request: &CredentialSelectionInput<'_>,
    ) -> bool {
        let upstream = Self::request_upstream(account, request);
        account.provider() == &self.provider_kind
            && match upstream {
                ResponsesUpstream::Codex => account.has_codex_credential(),
                ResponsesUpstream::Excel => account.excel_credential().is_some(),
            }
            && (request.attempt.is_diagnostic_required_account()
                || request.attempt.is_quality_retest()
                || upstream != ResponsesUpstream::Excel
                || !account.excel_credential().is_some_and(|excel| excel.suspended))
            // Model-resolved Excel routes retain their own eligibility contract.
            && !(upstream == gateway_core::account::ResponsesUpstream::Codex
                && account.authentication_kind() == CODEX_AUTHENTICATION_KIND_OAUTH
                && request.upstream_model.is_some_and(|model| {
                    let model = model.rsplit('/').next().unwrap_or(model);
                    model.eq_ignore_ascii_case("grok")
                        || model
                            .get(..5)
                            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grok-"))
                }))
            && (!request.image_request
                || upstream == gateway_core::account::ResponsesUpstream::Excel
                || account.authentication_kind() != CODEX_AUTHENTICATION_KIND_OAUTH
                || !account
                    .plan_type()
                    .is_some_and(|plan| plan.trim().eq_ignore_ascii_case("free")))
            && (!request.requires_oauth
                || (account.authentication_kind() == CODEX_AUTHENTICATION_KIND_OAUTH
                    && !account
                        .plan_type()
                        .is_some_and(|plan| plan.trim().eq_ignore_ascii_case("free"))))
            && request
                .attempt
                .excel_recovery_revision()
                .is_none_or(|revision| {
                    account.credential_revision().get() == revision
                        && account.responses_upstream()
                            == gateway_core::account::ResponsesUpstream::Excel
                        && upstream == gateway_core::account::ResponsesUpstream::Excel
                })
            && (request.attempt.is_diagnostic_required_account()
                || request.attempt.is_quality_retest()
                || self.excel_auth_block(account).is_none())
            && request
                .attempt
                .provider_route()
                .is_none_or(|route| route == upstream.as_str())
            && (upstream != gateway_core::account::ResponsesUpstream::Excel
                || (request
                    .request_url
                    .path()
                    .ends_with(crate::transport::endpoints::CODEX_RESPONSES_PATH)
                    || request.request_url.path().ends_with("/responses/compact")
                    || crate::transport::excel::image_generation::is_image_path(
                        request.request_url.path(),
                    )))
            && (request.attempt.is_diagnostic_required_account()
                || request.attempt.account_scope().is_some_and(|scope| {
                    match (
                        request.image_request || request.search_request,
                        request.upstream_model,
                    ) {
                        (_, Some(model)) => scope.allows_model(account.id(), model),
                        (true, None) => {
                            scope.allows(account.id())
                                && scope
                                    .directory()
                                    .account(account.id())
                                    .is_some_and(|entry| {
                                        entry.model_access().mode() == AccountModelAccessMode::All
                                    })
                        }
                        (false, None) => scope.allows(account.id()),
                    }
                }))
    }

    fn request_upstream(
        account: &ProviderAccount,
        request: &CredentialSelectionInput<'_>,
    ) -> gateway_core::account::ResponsesUpstream {
        if request.attempt.is_native_quality_probe() {
            gateway_core::account::ResponsesUpstream::Codex
        } else {
            request
                .attempt
                .credential_route()
                .unwrap_or_else(|| account.responses_upstream())
        }
    }

    pub async fn select(
        &self,
        request: &SelectCodexCredential<'_>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let input = CredentialSelectionInput {
            cache_preference_disabled: std::sync::atomic::AtomicBool::new(false),
            upstream_model: Some(request.upstream_model),
            responses_model: true,
            image_request: false,
            search_request: false,
            requires_oauth: false,
            request_url: request.request_url,
            attempt: request.attempt,
            session_affinity_key: request.session_affinity_key,
            session_affinity_observation: None,
            guardian: request.guardian,
        };
        self.select_inner(&input, None).await
    }

    pub(crate) async fn select_with_cyber_policy(
        &self,
        request: &SelectCodexCredential<'_>,
        cyber_policy_session_key: Option<&ProviderSessionAffinityKey>,
        session_affinity_observation: Option<&CodexSessionAffinity>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let input = CredentialSelectionInput {
            cache_preference_disabled: std::sync::atomic::AtomicBool::new(false),
            upstream_model: Some(request.upstream_model),
            responses_model: true,
            image_request: false,
            search_request: false,
            requires_oauth: false,
            request_url: request.request_url,
            attempt: request.attempt,
            session_affinity_key: request.session_affinity_key,
            session_affinity_observation,
            guardian: request.guardian,
        };
        self.select_inner(&input, cyber_policy_session_key).await
    }

    /// 为不属于 Responses 文本模型目录的 Provider 原生端点选择账号。
    ///
    /// 账号范围、健康度、配额、并发租约、cookie 与认证准备仍走同一套选择链路；
    /// 独立 Images、Search 和 Live 端点不套用 Responses 的目录资格。
    pub(crate) async fn select_for_provider_endpoint(
        &self,
        request: &SelectCodexProviderEndpointCredential<'_>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let input = CredentialSelectionInput {
            cache_preference_disabled: std::sync::atomic::AtomicBool::new(false),
            upstream_model: request.upstream_model,
            responses_model: false,
            image_request: request.image_request,
            search_request: request.search_request,
            requires_oauth: request.requires_oauth,
            request_url: request.request_url,
            attempt: request.attempt,
            session_affinity_key: request.session_affinity.and_then(CodexSessionAffinity::key),
            session_affinity_observation: request.session_affinity,
            guardian: false,
        };
        self.select_inner(&input, None).await
    }

    async fn select_inner(
        &self,
        request: &CredentialSelectionInput<'_>,
        cyber_policy_session_key: Option<&ProviderSessionAffinityKey>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let selection = async {
            let first = self
                .select_unfrozen(request, cyber_policy_session_key, true, None)
                .await;
            match first {
                Err(CredentialSelectionError::AccountWait(
                    reason @ (AccountWaitFailure::QueueFull | AccountWaitFailure::Timeout),
                )) if !request.follows_root()
                    && request.attempt.required_account().is_none()
                    && matches!(
                        request.attempt.continuation_attempt(),
                        ContinuationAttempt::None | ContinuationAttempt::ReplayAny
                    ) =>
                {
                    let rejection = match reason {
                        AccountWaitFailure::QueueFull => {
                            gateway_core::concurrency::QueueRejection::Full
                        }
                        _ => gateway_core::concurrency::QueueRejection::Timeout,
                    };
                    request
                        .attempt
                        .account_selection_policy()
                        .reselect_after_queue_rejection(
                            rejection,
                            request.attempt.deadline().remaining(),
                            |_| {
                                request.attempt.trace().record(
                                    "account.queue.reselect",
                                    serde_json::json!({"reason": rejection.to_string()}),
                                );
                                self.select_unfrozen(
                                    request,
                                    cyber_policy_session_key,
                                    false,
                                    Some(rejection),
                                )
                            },
                            |error| {
                                matches!(
                                    error,
                                    CredentialSelectionError::CapacityUnavailable { .. }
                                        | CredentialSelectionError::NoEligibleCredential
                                        | CredentialSelectionError::QuotaExhausted
                                )
                            },
                        )
                        .await
                }
                result => result,
            }
        };
        let result = tokio::select! {
            biased;
            () = request.attempt.cancellation().cancelled() => Err(CredentialSelectionError::Cancelled),
            result = selection => result,
        }.and_then(|lease| {
            if request
                .attempt
                .freeze_provider_route(lease.selected_route().as_str())
            {
                Ok(lease)
            } else {
                Err(CredentialSelectionError::NoEligibleCredential)
            }
        });
        result.map_err(|error| {
            if request.follows_root()
                && !matches!(
                    error,
                    CredentialSelectionError::SessionBound(_)
                        | CredentialSelectionError::ContinuationOwnerChanged
                        | CredentialSelectionError::QuotaPolicyFiltered
                        | CredentialSelectionError::SessionPolicyRestricted
                )
            {
                CredentialSelectionError::SessionBound(Box::new(error))
            } else {
                error
            }
        })
    }

    async fn select_unfrozen(
        &self,
        request: &CredentialSelectionInput<'_>,
        cyber_policy_session_key: Option<&ProviderSessionAffinityKey>,
        allow_queue: bool,
        queue_rejection: Option<gateway_core::concurrency::QueueRejection>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let diagnostic = request.attempt.is_diagnostic_required_account();
        if allow_queue
            && (request.attempt.request_tuning().account_busy_wait_enabled
                || request.attempt.is_quality_check())
            && !diagnostic
        {
            return self
                .select_with_capacity_wait(request, cyber_policy_session_key, None)
                .await;
        }
        let mut universe = None;
        let mut retry = 0;
        loop {
            let result = self
                .select_snapshot(
                    request,
                    cyber_policy_session_key,
                    &mut universe,
                    queue_rejection,
                )
                .await;
            if allow_queue
                && request.follows_root()
                && matches!(
                    result,
                    Err(CredentialSelectionError::NoEligibleCredential
                        | CredentialSelectionError::QuotaExhausted
                        | CredentialSelectionError::SessionPolicyRestricted
                        | CredentialSelectionError::QuotaPolicyFiltered
                        | CredentialSelectionError::CapacityUnavailable { .. })
                )
            {
                return self
                    .select_with_capacity_wait(request, cyber_policy_session_key, universe.as_ref())
                    .await;
            }
            if matches!(
                result,
                Err(CredentialSelectionError::AccountAdmissionChanged)
            ) {
                if request.attempt.cancellation().is_cancelled() {
                    return Err(CredentialSelectionError::Cancelled);
                }
                if request.attempt.deadline().is_elapsed() {
                    break;
                }
                tokio::task::yield_now().await;
                continue;
            }
            if !matches!(
                result,
                Err(CredentialSelectionError::AccountSnapshotChanged)
            ) {
                return result;
            }
            if retry == 3 {
                break;
            }
            retry += 1;
            request.attempt.trace().record(
                "account.snapshot_conflict",
                serde_json::json!({
                    "retryCount": retry, "maxRetries": 3,
                }),
            );
            if request.attempt.cancellation().is_cancelled() {
                return Err(CredentialSelectionError::Cancelled);
            }
            if request.attempt.deadline().is_elapsed() {
                break;
            }
        }
        Err(CredentialSelectionError::AccountSnapshotChanged)
    }

    async fn select_snapshot(
        &self,
        request: &CredentialSelectionInput<'_>,
        cyber_policy_session_key: Option<&ProviderSessionAffinityKey>,
        universe: &mut Option<BTreeSet<ProviderAccountId>>,
        queue_rejection: Option<gateway_core::concurrency::QueueRejection>,
    ) -> Result<CodexCredentialLease, CredentialSelectionError> {
        let diagnostic = request.attempt.is_diagnostic_required_account();
        let quality = request.attempt.is_quality_check();
        let mut projection = self.read_projection(request).await?;
        let mut accounts = match projection.owner() {
            Some(owner) => self
                .repository
                .store()
                .get_account(owner)
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
                .into_iter()
                .collect(),
            None => self.scoped_accounts(request).await?,
        };
        let quality_recovery = if request.attempt.is_quality_check()
            && let Some(required) = request.attempt.required_account()
        {
            self.repository
                .store()
                .quality_pause_is_owned(required)
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
        } else {
            false
        };
        self.retain_excel_auth_blocks(&accounts);
        // Only pinned diagnostics or verified quality recovery can load disabled rows.
        if (diagnostic || quality_recovery)
            && let Some(required) = request.attempt.required_account()
            && !accounts.iter().any(|account| account.id() == required)
            && let Some(account) = self
                .repository
                .store()
                .get_account(required)
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
        {
            accounts.push(account);
        }
        let mut accounts = accounts
            .into_iter()
            .filter_map(|account| Self::project_request_account(account, request))
            .filter(|account| self.account_in_scope(account, request))
            .collect::<Vec<_>>();
        self.retain_model_eligible(&mut accounts, request);
        for account in &mut accounts {
            let eligible_route = request.attempt.codex_quota_overdraft_eligible()
                && Self::request_upstream(account, request)
                    == gateway_core::account::ResponsesUpstream::Codex;
            if !eligible_route {
                *account = account.clone().with_codex_quota_overdraft_passed(false);
            }
        }
        let universe = universe.get_or_insert_with(|| {
            request
                .attempt
                .account_scope()
                .map(|scope| scope.allowed_account_ids().cloned().collect())
                .or_else(|| {
                    projection.candidates.as_deref().map(|candidates| {
                        candidates
                            .iter()
                            .map(|candidate| candidate.account.id().clone())
                            .collect()
                    })
                })
                .unwrap_or_else(|| {
                    accounts
                        .iter()
                        .map(|account| account.id().clone())
                        .collect()
                })
        });
        accounts.retain(|account| universe.contains(account.id()));
        if !diagnostic {
            self.prepare_route_scheduling(&accounts).await;
        }
        let rate_limits = if diagnostic {
            HashMap::new()
        } else {
            self.scheduling_cooldowns(&accounts, request).await
        };
        let account_ids = accounts
            .iter()
            .map(|account| account.id().clone())
            .collect::<Vec<_>>();
        let scheduling = self
            .leases
            .load_state_for_pool(
                request.attempt.client_api_key_ref(),
                &self.provider_kind,
                &account_ids,
                request.concurrency_pool(),
            )
            .await?;
        let round_robin_cursor = scheduling.round_robin_cursor();
        let mut candidates = accounts
            .into_iter()
            .map(|account| {
                let signals = scheduling.signals().get(account.id()).cloned().unwrap_or(
                    AccountRuntimeSignals {
                        in_flight: 0,
                        last_started_at: None,
                        quota_reset_at: None,
                        quota_reset_windows: None,
                        quota_remaining_rank: None,
                        rate_limited_until: None,
                        failure_rate_basis_points: None,
                        first_output_latency_ms: None,
                    },
                );
                let signals = self
                    .route_scheduling_signals(&account, signals)
                    .with_rate_limit(rate_limits.get(account.id()).copied().flatten());
                let signals = quality_selection_signals(signals, quality);
                AccountCandidate { account, signals }
            })
            .collect::<Vec<_>>();
        if let Some(cached) = projection.candidates.as_ref() {
            let Some(current) = candidates.pop() else {
                self.invalidate_projection(&projection);
                return Err(CredentialSelectionError::AccountSnapshotChanged);
            };
            if cached
                .iter()
                .find(|candidate| candidate.account.id() == current.account.id())
                .is_none_or(|previous| {
                    !only_quota_observation_advanced(&previous.account, &current.account)
                })
            {
                self.invalidate_projection(&projection);
                return Err(CredentialSelectionError::AccountSnapshotChanged);
            }
            candidates = cached
                .iter()
                .map(|candidate| {
                    if candidate.account.id() == current.account.id() {
                        current.clone()
                    } else {
                        candidate.clone()
                    }
                })
                .collect();
            candidates.retain(|candidate| universe.contains(candidate.account.id()));
        } else {
            self.publish_projection(&projection, &candidates);
        }
        drop(projection.fill.take());
        let continuation_account = match request.attempt.continuation_attempt() {
            ContinuationAttempt::Native => request
                .attempt
                .continuation()
                .and_then(gateway_core::engine::continuation::ContinuationBinding::pinned)
                .map(|continuation| continuation.account().clone()),
            ContinuationAttempt::ReplayOwner => request
                .attempt
                .account_state_owner()
                .filter(|owner| owner.provider() == &self.provider_kind)
                .map(|owner| owner.account().clone()),
            ContinuationAttempt::None | ContinuationAttempt::ReplayAny => None,
        };
        let required_account = request.attempt.required_account().cloned();
        if required_account
            .as_ref()
            .zip(continuation_account.as_ref())
            .is_some_and(|(required, continuation)| required != continuation)
        {
            return Err(CredentialSelectionError::NoEligibleCredential);
        }
        let mut pinned_account = required_account.or_else(|| continuation_account.clone());
        let mut affinity = if diagnostic {
            AffinitySelection::default()
        } else {
            self.resolve_session_affinity(
                request,
                request.affinity_key(),
                request
                    .session_affinity_observation
                    .and_then(CodexSessionAffinity::root_key),
                request
                    .session_affinity_observation
                    .and_then(CodexSessionAffinity::migration_key),
                request
                    .session_affinity_observation
                    .filter(|_| pinned_account.is_none())
                    .and_then(CodexSessionAffinity::guardian_parent_preference_key),
                &candidates,
                SystemTime::now(),
                request.attempt.codex_quota_overdraft_eligible(),
                projection.key.as_ref().map(|_| projection.binding.as_ref()),
            )
            .await?
        };
        if request
            .attempt
            .account_selection_policy()
            .openai_account_affinity()
            == gateway_core::account::AccountAffinity::Strict
            && affinity
                .bound_account()
                .zip(continuation_account.as_ref())
                .is_some_and(|(binding, owner)| binding != owner)
        {
            return Err(CredentialSelectionError::ContinuationOwnerChanged);
        }
        if !diagnostic && pinned_account.is_none() && request.follows_root() {
            pinned_account = affinity.bound_account().cloned();
        }
        let cyber_policy_scope = self
            .prepare_cyber_policy_scope(cyber_policy_session_key)
            .await;
        let mut excluded = request.attempt.excluded_accounts().clone();
        if !diagnostic {
            for candidate in &candidates {
                if candidate.account.quota_paused() {
                    if request
                        .attempt
                        .account_selection_policy()
                        .openai_account_affinity()
                        == gateway_core::account::AccountAffinity::Strict
                        && request
                            .session_affinity_observation
                            .is_some_and(CodexSessionAffinity::follow_only)
                        && affinity.bound_account() == Some(candidate.account.id())
                    {
                        return Err(CredentialSelectionError::QuotaPolicyFiltered);
                    }
                    excluded.insert(candidate.account.id().clone());
                }
            }
        }
        if let Some(state) = cyber_policy_scope
            .as_ref()
            .and_then(|scope| scope.state.as_ref())
        {
            excluded.extend(state.excluded_accounts().iter().cloned());
        }
        if let Some(required) = pinned_account.as_ref() {
            for candidate in &candidates {
                if candidate.account.id() != required {
                    excluded.insert(candidate.account.id().clone());
                }
            }
        }
        // Keep the original request scope for the terminal quota-pool check.
        // Accounts temporarily excluded after a busy lease must not disappear
        // from the diagnosis, otherwise a saturated pool could be mislabeled
        // as permanently exhausted.
        let base_excluded = excluded.clone();
        if pinned_account
            .as_ref()
            .zip(affinity.bound_account())
            .is_some_and(|(pinned, bound)| pinned != bound)
        {
            affinity.escape(AffinityEscapeReason::PinnedAccount);
        }
        // Legacy migration and Guardian preferences are not bindings for this key.
        let observed_affinity_binding = if affinity.inherited {
            None
        } else {
            affinity.bound_binding().cloned()
        };
        let assignment_cursor = self
            .assignment_cursor(
                request,
                observed_affinity_binding.as_ref(),
                pinned_account.is_some(),
            )
            .await?;
        let round_robin_cursor = assignment_cursor.unwrap_or(round_robin_cursor);
        let mut shortest_retry = None;
        let mut capacity_unavailable = false;
        let policy = request.attempt.account_selection_policy();

        loop {
            let preferred = pinned_account.clone().or_else(|| {
                (policy.openai_account_affinity()
                    != gateway_core::account::AccountAffinity::Relaxed)
                    .then(|| affinity.preferred_account().cloned())
                    .flatten()
            });
            let context = AccountSelectionContext {
                waiting_counts: Default::default(),
                policy,
                now: SystemTime::now(),
                excluded_accounts: excluded.clone(),
                preferred_account: preferred.clone(),
                preferred_account_overrides_weight: true,
                round_robin_cursor: Self::session_round_robin_cursor(
                    request,
                    observed_affinity_binding.as_ref(),
                    round_robin_cursor,
                ),
                eligibility: if diagnostic {
                    request.attempt.account_eligibility()
                } else if quality_recovery {
                    AccountEligibilityPolicy::IgnoreQualityPause
                } else {
                    AccountEligibilityPolicy::Enforce
                },
                allow_codex_quota_overdraft: request.attempt.codex_quota_overdraft_eligible(),
                account_scope: request.attempt.account_scope().cloned(),
                reserved_concurrency: request.reserved_concurrency(),
            };
            let capacity = AccountSelector.capacity_snapshot(&candidates, &context);
            capacity_unavailable |= AccountSelector.has_busy_candidate(&candidates, &context);
            let selection = AccountSelector.select(&candidates, &context);
            request
                .attempt
                .trace()
                .account_selection(&candidates, &context, selection.as_ref());
            if projection.candidates.is_some()
                && selection.as_ref().is_none_or(|selected| {
                    Some(selected.candidate().account.id()) != projection.owner()
                })
            {
                self.invalidate_projection(&projection);
                return Err(CredentialSelectionError::AccountSnapshotChanged);
            }
            let Some(selection) = selection else {
                let quota_exhausted = !diagnostic
                    && !quality
                    && candidates
                        .iter()
                        .filter(|candidate| !base_excluded.contains(candidate.account.id()))
                        .map(|candidate| {
                            candidate
                                .account
                                .status_projection(
                                    context.now,
                                    candidate.signals.rate_limited_until,
                                )
                                .status
                        })
                        .next()
                        == Some(AccountStatus::QuotaExhausted)
                    && candidates
                        .iter()
                        .filter(|candidate| !base_excluded.contains(candidate.account.id()))
                        .all(|candidate| {
                            candidate
                                .account
                                .status_projection(
                                    context.now,
                                    candidate.signals.rate_limited_until,
                                )
                                .status
                                == AccountStatus::QuotaExhausted
                        });
                return if capacity_unavailable {
                    Err(CredentialSelectionError::CapacityUnavailable {
                        retry_after: shortest_retry,
                    })
                } else if quota_exhausted {
                    Err(CredentialSelectionError::QuotaExhausted)
                } else if session_policy_excludes_all(
                    cyber_policy_scope.as_ref(),
                    &candidates,
                    pinned_account.as_ref(),
                ) {
                    Err(CredentialSelectionError::SessionPolicyRestricted)
                } else {
                    Err(CredentialSelectionError::NoEligibleCredential)
                };
            };
            affinity.observe_preferred_selection(selection.preferred());
            let selected = selection.candidate();
            let account = candidates
                .iter()
                .find(|candidate| candidate.account.id() == selected.account.id())
                .map(|candidate| candidate.account.clone())
                .ok_or(CredentialSelectionError::InvalidCredential(None))?;
            let allows_account_state_mutation = context
                .eligibility
                .allows_account_state_mutation(account.enabled());
            if !diagnostic && self.excel_auth_block(&account).is_some() {
                excluded.insert(account.id().clone());
                continue;
            }
            // Snapshot conflicts must not consume capacity or the request interval.
            let (account, runtime) =
                match self.repository.reload_selected_credential(&account).await {
                    Ok(reloaded) => reloaded,
                    Err(CredentialRepositoryError::InvalidCredentialData(_))
                        if pinned_account.is_none() =>
                    {
                        if affinity.bound_account() == Some(account.id()) {
                            affinity.escape(AffinityEscapeReason::HardUnavailable);
                        }
                        excluded.insert(account.id().clone());
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
            if !diagnostic && account.quota_paused() {
                self.invalidate_projection(&projection);
                return Err(CredentialSelectionError::AccountSnapshotChanged);
            }
            match self
                .leases
                .try_acquire(ProviderLeaseRequest::Scheduling(
                    ProviderSchedulingLeaseRequest::new(
                        self.provider_kind.clone(),
                        account.id().clone(),
                        account.revision(),
                        quality_concurrency_limit(context.concurrency_limit(&account), quality),
                        policy.request_interval(),
                        request.attempt.deadline(),
                    )
                    .with_cancellation(request.attempt.cancellation().clone())
                    .with_detail(request.attempt.lease_detail(request.upstream_model))
                    .with_quality_check(quality)
                    .with_concurrency_pool(request.concurrency_pool()),
                ))
                .await?
            {
                ProviderLeaseAcquisition::Busy { retry_after } => {
                    if projection.candidates.is_some() {
                        self.invalidate_projection(&projection);
                        return Err(CredentialSelectionError::AccountSnapshotChanged);
                    }
                    capacity_unavailable = true;
                    affinity.observe_lease_busy(account.id());
                    shortest_retry = minimum_duration(shortest_retry, retry_after);
                    excluded.insert(account.id().clone());
                }
                ProviderLeaseAcquisition::Acquired(guard) => {
                    let current = self.repository.current_selected_account(&account).await?;
                    if current
                        .as_ref()
                        .is_none_or(|current| !only_quota_observation_advanced(&account, current))
                    {
                        drop(guard);
                        self.invalidate_projection(&projection);
                        return Err(CredentialSelectionError::AccountSnapshotChanged);
                    }
                    let admitted = self
                        .admit_selected_session(
                            request,
                            observed_affinity_binding.as_ref(),
                            account.id(),
                            pinned_account.is_some(),
                            queue_rejection,
                            assignment_cursor.filter(|_| {
                                selection.preferred() != PreferredAccountSelection::Hit
                            }),
                        )
                        .await?;
                    let Some(affinity_expected_binding) = admitted else {
                        guard.release().await?;
                        self.invalidate_projection(&projection);
                        return Err(CredentialSelectionError::AccountAdmissionChanged);
                    };
                    let affinity_telemetry =
                        affinity.telemetry(account.id(), request.is_cache_preference());
                    let affinity_observation = request.session_affinity_observation;
                    tracing::info!(
                        request_id = %request.attempt.request_id(),
                        attempt_index = request.attempt.attempt_index().get(),
                        rotation_strategy = policy.strategy().as_str(),
                        account_id = %account.id(),
                        affinity_hit = affinity_telemetry.affinity_hit,
                        affinity_inherited = affinity.inherited,
                        escape_reason = affinity_telemetry
                            .escape_reason
                            .map_or("", AffinityEscapeReason::as_str),
                        account_switch = affinity_telemetry.account_switch,
                        affinity_key_hash = affinity_observation
                            .map_or("", CodexSessionAffinity::key_hash),
                        affinity_anchor_source = affinity_observation
                            .map_or("", CodexSessionAffinity::anchor_source),
                        affinity_anchor = affinity_observation
                            .map_or("", CodexSessionAffinity::anchor),
                        session_id = affinity_observation
                            .and_then(CodexSessionAffinity::session_id)
                            .unwrap_or(""),
                        session_id_present = affinity_observation
                            .is_some_and(CodexSessionAffinity::session_id_present),
                        "OpenAI account selected"
                    );
                    if !diagnostic
                        && request
                            .session_affinity_observation
                            .is_some_and(CodexSessionAffinity::follow_only)
                        && request.attempt.required_account().is_none()
                        && !matches!(
                            request.attempt.continuation_attempt(),
                            ContinuationAttempt::Native | ContinuationAttempt::ReplayOwner
                        )
                        && let Some(key) = request.affinity_key()
                    {
                        self.verify_followed_root(key, &affinity_expected_binding)
                            .await?;
                    }
                    let cookies = runtime
                        .cookies
                        .into_iter()
                        .filter(|cookie| {
                            cookie
                                .expires_at
                                .is_none_or(|expires| expires > chrono::Utc::now())
                                && self.cookie_policy.may_replay(
                                    request.request_url,
                                    &cookie.domain,
                                    &cookie.path,
                                    cookie.host_only,
                                    cookie.secure,
                                )
                        })
                        .collect();
                    if !diagnostic
                        && let Some(observation) = request.session_affinity_observation
                        && !self
                            .remember_turn_alias(observation, policy.openai_session_binding_ttl())
                            .await?
                    {
                        drop(guard);
                        return Err(binding_unavailable());
                    }
                    if !diagnostic && self.excel_auth_block(&account).is_some() {
                        drop(guard);
                        excluded.insert(account.id().clone());
                        continue;
                    }
                    if quality_recovery
                        && !self
                            .repository
                            .store()
                            .quality_pause_is_owned(account.id())
                            .await
                            .map_err(|source| {
                                CredentialSelectionError::Store(Some(source.into()))
                            })?
                    {
                        drop(guard);
                        self.invalidate_projection(&projection);
                        return Err(CredentialSelectionError::AccountSnapshotChanged);
                    }
                    return Ok(CodexCredentialLease {
                        selected_route: account.credential_route(),
                        session_binding_ttl: policy.openai_session_binding_ttl(),
                        installation_id: runtime.installation_id,
                        account,
                        authentication: runtime.authentication,
                        cookies,
                        cyber_policy_scope,
                        diagnostic,
                        allows_account_state_mutation,
                        affinity_telemetry,
                        affinity_expected_binding,
                        guardian_parent_record_key: request
                            .session_affinity_observation
                            .and_then(CodexSessionAffinity::guardian_parent_record_key)
                            .cloned(),
                        capacity: capacity.map(AccountCapacitySnapshot::with_acquired_request),
                        _guard: guard,
                    });
                }
            }
        }
    }

    #[expect(clippy::too_many_arguments)]
    async fn resolve_session_affinity(
        &self,
        request: &CredentialSelectionInput<'_>,
        key: Option<&ProviderSessionAffinityKey>,
        root_key: Option<&ProviderSessionAffinityKey>,
        migration_key: Option<&ProviderSessionAffinityKey>,
        guardian_parent_preference_key: Option<&ProviderSessionAffinityKey>,
        candidates: &[AccountCandidate],
        now: SystemTime,
        allow_codex_quota_overdraft: bool,
        observed: Option<Option<&ProviderSessionAffinityBinding>>,
    ) -> Result<AffinitySelection, CredentialSelectionError> {
        if let Some(key) = key {
            let lookup = match observed {
                Some(Some(binding)) => SessionAffinityLookup::Bound(binding.clone()),
                Some(None) => SessionAffinityLookup::Missing,
                None => self.lookup_request_affinity(request, key).await,
            };
            match lookup {
                SessionAffinityLookup::Bound(binding) => {
                    return Ok(affinity_selection_for_bound_account(
                        binding,
                        candidates,
                        now,
                        allow_codex_quota_overdraft,
                    ));
                }
                SessionAffinityLookup::Missing => {}
                SessionAffinityLookup::Unavailable => return Err(binding_unavailable()),
            }
        }

        for fallback_key in [root_key, migration_key].into_iter().flatten() {
            match self.lookup_request_affinity(request, fallback_key).await {
                SessionAffinityLookup::Bound(binding) => {
                    let mut inherited = affinity_selection_for_bound_account(
                        binding,
                        candidates,
                        now,
                        allow_codex_quota_overdraft,
                    );
                    inherited.inherited = true;
                    return Ok(inherited);
                }
                SessionAffinityLookup::Unavailable => return Err(binding_unavailable()),
                SessionAffinityLookup::Missing => {}
            }
        }

        if let Some(parent_key) = guardian_parent_preference_key {
            match self.lookup_session_affinity(parent_key).await {
                SessionAffinityLookup::Bound(binding) => {
                    let mut inherited = affinity_selection_for_bound_account(
                        binding,
                        candidates,
                        now,
                        allow_codex_quota_overdraft,
                    );
                    inherited.inherited = true;
                    return Ok(inherited);
                }
                SessionAffinityLookup::Unavailable | SessionAffinityLookup::Missing => {}
            }
        }

        Ok(AffinitySelection::default())
    }

    async fn lookup_request_affinity(
        &self,
        request: &CredentialSelectionInput<'_>,
        key: &ProviderSessionAffinityKey,
    ) -> SessionAffinityLookup {
        if request.affinity_key().is_none() && request.is_cache_preference() {
            return SessionAffinityLookup::Missing;
        }
        let lookup = self.lookup_session_affinity(key).await;
        if matches!(lookup, SessionAffinityLookup::Unavailable)
            && request.discard_optional_cache_preference()
        {
            SessionAffinityLookup::Missing
        } else {
            lookup
        }
    }

    pub(crate) async fn lookup_session_alias(
        &self,
        key: &ProviderSessionAffinityKey,
    ) -> Result<Option<ProviderSessionAlias>, ()> {
        match tokio::time::timeout(
            SESSION_AFFINITY_TIMEOUT,
            self.session_affinity.load_alias(&self.provider_kind, key),
        )
        .await
        {
            Ok(Ok(alias)) => Ok(alias),
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "OpenAI turn alias read failed");
                Err(())
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = SESSION_AFFINITY_TIMEOUT.as_millis(),
                    "OpenAI turn alias read timed out"
                );
                Err(())
            }
        }
    }

    pub(crate) async fn validate_followed_turn_alias(
        &self,
        affinity: &CodexSessionAffinity,
    ) -> Result<(), CredentialSelectionError> {
        if affinity.follow_only()
            && let Some(key) = affinity.binding_key()
        {
            match self.lookup_session_affinity(key).await {
                SessionAffinityLookup::Bound(_) => {}
                // A retained alias names the session, not an account. Selection will
                // atomically claim a missing binding while preserving any winning owner.
                SessionAffinityLookup::Missing => {}
                SessionAffinityLookup::Unavailable => {
                    return Err(binding_unavailable());
                }
            }
        }
        Ok(())
    }

    async fn remember_turn_alias(
        &self,
        affinity: &CodexSessionAffinity,
        session_binding_ttl: Duration,
    ) -> Result<bool, CredentialSelectionError> {
        let Some(alias_key) = affinity.turn_alias_key() else {
            return Ok(true);
        };
        let Some(alias) = affinity.alias_record() else {
            return Ok(true);
        };
        let recorded = match tokio::time::timeout(
            SESSION_AFFINITY_TIMEOUT,
            self.session_affinity.bind_alias(
                &self.provider_kind,
                alias_key,
                &alias,
                session_binding_ttl,
            ),
        )
        .await
        {
            Ok(Ok(bound)) => bound,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "OpenAI turn alias write failed");
                return Err(binding_store_error(error));
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = SESSION_AFFINITY_TIMEOUT.as_millis(),
                    "OpenAI turn alias write timed out"
                );
                return Err(binding_unavailable());
            }
        };
        // Exact continuation ownership outranks this routing hint. Keep a conflicting
        // alias unchanged, but do not reject its independently authorized owner.
        Ok(recorded || !affinity.turn_alias_required())
    }

    async fn lookup_session_affinity(
        &self,
        key: &ProviderSessionAffinityKey,
    ) -> SessionAffinityLookup {
        match tokio::time::timeout(
            SESSION_AFFINITY_TIMEOUT,
            self.session_affinity.load_binding(&self.provider_kind, key),
        )
        .await
        {
            Ok(Ok(Some(binding))) => SessionAffinityLookup::Bound(binding),
            Ok(Ok(None)) => SessionAffinityLookup::Missing,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "OpenAI session affinity read failed");
                SessionAffinityLookup::Unavailable
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = SESSION_AFFINITY_TIMEOUT.as_millis(),
                    "OpenAI session affinity read timed out"
                );
                SessionAffinityLookup::Unavailable
            }
        }
    }

    async fn verify_followed_root(
        &self,
        key: &ProviderSessionAffinityKey,
        expected: &ProviderSessionAffinityBinding,
    ) -> Result<(), CredentialSelectionError> {
        match self.lookup_session_affinity(key).await {
            SessionAffinityLookup::Bound(current) if &current == expected => Ok(()),
            SessionAffinityLookup::Unavailable => Err(binding_unavailable()),
            SessionAffinityLookup::Bound(_) | SessionAffinityLookup::Missing => {
                Err(CredentialSelectionError::CapacityUnavailable {
                    retry_after: Some(Duration::from_millis(100)),
                })
            }
        }
    }

    async fn prepare_cyber_policy_scope(
        &self,
        key: Option<&ProviderSessionAffinityKey>,
    ) -> Option<CodexCyberPolicyScope> {
        let key = key?.clone();
        let state = match tokio::time::timeout(
            CYBER_POLICY_TIMEOUT,
            self.session_exclusions.load(&self.provider_kind, &key),
        )
        .await
        {
            Ok(Ok(state)) => state,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "OpenAI cyber policy state read failed open");
                None
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = CYBER_POLICY_TIMEOUT.as_millis(),
                    "OpenAI cyber policy state read timed out"
                );
                None
            }
        };
        Some(CodexCyberPolicyScope { key, state })
    }

    pub(crate) async fn record_cyber_policy_failure(
        &self,
        scope: Option<&CodexCyberPolicyScope>,
        account: &ProviderAccount,
    ) {
        let Some(scope) = scope else {
            return;
        };
        match tokio::time::timeout(
            CYBER_POLICY_TIMEOUT,
            self.session_exclusions.record_failure(
                &self.provider_kind,
                &scope.key,
                account.id(),
                CYBER_POLICY_SESSION_TTL,
            ),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    account_id = %account.id(),
                    error = %error,
                    "OpenAI cyber policy exclusion write failed open"
                );
            }
            Err(_) => {
                tracing::warn!(
                    account_id = %account.id(),
                    timeout_ms = CYBER_POLICY_TIMEOUT.as_millis(),
                    "OpenAI cyber policy exclusion write timed out"
                );
            }
        }
    }

    pub(crate) async fn observe_cyber_policy_success(&self, scope: Option<&CodexCyberPolicyScope>) {
        let Some(scope) = scope.filter(|scope| scope.state.is_some()) else {
            return;
        };
        let Some(state) = scope.state.as_ref() else {
            return;
        };
        match tokio::time::timeout(
            CYBER_POLICY_TIMEOUT,
            self.session_exclusions
                .clear(&self.provider_kind, &scope.key, state.revision()),
        )
        .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "OpenAI cyber policy exclusion clear failed open");
            }
            Err(_) => {
                tracing::warn!(
                    timeout_ms = CYBER_POLICY_TIMEOUT.as_millis(),
                    "OpenAI cyber policy exclusion clear timed out"
                );
            }
        }
    }

    pub async fn record_failure(
        &self,
        account: &ProviderAccount,
        failure: CodexAccountFailure,
        message: Option<String>,
    ) -> Result<(), CredentialSelectionError> {
        self.record_failure_inner(account, failure, message, false)
            .await
    }

    pub async fn record_diagnostic_failure(
        &self,
        account: &ProviderAccount,
        failure: CodexAccountFailure,
        message: Option<String>,
    ) -> Result<(), CredentialSelectionError> {
        self.record_failure_inner(account, failure, message, true)
            .await
    }

    async fn record_failure_inner(
        &self,
        account: &ProviderAccount,
        failure: CodexAccountFailure,
        message: Option<String>,
        diagnostic: bool,
    ) -> Result<(), CredentialSelectionError> {
        let now = SystemTime::now();
        let message = message.filter(|value| !value.trim().is_empty());
        if account.credential_route() == ResponsesUpstream::Excel {
            match failure {
                CodexAccountFailure::QuotaExhausted => {
                    return self
                        .record_excel_quota(
                            account,
                            gateway_core::account::QuotaState::exhausted(
                                QuotaEvidence::PaymentRequired,
                                now,
                                None,
                            ),
                            now,
                        )
                        .await;
                }
                CodexAccountFailure::UsageLimitExhausted { reset_at } => {
                    return self
                        .record_excel_quota(
                            account,
                            gateway_core::account::QuotaState::exhausted(
                                QuotaEvidence::UsageLimitReached,
                                now,
                                reset_at,
                            ),
                            now,
                        )
                        .await;
                }
                CodexAccountFailure::RateLimited { .. }
                | CodexAccountFailure::CloudflareChallenge { .. }
                | CodexAccountFailure::CloudflarePathBlocked => return Ok(()),
                _ => {}
            }
        }
        match failure {
            CodexAccountFailure::CredentialExpired => {
                let refreshable = account.authentication_kind() == CODEX_AUTHENTICATION_KIND_OAUTH
                    && account.has_refresh_token();
                self.apply_credential_state(
                    account,
                    CredentialState::Expired,
                    if refreshable {
                        AccountErrorReason::AccessTokenExpired
                    } else {
                        AccountErrorReason::CredentialExpired
                    },
                    now,
                    message,
                    diagnostic,
                )
                .await
            }
            CodexAccountFailure::CredentialRevoked => {
                self.apply_credential_state(
                    account,
                    CredentialState::Expired,
                    AccountErrorReason::CredentialExpired,
                    now,
                    message,
                    diagnostic,
                )
                .await
            }
            CodexAccountFailure::IdentityVerificationRequired => {
                self.apply_credential_state(
                    account,
                    CredentialState::Invalid,
                    AccountErrorReason::AccountUnverified,
                    now,
                    message,
                    diagnostic,
                )
                .await
            }
            CodexAccountFailure::Banned => {
                self.apply_credential_state(
                    account,
                    CredentialState::Banned,
                    AccountErrorReason::AccountBanned,
                    now,
                    message,
                    diagnostic,
                )
                .await
            }
            CodexAccountFailure::QuotaExhausted => self
                .quota
                .record_confirmed_exhaustion(account, QuotaEvidence::PaymentRequired, None, now)
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into()))),
            CodexAccountFailure::UsageLimitExhausted { reset_at } => self
                .quota
                .record_confirmed_exhaustion(
                    account,
                    QuotaEvidence::UsageLimitReached,
                    reset_at,
                    now,
                )
                .await
                .map_err(|source| CredentialSelectionError::Store(Some(source.into()))),
            // 429：临时限流只写运行时冷却，不改变凭据或额度事实。
            CodexAccountFailure::RateLimited { retry_after } => {
                self.quota
                    .apply_rate_limit_429(account, retry_after, now)
                    .await
                    .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?;
                Ok(())
            }
            // Cloudflare 挑战：内存退避表（记录风险计数），不写账号事实。
            CodexAccountFailure::CloudflareChallenge { retry_after } => {
                let delay = self.cloudflare_challenge_delay(account.id(), now, retry_after);
                let recovery = now
                    .checked_add(delay)
                    .map_or(CookieRecovery::Clear, CookieRecovery::ExpireAt);
                self.apply_cookie_recovery(account, recovery).await?;
                Ok(())
            }
            // Cloudflare 路径被封：连续超阈值才置 Invalid；否则内存退避。
            CodexAccountFailure::CloudflarePathBlocked => {
                let blocked = self.record_cloudflare_path_block(account.id(), now);
                if blocked >= CLOUDFLARE_PATH_BLOCK_THRESHOLD {
                    self.apply_credential_state(
                        account,
                        CredentialState::Invalid,
                        AccountErrorReason::CredentialInvalid,
                        now,
                        message,
                        diagnostic,
                    )
                    .await?;
                }
                self.apply_cookie_recovery(account, CookieRecovery::Clear)
                    .await?;
                Ok(())
            }
        }
    }

    async fn apply_credential_state(
        &self,
        account: &ProviderAccount,
        credential_state: CredentialState,
        error_reason: AccountErrorReason,
        observed_at: SystemTime,
        message: Option<String>,
        diagnostic: bool,
    ) -> Result<(), CredentialSelectionError> {
        if diagnostic {
            self.repository
                .apply_diagnostic_state_with_reason(
                    account,
                    credential_state,
                    observed_at,
                    Some(error_reason),
                    message,
                )
                .await?;
        } else {
            self.repository
                .apply_state_with_reason(
                    account,
                    credential_state,
                    observed_at,
                    Some(error_reason),
                    message,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn record_success(
        &self,
        account: &ProviderAccount,
        request_started_at: SystemTime,
        session_affinity_key: Option<&ProviderSessionAffinityKey>,
        expected_affinity_account_id: &ProviderAccountId,
    ) {
        self.record_success_with_parent_binding(
            account,
            request_started_at,
            session_affinity_key,
            &ProviderSessionAffinityBinding::legacy(expected_affinity_account_id.clone()),
            None,
            CODEX_ROOT_SESSION_TTL,
        )
        .await;
    }

    pub(crate) async fn record_success_with_parent_binding(
        &self,
        account: &ProviderAccount,
        request_started_at: SystemTime,
        _session_affinity_key: Option<&ProviderSessionAffinityKey>,
        _expected_affinity_binding: &ProviderSessionAffinityBinding,
        guardian_parent_record_key: Option<&ProviderSessionAffinityKey>,
        session_binding_ttl: Duration,
    ) {
        if self.excel_auth_block(account).is_none() {
            self.restore_recoverable_account_state(account, request_started_at, false)
                .await;
        }
        if account.credential_route() == ResponsesUpstream::Codex {
            self.risk_recovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(account.id().as_str());
        }
        if let Some(parent_key) = guardian_parent_record_key {
            match tokio::time::timeout(
                SESSION_AFFINITY_TIMEOUT,
                self.session_affinity.bind(
                    &self.provider_kind,
                    parent_key,
                    account.id(),
                    session_binding_ttl,
                ),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "OpenAI Guardian parent affinity write failed open");
                }
                Err(_) => {
                    tracing::warn!(
                        timeout_ms = SESSION_AFFINITY_TIMEOUT.as_millis(),
                        "OpenAI Guardian parent affinity write timed out"
                    );
                }
            }
        }
    }

    pub async fn record_diagnostic_success(
        &self,
        account: &ProviderAccount,
        request_started_at: SystemTime,
    ) {
        let block = self.excel_auth_block(account);
        self.restore_recoverable_account_state(account, request_started_at, true)
            .await;
        if let Some(block) = block
            && let Ok(Some(current)) = self.repository.current_selected_account(account).await
            && current.revision() == account.revision()
            && current.credential_revision() == account.credential_revision()
            && current.credential_state() == CredentialState::Ready
        {
            self.clear_excel_auth_block(account, block);
        }
        if account.credential_route() == ResponsesUpstream::Codex {
            self.risk_recovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(account.id().as_str());
        }
    }

    async fn restore_recoverable_account_state(
        &self,
        account: &ProviderAccount,
        request_started_at: SystemTime,
        allow_disabled: bool,
    ) {
        let Ok(Some(current)) = self.repository.current_selected_account(account).await else {
            return;
        };
        if current.provider() != &self.provider_kind
            || current.revision() != account.revision()
            || current.credential_revision() != account.credential_revision()
            || (!allow_disabled && !current.enabled())
            || (!allow_disabled && self.excel_auth_block(&current).is_some())
        {
            return;
        }
        // A successful response only proves the credential worked when this attempt
        // started. The store atomically rejects recovery over a later auth failure,
        // even if that failure arrives after the account reload above.
        let observed_at = request_started_at;
        if (allow_disabled || current.credential_state() != CredentialState::Ready)
            && let Err(error) = if allow_disabled {
                self.repository
                    .apply_diagnostic_state_with_reason(
                        &current,
                        CredentialState::Ready,
                        observed_at,
                        None,
                        None,
                    )
                    .await
            } else {
                self.repository
                    .apply_state(&current, CredentialState::Ready, observed_at)
                    .await
            }
        {
            if matches!(error, CredentialRepositoryError::RevisionConflict(_)) {
                tracing::debug!(
                    account_id = %account.id(),
                    "Skipped OpenAI credential recovery after a newer account observation"
                );
            } else {
                tracing::warn!(
                    account_id = %account.id(),
                    error = %error,
                    "OpenAI credential recovery after successful upstream response failed"
                );
            }
        }
        if current.credential_route() == ResponsesUpstream::Excel {
            let _ = self
                .record_excel_quota(
                    &current,
                    gateway_core::account::QuotaState::allowed(request_started_at),
                    request_started_at,
                )
                .await;
            return;
        }
        if let Err(error) = self
            .quota
            .record_successful_inference(&current, SystemTime::now())
            .await
        {
            tracing::warn!(
                account_id = %account.id(),
                error = %error,
                "OpenAI quota recovery after successful upstream response failed"
            );
        }
    }

    pub async fn current_account(
        &self,
        account_id: &ProviderAccountId,
    ) -> Result<ProviderAccount, CredentialSelectionError> {
        self.repository
            .store()
            .get_account(account_id)
            .await
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))?
            .ok_or(CredentialSelectionError::InvalidCredential(None))
    }

    pub(crate) async fn apply_excel_403_action(
        &self,
        account: &ProviderAccount,
        model: Option<&str>,
    ) -> Result<bool, CredentialSelectionError> {
        self.repository
            .store()
            .apply_excel_403_action_for_model(account, model.unwrap_or_default())
            .await
            .map_err(|source| CredentialSelectionError::Store(Some(source.into())))
    }

    pub async fn capture_response_cookies(
        &self,
        account: &ProviderAccount,
        response_origin: &Url,
        headers: &[String],
    ) -> Result<CodexCookieCaptureOutcome, CredentialSelectionError> {
        if account.credential_route() != ResponsesUpstream::Codex {
            return Ok(CodexCookieCaptureOutcome {
                credential_revision: None,
                rejected: 0,
            });
        }
        let parsed = self.cookie_policy.parse_response_headers(
            account.id().as_str(),
            account.revision().get(),
            response_origin,
            headers,
            chrono::Utc::now(),
        );
        if parsed.inputs.is_empty() {
            return Ok(CodexCookieCaptureOutcome {
                credential_revision: None,
                rejected: parsed.rejected,
            });
        }
        let mut data = self.repository.load_complete_data(account).await?;
        let cookies = data.cookies_mut();
        let previous = cookies.clone();
        for input in parsed.inputs {
            let scope = self.cookie_policy.validate_capture(
                &input.response_origin,
                input.domain_attribute.as_deref(),
                &input.name,
                &input.path,
            )?;
            let position = cookies.iter().position(|cookie| {
                cookie.name == input.name
                    && cookie.domain == scope.domain
                    && cookie.path == input.path
            });
            cookies.retain(|cookie| {
                !(cookie.name == input.name
                    && cookie.domain == scope.domain
                    && cookie.path == input.path)
            });
            if !input.delete {
                let cookie = CodexCookie {
                    name: input.name,
                    value: input.value.expose_secret().to_owned(),
                    domain: scope.domain,
                    path: input.path,
                    host_only: scope.host_only,
                    secure: input.secure,
                    expires_at: input.expires_at,
                };
                cookies.insert(position.unwrap_or(cookies.len()), cookie);
            }
        }
        if *cookies == previous {
            return Ok(CodexCookieCaptureOutcome {
                credential_revision: None,
                rejected: parsed.rejected,
            });
        }
        let revision = self.repository.compare_and_swap_data(account, data).await?;
        Ok(CodexCookieCaptureOutcome {
            credential_revision: Some(revision.get()),
            rejected: parsed.rejected,
        })
    }

    fn cloudflare_challenge_delay(
        &self,
        account_id: &ProviderAccountId,
        now: SystemTime,
        retry_after: Option<Duration>,
    ) -> Duration {
        let mut recovery = self
            .risk_recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = active_risk_recovery(&mut recovery, account_id.as_str(), now);
        state.challenge_count = state.challenge_count.saturating_add(1);
        state.observed_at = now;
        let index = usize::try_from(state.challenge_count.saturating_sub(1))
            .unwrap_or(usize::MAX)
            .min(CLOUDFLARE_CHALLENGE_BACKOFF.len() - 1);
        retry_after
            .unwrap_or_default()
            .max(CLOUDFLARE_CHALLENGE_BACKOFF[index])
    }

    fn record_cloudflare_path_block(&self, account_id: &ProviderAccountId, now: SystemTime) -> u32 {
        let mut recovery = self
            .risk_recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = active_risk_recovery(&mut recovery, account_id.as_str(), now);
        state.path_block_count = state.path_block_count.saturating_add(1);
        state.observed_at = now;
        state.path_block_count
    }

    async fn apply_cookie_recovery(
        &self,
        account: &ProviderAccount,
        recovery: CookieRecovery,
    ) -> Result<(), CredentialSelectionError> {
        let mut data = self.repository.load_complete_data(account).await?;
        if data.cookies().is_empty() {
            return Ok(());
        }
        match recovery {
            CookieRecovery::ExpireAt(expires_at) => {
                let expires_at = chrono::DateTime::<chrono::Utc>::from(expires_at);
                for cookie in data.cookies_mut() {
                    cookie.expires_at = Some(
                        cookie
                            .expires_at
                            .map_or(expires_at, |current| current.min(expires_at)),
                    );
                }
            }
            CookieRecovery::Clear => data.cookies_mut().clear(),
        }
        self.repository.compare_and_swap_data(account, data).await?;
        Ok(())
    }
}

fn affinity_selection_for_bound_account(
    binding: ProviderSessionAffinityBinding,
    candidates: &[AccountCandidate],
    now: SystemTime,
    allow_codex_quota_overdraft: bool,
) -> AffinitySelection {
    let account_id = binding.account_id().clone();
    let Some(candidate) = candidates
        .iter()
        .find(|candidate| candidate.account.id() == &account_id)
    else {
        return AffinitySelection::escaped(binding, AffinityEscapeReason::HardUnavailable);
    };
    let status = candidate
        .account
        .status_projection(now, candidate.signals.rate_limited_until)
        .status;
    let status = if status == AccountStatus::QuotaExhausted
        && allow_codex_quota_overdraft
        && candidate.account.codex_quota_overdraft_passed()
        && candidate
            .signals
            .rate_limited_until
            .is_none_or(|until| until <= now)
    {
        AccountStatus::Normal
    } else {
        status
    };
    match status {
        AccountStatus::Normal => AffinitySelection::preferred(binding),
        AccountStatus::QuotaPaused
        | AccountStatus::QuotaExhausted
        | AccountStatus::RateLimited
        | AccountStatus::Disabled
        | AccountStatus::Error => {
            AffinitySelection::escaped(binding, affinity_unavailable_reason(candidate, now))
        }
    }
}

fn affinity_unavailable_reason(
    candidate: &AccountCandidate,
    now: SystemTime,
) -> AffinityEscapeReason {
    match candidate
        .account
        .status_projection(now, candidate.signals.rate_limited_until)
        .status
    {
        AccountStatus::QuotaExhausted => AffinityEscapeReason::QuotaExhausted,
        AccountStatus::RateLimited | AccountStatus::QuotaPaused => AffinityEscapeReason::Cooldown,
        AccountStatus::Normal => AffinityEscapeReason::SelectionInvariant,
        AccountStatus::Disabled | AccountStatus::Error => AffinityEscapeReason::HardUnavailable,
    }
}

fn active_risk_recovery<'a>(
    recovery: &'a mut HashMap<String, RiskRecoveryState>,
    account_id: &str,
    now: SystemTime,
) -> &'a mut RiskRecoveryState {
    recovery.retain(|_, state| match now.duration_since(state.observed_at) {
        Ok(elapsed) => elapsed <= CLOUDFLARE_RECOVERY_STALE_AFTER,
        Err(_) => true,
    });
    recovery
        .entry(account_id.to_owned())
        .or_insert(RiskRecoveryState {
            challenge_count: 0,
            path_block_count: 0,
            observed_at: now,
        })
}

impl fmt::Debug for CodexCredentialSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexCredentialSelector")
            .field("repository", &"ProviderAccountStore")
            .field("leases", &"ProviderLeasePort")
            .field("quota", &"CodexCredentialQuotaService")
            .field("cookie_policy", &self.cookie_policy)
            .finish()
    }
}

pub struct CodexCredentialLease {
    selected_route: ResponsesUpstream,
    session_binding_ttl: Duration,
    account: ProviderAccount,
    authentication: CodexRuntimeAuthentication,
    cookies: Vec<RuntimeCodexCookie>,
    installation_id: String,
    cyber_policy_scope: Option<CodexCyberPolicyScope>,
    diagnostic: bool,
    allows_account_state_mutation: bool,
    affinity_telemetry: AffinityTelemetry,
    affinity_expected_binding: ProviderSessionAffinityBinding,
    guardian_parent_record_key: Option<ProviderSessionAffinityKey>,
    capacity: Option<AccountCapacitySnapshot>,
    _guard: Box<dyn ProviderLeaseGuard>,
}

impl CodexCredentialLease {
    #[must_use]
    pub const fn selected_route(&self) -> ResponsesUpstream {
        self.selected_route
    }

    pub(crate) const fn session_binding_ttl(&self) -> Duration {
        self.session_binding_ttl
    }

    #[must_use]
    pub const fn account(&self) -> &ProviderAccount {
        &self.account
    }

    #[must_use]
    pub const fn account_id(&self) -> &ProviderAccountId {
        self.account.id()
    }

    #[must_use]
    pub const fn authentication(&self) -> &CodexRuntimeAuthentication {
        &self.authentication
    }

    #[must_use]
    pub fn cookies(&self) -> &[RuntimeCodexCookie] {
        &self.cookies
    }

    #[must_use]
    pub fn installation_id(&self) -> &str {
        &self.installation_id
    }

    #[must_use]
    pub(crate) const fn cyber_policy_scope(&self) -> Option<&CodexCyberPolicyScope> {
        self.cyber_policy_scope.as_ref()
    }

    /// 管理端诊断会回写账号级健康事实，但不会修改 `enabled`。
    #[must_use]
    pub(crate) const fn allows_account_state_mutation(&self) -> bool {
        self.allows_account_state_mutation
    }

    #[must_use]
    pub(crate) const fn is_diagnostic(&self) -> bool {
        self.diagnostic
    }

    #[must_use]
    pub const fn affinity_hit(&self) -> bool {
        self.affinity_telemetry.affinity_hit
    }

    #[must_use]
    pub const fn escape_reason(&self) -> Option<&'static str> {
        match self.affinity_telemetry.escape_reason {
            Some(reason) => Some(reason.as_str()),
            None => None,
        }
    }

    #[must_use]
    pub const fn account_switch(&self) -> bool {
        self.affinity_telemetry.account_switch
    }

    #[must_use]
    pub const fn capacity_snapshot(&self) -> Option<AccountCapacitySnapshot> {
        self.capacity
    }

    /// 成功反馈只能从选择时观察到的绑定迁移，避免迟到请求覆盖较新 winner。
    #[must_use]
    pub(crate) const fn affinity_expected_binding(&self) -> &ProviderSessionAffinityBinding {
        &self.affinity_expected_binding
    }

    #[must_use]
    pub(crate) const fn guardian_parent_record_key(&self) -> Option<&ProviderSessionAffinityKey> {
        self.guardian_parent_record_key.as_ref()
    }
}

impl fmt::Debug for CodexCredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexCredentialLease")
            .field("account", &self.account)
            .field("authentication", &"<redacted>")
            .field("cookies", &self.cookies)
            .field("installation_id", &"<pseudonymous>")
            .finish()
    }
}

fn binding_store_error(
    source: impl std::error::Error + Send + Sync + 'static,
) -> CredentialSelectionError {
    CredentialSelectionError::SessionBound(Box::new(CredentialSelectionError::Store(Some(
        gateway_core::error::ErrorSource::new(source),
    ))))
}

fn binding_unavailable() -> CredentialSelectionError {
    CredentialSelectionError::SessionBound(Box::new(CredentialSelectionError::Coordinator(None)))
}

#[derive(Debug, Error)]
pub enum CredentialSelectionError {
    #[error("session account binding cannot be bypassed: {0}")]
    SessionBound(#[source] Box<CredentialSelectionError>),
    #[error("native continuation owner no longer matches the session binding")]
    ContinuationOwnerChanged,
    #[error("Codex account wait unavailable: {0}")]
    AccountWait(AccountWaitFailure),
    #[error("Codex account selection cancelled")]
    Cancelled,
    #[error("Codex pinned account selection failed: {source}")]
    PinnedAccount {
        source: Box<CredentialSelectionError>,
        owner_lost: bool,
    },
    #[error("no eligible Codex account")]
    NoEligibleCredential,
    #[error("upstream safety policy temporarily excludes accounts for this session")]
    SessionPolicyRestricted,
    #[error("account quota policy prevents scheduling for this session")]
    QuotaPolicyFiltered,
    #[error("all eligible Codex accounts have exhausted their quota")]
    QuotaExhausted,
    #[error("Codex account capacity is unavailable")]
    CapacityUnavailable { retry_after: Option<Duration> },
    #[error("Codex account data is invalid")]
    InvalidCredential(#[source] Option<gateway_core::error::ErrorSource>),
    #[error("Codex account changed repeatedly during selection")]
    AccountSnapshotChanged,
    #[error("account admission changed")]
    AccountAdmissionChanged,
    #[error("Codex account store is unavailable")]
    Store(#[source] Option<gateway_core::error::ErrorSource>),
    #[error("Codex account lease runtime is unavailable")]
    Coordinator(#[source] Option<gateway_core::error::ErrorSource>),
    #[error("Codex Cookie policy rejected the value")]
    CookiePolicy(#[source] Option<gateway_core::error::ErrorSource>),
}

impl From<CredentialRepositoryError> for CredentialSelectionError {
    fn from(error: CredentialRepositoryError) -> Self {
        match &error {
            CredentialRepositoryError::InvalidCredentialData(_) => {
                Self::InvalidCredential(Some(error.into()))
            }
            CredentialRepositoryError::RevisionConflict(_) => Self::AccountSnapshotChanged,
            CredentialRepositoryError::Store(_) => Self::Store(Some(error.into())),
        }
    }
}

impl From<ProviderStoreError> for CredentialSelectionError {
    fn from(error: ProviderStoreError) -> Self {
        Self::Coordinator(Some(error.into()))
    }
}

impl From<super::cookie::CookiePolicyError> for CredentialSelectionError {
    fn from(error: super::cookie::CookiePolicyError) -> Self {
        Self::CookiePolicy(Some(error.into()))
    }
}

fn minimum_duration(current: Option<Duration>, candidate: Option<Duration>) -> Option<Duration> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.min(candidate)),
        (Some(current), None) => Some(current),
        (None, candidate) => candidate,
    }
}

fn quality_selection_signals(
    mut signals: AccountRuntimeSignals,
    quality: bool,
) -> AccountRuntimeSignals {
    if quality {
        // Admin quality work must not be rejected because ordinary traffic
        // already filled the account's business concurrency.
        signals.in_flight = 0;
    }
    signals
}

fn quality_concurrency_limit(ordinary: NonZeroU32, quality: bool) -> NonZeroU32 {
    if quality { NonZeroU32::MAX } else { ordinary }
}

fn quality_interval_delay(
    quality: bool,
    interval_deadline: Option<SystemTime>,
    now: SystemTime,
) -> Option<Duration> {
    if !quality {
        return None;
    }
    interval_deadline.map(|deadline| {
        deadline
            .duration_since(now)
            .unwrap_or_default()
            .max(Duration::from_millis(1))
    })
}

impl From<gateway_core::concurrency::QueueRejection> for CredentialSelectionError {
    fn from(reason: gateway_core::concurrency::QueueRejection) -> Self {
        Self::AccountWait(match reason {
            gateway_core::concurrency::QueueRejection::Full => AccountWaitFailure::QueueFull,
            gateway_core::concurrency::QueueRejection::Timeout => AccountWaitFailure::Timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_interval_wait_is_scoped_and_handles_clock_boundaries() {
        let now = SystemTime::now();
        let deadline = now + Duration::from_millis(250);
        assert_eq!(quality_interval_delay(false, Some(deadline), now), None);
        assert_eq!(quality_interval_delay(true, None, now), None);
        assert_eq!(
            quality_interval_delay(true, Some(deadline), now),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            quality_interval_delay(true, Some(now), now),
            Some(Duration::from_millis(1))
        );
        assert_eq!(
            quality_interval_delay(true, Some(now - Duration::from_millis(1)), now),
            Some(Duration::from_millis(1))
        );
    }

    #[test]
    fn quality_selection_bypasses_only_business_concurrency() {
        let last_started_at = SystemTime::now();
        let ordinary = AccountRuntimeSignals {
            in_flight: 7,
            last_started_at: Some(last_started_at),
            quota_reset_at: None,
            quota_reset_windows: None,
            quota_remaining_rank: None,
            rate_limited_until: None,
            failure_rate_basis_points: None,
            first_output_latency_ms: None,
        };
        let quality = quality_selection_signals(ordinary.clone(), true);
        assert_eq!(quality.in_flight, 0);
        assert_eq!(quality.last_started_at, Some(last_started_at));
        assert_eq!(
            quality_concurrency_limit(NonZeroU32::new(2).unwrap(), true),
            NonZeroU32::MAX
        );
        assert_eq!(quality_selection_signals(ordinary.clone(), false), ordinary);
        assert_eq!(
            quality_concurrency_limit(NonZeroU32::new(2).unwrap(), false).get(),
            2
        );
    }
}
