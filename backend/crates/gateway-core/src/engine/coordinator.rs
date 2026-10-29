//! 唯一的账号重试、发送与下游 commit barrier owner。

use crate::diagnostics::TraceContext;
use serde_json::json;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use super::client_retry::{ClientRetryGuard, ClientRetryKey};
use super::observation::ResponseObservation;

use crate::engine::continuation::{
    ContinuationBinding, NativeContinuationPin, NativeContinuationScope, PreviousResponseId,
};

use crate::engine::provider::{Provider, ProviderCallMetadata, ProviderRequest, ProviderStream};
use crate::engine::{
    AccountAttemptContext, AttemptContext, AttemptRecord, AttemptTransport, AttemptTrigger,
    CommitRequirement, ContinuationAttempt, CoordinatedEvent, EngineError, ExecutionOutcome,
    ExecutionStore, GatewayEngine, IntermediateFailure, ModelRequestFailureObservation,
    ModelRequestFinalization, ModelRequestId, NewModelRequest, ProviderAccountStateOwner,
    ProviderAttemptOutcome, RequestAttemptContext, UpstreamSendState,
};
use crate::error::{
    ContinuationRecoveryDisposition, GatewayError, GatewayErrorKind, ProviderError,
    ProviderErrorKind, StoreError,
};
use crate::event::{
    GatewayEvent, ProviderEvent, ProviderResponseHeader, ProviderResponseObservation,
};
use crate::lifecycle::{CancellationToken, Deadline, LeaseGuard};
use crate::metering::Decimal;
use crate::operation::{Operation, ProviderSessionState};
use crate::routing::RoutingPlan;
use futures::future::{BoxFuture, Fuse};
use futures::{FutureExt, StreamExt, pin_mut, select_biased};
use futures_timer::Delay;

fn is_return_error_retry(error: &ProviderError) -> bool {
    matches!(
        error.pre_delivery_retry(),
        Some(crate::error::PreDeliveryRetry::SameAccountTransientRetry {
            policy: crate::error::TransientRetryPolicy::RetryThenReturnError,
            ..
        })
    )
}

/// Request 级协调器；不会创建或写入 `request_attempts`。
pub struct AttemptCoordinator<S: ?Sized> {
    engine: Arc<GatewayEngine<S>>,
}

#[derive(Debug, Clone)]
enum AccountSelection {
    Scheduled(Option<crate::account::ProviderAccountId>),
    Quality(crate::account::ProviderAccountId, bool),
    Diagnostic(crate::account::ProviderAccountId),
}

impl AccountSelection {
    fn required_account(&self) -> Option<&crate::account::ProviderAccountId> {
        match self {
            Self::Scheduled(account) => account.as_ref(),
            Self::Diagnostic(account) | Self::Quality(account, _) => Some(account),
        }
    }

    const fn is_diagnostic(&self) -> bool {
        matches!(self, Self::Diagnostic(_))
    }
}

impl<S: ?Sized> AttemptCoordinator<S>
where
    S: ExecutionStore + 'static,
{
    #[must_use]
    pub fn new(engine: GatewayEngine<S>) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    /// 接纳请求并返回由 Core 完整拥有 retry/commit 的流式会话。
    ///
    /// # Errors
    ///
    /// 取消、已过 deadline 或 continuation 与路由不匹配时返回稳定错误。
    pub async fn start(
        &self,
        request: NewModelRequest,
        operation: Operation,
        plan: RoutingPlan,
        required_account: Option<crate::account::ProviderAccountId>,
        continuation: Option<ContinuationBinding>,
        cancellation: CancellationToken,
    ) -> Result<ResponseExecutionSession<S>, EngineError> {
        self.start_with_account_selection(
            request,
            operation,
            plan,
            AccountSelection::Scheduled(required_account),
            continuation,
            cancellation,
        )
        .await
    }

    /// 对固定账号执行诊断请求。
    ///
    /// 仅跳过该账号的本地可用性投影；账号租约、并发和请求间隔仍必须满足。
    pub async fn start_diagnostic(
        &self,
        request: NewModelRequest,
        operation: Operation,
        plan: RoutingPlan,
        required_account: crate::account::ProviderAccountId,
        continuation: Option<ContinuationBinding>,
        cancellation: CancellationToken,
    ) -> Result<ResponseExecutionSession<S>, EngineError> {
        self.start_with_account_selection(
            request,
            operation,
            plan,
            AccountSelection::Diagnostic(required_account),
            continuation,
            cancellation,
        )
        .await
    }

    pub(crate) async fn start_quality(
        &self,
        request: NewModelRequest,
        operation: Operation,
        plan: RoutingPlan,
        account: crate::account::ProviderAccountId,
        cancellation: CancellationToken,
        retest: bool,
    ) -> Result<ResponseExecutionSession<S>, EngineError> {
        self.start_with_account_selection(
            request,
            operation,
            plan,
            AccountSelection::Quality(account, retest),
            None,
            cancellation,
        )
        .await
    }

    async fn start_with_account_selection(
        &self,
        request: NewModelRequest,
        operation: Operation,
        plan: RoutingPlan,
        account_selection: AccountSelection,
        continuation: Option<ContinuationBinding>,
        cancellation: CancellationToken,
    ) -> Result<ResponseExecutionSession<S>, EngineError> {
        let request_id = request.id.clone();
        let client_api_key_ref = request.client_api_key_ref.clone();
        let timing_started_at = Instant::now();
        let deadline = request.deadline_at;
        let account_state_owner = continuation
            .as_ref()
            .and_then(ContinuationBinding::pinned)
            .map(ProviderAccountStateOwner::from_continuation);
        let continuation_attempt =
            initial_continuation_attempt(&operation, &plan, continuation.as_ref());
        let candidate_index = continuation
            .as_ref()
            .and_then(ContinuationBinding::pinned)
            .map_or(Ok(0), |pin| {
                plan.candidates()
                    .iter()
                    .position(|candidate| candidate.provider() == pin.provider())
                    .ok_or(EngineError::ContinuationPinMismatch)
            })?;
        // Only trusted internal selection can opt out; client metadata cannot hide captures.
        let capture = self.engine.captures.as_ref().and_then(|factory| {
            if matches!(account_selection, AccountSelection::Quality(..)) {
                return None;
            }
            factory.start(
                request_id.as_str(),
                client_api_key_ref.as_str(),
                &request
                    .routing
                    .groups_snapshot()
                    .iter()
                    .map(|group| group.id().as_str())
                    .collect::<Vec<_>>(),
            )
        });
        let trace = if matches!(account_selection, AccountSelection::Quality(..)) {
            TraceContext::audit_only(request_id.as_str())
        } else {
            TraceContext::new(request_id.as_str())
        }
        .with_capture(capture);
        trace.record(
            "request.routed",
            json!({
                "endpoint": request.endpoint, "protocol": request.protocol,
                "clientTransport": request.client_transport,
                "model": request.requested_model.as_ref().map(|model| model.as_str()),
                "configRevision": format!("{:?}", request.config_revision),
                "admissionMs": request.admission_decision_ms,
                "continuation": format!("{continuation_attempt:?}"),
                "candidateCount": plan.candidates().len(),
                "accountPolicy": {
                    "strategy": plan.account_selection_policy().strategy().as_str(),
                    "affinity": plan.account_selection_policy().openai_account_affinity().as_str(),
                    "maxAccountRotations": plan.request_tuning().max_account_switches,
                    "maxRoutingAttempts": plan.max_attempts().get(),
                    "maxConcurrentPerAccount": plan.account_selection_policy().max_concurrent_per_account().get(),
                    "accountBusyWaitEnabled": plan.request_tuning().account_busy_wait_enabled,
                    "stickyMaxWaiting": plan.request_tuning().account_busy_wait_sticky_max_waiting,
                    "stickyTimeoutSeconds": plan.request_tuning().account_busy_wait_sticky_timeout_seconds,
                    "fallbackMaxWaiting": plan.request_tuning().account_busy_wait_fallback_max_waiting,
                    "fallbackTimeoutSeconds": plan.request_tuning().account_busy_wait_fallback_timeout_seconds,
                    "requestIntervalMs": plan.account_selection_policy().request_interval().as_millis(),
                    "sessionAffinityTtlMs": plan.account_selection_policy().openai_session_binding_ttl().as_millis(),
                    "smartWeights": plan.account_selection_policy().smart_scheduling().weights(),
                    "preferHigherWeight": plan.account_selection_policy().smart_scheduling().prefer_higher_weight(),
                },
                "requiredAccount": account_selection.required_account().map(|account| account.as_str()),
            }),
        );
        let image_generation_requested = operation.image_generation_requested();
        let mut session = ResponseExecutionSession {
            response_control: super::response_control::ResponseControl::default(),
            engine: Arc::clone(&self.engine),
            request_id,
            client_api_key_ref,
            client_ip: request.client_ip,
            observation: ResponseObservation::new(timing_started_at),
            budget_prior_attempts_usd: Decimal::ZERO,
            budget_attempt_already_counted: false,
            response_commit: super::execution::ResponseCommit::new(trace.clone()),
            trace,
            deadline,
            account_wait_budget: Arc::new(super::AccountWaitBudget::new(
                deadline,
                plan.request_tuning(),
            )),
            deadline_timer: deadline.wait().fuse(),
            lease: Some(self.engine.store.maintain_request(&request.id, deadline)),
            pending_request: Some(request),
            request_persisted: false,
            operation,
            plan,
            account_selection,
            continuation,
            continuation_attempt,
            account_state_owner,
            cancellation,
            attempts: 0,
            routing_attempts: 0,
            account_switches: 0,
            last_selected_account: None,
            candidate_index,
            excluded_accounts: BTreeSet::new(),
            credential_recovery_attempted_accounts: BTreeSet::new(),
            recovery_account: None,
            pending_retry: None,
            transient_retry_counts: BTreeMap::new(),
            connection_budget: super::connection::ConnectionBudget::default(),
            connection_retries: 0,
            connection_rotation: None,
            request_profiles: BTreeMap::new(),
            provider_routes: BTreeMap::new(),
            current: None,
            send_state_watermark: UpstreamSendState::NotSent,
            downstream_committed_at: None,
            replay_barrier_reached: false,
            client_status_code: None,
            delivery_pending: false,
            delivery_has_terminal_fact: false,
            upstream_complete: false,
            finalization: None,
            finalized_at: None,
            image_generation_requested,
            last_retryable_failure: None,
            last_retryable_failure_events: Vec::new(),
            provider_attempt_outcomes: Vec::new(),
            pending_terminal_failure: None,
            pending_stream_failure: None,
            client_retry: None,
        };

        if session.cancellation.is_cancelled() {
            session.finish_interruption(&EngineError::Cancelled).await?;
            return Err(EngineError::Cancelled);
        }
        if deadline.is_elapsed() {
            session.finish_interruption(&EngineError::Deadline).await?;
            return Err(EngineError::Deadline);
        }
        if matches!(session.account_selection, AccountSelection::Scheduled(_))
            && let Some(store) = &self.engine.client_retry
            && let Operation::Generate(generate) = &session.operation
            && let Some(turn) = generate.client_turn()
        {
            let guard = ClientRetryGuard::load(
                store.clone(),
                ClientRetryKey {
                    client_key_id: session.client_api_key_ref.clone(),
                    turn: turn.clone(),
                },
                session.request_id.clone(),
            )
            .await;
            session.trace.record(
                "client_retry.loaded",
                json!({"previousFailure":guard.has_failure()}),
            );
            session.client_retry = Some(guard);
        }
        Ok(session)
    }
}

struct CurrentAttempt {
    stream: ProviderStream,
    metadata: ProviderCallMetadata,
    trigger: AttemptTrigger,
    transport: AttemptTransport,
    connection_rotation_reserved: bool,
    index: NonZeroU32,
    started_at: SystemTime,
    send_observed: bool,
    response_observation: Option<ProviderResponseObservation>,
}

impl CurrentAttempt {
    fn upstream_request_id(&self) -> Option<&str> {
        self.response_observation
            .as_ref()
            .and_then(ProviderResponseObservation::request_id)
            .or_else(|| self.metadata.upstream_request_id())
            .map(|id| id.as_str())
    }
}

struct FailureFinalization {
    outcome: ExecutionOutcome,
    send_state: UpstreamSendState,
    mark_send_state: bool,
    error: GatewayError,
    upstream_status_code: Option<u16>,
    upstream_request_id: Option<String>,
    provider_error_code: Option<String>,
    raw_upstream_error: Option<String>,
    error_details: Option<String>,
    retry_after_ms: Option<u64>,
    observation: ModelRequestFailureObservation,
}

#[derive(Debug, Clone)]
struct PendingAttemptRetry {
    account: crate::account::ProviderAccountId,
    transport: AttemptTransport,
    delay: Duration,
    transport_recovery: bool,
    reserve_connection_rotation: bool,
}

/// API 可逐事件消费的 Core 执行会话。
///
/// API 只能提交下游 delivery 边界；账号重试、断流终结与
/// `model_requests` 写回均留在本类型内。
pub struct ResponseExecutionSession<S: ?Sized> {
    client_ip: Option<std::net::IpAddr>,
    response_control: super::response_control::ResponseControl,
    engine: Arc<GatewayEngine<S>>,
    request_id: ModelRequestId,
    client_api_key_ref: crate::policy::ClientApiKeyId,
    observation: ResponseObservation,
    budget_prior_attempts_usd: Decimal,
    budget_attempt_already_counted: bool,
    trace: TraceContext,
    deadline: Deadline,
    lease: Option<Box<dyn LeaseGuard>>,
    account_wait_budget: Arc<super::AccountWaitBudget>,
    /// 会话级 deadline 计时器；deadline 固定，帧循环内复用而非逐事件新建。
    deadline_timer: Fuse<BoxFuture<'static, ()>>,
    pending_request: Option<NewModelRequest>,
    request_persisted: bool,
    operation: Operation,
    plan: RoutingPlan,
    account_selection: AccountSelection,
    continuation: Option<ContinuationBinding>,
    continuation_attempt: ContinuationAttempt,
    account_state_owner: Option<ProviderAccountStateOwner>,
    provider_routes: BTreeMap<crate::routing::ProviderKind, Arc<std::sync::OnceLock<String>>>,
    cancellation: CancellationToken,
    /// 所有实际上游 attempt 数；包含同账号传输重试，作为持久化序号。
    attempts: u32,
    /// 路由预算只统计正常选号/账号恢复，不被 Provider-owned 传输预算消耗。
    routing_attempts: u32,
    /// 本次请求实际从一个账号切换到另一个账号的次数。
    account_switches: u32,
    last_selected_account: Option<crate::account::ProviderAccountId>,
    candidate_index: usize,
    excluded_accounts: BTreeSet<crate::account::ProviderAccountId>,
    credential_recovery_attempted_accounts: BTreeSet<crate::account::ProviderAccountId>,
    /// 凭据恢复后的一次性同账号钉选；只约束紧随其后的 replay attempt，
    /// attempt 建立时即被消费，后续可重试错误仍可换号消耗剩余重试预算。
    /// 与 `required_account`（外部指定、贯穿整个请求）语义不同，不可合并。
    recovery_account: Option<crate::account::ProviderAccountId>,
    /// 提交前同账号退避/传输恢复共用的等待与钉选状态；业务重试消耗路由预算。
    pending_retry: Option<PendingAttemptRetry>,
    /// 请求内按账号累计的瞬时拒绝重试次数，不能跨请求污染账号健康状态。
    transient_retry_counts: BTreeMap<crate::account::ProviderAccountId, u32>,
    connection_budget: super::connection::ConnectionBudget,
    connection_retries: u32,
    connection_rotation: Option<PendingAttemptRetry>,
    /// 公共画像按 Provider 在首次进入时冻结；账号私有状态仍逐 attempt 绑定。
    request_profiles:
        BTreeMap<crate::routing::ProviderKind, Option<Arc<crate::account::OpaqueProviderData>>>,
    current: Option<CurrentAttempt>,
    /// 请求级发送状态水位；跨 attempt 单调不降，终态写回不得低于此档。
    send_state_watermark: UpstreamSendState,
    downstream_committed_at: Option<SystemTime>,
    response_commit: super::execution::ResponseCommit,
    replay_barrier_reached: bool,
    client_status_code: Option<u16>,
    delivery_pending: bool,
    delivery_has_terminal_fact: bool,
    upstream_complete: bool,
    finalization: Option<RequestFinalization>,
    finalized_at: Option<SystemTime>,
    image_generation_requested: bool,
    /// 最近一次为无感恢复而被丢弃的原始上游错误；只在后续空选路时成为终态。
    last_retryable_failure: Option<ProviderError>,
    /// 与 `last_retryable_failure` 同属一个 attempt 的原始失败批次。
    last_retryable_failure_events: Vec<ProviderEvent>,
    provider_attempt_outcomes: Vec<ProviderAttemptOutcome>,
    /// 原子失败批次已交给协议层、但尚待下游提交后收敛的原 Provider 错误。
    pending_terminal_failure: Option<PendingTerminalFailure>,
    pending_stream_failure: Option<PendingStreamFailure>,
    client_retry: Option<ClientRetryGuard>,
}

struct PendingTerminalFailure {
    error: Arc<ProviderError>,
    send_state: UpstreamSendState,
    events: Vec<ProviderEvent>,
}

struct PendingStreamFailure {
    completion: BoxFuture<'static, ()>,
    outcome: StreamErrorOutcome,
}

enum RequestFinalization {
    Pending(BoxFuture<'static, bool>),
    Complete,
}

impl<S: ?Sized> ResponseExecutionSession<S>
where
    S: ExecutionStore + 'static,
{
    pub(super) fn with_account_wait_budget(
        mut self,
        budget: Arc<super::AccountWaitBudget>,
    ) -> Self {
        self.account_wait_budget = budget;
        self
    }

    /// 当前请求共享的诊断上下文。
    pub fn trace(&self) -> TraceContext {
        self.trace.clone()
    }

    pub fn response_commit(&self) -> super::execution::ResponseCommit {
        self.response_commit.clone()
    }

    #[must_use]
    pub fn response_control(&self) -> super::response_control::ResponseControl {
        self.response_control.clone()
    }

    /// Internal probes consume canonical facts, not the protocol-only failure batch.
    pub(crate) fn pending_provider_error(&self) -> Option<&ProviderError> {
        self.pending_terminal_failure
            .as_ref()
            .map(|pending| pending.error.as_ref())
    }

    /// 读取下一条 canonical event；首条未提交事件会携带 commit 要求。
    ///
    /// # Errors
    ///
    /// 未提交上一条首事件、Provider 失败、取消或超时时返回错误；观测写入失败只记录告警。
    pub async fn next_event(&mut self) -> Result<Option<CoordinatedEvent>, EngineError> {
        self.resume_finalization().await;
        if self.delivery_pending {
            return Err(EngineError::DownstreamCommitRequired);
        }
        if self.is_finalized() {
            return Ok(None);
        }
        if self.pending_terminal_failure.is_some() {
            return self.next_terminal_failure().await;
        }
        loop {
            match self.pull().await? {
                PullOutcome::Event(event) => {
                    self.replay_barrier_reached |= !event.is_replayable_prelude();
                    if self.downstream_committed_at.is_some() {
                        return Ok(Some(CoordinatedEvent::single(
                            event,
                            CommitRequirement::AlreadyCommitted,
                        )));
                    }
                    self.delivery_pending = true;
                    self.delivery_has_terminal_fact = event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)));
                    return Ok(Some(CoordinatedEvent::single(
                        event,
                        CommitRequirement::CommitBeforeDelivery,
                    )));
                }
                PullOutcome::AttemptDiscarded => {}
                PullOutcome::TerminalFailure {
                    events,
                    error,
                    send_state,
                } => {
                    self.pending_terminal_failure = Some(PendingTerminalFailure {
                        error: Arc::new(error),
                        send_state,
                        events,
                    });
                    return self.next_terminal_failure().await;
                }
                PullOutcome::End => {
                    if self.downstream_committed_at.is_some() {
                        self.finish_success().await?;
                    }
                    return Ok(None);
                }
            }
        }
    }

    /// 非流式协议可在任何下游提交前收集一个完整、可丢弃重试的结果。
    ///
    /// # Errors
    ///
    /// 会话已提交、已有待提交结果或执行失败时返回错误。
    pub async fn collect_uncommitted(&mut self) -> Result<Vec<ProviderEvent>, EngineError> {
        self.resume_finalization().await;
        if self.downstream_committed_at.is_some() || self.delivery_pending {
            return Err(EngineError::InvalidDeliveryState);
        }
        if self.is_finalized() {
            return Ok(Vec::new());
        }

        let mut events = Vec::new();
        loop {
            match self.pull().await? {
                PullOutcome::Event(event) => events.push(event),
                PullOutcome::AttemptDiscarded => events.clear(),
                PullOutcome::TerminalFailure {
                    error, send_state, ..
                } => {
                    self.pending_terminal_failure = Some(PendingTerminalFailure {
                        error: Arc::new(error),
                        send_state,
                        events: Vec::new(),
                    });
                    return Err(provider_engine_error(
                        self.finalize_pending_terminal_failure().await?,
                    ));
                }
                PullOutcome::End => {
                    if events.is_empty() {
                        return Err(EngineError::InvalidDeliveryState);
                    }
                    self.delivery_pending = true;
                    return Ok(events);
                }
            }
        }
    }

    /// 协议 adapter 消费了无需输出的非终态事件，继续等待可交付结果。
    ///
    /// # Errors
    ///
    /// 仅允许尚未提交且存在待交付非终态事件的会话调用。
    pub fn defer_downstream_commit(&mut self) -> Result<(), EngineError> {
        if !self.delivery_pending
            || self.downstream_committed_at.is_some()
            || self.finalization.is_some()
            || self.upstream_complete
            || self.delivery_has_terminal_fact
            || self.pending_terminal_failure.is_some()
        {
            return Err(EngineError::InvalidDeliveryState);
        }
        self.delivery_pending = false;
        self.replay_barrier_reached = false;
        Ok(())
    }

    /// 在协议 adapter 真正写出首字节前记录下游不可撤回边界。
    ///
    /// # Errors
    ///
    /// 没有待提交结果或重复提交时返回错误；观测 Store 失败只记服务端告警。
    pub async fn commit_downstream(
        &mut self,
        client_status_code: Option<u16>,
    ) -> Result<(), EngineError> {
        if !self.delivery_pending || self.downstream_committed_at.is_some() || self.is_finalized() {
            return Err(EngineError::InvalidDeliveryState);
        }
        self.response_commit.record(client_status_code);
        let committed_at = self
            .response_commit
            .committed_at()
            .ok_or(EngineError::InvalidDeliveryState)?;
        let client_status_code = self.response_commit.status_code();
        if self.request_persisted {
            best_effort_store_write(
                "mark_downstream_committed",
                &self.request_id,
                self.engine.store().mark_downstream_committed(
                    &self.request_id,
                    committed_at,
                    client_status_code,
                ),
            )
            .await;
        }
        self.downstream_committed_at = Some(committed_at);
        self.client_status_code = client_status_code;
        self.delivery_pending = false;
        if self.upstream_complete {
            self.finish_success().await?;
        }
        Ok(())
    }

    /// 在 HTTP adapter 已确定首字节前错误响应后补写最终状态。
    ///
    /// # Errors
    ///
    /// 状态已经写入时返回错误；观测 Store 失败只记服务端告警。
    pub async fn record_client_status(
        &mut self,
        client_status_code: u16,
    ) -> Result<(), EngineError> {
        if self.client_status_code.is_some() {
            return Err(EngineError::InvalidDeliveryState);
        }
        let committed_status = self.response_commit.status_code();
        let client_status_code = committed_status.unwrap_or(client_status_code);
        if self.request_persisted && !(self.is_finalized() && committed_status.is_some()) {
            best_effort_store_write(
                "record_client_status",
                &self.request_id,
                self.engine
                    .store()
                    .record_client_status(&self.request_id, client_status_code),
            )
            .await;
        }
        self.client_status_code = Some(client_status_code);
        Ok(())
    }

    #[must_use]
    pub const fn is_finalized(&self) -> bool {
        matches!(self.finalization, Some(RequestFinalization::Complete))
    }

    #[must_use]
    pub fn budget_charge(&self) -> super::budget::ClientBudgetCharge {
        // 超出数据库可表示范围时保留最大金额，避免溢出后误记为零。
        let amount_usd = self
            .budget_prior_attempts_usd
            .checked_add(self.budget_attempt_usd())
            .unwrap_or(Decimal::MAX);
        super::budget::ClientBudgetCharge {
            key_id: self.client_api_key_ref.clone(),
            request_id: self.request_id.clone(),
            amount_usd,
            completed_at: self.finalized_at.unwrap_or_else(SystemTime::now),
        }
    }

    fn budget_attempt_usd(&self) -> Decimal {
        if self.budget_attempt_already_counted {
            return Decimal::ZERO;
        }
        // Key 只累计已取得的 USD 费用；缺少费用的请求按零结算。
        // 发送状态仍用于判断重放是否安全并保留在诊断中，不能据此推断存在欠费。
        self.observation
            .cost
            .total()
            .filter(|money| money.currency().as_str() == "USD")
            .map(crate::metering::Money::amount)
            .unwrap_or(Decimal::ZERO)
    }

    /// 返回截至当前已完成的实际上游调用结果。
    ///
    /// 调用方可保存已消费下标；该切片在会话生命周期内只追加、不改写。
    #[must_use]
    pub fn provider_attempt_outcomes(&self) -> &[ProviderAttemptOutcome] {
        &self.provider_attempt_outcomes
    }

    /// 返回最终选中 attempt 已公开给协议层的安全响应头。
    #[must_use]
    pub fn response_headers(&self) -> &[ProviderResponseHeader] {
        self.current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .map(ProviderResponseObservation::client_headers)
            .unwrap_or_default()
    }

    /// 返回最终选中 attempt 观察到的上游 HTTP 状态码。
    #[must_use]
    pub fn response_status_code(&self) -> Option<u16> {
        self.current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .and_then(ProviderResponseObservation::status_code)
    }

    /// 将已完成响应的账号事实与 Provider 私有状态封装为可丢失的亲和记录。
    ///
    /// Core 不读取 `state` 内容；Provider 将在后续同账号 continuation 时自行解释。
    #[must_use]
    pub fn native_continuation_pin(
        &self,
        state: &ProviderSessionState,
    ) -> Option<NativeContinuationPin> {
        let current = self.current.as_ref()?;
        let provider = current.metadata.provider().clone();
        if state.provider() != provider.as_str() {
            return None;
        }
        let account = current.metadata.provider_account_id().clone();
        let response_id = self.observation.upstream_response_id.as_deref()?;
        let previous_response_id = PreviousResponseId::new(response_id.to_owned());
        let upstream_response_id = PreviousResponseId::new(response_id.to_owned());
        Some(
            NativeContinuationPin::new(
                previous_response_id,
                upstream_response_id,
                self.client_api_key_ref.clone(),
                provider,
                account,
            )
            .with_scope(NativeContinuationScope::Persisted)
            .with_session_state(state.clone()),
        )
    }

    /// 请求取消；实际终态在下一次会话 poll 时由 Core 持久化。
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// 丢弃尚未提交的 delivery，并收敛为取消终态；已有终态写入继续保留原结果。
    ///
    /// 观测写入失败只记录告警，不改变取消结果。
    pub async fn cancel_and_finalize(&mut self) -> Result<(), EngineError> {
        self.cancellation.cancel();
        self.retain_pending_terminal_failure();
        self.resume_finalization().await;
        if self.is_finalized() {
            return Ok(());
        }
        self.delivery_pending = false;
        if self.pending_terminal_failure.is_some() {
            self.finalize_pending_terminal_failure().await?;
            return Ok(());
        }
        self.finish_interruption(&EngineError::Cancelled).await
    }

    /// Cancel remaining work after a local delivery failure, retaining any
    /// already-started finalization or authoritative pending provider failure.
    ///
    /// # Errors
    ///
    /// Observation write failures remain best effort, as for other finalization.
    pub async fn fail_delivery(&mut self, error: GatewayError) -> Result<(), EngineError> {
        self.cancellation.cancel();
        self.retain_pending_terminal_failure();
        self.resume_finalization().await;
        if self.is_finalized() {
            return Ok(());
        }
        self.delivery_pending = false;
        if self.pending_terminal_failure.is_some() {
            self.finalize_pending_terminal_failure().await?;
            return Ok(());
        }
        self.response_commit.finish();
        let outcome = if self.response_commit.committed_at().is_some() {
            ExecutionOutcome::Incomplete
        } else {
            ExecutionOutcome::Failed
        };
        self.finish_local_failure(outcome, error).await
    }

    async fn pull(&mut self) -> Result<PullOutcome, EngineError> {
        loop {
            if self.pending_stream_failure.is_some() {
                return self.resume_stream_failure().await;
            }
            if self.current.is_none() {
                match self.prepare_attempt().await {
                    Ok(Some(outcome)) => return Ok(outcome),
                    Ok(None) => {}
                    Err(error) => {
                        if !self.is_finalized() {
                            self.finish_interruption(&error).await?;
                        }
                        return Err(error);
                    }
                }
            }

            let boundary = {
                let current = self.current.as_mut().ok_or(EngineError::NoActiveAttempt)?;
                poll_stream_item(
                    &mut current.stream,
                    self.cancellation.clone(),
                    self.deadline,
                    &mut self.deadline_timer,
                )
                .await
            };

            match boundary {
                PollBoundary::Cancelled => {
                    self.finish_interruption(&EngineError::Cancelled).await?;
                    return Err(EngineError::Cancelled);
                }
                PollBoundary::Deadline => {
                    // 会话 deadline 是网关自身的请求预算，不是上游超时；
                    // 真正的上游超时会作为流错误进入 `begin_stream_failure` 记账。
                    // 这里不写 provider 失败，避免长流集中到期误触 provider 熔断。
                    self.finish_interruption(&EngineError::Deadline).await?;
                    return Err(EngineError::Deadline);
                }
                PollBoundary::Item(Some(Ok(mut event))) => {
                    if let Some(checkpoint) = event.take_metering() {
                        self.observation.observe_metering(&checkpoint);
                    }
                    if let Some(wire) = event.wire_event() {
                        self.trace
                            .attempt(self.attempts)
                            .wire_event(wire.protocol(), wire.event_type());
                    }
                    if let Some(state) = event.session_update() {
                        self.observe_session_update(state);
                    }
                    if let Some(observation) = event.take_observation() {
                        self.observe_response(observation);
                    }
                    for fact in event.canonical_facts() {
                        self.observation.observe_identity(fact);
                        if self.trace.captures_bodies()
                            && matches!(fact, GatewayEvent::Completed(_))
                        {
                            self.trace
                                .attempt(self.attempts)
                                .record("upstream.completed", json!({}));
                        }
                    }
                    for fact in event.canonical_facts() {
                        self.observe_event(fact).await;
                    }
                    if event.wire_event().is_some() && !event.has_canonical_facts() {
                        self.observe_wire_event().await;
                    }
                    if !event.has_client_event() {
                        continue;
                    }
                    if event
                        .canonical_facts()
                        .iter()
                        .any(|fact| matches!(fact, GatewayEvent::Completed(_)))
                        && let Some(guard) = &mut self.client_retry
                    {
                        guard.success().await;
                    }
                    return Ok(PullOutcome::Event(event));
                }
                PollBoundary::Item(Some(Err(error))) => {
                    self.begin_stream_failure(error)?;
                    return self.resume_stream_failure().await;
                }
                PollBoundary::Item(None) => {
                    self.record_current_provider_success();
                    self.upstream_complete = true;
                    return Ok(PullOutcome::End);
                }
            }
        }
    }

    async fn prepare_attempt(&mut self) -> Result<Option<PullOutcome>, EngineError> {
        let pending_retry = self.pending_retry.take();
        let is_connection_recovery = (pending_retry.is_some()
            || self.connection_rotation.is_some())
            && self.last_retryable_failure.as_ref().is_some_and(|error| {
                matches!(
                    error.pre_delivery_retry(),
                    Some(crate::error::PreDeliveryRetry::SameAccountConnectionRetry { .. })
                )
            });
        let is_transport_recovery = pending_retry
            .as_ref()
            .is_some_and(|retry| retry.transport_recovery);
        let connection_rotation_reserved = pending_retry
            .as_ref()
            .is_some_and(|retry| retry.reserve_connection_rotation);
        if !is_transport_recovery && self.routing_attempts >= self.plan.max_attempts().get() {
            return self.finish_exhausted_routing().await;
        }
        if let Some(recovery) = pending_retry.as_ref()
            && !recovery.delay.is_zero()
        {
            match poll_retry_delay(recovery.delay, self.cancellation.clone(), self.deadline).await {
                RetryDelayBoundary::Elapsed => {}
                RetryDelayBoundary::Cancelled => {
                    self.finish_interruption(&EngineError::Cancelled).await?;
                    return Err(EngineError::Cancelled);
                }
                RetryDelayBoundary::Deadline => {
                    self.finish_interruption(&EngineError::Deadline).await?;
                    return Err(EngineError::Deadline);
                }
            }
        }
        if is_connection_recovery && self.connection_budget.remaining().is_none() {
            return self.finish_exhausted_routing().await;
        }
        let Some(candidate) = self.plan.candidates().get(self.candidate_index).cloned() else {
            if self.last_retryable_failure.is_some() {
                return self.finish_exhausted_routing().await;
            }
            let error = GatewayError::new(
                GatewayErrorKind::NoAvailableProvider,
                "no upstream Provider is available",
            );
            self.finish_failure(FailureFinalization {
                outcome: ExecutionOutcome::Failed,
                send_state: self.current_send_state(),
                mark_send_state: false,
                error,
                upstream_status_code: None,
                upstream_request_id: None,
                provider_error_code: None,
                raw_upstream_error: None,
                retry_after_ms: None,
                error_details: None,
                observation: ModelRequestFailureObservation::default(),
            })
            .await?;
            return Err(EngineError::EmptyRoutingPlan);
        };
        let next_attempt = self
            .attempts
            .checked_add(1)
            .and_then(NonZeroU32::new)
            .ok_or(EngineError::EmptyRoutingPlan)?;
        // 请求局部恢复钉选在此被一次性消费，只绑定本次 replay attempt；
        // 外部 required_account 每次 attempt 都重新生效。
        let (pinned_account, attempt_transport) = if let Some(recovery) = pending_retry {
            (Some(recovery.account), recovery.transport)
        } else {
            match &self.account_selection {
                AccountSelection::Diagnostic(account) => {
                    (Some(account.clone()), AttemptTransport::Default)
                }
                AccountSelection::Scheduled(_) | AccountSelection::Quality(_, _) => (
                    self.recovery_account
                        .take()
                        .or_else(|| self.account_selection.required_account().cloned()),
                    self.connection_rotation
                        .as_ref()
                        .map_or(AttemptTransport::Default, |recovery| recovery.transport),
                ),
            }
        };
        if !is_transport_recovery
            && pinned_account.is_none()
            && self.last_selected_account.is_some()
            && self.account_switches >= self.plan.request_tuning().max_account_switches
        {
            return self.finish_exhausted_routing().await;
        }
        let account_context = match &self.account_selection {
            AccountSelection::Diagnostic(account) => AccountAttemptContext::diagnostic(
                self.excluded_accounts.clone(),
                account.clone(),
                self.account_state_owner.clone(),
            ),
            AccountSelection::Scheduled(_) | AccountSelection::Quality(_, _) => {
                AccountAttemptContext::new(
                    self.excluded_accounts.clone(),
                    pinned_account.clone(),
                    self.account_state_owner.clone(),
                )
                .with_account_scope(Arc::clone(candidate.account_scope()))
                .with_quality_check(matches!(
                    &self.account_selection,
                    AccountSelection::Quality(_, _)
                ))
                .with_quality_retest(matches!(
                    &self.account_selection,
                    AccountSelection::Quality(_, true)
                ))
                .with_native_quality_probe(matches!(
                    &self.operation,
                    Operation::Generate(generate) if generate.quality_probe().is_some()
                ))
                .with_excel_recovery(match &self.operation {
                    Operation::Generate(generate) => generate.excel_recovery_revision(),
                    _ => None,
                })
            }
        }
        .with_credential_recovery_attempted(pinned_account.as_ref().is_some_and(|account| {
            self.credential_recovery_attempted_accounts
                .contains(account)
        }));
        let provider = self
            .engine
            .providers()
            .get(candidate.provider())
            .cloned()
            .ok_or_else(|| EngineError::ProviderNotRegistered {
                provider: candidate.provider().as_str().to_owned(),
            })?;
        if !self.request_profiles.contains_key(candidate.provider()) {
            match provider.resolve_request_profile() {
                Ok(profile) => {
                    self.request_profiles
                        .insert(candidate.provider().clone(), profile.map(Arc::new));
                }
                Err(error) => {
                    self.finish_provider_error(&error).await?;
                    return Err(provider_engine_error(error));
                }
            }
        }
        let context = AttemptContext::new(
            RequestAttemptContext::new(self.request_id.clone(), self.client_api_key_ref.clone())
                .with_client_ip(self.client_ip)
                .with_connection_budget(if connection_rotation_reserved {
                    self.connection_budget.clone().reserve_next_opening()
                } else {
                    self.connection_budget.clone()
                })
                .with_response_control(Some(self.response_control.clone()))
                .with_provider_route(Arc::clone(
                    self.provider_routes
                        .entry(candidate.provider().clone())
                        .or_default(),
                ))
                .with_request_profile(
                    self.request_profiles
                        .get(candidate.provider())
                        .cloned()
                        .flatten(),
                )
                .with_disable_fast(self.plan.disable_fast())
                .with_codex_quota_overdraft(
                    self.plan.codex_quota_overdraft_enabled(),
                    self.plan.codex_quota_overdraft_probe_model().clone(),
                    self.plan.config_revision(),
                )
                .with_request_location(self.plan.request_location().cloned())
                .with_timing_started_at(self.observation.timing_started_at)
                .with_trace(self.trace.clone()),
            next_attempt,
            self.deadline,
            self.plan.account_selection_policy(),
            account_context,
            self.continuation.clone(),
            self.cancellation.clone(),
        )
        .with_continuation_attempt(self.continuation_attempt)
        .with_transport(attempt_transport)
        .with_account_wait_budget(Arc::clone(&self.account_wait_budget))
        .with_request_tuning(self.plan.request_tuning());
        let trigger = if self.attempts == 0 {
            AttemptTrigger::Initial
        } else {
            AttemptTrigger::AccountRetry
        };
        let attempt_trace = self.trace.attempt(next_attempt.get());
        attempt_trace.record(
            "attempt.started",
            json!({
                "provider": candidate.provider().as_str(),
                "model": candidate.upstream_model().map(|model| model.as_str()),
                "transportPolicy": format!("{attempt_transport:?}"),
                "continuation": format!("{:?}", self.continuation_attempt),
                "pinnedAccount": pinned_account.as_ref().map(|id| id.as_str()),
                "transportRecovery": is_transport_recovery,
            }),
        );
        let provider_request = ProviderRequest::new(self.operation.clone(), candidate.clone());
        let stream = match poll_provider(
            provider,
            provider_request,
            context,
            self.cancellation.clone(),
            self.deadline,
        )
        .await
        {
            ProviderBoundary::Cancelled => {
                self.finish_interruption(&EngineError::Cancelled).await?;
                return Err(EngineError::Cancelled);
            }
            ProviderBoundary::Deadline => {
                // 网关预算到期同样不是候选 Provider 的上游超时，不计入熔断；
                // Provider 自身的握手/传输超时会以 `ProviderErrorKind::Timeout`
                // 错误返回并在下方 `Result` 分支记账。
                self.finish_interruption(&EngineError::Deadline).await?;
                return Err(EngineError::Deadline);
            }
            ProviderBoundary::Result(result) => match *result {
                Ok(stream) => stream,
                Err(error) => {
                    record_trace_error(&attempt_trace, &error);
                    if self.connection_rotation.is_some()
                        && self.connection_budget.remaining().is_some()
                        && !error.retry_is_prohibited()
                        && error.send_state() == UpstreamSendState::NotSent
                        && matches!(
                            error.kind(),
                            ProviderErrorKind::NoEligibleAccount
                                | ProviderErrorKind::AccountCapacityUnavailable
                                | ProviderErrorKind::QuotaExhausted
                        )
                    {
                        let recovery = self
                            .connection_rotation
                            .take()
                            .expect("pending connection rotation");
                        self.excluded_accounts.remove(&recovery.account);
                        self.pending_retry = Some(recovery);
                        attempt_trace.record(
                            "retry.decided",
                            json!({
                                "phase": "prepare", "retryable": true, "connectionFallback": true,
                            }),
                        );
                        return Ok(Some(PullOutcome::AttemptDiscarded));
                    }
                    if self.prepare_unavailable_native_continuation_replay(&error) {
                        attempt_trace.record(
                            "retry.decided",
                            json!({
                                "phase": "prepare", "retryable": true,
                                "continuationRetry": true,
                                "accountSwitches": self.account_switches,
                            }),
                        );
                        return Ok(Some(PullOutcome::AttemptDiscarded));
                    }
                    if !error.retry_is_prohibited()
                        && matches!(
                            error.kind(),
                            ProviderErrorKind::AccountCapacityUnavailable
                                | ProviderErrorKind::NoEligibleAccount
                                | ProviderErrorKind::QuotaExhausted
                                | ProviderErrorKind::ProviderInfrastructureUnavailable
                        )
                        && !self.account_wait_budget.is_exhausted()
                        && error.send_state() == UpstreamSendState::NotSent
                        && matches!(
                            self.continuation_attempt,
                            ContinuationAttempt::None | ContinuationAttempt::ReplayAny
                        )
                        && self.advance_provider_candidate()
                    {
                        attempt_trace.record(
                            "retry.decided",
                            json!({
                                "phase": "prepare", "retryable": true,
                                "advanceProvider": true,
                                "accountSwitches": self.account_switches,
                                "accountWaitExhausted": self.account_wait_budget.is_exhausted(),
                            }),
                        );
                        return Ok(Some(PullOutcome::AttemptDiscarded));
                    }
                    attempt_trace.record(
                        "retry.decided",
                        json!({
                            "phase": "prepare", "retryable": false,
                            "retryProhibited": error.retry_is_prohibited(),
                            "sendState": format!("{:?}", error.send_state()),
                            "accountSwitches": self.account_switches,
                            "accountWaitExhausted": self.account_wait_budget.is_exhausted(),
                        }),
                    );
                    if (error.kind() == ProviderErrorKind::SessionPolicyRestricted
                        || (!error.retry_is_prohibited()
                            && matches!(
                                error.kind(),
                                ProviderErrorKind::AccountCapacityUnavailable
                                    | ProviderErrorKind::NoEligibleAccount
                                    | ProviderErrorKind::QuotaExhausted
                            )))
                        && let Some(last_failure) = self.last_retryable_failure.take()
                    {
                        if last_failure.kind() == ProviderErrorKind::Unauthorized {
                            // 本地账号池已耗尽时，之前账号的认证 401 不能继续冒充
                            // 账号池错误。真实上游 429/502 等终态仍按原有合同保留。
                            self.last_retryable_failure_events.clear();
                            self.finish_provider_error(&error).await?;
                            return Err(provider_engine_error(error));
                        }
                        return self.finish_retained_failure(last_failure).await;
                    }
                    if !(matches!(
                        error.kind(),
                        ProviderErrorKind::AccountCapacityUnavailable
                            | ProviderErrorKind::NoEligibleAccount
                            | ProviderErrorKind::SessionPolicyRestricted
                            | ProviderErrorKind::QuotaExhausted
                            | ProviderErrorKind::ProviderInfrastructureUnavailable
                    ) && error.send_state() == UpstreamSendState::NotSent)
                    {
                        self.record_provider_failure(candidate.provider().clone(), error.kind());
                    }
                    self.finish_provider_error(&error).await?;
                    return Err(provider_engine_error(error));
                }
            },
        };
        if !stream.metadata().confirms(&candidate) {
            drop(stream);
            self.record_provider_failure(candidate.provider().clone(), ProviderErrorKind::Protocol);
            let error = GatewayError::new(
                GatewayErrorKind::Internal,
                "provider metadata did not match the frozen candidate",
            );
            self.finish_failure(FailureFinalization {
                outcome: ExecutionOutcome::Failed,
                send_state: self.current_send_state(),
                mark_send_state: false,
                error,
                upstream_status_code: None,
                upstream_request_id: None,
                provider_error_code: None,
                raw_upstream_error: None,
                retry_after_ms: None,
                error_details: None,
                observation: ModelRequestFailureObservation::default(),
            })
            .await?;
            return Err(EngineError::ProviderMetadataMismatch);
        }

        let metadata = stream.metadata().clone();
        self.connection_rotation = None;
        attempt_trace.record("account.selected", json!({
            "provider": metadata.provider().as_str(), "accountId": metadata.provider_account_id().as_str(),
            "transport": metadata.transport().as_str(),
            "selectionMs": metadata.selection_observation().map(|o| o.account_selection_wait_ms()),
        }));
        let selection_observation = metadata.selection_observation();
        let capacity = selection_observation.and_then(|observation| observation.capacity());
        if !self.account_selection.is_diagnostic()
            && !candidate
                .account_scope()
                .allows(metadata.provider_account_id())
        {
            drop(stream);
            let error = GatewayError::new(
                GatewayErrorKind::Internal,
                "provider selected an account outside the frozen client scope",
            );
            self.finish_failure(FailureFinalization {
                outcome: ExecutionOutcome::Failed,
                send_state: self.current_send_state(),
                mark_send_state: false,
                error,
                upstream_status_code: None,
                upstream_request_id: None,
                provider_error_code: None,
                raw_upstream_error: None,
                retry_after_ms: None,
                error_details: None,
                observation: ModelRequestFailureObservation::default(),
            })
            .await?;
            return Err(EngineError::AccountOutsideClientScope);
        }
        if pinned_account
            .as_ref()
            .is_some_and(|required| metadata.provider_account_id() != required)
        {
            drop(stream);
            let error = GatewayError::new(
                GatewayErrorKind::Internal,
                "provider did not use the required account",
            );
            self.finish_failure(FailureFinalization {
                outcome: ExecutionOutcome::Failed,
                send_state: self.current_send_state(),
                mark_send_state: false,
                error,
                upstream_status_code: None,
                upstream_request_id: None,
                provider_error_code: None,
                raw_upstream_error: None,
                retry_after_ms: None,
                error_details: None,
                observation: ModelRequestFailureObservation::default(),
            })
            .await?;
            return Err(EngineError::RequiredAccountMismatch);
        }
        if let Some(pin) = self
            .continuation
            .as_ref()
            .and_then(ContinuationBinding::pinned)
            && self.continuation_attempt == ContinuationAttempt::Native
            && !pin.matches(metadata.provider(), metadata.provider_account_id())
        {
            drop(stream);
            let error = GatewayError::new(
                GatewayErrorKind::Internal,
                "native continuation binding did not match selected account",
            );
            self.finish_failure(FailureFinalization {
                outcome: ExecutionOutcome::Failed,
                send_state: self.current_send_state(),
                mark_send_state: false,
                error,
                upstream_status_code: None,
                upstream_request_id: None,
                provider_error_code: None,
                raw_upstream_error: None,
                retry_after_ms: None,
                error_details: None,
                observation: ModelRequestFailureObservation::default(),
            })
            .await?;
            return Err(EngineError::ContinuationPinMismatch);
        }
        let stream = if matches!(self.account_selection, AccountSelection::Scheduled(_))
            && self.plan.account_error_protection().policy().enabled
            && let Some(store) = &self.engine.account_error_protection
        {
            stream
                .with_account_error_protection(
                    Arc::clone(store),
                    self.plan.account_error_protection(),
                    self.plan.config_revision().get(),
                )
                .await
        } else {
            stream
        };
        if self.account_state_owner.is_none() {
            self.account_state_owner = Some(ProviderAccountStateOwner::new(
                metadata.provider().clone(),
                metadata.provider_account_id().clone(),
            ));
        }
        let attempt_record = AttemptRecord {
            request_id: self.request_id.clone(),
            attempt_count: next_attempt,
            trigger,
            provider_kind: metadata.provider().clone(),
            provider_account_id: Some(metadata.provider_account_id().clone()),
            provider_account_ref: Some(metadata.provider_account_id().clone()),
            upstream_model_id: metadata.upstream_model().cloned(),
            upstream_transport: metadata.transport().as_str().to_owned(),
            http_version: None,
            account_selection_wait_ms: selection_observation
                .map(|observation| observation.account_selection_wait_ms()),
            capacity_used_slots: capacity.map(|snapshot| snapshot.used_slots()),
            capacity_total_slots: capacity.map(|snapshot| snapshot.total_slots()),
        };
        // 合法冷流在可取消的观测写入前归会话所有，首写等待中断也不能退回零次或重新选号。
        self.attempts = next_attempt.get();
        if !is_transport_recovery {
            self.routing_attempts = self.routing_attempts.saturating_add(1);
            if let Some(previous) = self.last_selected_account.as_ref()
                && previous != metadata.provider_account_id()
            {
                self.account_switches = self.account_switches.saturating_add(1);
            }
            self.last_selected_account = Some(metadata.provider_account_id().clone());
        }
        self.current = Some(CurrentAttempt {
            stream,
            metadata,
            trigger,
            transport: attempt_transport,
            connection_rotation_reserved,
            index: next_attempt,
            started_at: SystemTime::now(),
            send_observed: false,
            response_observation: None,
        });
        if self.request_persisted {
            best_effort_store_write(
                "record_attempt",
                &self.request_id,
                self.engine.store().record_attempt(attempt_record),
            )
            .await;
        } else {
            let request = self
                .pending_request
                .as_ref()
                .ok_or(EngineError::InvalidDeliveryState)?;
            if best_effort_store_write(
                "create_model_request_with_attempt",
                &self.request_id,
                self.engine
                    .store()
                    .create_model_request_with_attempt(request.clone(), attempt_record),
            )
            .await
            .is_some()
            {
                self.pending_request = None;
                self.request_persisted = true;
            }
        }
        Ok(None)
    }

    fn advance_provider_candidate(&mut self) -> bool {
        let Some(next) = self.candidate_index.checked_add(1) else {
            return false;
        };
        if next >= self.plan.candidates().len() {
            return false;
        }
        self.candidate_index = next;
        // An original account is only a recovery candidate within its Provider.
        self.connection_rotation = None;
        true
    }

    async fn observe_event(&mut self, event: &GatewayEvent) {
        self.observation.observe_event(event);
        self.mark_send_observed().await;
    }

    async fn observe_wire_event(&mut self) {
        self.mark_send_observed().await;
    }

    fn observe_session_update(&mut self, state: &ProviderSessionState) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if state.provider() != current.metadata.provider().as_str() {
            return;
        }
        self.operation.set_provider_session_state(state.clone());
    }

    async fn mark_send_observed(&mut self) {
        self.connection_budget.complete();
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if !current.send_observed {
            if self.request_persisted {
                best_effort_store_write(
                    "mark_send_state",
                    &self.request_id,
                    self.engine
                        .store()
                        .mark_send_state(&self.request_id, UpstreamSendState::Sent),
                )
                .await;
            }
            current.send_observed = true;
            self.send_state_watermark = UpstreamSendState::Sent;
        }
    }

    fn observe_response(&mut self, observation: ProviderResponseObservation) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        if current
            .response_observation
            .as_ref()
            .is_some_and(|existing| existing.transport() != observation.transport())
        {
            return;
        }
        self.observation.observe_response(&observation);
        current.response_observation = Some(observation);
    }

    /// 失败可重试时要求丢弃本 attempt；预算耗尽时可返回最后一批原始失败事件。
    fn begin_stream_failure(&mut self, mut error: ProviderError) -> Result<(), EngineError> {
        // 原始 wire 只活在 request-local 决策状态；clone、attempt 记录与持久化终态
        // 均只接触已剥离的稳定错误字段。
        record_trace_error(&self.trace.attempt(self.attempts), &error);
        let mut atomic_client_events = error.take_atomic_client_events();
        // Selection/lease waiting can consume the window after the retry was
        // scheduled. Keep the actual connection failure, not the local stop marker.
        if std::error::Error::source(&error)
            .is_some_and(|source| source.is::<super::connection::ConnectionBudgetExhausted>())
            && error.send_state() == UpstreamSendState::NotSent
            && self.current_send_state() == UpstreamSendState::NotSent
            && !self.replay_barrier_reached
            && !self.delivery_pending
            && self
                .last_retryable_failure
                .as_ref()
                .is_some_and(|previous| {
                    matches!(
                        previous.pre_delivery_retry(),
                        Some(crate::error::PreDeliveryRetry::SameAccountConnectionRetry { .. })
                    )
                })
            && let Some(previous) = self.last_retryable_failure.take()
        {
            error = previous.with_retry_prohibited();
            atomic_client_events = std::mem::take(&mut self.last_retryable_failure_events);
        }
        let current = self.current.take().ok_or(EngineError::NoActiveAttempt)?;
        self.record_provider_failure(current.metadata.provider().clone(), error.kind());
        // attempt_send_state 是本 attempt 自身的发送事实，驱动重试门；
        // 持久化与终态用请求级水位，二者不可混用（水位会把早先 attempt 的
        // sent 传染给本 attempt，从而错误放行/拦截重试）。
        let attempt_send_state = if current.send_observed {
            UpstreamSendState::Sent
        } else {
            error.send_state()
        };
        let send_state = self.raise_send_watermark(attempt_send_state);
        if send_state != UpstreamSendState::NotSent {
            self.connection_budget.complete();
        }
        let provider_proved_replay_safe = provider_proved_replay_safe(&error);
        let continuation_retry = self.prepare_continuation_retry(
            &current,
            &error,
            attempt_send_state,
            provider_proved_replay_safe,
        );
        let account_rotation_retry = self.account_selection.required_account().is_none()
            && self.continuation_attempt == ContinuationAttempt::None
            && !self.replay_barrier_reached
            && !self.delivery_pending
            && attempt_send_state != UpstreamSendState::Ambiguous
            && matches!(
                error.pre_delivery_retry(),
                Some(crate::error::PreDeliveryRetry::AccountRotation)
            )
            && self.routing_attempts < self.plan.max_attempts().get();
        let connection_retry_requested = matches!(
            error.pre_delivery_retry(),
            Some(crate::error::PreDeliveryRetry::SameAccountConnectionRetry { .. })
        );
        let transport_recovery = match error.pre_delivery_retry() {
            Some(crate::error::PreDeliveryRetry::SameAccountConnectionRetry { transport })
                if !self.replay_barrier_reached
                    && !self.delivery_pending
                    && send_state == UpstreamSendState::NotSent
                    && self.continuation_attempt == ContinuationAttempt::None =>
            {
                self.connection_budget
                    .retry_delay(self.connection_retries, self.request_id.as_str())
                    .map(|delay| {
                        self.connection_retries += 1;
                        (transport, delay)
                    })
            }
            Some(crate::error::PreDeliveryRetry::SameAccountTransportRetry {
                retry_index,
                delay,
            }) if !self.replay_barrier_reached
                && !self.delivery_pending
                && attempt_send_state != UpstreamSendState::Ambiguous =>
            {
                Some((AttemptTransport::Retry(retry_index), delay))
            }
            Some(crate::error::PreDeliveryRetry::SameAccountTransportFallback)
                if !self.replay_barrier_reached
                    && !self.delivery_pending
                    && attempt_send_state != UpstreamSendState::Ambiguous =>
            {
                Some((
                    AttemptTransport::Fallback,
                    error.retry_after().unwrap_or_default(),
                ))
            }
            _ => None,
        };
        let can_rotate_connection = connection_retry_requested
            && self.account_selection.required_account().is_none()
            && self.routing_attempts < self.plan.max_attempts().get()
            && self.account_switches < self.plan.request_tuning().max_account_switches;
        let connection_rotation = transport_recovery
            .filter(|_| can_rotate_connection && current.connection_rotation_reserved);
        let reserve_connection_rotation = can_rotate_connection
            && !current.connection_rotation_reserved
            && self.connection_budget.remaining_openings() >= 2;
        let transport_recovery = transport_recovery.filter(|_| connection_rotation.is_none());
        let ordinary_retry = !connection_retry_requested
            && self.account_selection.required_account().is_none()
            && self.continuation_attempt == ContinuationAttempt::None
            && !self.replay_barrier_reached
            && !self.delivery_pending
            && attempt_send_state != UpstreamSendState::Ambiguous
            && provider_proved_replay_safe
            && self.routing_attempts < self.plan.max_attempts().get();
        let suppress_transient_retry = is_return_error_retry(&error)
            && self
                .client_retry
                .as_ref()
                .is_some_and(ClientRetryGuard::has_failure);
        let transient_retry = match error.pre_delivery_retry() {
            Some(crate::error::PreDeliveryRetry::SameAccountTransientRetry {
                max_retries,
                initial_delay,
                max_delay,
                jitter,
                ..
            }) if ordinary_retry && !suppress_transient_retry => {
                let retries = self
                    .transient_retry_counts
                    .entry(current.metadata.provider_account_id().clone())
                    .or_default();
                if *retries < max_retries.get() {
                    let multiplier = 1_u32.checked_shl(*retries).unwrap_or(u32::MAX);
                    let delay = error.retry_after().unwrap_or_else(|| {
                        initial_delay
                            .saturating_mul(multiplier)
                            .min(max_delay)
                            .saturating_add(jitter)
                    });
                    *retries = retries.saturating_add(1);
                    Some(delay)
                } else {
                    None
                }
            }
            _ => None,
        };
        let same_account_retry = error.retries_same_account()
            && provider_proved_replay_safe
            && !self.replay_barrier_reached
            && !self.delivery_pending
            && attempt_send_state != UpstreamSendState::Ambiguous
            && self.routing_attempts < self.plan.max_attempts().get()
            && !self
                .credential_recovery_attempted_accounts
                .contains(current.metadata.provider_account_id());
        let recovery_probe = matches!(&self.operation, Operation::Generate(generate) if generate.excel_recovery_revision().is_some());
        let rotation_allowed = !matches!(
            error.pre_delivery_retry(),
            Some(crate::error::PreDeliveryRetry::SameAccountTransientRetry {
                policy: crate::error::TransientRetryPolicy::RetryThenReturnError,
                ..
            })
        );
        let retryable = !recovery_probe
            && !error.retry_is_prohibited()
            && (continuation_retry
                || same_account_retry
                || transient_retry.is_some()
                || (rotation_allowed && (ordinary_retry || account_rotation_retry))
                || transport_recovery.is_some()
                || connection_rotation.is_some());

        self.trace.attempt(current.index.get()).record("retry.decided", json!({
            "phase": "execute",
            "retryable": retryable, "continuationRetry": continuation_retry,
            "sameAccountRetry": same_account_retry, "accountRotationRetry": account_rotation_retry,
            "ordinaryRetry": ordinary_retry, "transportRecovery": transport_recovery.is_some(),
            "connectionRotation": connection_rotation.is_some(),
            "reserveConnectionRotation": reserve_connection_rotation,
            "transientRetry": transient_retry.is_some(),
            "clientRetrySuppressed": suppress_transient_retry,
            "connectionRetry": connection_retry_requested,
            "connectionRetries": self.connection_retries,
            "delayMs": transient_retry.or(transport_recovery.map(|(_, delay)| delay)).map(duration_ms),
            "downstreamCommitted": self.response_commit.committed_at().is_some(),
            "replayBarrierReached": self.replay_barrier_reached,
            "deliveryPending": self.delivery_pending,
            "providerProvedReplaySafe": provider_proved_replay_safe,
            "retryProhibited": error.retry_is_prohibited(),
            "requiredAccount": self.account_selection.required_account().map(|account| account.as_str()),
            "continuation": format!("{:?}", self.continuation_attempt),
            "connectionRecoveryAvailable": self.connection_budget.remaining().is_some(),
            "remainingRoutingAttempts": self.plan.max_attempts().get().saturating_sub(self.routing_attempts),
            "maxAccountRotations": self.plan.request_tuning().max_account_switches,
            "accountRotations": self.account_switches,
            "transientRetriesUsed": self.transient_retry_counts.get(current.metadata.provider_account_id()).copied().unwrap_or_default(),
            "credentialRecoveryAttempted": self.credential_recovery_attempted_accounts.contains(current.metadata.provider_account_id()),
            "sendState": format!("{attempt_send_state:?}"),
        }));
        let mut intermediate = None;
        let outcome = if retryable {
            for event in &atomic_client_events {
                for fact in event.canonical_facts() {
                    self.observation.observe_event(fact);
                }
            }
            // 普通 clone 只保留稳定事实；原始 wire/HTTP response 由 request-local
            // 所有权保留到下一次 attempt 成功，或最终空选路时返回客户端。
            let persistence_error = self.request_persisted.then(|| error.clone());
            if same_account_retry {
                let account = current.metadata.provider_account_id().clone();
                self.credential_recovery_attempted_accounts
                    .insert(account.clone());
                // 只钉住紧随其后的 replay attempt；replay 再遇可重试错误时，
                // ordinary/continuation 重试门不受影响，仍可换号。
                self.recovery_account = Some(account);
            } else if let Some(delay) = transient_retry {
                self.pending_retry = Some(PendingAttemptRetry {
                    account: current.metadata.provider_account_id().clone(),
                    transport: current.transport,
                    delay,
                    transport_recovery: false,
                    reserve_connection_rotation: false,
                });
            } else if let Some((transport, delay)) = transport_recovery {
                self.pending_retry = Some(PendingAttemptRetry {
                    account: current.metadata.provider_account_id().clone(),
                    transport,
                    delay,
                    transport_recovery: true,
                    reserve_connection_rotation,
                });
            } else if let Some((transport, delay)) = connection_rotation {
                let account = current.metadata.provider_account_id().clone();
                self.excluded_accounts.insert(account.clone());
                self.connection_rotation = Some(PendingAttemptRetry {
                    account,
                    transport,
                    delay,
                    transport_recovery: true,
                    reserve_connection_rotation: false,
                });
            } else if !continuation_retry {
                self.excluded_accounts
                    .insert(current.metadata.provider_account_id().clone());
            }
            self.last_retryable_failure_events = atomic_client_events;
            self.last_retryable_failure = Some(error);
            if let Some(error) = persistence_error {
                intermediate = Some(IntermediateFailure {
                    request_id: self.request_id.clone(),
                    attempt_index: current.index,
                    trigger: current.trigger,
                    provider_kind: current.metadata.provider().clone(),
                    account_id: Some(current.metadata.provider_account_id().clone()),
                    upstream_model_id: current.metadata.upstream_model().cloned(),
                    upstream_status_code: error.upstream_status().or_else(|| {
                        current
                            .response_observation
                            .as_ref()
                            .and_then(ProviderResponseObservation::status_code)
                    }),
                    upstream_request_id: error
                        .upstream_request_id()
                        .map(|id| id.as_str())
                        .or_else(|| current.upstream_request_id())
                        .map(str::to_owned),
                    latency: current.started_at.elapsed().unwrap_or_default(),
                    error,
                });
            }
            self.current = Some(current);
            StreamErrorOutcome::AttemptDiscarded
        } else {
            self.current = Some(current);
            self.observe_atomic_terminal_events(&mut atomic_client_events);
            StreamErrorOutcome::TerminalFailure {
                events: atomic_client_events,
                error,
                send_state,
            }
        };
        let store = Arc::clone(self.engine.store());
        let request_id = self.request_id.clone();
        let persisted = self.request_persisted;
        self.pending_stream_failure = Some(PendingStreamFailure {
            outcome,
            completion: Box::pin(async move {
                if persisted {
                    best_effort_store_write(
                        "mark_send_state",
                        &request_id,
                        store.mark_send_state(&request_id, send_state),
                    )
                    .await;
                }
                if let Some(intermediate) = intermediate {
                    best_effort_store_write(
                        "record_intermediate_failure",
                        &request_id,
                        store.record_intermediate_failure(intermediate),
                    )
                    .await;
                }
            }),
        });
        Ok(())
    }

    fn retain_pending_terminal_failure(&mut self) {
        if let Some(pending) = self.pending_stream_failure.take()
            && let StreamErrorOutcome::TerminalFailure {
                events,
                error,
                send_state,
            } = pending.outcome
        {
            self.pending_terminal_failure = Some(PendingTerminalFailure {
                error: Arc::new(error),
                send_state,
                events,
            });
        }
    }

    async fn resume_stream_failure(&mut self) -> Result<PullOutcome, EngineError> {
        let pending = self
            .pending_stream_failure
            .as_mut()
            .ok_or(EngineError::NoActiveAttempt)?;
        pending.completion.as_mut().await;
        let pending = self
            .pending_stream_failure
            .take()
            .ok_or(EngineError::NoActiveAttempt)?;
        if matches!(pending.outcome, StreamErrorOutcome::AttemptDiscarded) {
            self.current.take();
            self.reset_uncommitted_observations();
        }
        Ok(pending.outcome.into_pull_outcome())
    }

    async fn next_terminal_failure(&mut self) -> Result<Option<CoordinatedEvent>, EngineError> {
        let pending = self
            .pending_terminal_failure
            .as_ref()
            .ok_or(EngineError::NoActiveAttempt)?;
        if pending.events.is_empty() {
            return Err(provider_engine_error(
                self.finalize_pending_terminal_failure().await?,
            ));
        }
        let error = Arc::clone(&pending.error);
        self.record_client_retry_failure(&error).await;
        let pending = self
            .pending_terminal_failure
            .as_mut()
            .ok_or(EngineError::NoActiveAttempt)?;
        let events = std::mem::take(&mut pending.events);
        let requirement = if self.downstream_committed_at.is_some() {
            CommitRequirement::AlreadyCommitted
        } else {
            self.delivery_pending = true;
            CommitRequirement::CommitBeforeDelivery
        };
        self.replay_barrier_reached = true;
        CoordinatedEvent::try_batch(events, requirement).map(Some)
    }

    async fn finalize_pending_terminal_failure(&mut self) -> Result<ProviderError, EngineError> {
        let pending = self
            .pending_terminal_failure
            .as_ref()
            .ok_or(EngineError::NoActiveAttempt)?;
        let error = Arc::clone(&pending.error);
        let send_state = pending.send_state;
        self.finish_provider_error_with_send_state(&error, send_state)
            .await?;
        drop(error);
        let pending = self
            .pending_terminal_failure
            .take()
            .ok_or(EngineError::NoActiveAttempt)?;
        Ok(Arc::try_unwrap(pending.error).expect("terminal failure is owned by the session"))
    }

    fn record_final_usage(&self) {
        let usage = &self.observation.usage;
        self.trace.record_evidence("request.usage", json!({
            "boundary": "gateway_finalization",
            "inputTokens": usage.input_tokens, "outputTokens": usage.output_tokens,
            "cachedInputTokens": usage.cached_tokens, "cacheWriteInputTokens": usage.cache_write_tokens,
            "reasoningOutputTokens": usage.reasoning_tokens, "totalTokens": usage.total_tokens,
            "imageInputTokens": usage.image_input_tokens, "imageOutputTokens": usage.image_output_tokens,
        }));
    }

    fn observe_atomic_terminal_events(&mut self, events: &mut [ProviderEvent]) {
        for event in events {
            if let Some(checkpoint) = event.take_metering() {
                self.observation.observe_metering(&checkpoint);
            }
            if let Some(observation) = event.take_observation() {
                self.observe_response(observation);
            }
            for fact in event.canonical_facts() {
                self.observation.observe_identity(fact);
                self.observation.observe_event(fact);
            }
        }
    }

    fn prepare_continuation_retry(
        &mut self,
        current: &CurrentAttempt,
        error: &ProviderError,
        send_state: UpstreamSendState,
        provider_proved_replay_safe: bool,
    ) -> bool {
        if matches!(
            error.pre_delivery_retry(),
            Some(crate::error::PreDeliveryRetry::SameAccountConnectionRetry { .. })
        ) || error.retry_is_prohibited()
            || self.account_selection.required_account().is_some()
            || self.continuation_attempt == ContinuationAttempt::None
            || self.replay_barrier_reached
            || self.delivery_pending
            || send_state == UpstreamSendState::Ambiguous
            || !provider_proved_replay_safe
            || self.routing_attempts >= self.plan.max_attempts().get()
            || self
                .operation
                .provider_session_state(current.metadata.provider().as_str())
                .is_none()
        {
            return false;
        }

        match self.continuation_attempt {
            ContinuationAttempt::Native => match error.continuation_recovery_disposition() {
                Some(ContinuationRecoveryDisposition::RetryExactConnection) => {
                    self.recovery_account = Some(current.metadata.provider_account_id().clone());
                }
                Some(ContinuationRecoveryDisposition::ProviderReplayAllowed) => {
                    self.continuation_attempt = ContinuationAttempt::ReplayOwner;
                }
                Some(ContinuationRecoveryDisposition::ClientReplayRequired) | None => return false,
            },
            ContinuationAttempt::ReplayOwner | ContinuationAttempt::ReplayAny
                if matches!(
                    error.continuation_recovery_disposition(),
                    Some(
                        ContinuationRecoveryDisposition::RetryExactConnection
                            | ContinuationRecoveryDisposition::ClientReplayRequired
                    )
                ) =>
            {
                return false;
            }
            ContinuationAttempt::ReplayOwner => {
                self.continuation_attempt = ContinuationAttempt::ReplayAny;
                self.excluded_accounts
                    .insert(current.metadata.provider_account_id().clone());
            }
            ContinuationAttempt::ReplayAny => {
                self.excluded_accounts
                    .insert(current.metadata.provider_account_id().clone());
            }
            ContinuationAttempt::None => return false,
        }
        true
    }

    /// 原生续写的原账号在本地调度阶段已不可用时，交给 Provider 执行跨账号恢复。
    ///
    /// 这不是强制 Smart：下一次 attempt 仍使用请求配置的调度策略。只有网关持有
    /// 对应 Provider 的会话状态时才进入恢复；是否保留 native handle、执行 probe
    /// 或使用完整 transcript，由 Provider 自己的协议边界决定。
    fn prepare_unavailable_native_continuation_replay(&mut self, error: &ProviderError) -> bool {
        if error.retry_is_prohibited()
            || self.account_selection.required_account().is_some()
            || self.account_wait_budget.is_exhausted()
            || (self.plan.request_tuning().account_busy_wait_enabled
                && matches!(
                    error.continuation_recovery_disposition(),
                    Some(
                        ContinuationRecoveryDisposition::RetryExactConnection
                            | ContinuationRecoveryDisposition::ClientReplayRequired
                    )
                ))
            || self.continuation_attempt != ContinuationAttempt::Native
            || !matches!(
                error.kind(),
                ProviderErrorKind::NoEligibleAccount
                    | ProviderErrorKind::AccountCapacityUnavailable
                    | ProviderErrorKind::QuotaExhausted
            )
        {
            return false;
        }
        let Some(pin) = self
            .continuation
            .as_ref()
            .and_then(ContinuationBinding::pinned)
        else {
            return false;
        };
        if self
            .operation
            .provider_session_state(pin.provider().as_str())
            .is_none()
        {
            return false;
        }

        self.continuation_attempt = ContinuationAttempt::ReplayAny;
        self.excluded_accounts.insert(pin.account().clone());
        true
    }

    fn reset_uncommitted_observations(&mut self) {
        // 响应观测按尝试隔离；被丢弃尝试中已取得的费用仍计入 Key。
        self.budget_prior_attempts_usd = self
            .budget_prior_attempts_usd
            .checked_add(self.budget_attempt_usd())
            .unwrap_or(Decimal::MAX);
        self.budget_attempt_already_counted = false;
        self.observation.reset_for_attempt();
        self.upstream_complete = false;
    }

    async fn finish_success(&mut self) -> Result<(), EngineError> {
        if self.is_finalized() {
            return Ok(());
        }
        if let Some(guard) = &mut self.client_retry {
            guard.success().await;
        }
        let completed_at = SystemTime::now();
        self.observation.finish();
        let upstream_request_id = self
            .current
            .as_ref()
            .and_then(CurrentAttempt::upstream_request_id)
            .map(str::to_owned);
        let upstream_status_code = self
            .current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .and_then(ProviderResponseObservation::status_code);
        let (upstream_transport, http_version, websocket_pool) =
            self.current_transport_observation();
        let service_tier = self.current_service_tier();
        let provider_metadata_json = self.current_provider_metadata_json();
        self.record_final_usage();
        self.trace.record(
            "request.finished",
            json!({"outcome": "succeeded", "attempts": self.attempts}),
        );
        self.persist_finalization(
            ModelRequestFinalization {
                request_id: self.request_id.clone(),
                outcome: ExecutionOutcome::Succeeded,
                send_state: UpstreamSendState::Sent,
                attempt_count: self.attempts,
                downstream_committed_at: self.response_commit.committed_at(),
                client_status_code: self
                    .response_commit
                    .status_code()
                    .or(self.client_status_code),
                upstream_status_code,
                client_response_id: self.observation.client_response_id.clone(),
                upstream_request_id,
                upstream_response_id: self.observation.upstream_response_id.clone(),
                upstream_transport,
                http_version,
                websocket_pool,
                service_tier,
                provider_metadata_json,
                diagnostic_trace_json: self.trace.snapshot().map(|value| value.to_string()),
                error: None,
                provider_error_code: None,
                raw_upstream_error: None,
                error_details: None,
                failure_observation: ModelRequestFailureObservation::default(),
                retry_after_ms: None,
                usage: self.observation.usage.clone(),
                image_generation_succeeded: self.image_generation_succeeded(),
                cost: self.observation.cost.clone(),
                timings: self.observation.timings.clone(),
                completed_at,
            },
            false,
        )
        .await;
        Ok(())
    }

    async fn finish_exhausted_routing(&mut self) -> Result<Option<PullOutcome>, EngineError> {
        let Some(error) = self.last_retryable_failure.take() else {
            return Err(EngineError::EmptyRoutingPlan);
        };
        self.finish_retained_failure(error).await
    }

    async fn finish_retained_failure(
        &mut self,
        error: ProviderError,
    ) -> Result<Option<PullOutcome>, EngineError> {
        let send_state = self.current_send_state();
        let mut events = std::mem::take(&mut self.last_retryable_failure_events);
        if !events.is_empty() {
            self.observe_atomic_terminal_events(&mut events);
            self.budget_attempt_already_counted = true;
            return Ok(Some(PullOutcome::TerminalFailure {
                events,
                error,
                send_state,
            }));
        }
        self.finish_provider_error(&error).await?;
        Err(provider_engine_error(error))
    }

    async fn record_client_retry_failure(&mut self, error: &ProviderError) {
        if self.attempts > 0
            && is_return_error_retry(error)
            && let Some(guard) = &mut self.client_retry
        {
            guard.failure().await;
        }
    }

    async fn finish_provider_error(&mut self, error: &ProviderError) -> Result<(), EngineError> {
        self.finish_provider_error_with_send_state(
            error,
            escalate_send_state(self.current_send_state(), error.send_state()),
        )
        .await
    }

    async fn finish_provider_error_with_send_state(
        &mut self,
        error: &ProviderError,
        send_state: UpstreamSendState,
    ) -> Result<(), EngineError> {
        self.record_client_retry_failure(error).await;
        let send_state = self.raise_send_watermark(send_state);
        self.response_commit.finish();
        let outcome = if error.kind() == ProviderErrorKind::Cancelled {
            ExecutionOutcome::Cancelled
        } else if self.response_commit.committed_at().is_some() {
            ExecutionOutcome::Incomplete
        } else {
            ExecutionOutcome::Failed
        };
        self.finish_failure(FailureFinalization {
            outcome,
            send_state,
            mark_send_state: false,
            error: GatewayError::from_provider(error),
            upstream_status_code: error.upstream_status(),
            upstream_request_id: error.upstream_request_id().map(|id| id.as_str().to_owned()),
            provider_error_code: error.upstream_code().map(|code| code.as_str().to_owned()),
            error_details: error.error_details(),
            raw_upstream_error: error
                .raw_upstream_error()
                .map(|raw| raw.as_str().to_owned()),
            retry_after_ms: error.retry_after().map(duration_ms),
            observation: ModelRequestFailureObservation {
                continuation_unavailable_reason: error
                    .continuation_unavailable_reason()
                    .map(str::to_owned),
                upstream_connection: error.connection_observation().cloned(),
            },
        })
        .await
    }

    async fn finish_interruption(&mut self, error: &EngineError) -> Result<(), EngineError> {
        self.response_commit.finish();
        let (outcome, gateway_error) = match error {
            EngineError::Cancelled => (
                ExecutionOutcome::Cancelled,
                GatewayError::new(
                    GatewayErrorKind::Cancelled,
                    self.cancellation
                        .reason()
                        .unwrap_or(crate::lifecycle::CancellationReason::Requested)
                        .message(),
                ),
            ),
            EngineError::Deadline => (
                if self.response_commit.committed_at().is_some() {
                    ExecutionOutcome::Incomplete
                } else {
                    ExecutionOutcome::Failed
                },
                GatewayError::new(GatewayErrorKind::Timeout, "request deadline elapsed"),
            ),
            _ => (
                ExecutionOutcome::Failed,
                GatewayError::new(GatewayErrorKind::Internal, "request execution failed"),
            ),
        };
        self.finish_local_failure(outcome, gateway_error).await
    }

    async fn finish_local_failure(
        &mut self,
        outcome: ExecutionOutcome,
        error: GatewayError,
    ) -> Result<(), EngineError> {
        let send_state = if self.attempts == 0 {
            UpstreamSendState::NotSent
        } else if self.downstream_committed_at.is_some()
            || self
                .current
                .as_ref()
                .is_some_and(|current| current.send_observed)
        {
            UpstreamSendState::Sent
        } else {
            UpstreamSendState::Ambiguous
        };
        let send_state = self.raise_send_watermark(send_state);
        self.finish_failure(FailureFinalization {
            outcome,
            send_state,
            mark_send_state: self.attempts > 0 && self.request_persisted,
            error,
            upstream_status_code: None,
            upstream_request_id: None,
            provider_error_code: None,
            raw_upstream_error: None,
            retry_after_ms: None,
            error_details: None,
            observation: ModelRequestFailureObservation::default(),
        })
        .await
    }

    async fn finish_failure(
        &mut self,
        finalization: FailureFinalization,
    ) -> Result<(), EngineError> {
        if self.is_finalized() {
            return Ok(());
        }
        let completed_at = SystemTime::now();
        self.observation.finish();
        let upstream_request_id = self
            .current
            .as_ref()
            .and_then(CurrentAttempt::upstream_request_id)
            .map(str::to_owned);
        let observed_status_code = self
            .current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .and_then(ProviderResponseObservation::status_code);
        let (upstream_transport, http_version, websocket_pool) =
            self.current_transport_observation();
        let service_tier = self.current_service_tier();
        let provider_metadata_json = self.current_provider_metadata_json();
        self.record_final_usage();
        self.trace.record(
            "request.finished",
            json!({
                "outcome": format!("{:?}", finalization.outcome),
                "errorKind": finalization.error.kind().as_str(),
                "sendState": format!("{:?}", finalization.send_state), "attempts": self.attempts,
            }),
        );
        self.persist_finalization(
            ModelRequestFinalization {
                request_id: self.request_id.clone(),
                outcome: finalization.outcome,
                send_state: finalization.send_state,
                attempt_count: self.attempts,
                downstream_committed_at: self.response_commit.committed_at(),
                client_status_code: self
                    .response_commit
                    .status_code()
                    .or(self.client_status_code),
                upstream_status_code: finalization.upstream_status_code.or(observed_status_code),
                client_response_id: self.observation.client_response_id.clone(),
                upstream_request_id: finalization.upstream_request_id.or(upstream_request_id),
                upstream_response_id: self.observation.upstream_response_id.clone(),
                upstream_transport,
                http_version,
                websocket_pool,
                service_tier,
                provider_metadata_json,
                diagnostic_trace_json: self.trace.snapshot().map(|value| value.to_string()),
                error_details: finalization.error_details.or_else(|| {
                    finalization
                        .error
                        .error_details()
                        .map(|details| details.into_string())
                }),
                error: Some(finalization.error),
                provider_error_code: finalization.provider_error_code,
                raw_upstream_error: finalization.raw_upstream_error,
                failure_observation: finalization.observation,
                retry_after_ms: finalization.retry_after_ms,
                usage: self.observation.usage.clone(),
                image_generation_succeeded: self.image_generation_succeeded(),
                cost: self.observation.cost.clone(),
                timings: self.observation.timings.clone(),
                completed_at,
            },
            finalization.mark_send_state,
        )
        .await;
        Ok(())
    }

    async fn persist_finalization(
        &mut self,
        finalization: ModelRequestFinalization,
        mark_send_state: bool,
    ) {
        self.response_commit.finish();
        self.finalized_at = Some(finalization.completed_at);
        let mut persisted = self.request_persisted;
        // 首次合并写失败后不能以零 attempt 补行；建流前也只有确定未发送的请求可按零次收敛。
        let request = if !persisted
            && finalization.attempt_count == 0
            && finalization.send_state == UpstreamSendState::NotSent
        {
            self.pending_request.take()
        } else {
            None
        };
        let store = Arc::clone(self.engine.store());
        let request_id = self.request_id.clone();
        // 终态写入由会话持有；取消事件等待只暂停同一个 future，不重建请求或覆盖原失败。
        self.finalization = Some(RequestFinalization::Pending(Box::pin(async move {
            if let Some(request) = request {
                persisted = best_effort_store_write(
                    "create_model_request",
                    &request_id,
                    store.create_model_request(request),
                )
                .await
                .is_some();
            }
            if persisted {
                if mark_send_state {
                    best_effort_store_write(
                        "mark_send_state",
                        &request_id,
                        store.mark_send_state(&request_id, finalization.send_state),
                    )
                    .await;
                }
                best_effort_store_write(
                    "finalize_model_request",
                    &request_id,
                    store.finalize_model_request(finalization),
                )
                .await;
            }
            persisted
        })));
        self.resume_finalization().await;
    }

    async fn resume_finalization(&mut self) {
        if let Some(RequestFinalization::Pending(write)) = self.finalization.as_mut() {
            self.request_persisted = write.await;
            self.finalization = Some(RequestFinalization::Complete);
            self.lease.take();
        }
    }

    fn current_transport_observation(&self) -> (Option<String>, Option<String>, Option<String>) {
        let Some(current) = self.current.as_ref() else {
            return (None, None, None);
        };
        let Some(observation) = current.response_observation.as_ref() else {
            return (None, None, None);
        };
        (
            Some(observation.transport().as_str().to_owned()),
            observation
                .http_version()
                .map(|version| version.as_str().to_owned()),
            observation
                .websocket_pool()
                .map(|kind| kind.as_str().to_owned()),
        )
    }

    fn current_provider_metadata_json(&self) -> Option<String> {
        self.current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .and_then(ProviderResponseObservation::provider_metadata)
            .map(|metadata| metadata.as_json().to_owned())
    }

    fn current_service_tier(&self) -> Option<String> {
        self.current
            .as_ref()
            .and_then(|current| current.response_observation.as_ref())
            .and_then(ProviderResponseObservation::service_tier)
            .map(str::to_owned)
    }

    fn image_generation_succeeded(&self) -> Option<bool> {
        self.image_generation_requested.then(|| {
            if matches!(self.operation, Operation::GenerateImage(_)) {
                self.upstream_complete
            } else {
                self.observation
                    .usage
                    .image_output_tokens
                    .unwrap_or_default()
                    > 0
            }
        })
    }

    fn current_send_state(&self) -> UpstreamSendState {
        let observed = if self
            .current
            .as_ref()
            .is_some_and(|current| current.send_observed)
        {
            UpstreamSendState::Sent
        } else {
            UpstreamSendState::NotSent
        };
        escalate_send_state(self.send_state_watermark, observed)
    }

    /// 抬升并返回请求级发送水位；attempt 间切换（`current` 被取走）后，
    /// 后续终态沿用已达到的最高档，不会把已落库的 `sent` 写回 `not_sent`。
    fn raise_send_watermark(&mut self, observed: UpstreamSendState) -> UpstreamSendState {
        self.send_state_watermark = escalate_send_state(self.send_state_watermark, observed);
        self.send_state_watermark
    }

    fn record_current_provider_success(&mut self) {
        let provider_kind = self
            .current
            .as_ref()
            .map(|current| current.metadata.provider().clone());
        if let Some(provider_kind) = provider_kind {
            self.provider_attempt_outcomes
                .push(ProviderAttemptOutcome::Succeeded { provider_kind });
        }
    }

    fn record_provider_failure(
        &mut self,
        provider_kind: crate::identity::ProviderKind,
        error_kind: ProviderErrorKind,
    ) {
        self.provider_attempt_outcomes
            .push(ProviderAttemptOutcome::Failed {
                provider_kind,
                error_kind,
            });
    }
}

async fn best_effort_store_write<T>(
    operation: &'static str,
    request_id: &ModelRequestId,
    write: impl Future<Output = Result<T, StoreError>>,
) -> Option<T> {
    match write.await {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(
                operation,
                request_id = request_id.as_str(),
                error_kind = ?error.kind(),
                "执行观测写入失败，数据面不受影响"
            );
            None
        }
    }
}

enum PullOutcome {
    Event(ProviderEvent),
    AttemptDiscarded,
    TerminalFailure {
        events: Vec<ProviderEvent>,
        error: ProviderError,
        send_state: UpstreamSendState,
    },
    End,
}

enum StreamErrorOutcome {
    AttemptDiscarded,
    TerminalFailure {
        events: Vec<ProviderEvent>,
        error: ProviderError,
        send_state: UpstreamSendState,
    },
}

impl StreamErrorOutcome {
    fn into_pull_outcome(self) -> PullOutcome {
        match self {
            Self::AttemptDiscarded => PullOutcome::AttemptDiscarded,
            Self::TerminalFailure {
                events,
                error,
                send_state,
            } => PullOutcome::TerminalFailure {
                events,
                error,
                send_state,
            },
        }
    }
}

enum PollBoundary {
    Item(Option<Result<ProviderEvent, ProviderError>>),
    Cancelled,
    Deadline,
}

async fn poll_stream_item(
    stream: &mut ProviderStream,
    cancellation: CancellationToken,
    deadline: Deadline,
    mut deadline_timer: &mut Fuse<BoxFuture<'static, ()>>,
) -> PollBoundary {
    if deadline.is_elapsed() {
        return PollBoundary::Deadline;
    }
    let next = stream.next().fuse();
    let cancelled = cancellation.cancelled().fuse();
    pin_mut!(next, cancelled);
    select_biased! {
        _ = cancelled => PollBoundary::Cancelled,
        _ = deadline_timer => PollBoundary::Deadline,
        item = next => PollBoundary::Item(item),
    }
}

enum ProviderBoundary {
    Result(Box<Result<ProviderStream, ProviderError>>),
    Cancelled,
    Deadline,
}

enum RetryDelayBoundary {
    Elapsed,
    Cancelled,
    Deadline,
}

async fn poll_retry_delay(
    delay: Duration,
    cancellation: CancellationToken,
    deadline: Deadline,
) -> RetryDelayBoundary {
    if deadline.is_elapsed() {
        return RetryDelayBoundary::Deadline;
    }
    let retry_delay = Delay::new(delay).fuse();
    let cancelled = cancellation.cancelled().fuse();
    let timeout = deadline.wait().fuse();
    pin_mut!(retry_delay, cancelled, timeout);
    select_biased! {
        _ = cancelled => RetryDelayBoundary::Cancelled,
        _ = timeout => RetryDelayBoundary::Deadline,
        _ = retry_delay => RetryDelayBoundary::Elapsed,
    }
}

async fn poll_provider(
    provider: Arc<dyn Provider>,
    request: ProviderRequest,
    context: AttemptContext,
    cancellation: CancellationToken,
    deadline: Deadline,
) -> ProviderBoundary {
    if deadline.is_elapsed() {
        return ProviderBoundary::Deadline;
    }
    let execution = provider.execute(request, context).fuse();
    let cancelled = cancellation.cancelled().fuse();
    let timeout = deadline.wait().fuse();
    pin_mut!(execution, cancelled, timeout);
    select_biased! {
        _ = cancelled => ProviderBoundary::Cancelled,
        _ = timeout => ProviderBoundary::Deadline,
        result = execution => ProviderBoundary::Result(Box::new(result)),
    }
}

fn initial_continuation_attempt(
    operation: &Operation,
    plan: &RoutingPlan,
    continuation: Option<&ContinuationBinding>,
) -> ContinuationAttempt {
    match continuation {
        None => ContinuationAttempt::None,
        Some(ContinuationBinding::External(_))
            if plan.candidates().first().is_some_and(|candidate| {
                operation
                    .provider_session_state(candidate.provider().as_str())
                    .is_some()
            }) =>
        {
            ContinuationAttempt::ReplayAny
        }
        Some(_) => ContinuationAttempt::Native,
    }
}

fn provider_engine_error(error: ProviderError) -> EngineError {
    if error.kind() == ProviderErrorKind::Cancelled {
        EngineError::Cancelled
    } else {
        EngineError::Provider(error)
    }
}

fn provider_proved_replay_safe(error: &ProviderError) -> bool {
    error.send_state() == UpstreamSendState::NotSent
        || (error.send_state() != UpstreamSendState::Ambiguous && error.replay_is_safe())
}

/// 发送状态合并档位：`Sent` > `Ambiguous` > `NotSent`。
/// 只要任一 attempt 达到过高档，请求整体就不允许回落到低档。
const fn escalate_send_state(a: UpstreamSendState, b: UpstreamSendState) -> UpstreamSendState {
    match (a, b) {
        (UpstreamSendState::Sent, _) | (_, UpstreamSendState::Sent) => UpstreamSendState::Sent,
        (UpstreamSendState::Ambiguous, _) | (_, UpstreamSendState::Ambiguous) => {
            UpstreamSendState::Ambiguous
        }
        (UpstreamSendState::NotSent, UpstreamSendState::NotSent) => UpstreamSendState::NotSent,
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn record_trace_error(trace: &TraceContext, error: &ProviderError) {
    trace.record_provider_failure(error);
}
